//! Source-wide capacity, exclusive connection leases and bounded disposal.

use super::*;
use std::collections::{HashMap, HashSet, VecDeque};
use tokio::sync::Notify;

struct Idle {
    id: u64,
    connection: Connection,
    read_only: bool,
    born: Instant,
    uses: u64,
}

#[derive(Default)]
struct State {
    total: usize,
    next_id: u64,
    next_waiter: u64,
    idle: Vec<Idle>,
    waiters: VecDeque<u64>,
    drivers: HashMap<u64, AbortHandle>,
    timers: HashMap<u64, Timers>,
    retiring: HashSet<u64>,
}

#[derive(Default)]
struct Timers {
    idle: Option<JoinHandle<()>>,
    lifetime: Option<JoinHandle<()>>,
}
impl Drop for Timers {
    fn drop(&mut self) {
        for task in [&self.idle, &self.lifetime].into_iter().flatten() {
            task.abort();
        }
    }
}

#[cfg(test)]
struct TimerGuard(Arc<std::sync::atomic::AtomicUsize>);
#[cfg(test)]
impl Drop for TimerGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) struct TargetPool {
    pub(super) target: Target,
    state: SyncMutex<State>,
    changed: Notify,
    stopped: AtomicBool,
    stop_signal: watch::Sender<bool>,
    pub(super) cancellations: Cancellations,
    resolver: Arc<dyn Resolver>,
    #[cfg(test)]
    active_timers: Arc<std::sync::atomic::AtomicUsize>,
}

impl TargetPool {
    pub(super) fn subscribe(&self) -> watch::Receiver<bool> {
        self.stop_signal.subscribe()
    }

    pub(super) fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    pub(super) fn new(target: Target, resolver: Arc<dyn Resolver>) -> Arc<Self> {
        Arc::new(Self {
            target,
            state: SyncMutex::new(State::default()),
            changed: Notify::new(),
            stopped: AtomicBool::new(false),
            stop_signal: watch::channel(false).0,
            cancellations: Arc::new(SyncMutex::new(Vec::new())),
            resolver,
            #[cfg(test)]
            active_timers: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        })
    }

    pub(super) async fn acquire(self: &Arc<Self>, read_only: bool) -> Result<Lease, QueryError> {
        let mut stop = self.stop_signal.subscribe();
        if self.stopped.load(Ordering::Acquire) || !self.target.pool.valid() {
            return Err(QueryError::Unavailable);
        }
        tokio::select! {
            biased;
            _ = stop.changed() => Err(QueryError::Unavailable),
            result = self.acquire_inner(read_only) => result,
        }
    }

    async fn acquire_inner(self: &Arc<Self>, read_only: bool) -> Result<Lease, QueryError> {
        let deadline = match self.target.pool.checkout_policy() {
            // The v1 adapter bounded count's entire operation, but its slot
            // wait was not part of the physical connection deadline. Held
            // configurations retain that policy rather than gaining a new
            // failure before any connection is attempted.
            CheckoutPolicy::LegacySlot => None,
            CheckoutPolicy::PgPool => positive_duration(self.target.connect_timeout_ms)
                .map(|limit| Instant::now() + limit),
        };
        let (waiter, mut queued) = {
            let mut state = self.state.lock().map_err(|_| QueryError::Unavailable)?;
            let waiter = state.next_waiter;
            state.next_waiter = state.next_waiter.wrapping_add(1);
            let queued = !state.waiters.is_empty()
                || (state.idle.is_empty() && state.total >= self.target.pool.max);
            state.waiters.push_back(waiter);
            (waiter, queued)
        };
        let mut waiter = Waiter {
            pool: Arc::clone(self),
            id: Some(waiter),
        };
        loop {
            // Register before inspecting state so releases cannot be missed.
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            enum Next {
                Wait,
                Ready(Idle, Option<JoinHandle<()>>),
                Connect(u64),
                Retire(Idle),
            }
            let next = {
                let mut state = self.state.lock().map_err(|_| QueryError::Unavailable)?;
                if self.stopped.load(Ordering::Acquire) {
                    return Err(QueryError::Unavailable);
                }
                if state.waiters.front().copied() != waiter.id {
                    Next::Wait
                } else if let Some(index) = state
                    .idle
                    .iter()
                    .rposition(|idle| idle.read_only == read_only)
                {
                    let idle = state.idle.remove(index);
                    if idle.connection.client.is_closed() || self.expired(&idle) {
                        state.retiring.insert(idle.id);
                        Next::Retire(idle)
                    } else {
                        state.waiters.pop_front();
                        waiter.id = None;
                        let timer = state
                            .timers
                            .get_mut(&idle.id)
                            .and_then(|timers| timers.idle.take());
                        Next::Ready(idle, timer)
                    }
                } else if state.total < self.target.pool.max {
                    let id = state.next_id;
                    state.next_id = state.next_id.wrapping_add(1);
                    state.total += 1;
                    state.waiters.pop_front();
                    waiter.id = None;
                    Next::Connect(id)
                } else if let Some(idle) = state.idle.pop() {
                    // Read-only and writable sessions share the source budget.
                    // An incompatible idle session must not starve max=1.
                    state.retiring.insert(idle.id);
                    Next::Retire(idle)
                } else {
                    Next::Wait
                }
            };
            match next {
                Next::Ready(idle, timer) => {
                    self.changed.notify_waiters();
                    let lease = Lease::from_idle(Arc::clone(self), idle);
                    if let Some(timer) = timer {
                        timer.abort();
                        let _ = timer.await;
                    }
                    return Ok(lease);
                }
                Next::Retire(idle) => {
                    self.retire(idle.id, idle.connection);
                }
                Next::Connect(id) => {
                    self.changed.notify_waiters();
                    let mut reservation = Reservation {
                        pool: Arc::clone(self),
                        id,
                        active: true,
                    };
                    let establish = establish_connection(
                        &self.target,
                        &self.cancellations,
                        self.resolver.as_ref(),
                    );
                    // A queued pg-pool checkout keeps its original deadline
                    // even when capacity subsequently permits a new connect.
                    let connection = if queued {
                        match deadline {
                            Some(deadline) => timeout_at(deadline, establish)
                                .await
                                .map_err(|_| QueryError::CheckoutTimeout)??,
                            None => establish.await?,
                        }
                    } else {
                        establish.await?
                    };
                    let born = Instant::now();
                    self.state
                        .lock()
                        .map_err(|_| QueryError::Unavailable)?
                        .drivers
                        .insert(
                            id,
                            connection
                                .driver
                                .as_ref()
                                .expect("live PostgreSQL driver")
                                .abort_handle(),
                        );
                    let lease = Lease {
                        pool: Arc::clone(self),
                        id,
                        connection: Some(connection),
                        read_only,
                        born,
                        uses: 0,
                    };
                    reservation.active = false;
                    self.schedule_lifetime(id, born);
                    // Preserve the existing adapter boundary: session setup is
                    // outside connect/query timers, inside stop/count bounds.
                    if read_only {
                        set_read_only(lease.connection()).await?;
                    }
                    return Ok(lease);
                }
                Next::Wait => {
                    queued = true;
                    match deadline {
                        Some(deadline) => timeout_at(deadline, &mut changed)
                            .await
                            .map_err(|_| QueryError::CheckoutTimeout)?,
                        None => changed.await,
                    }
                }
            }
        }
    }

    fn expired(&self, idle: &Idle) -> bool {
        self.target
            .pool
            .max_lifetime_ms
            .is_some_and(|ms| idle.born.elapsed() >= Duration::from_millis(ms.into()))
    }

    fn schedule_lifetime(self: &Arc<Self>, id: u64, born: Instant) {
        let Some(ms) = self.target.pool.max_lifetime_ms else {
            return;
        };
        let weak = Arc::downgrade(self);
        #[cfg(test)]
        let guard = self.timer_guard();
        let task = tokio::spawn(async move {
            #[cfg(test)]
            let _guard = guard;
            tokio::time::sleep_until(born + Duration::from_millis(ms.into())).await;
            if let Some(pool) = weak.upgrade() {
                pool.expire_idle(id, None);
            }
        });
        if let Ok(mut state) = self.state.lock() {
            state.timers.entry(id).or_default().lifetime = Some(task);
        } else {
            task.abort();
        }
    }

    fn expire_idle(self: &Arc<Self>, id: u64, idle_generation: Option<u64>) {
        let idle = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            if idle_generation.is_some()
                && state.total.saturating_sub(state.retiring.len()) <= self.target.pool.min
            {
                return;
            }
            let idle = state
                .idle
                .iter()
                .position(|idle| {
                    idle.id == id
                        && idle_generation.is_none_or(|generation| idle.uses == generation)
                })
                .map(|index| state.idle.remove(index));
            if let Some(idle) = &idle {
                // pg-pool removes an entry before its socket finishes closing.
                // Keep its max reservation while excluding it from min's floor.
                state.retiring.insert(idle.id);
            }
            idle
        };
        if let Some(idle) = idle {
            self.retire(idle.id, idle.connection);
        }
    }

    fn return_idle(self: &Arc<Self>, mut idle: Idle) {
        idle.uses = idle.uses.saturating_add(1);
        if self.stopped.load(Ordering::Acquire)
            || idle.connection.client.is_closed()
            || self.expired(&idle)
            || self
                .target
                .pool
                .max_uses
                .is_some_and(|limit| idle.uses >= limit)
        {
            self.retire(idle.id, idle.connection);
            return;
        }
        let id = idle.id;
        let generation = idle.uses;
        {
            let Ok(mut state) = self.state.lock() else {
                self.retire(idle.id, idle.connection);
                return;
            };
            if self.stopped.load(Ordering::Acquire) {
                drop(state);
                self.retire(idle.id, idle.connection);
                return;
            }
            let timer = (state.total.saturating_sub(state.retiring.len()) > self.target.pool.min)
                .then_some(self.target.pool.idle_timeout_ms)
                .flatten();
            state.idle.push(idle);
            if let Some(ms) = timer {
                let weak = Arc::downgrade(self);
                #[cfg(test)]
                let guard = self.timer_guard();
                let task = tokio::spawn(async move {
                    #[cfg(test)]
                    let _guard = guard;
                    tokio::time::sleep(Duration::from_millis(ms.into())).await;
                    if let Some(pool) = weak.upgrade() {
                        pool.expire_idle(id, Some(generation));
                    }
                });
                state.timers.entry(id).or_default().idle = Some(task);
            }
        }
        self.changed.notify_waiters();
    }

    fn retire(self: &Arc<Self>, id: u64, mut connection: Connection) {
        let driver = connection.driver.take();
        if let Some(driver) = &driver {
            driver.abort();
        }
        let mut timers = self
            .state
            .lock()
            .ok()
            .and_then(|mut state| {
                state.retiring.insert(id);
                state.timers.remove(&id)
            })
            .unwrap_or_default();
        for timer in [&timers.idle, &timers.lifetime].into_iter().flatten() {
            timer.abort();
        }
        let cancellation = connection.start_cancellation();
        drop(connection);
        let pool = Arc::clone(self);
        let task = tokio::spawn(async move {
            // Abort is only a request. Keep capacity until the driver future
            // has actually dropped its stream, plus bounded protocol cleanup.
            if let Some(driver) = driver {
                let _ = driver.await;
            }
            if let Some(cancellation) = cancellation {
                let _ = cancellation.await;
            }
            for timer in [timers.idle.take(), timers.lifetime.take()]
                .into_iter()
                .flatten()
            {
                let _ = timer.await;
            }
            pool.finish_retire(id);
        });
        if let Ok(mut pending) = self.cancellations.lock() {
            pending.retain(|task| !task.is_finished());
            pending.push(task);
        }
    }

    fn finish_retire(&self, id: u64) {
        if let Ok(mut state) = self.state.lock() {
            state.total = state.total.saturating_sub(1);
            state.drivers.remove(&id);
            state.retiring.remove(&id);
        }
        self.changed.notify_waiters();
    }

    pub(super) async fn shutdown(self: &Arc<Self>) {
        self.stopped.store(true, Ordering::Release);
        self.stop_signal.send_replace(true);
        self.changed.notify_waiters();
        let idle = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            for driver in state.drivers.values() {
                driver.abort();
            }
            std::mem::take(&mut state.idle)
        };
        for idle in idle {
            self.retire(idle.id, idle.connection);
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.state.lock().map_or(true, |state| state.total == 0) {
                break;
            }
            if timeout_at(deadline, changed).await.is_err() {
                break;
            }
        }
        let pending = self
            .cancellations
            .lock()
            .map(|mut tasks| std::mem::take(&mut *tasks))
            .unwrap_or_default();
        for task in pending {
            let _ = timeout_at(deadline, task).await;
        }
    }

    #[cfg(test)]
    pub(super) fn idle_count(&self) -> usize {
        self.state.lock().unwrap().idle.len()
    }

    #[cfg(test)]
    fn timer_guard(&self) -> TimerGuard {
        self.active_timers.fetch_add(1, Ordering::AcqRel);
        TimerGuard(Arc::clone(&self.active_timers))
    }

    #[cfg(test)]
    pub(super) fn active_timer_count(&self) -> usize {
        self.active_timers.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(super) fn timer_counter(&self) -> Arc<std::sync::atomic::AtomicUsize> {
        Arc::clone(&self.active_timers)
    }

    #[cfg(test)]
    pub(super) fn total_count(&self) -> usize {
        self.state.lock().unwrap().total
    }

    #[cfg(test)]
    pub(super) fn hold_capacity(self: &Arc<Self>) -> impl Drop {
        let mut state = self.state.lock().unwrap();
        let id = state.next_id;
        state.next_id += 1;
        state.total += 1;
        Reservation {
            pool: Arc::clone(self),
            id,
            active: true,
        }
    }

    #[cfg(test)]
    pub(super) fn idle_closed(&self) -> bool {
        self.state
            .lock()
            .unwrap()
            .idle
            .last()
            .is_some_and(|idle| idle.connection.client.is_closed())
    }

    #[cfg(test)]
    pub(super) fn idle_context(&self) -> Option<TransportContext> {
        self.state
            .lock()
            .unwrap()
            .idle
            .last()
            .and_then(|idle| idle.connection.transport_context())
    }
}

struct Waiter {
    pool: Arc<TargetPool>,
    id: Option<u64>,
}
impl Drop for Waiter {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            if let Ok(mut state) = self.pool.state.lock() {
                state.waiters.retain(|waiter| *waiter != id);
            }
            self.pool.changed.notify_waiters();
        }
    }
}

struct Reservation {
    pool: Arc<TargetPool>,
    id: u64,
    active: bool,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if self.active {
            self.pool.finish_retire(self.id);
        }
    }
}

pub(super) struct Lease {
    pool: Arc<TargetPool>,
    id: u64,
    connection: Option<Connection>,
    read_only: bool,
    born: Instant,
    uses: u64,
}
impl Lease {
    fn from_idle(pool: Arc<TargetPool>, idle: Idle) -> Self {
        Self {
            pool,
            id: idle.id,
            connection: Some(idle.connection),
            read_only: idle.read_only,
            born: idle.born,
            uses: idle.uses,
        }
    }

    pub(super) fn connection(&self) -> &Connection {
        self.connection.as_ref().expect("live PostgreSQL lease")
    }

    #[cfg(test)]
    pub(super) fn connection_mut(&mut self) -> &mut Connection {
        self.connection.as_mut().expect("live PostgreSQL lease")
    }

    pub(super) fn release(mut self) {
        let Some(connection) = self.connection.take() else {
            return;
        };
        self.pool.return_idle(Idle {
            id: self.id,
            connection,
            read_only: self.read_only,
            born: self.born,
            uses: self.uses,
        });
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            self.pool.retire(self.id, connection);
        }
    }
}
