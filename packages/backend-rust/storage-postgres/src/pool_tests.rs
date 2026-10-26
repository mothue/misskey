use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, oneshot},
};

struct LiveQuery {
    session: usize,
    read_only: bool,
    release: oneshot::Sender<()>,
}

struct Peer {
    port: u16,
    live: mpsc::UnboundedReceiver<LiveQuery>,
    accepted: Arc<std::sync::atomic::AtomicUsize>,
    cancelled: Arc<std::sync::atomic::AtomicUsize>,
    task: JoinHandle<()>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Peer {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (live_tx, live) = mpsc::unbounded_channel();
        let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let cancelled = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let accepts = Arc::clone(&accepted);
        let cancellations = Arc::clone(&cancelled);
        let task = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    result = listener.accept() => {
                        let (mut socket, _) = result.unwrap();
                        let live_tx = live_tx.clone();
                        let accepts = Arc::clone(&accepts);
                        let cancellations = Arc::clone(&cancellations);
                        tasks.spawn(async move {
                            let length = socket.read_u32().await.unwrap() as usize;
                            let mut startup = vec![0; length - 4];
                            socket.read_exact(&mut startup).await.unwrap();
                            if startup.starts_with(&80877102_u32.to_be_bytes()) {
                                assert_eq!(length, 16);
                                assert_eq!(&startup[4..], &[0, 0, 0, 17, 0, 0, 0, 23]);
                                cancellations.fetch_add(1, Ordering::AcqRel);
                                return;
                            }
                            let session = accepts.fetch_add(1, Ordering::AcqRel);
                            socket.write_all(b"R\0\0\0\x08\0\0\0\0K\0\0\0\x0c\0\0\0\x11\0\0\0\x17Z\0\0\0\x05I").await.unwrap();
                            let mut read_only = false;
                            loop {
                                let kind = match socket.read_u8().await {
                                    Ok(kind) => kind,
                                    Err(_) => return,
                                };
                                let length = socket.read_u32().await.unwrap() as usize;
                                let mut payload = vec![0; length - 4];
                                socket.read_exact(&mut payload).await.unwrap();
                                if kind == b'Q' {
                                    assert_eq!(payload, b"SET default_transaction_read_only = on\0");
                                    read_only = true;
                                    socket.write_all(b"C\0\0\0\x08SET\0Z\0\0\0\x05I").await.unwrap();
                                    continue;
                                }
                                if kind == b'X' { return; }
                                if kind == b'C' {
                                    assert_eq!(super::tests::read_frontend(&mut socket).await.0, b'S');
                                    socket.write_all(b"3\0\0\0\x04Z\0\0\0\x05I").await.unwrap();
                                    continue;
                                }
                                assert_eq!(kind, b'P');
                                let sql = payload.split(|byte| *byte == 0).nth(1).unwrap();
                                let count = sql.starts_with(b"SELECT COUNT(*)");
                                let rejected = sql == b"SELECT fixture_error";
                                while super::tests::read_frontend(&mut socket).await.0 != b'S' {}
                                socket.write_all(b"1\0\0\0\x04").await.unwrap();
                                if count {
                                    socket.write_all(b"t\0\0\0\x0a\0\x01\0\0\x04\xa0").await.unwrap();
                                    let mut description = b"\0\x01count\0".to_vec();
                                    description.extend_from_slice(&[0,0,0,0,0,0,0,0,0,20,0,8,255,255,255,255,0,0]);
                                    socket.write_u8(b'T').await.unwrap();
                                    socket.write_u32((description.len() + 4) as u32).await.unwrap();
                                    socket.write_all(&description).await.unwrap();
                                } else {
                                    socket.write_all(b"t\0\0\0\x06\0\0n\0\0\0\x04").await.unwrap();
                                }
                                socket.write_all(b"Z\0\0\0\x05I").await.unwrap();
                                while super::tests::read_frontend(&mut socket).await.0 != b'S' {}
                                let (release, released) = oneshot::channel();
                                if live_tx.send(LiveQuery { session, read_only, release }).is_err() { return; }
                                if released.await.is_err() { return; }
                                socket.write_all(b"2\0\0\0\x04").await.unwrap();
                                if rejected {
                                    let error = b"SERROR\0C42P01\0Mfixture SQL rejection\0\0";
                                    socket.write_u8(b'E').await.unwrap();
                                    socket.write_u32((error.len() + 4) as u32).await.unwrap();
                                    socket.write_all(error).await.unwrap();
                                    socket.write_all(b"Z\0\0\0\x05I").await.unwrap();
                                    continue;
                                }
                                if count {
                                    socket.write_all(b"D\0\0\0\x12\0\x01\0\0\0\x08\0\0\0\0\0\0\0\0").await.unwrap();
                                }
                                socket.write_all(b"C\0\0\0\x0dSELECT 0\0Z\0\0\0\x05I").await.unwrap();
                            }
                        });
                    }
                    Some(result) = tasks.join_next(), if !tasks.is_empty() => { result.unwrap(); }
                }
            }
        });
        Self {
            port,
            live,
            accepted,
            cancelled,
            task,
        }
    }

    fn target(&self, max: usize) -> Target {
        let mut target: Target = super::tests::fixture().targets.remove(0);
        target.port = self.port;
        target.connect_timeout_ms = None;
        target.source = PoolSource::Primary;
        target.pool = PoolSettings {
            max,
            min: 0,
            idle_timeout_ms: None,
            max_uses: None,
            max_lifetime_ms: None,
            held: None,
        };
        target
    }

    async fn query(&mut self) -> LiveQuery {
        tokio::time::timeout(Duration::from_secs(2), self.live.recv())
            .await
            .unwrap()
            .unwrap()
    }

    async fn no_query(&mut self) {
        assert!(
            tokio::time::timeout(Duration::from_millis(25), self.live.recv())
                .await
                .is_err()
        );
    }
}

fn query(
    database: &Arc<Database>,
    read: bool,
) -> JoinHandle<Result<Vec<tokio_postgres::Row>, QueryError>> {
    query_sql(database, read, "SELECT 1")
}

fn query_sql(
    database: &Arc<Database>,
    read: bool,
    sql: &'static str,
) -> JoinHandle<Result<Vec<tokio_postgres::Row>, QueryError>> {
    let database = Arc::clone(database);
    tokio::spawn(async move { database.query_typed(read, sql, &[]).await })
}

#[tokio::test]
async fn two_queries_are_on_the_wire_together_and_a_third_waits() {
    let mut peer = Peer::new().await;
    let database = Arc::new(Database::new(None, Some(peer.target(2))));
    let first = query(&database, false);
    let second = query(&database, false);
    let one = peer.query().await;
    let two = peer.query().await;
    assert_ne!(one.session, two.session);
    let third = query(&database, false);
    peer.no_query().await;
    assert_eq!(peer.accepted.load(Ordering::Acquire), 2);
    one.release.send(()).unwrap();
    let three = peer.query().await;
    assert_eq!(three.session, one.session);
    two.release.send(()).unwrap();
    three.release.send(()).unwrap();
    for task in [first, second, third] {
        assert!(task.await.unwrap().is_ok());
    }
    database.shutdown().await;
}

#[tokio::test]
async fn count_and_auth_share_capacity_and_max_one_switches_access_without_starvation() {
    let mut peer = Peer::new().await;
    let target = peer.target(1);
    let online = Arc::new(OnlineUsers::new(Some(PostgresConfig {
        targets: vec![target.clone()],
    })));
    let database = Arc::new(online.database(Some(target)));
    assert!(Arc::ptr_eq(&online.slots[0], &database.primary.slots[0]));
    let count = {
        let online = Arc::clone(&online);
        tokio::spawn(async move { online.count_at(UNIX_EPOCH).await })
    };
    let one = peer.query().await;
    assert!(one.read_only);
    let auth = query(&database, true);
    peer.no_query().await;
    one.release.send(()).unwrap();
    assert_eq!(count.await.unwrap(), Ok(0.0));
    let two = peer.query().await;
    assert_eq!(two.session, one.session);
    two.release.send(()).unwrap();
    assert!(auth.await.unwrap().is_ok());
    let write = query(&database, false);
    let three = peer.query().await;
    assert!(!three.read_only);
    assert_ne!(three.session, two.session);
    three.release.send(()).unwrap();
    assert!(write.await.unwrap().is_ok());
    database.shutdown().await;
}

#[tokio::test]
async fn checkout_timeout_and_aborted_waiter_do_not_send_sql_or_leak_capacity() {
    let mut peer = Peer::new().await;
    let mut target = peer.target(1);
    target.connect_timeout_ms = Some(50);
    let database = Arc::new(Database::new(None, Some(target)));
    let first = query(&database, false);
    let one = peer.query().await;
    let aborted = query(&database, false);
    tokio::task::yield_now().await;
    aborted.abort();
    assert!(aborted.await.unwrap_err().is_cancelled());
    let timeout = query(&database, false).await.unwrap().unwrap_err();
    assert!(matches!(timeout, QueryError::CheckoutTimeout));
    assert!(timeout.legacy_error().is_none());
    assert_eq!(peer.cancelled.load(Ordering::Acquire), 0);
    assert_eq!(peer.accepted.load(Ordering::Acquire), 1);
    one.release.send(()).unwrap();
    assert!(first.await.unwrap().is_ok());
    let later = query(&database, false);
    let two = peer.query().await;
    assert_eq!(two.session, one.session);
    two.release.send(()).unwrap();
    assert!(later.await.unwrap().is_ok());
    database.shutdown().await;
}

#[tokio::test]
async fn cancelling_one_query_keeps_another_lease_live_and_shutdown_rejects_waiters() {
    let mut peer = Peer::new().await;
    let database = Arc::new(Database::new(None, Some(peer.target(2))));
    let first = query(&database, false);
    let one = peer.query().await;
    let second = query(&database, false);
    let two = peer.query().await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    drop(one);
    let third = query(&database, false);
    let three = peer.query().await;
    assert_ne!(three.session, two.session);
    assert_eq!(peer.cancelled.load(Ordering::Acquire), 1);
    two.release.send(()).unwrap();
    assert!(second.await.unwrap().is_ok());
    let fourth = query(&database, false);
    let four = peer.query().await;
    let waiting = query(&database, false);
    peer.no_query().await;
    database.shutdown().await;
    for task in [third, fourth, waiting] {
        assert!(matches!(task.await.unwrap(), Err(QueryError::Unavailable)));
    }
    drop((three, four));
    assert!(matches!(
        database.query_typed(false, "SELECT 1", &[]).await,
        Err(QueryError::Unavailable)
    ));
    assert_eq!(database.primary.slots[0].idle_count(), 0);
}

#[tokio::test]
async fn max_uses_retires_on_release_and_lifetime_does_not_interrupt_a_lease() {
    let mut peer = Peer::new().await;
    let mut target = peer.target(1);
    target.pool.max_uses = Some(2);
    let database = Arc::new(Database::new(None, Some(target)));
    for expected in [0, 0, 1] {
        let pending = query(&database, false);
        let live = peer.query().await;
        assert_eq!(live.session, expected);
        live.release.send(()).unwrap();
        assert!(pending.await.unwrap().is_ok());
    }
    database.shutdown().await;

    let mut target = peer.target(1);
    target.pool.max_lifetime_ms = Some(25);
    let database = Arc::new(Database::new(None, Some(target)));
    let pending = query(&database, false);
    let live = peer.query().await;
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(!pending.is_finished());
    live.release.send(()).unwrap();
    assert!(pending.await.unwrap().is_ok());
    assert_eq!(database.primary.slots[0].idle_count(), 0);
    database.shutdown().await;
}

#[tokio::test]
async fn idle_eviction_respects_min_without_prewarming() {
    let mut peer = Peer::new().await;
    let mut target = peer.target(2);
    target.pool.min = 1;
    target.pool.idle_timeout_ms = Some(25);
    let database = Arc::new(Database::new(None, Some(target)));
    assert_eq!(peer.accepted.load(Ordering::Acquire), 0);
    let first = query(&database, false);
    let second = query(&database, false);
    let one = peer.query().await;
    let two = peer.query().await;
    one.release.send(()).unwrap();
    two.release.send(()).unwrap();
    assert!(first.await.unwrap().is_ok());
    assert!(second.await.unwrap().is_ok());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(database.primary.slots[0].idle_count(), 1);
    assert_eq!(peer.accepted.load(Ordering::Acquire), 2);
    database.shutdown().await;
}

#[tokio::test]
async fn old_idle_timer_cannot_evict_a_recently_returned_session() {
    let mut peer = Peer::new().await;
    let mut target = peer.target(1);
    target.pool.idle_timeout_ms = Some(100);
    let database = Arc::new(Database::new(None, Some(target)));
    for delay in [60, 60] {
        let pending = query(&database, false);
        let live = peer.query().await;
        assert_eq!(live.session, 0);
        live.release.send(()).unwrap();
        assert!(pending.await.unwrap().is_ok());
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }
    assert_eq!(database.primary.slots[0].idle_count(), 1);
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(database.primary.slots[0].idle_count(), 0);
    database.shutdown().await;
}

#[tokio::test]
async fn completed_server_error_returns_the_healthy_session_without_cancellation() {
    let mut peer = Peer::new().await;
    let database = Arc::new(Database::new(None, Some(peer.target(1))));
    let rejected = query_sql(&database, false, "SELECT fixture_error");
    let one = peer.query().await;
    one.release.send(()).unwrap();
    assert!(matches!(
        rejected.await.unwrap(),
        Err(QueryError::Server { .. })
    ));
    let pending = query(&database, false);
    let two = peer.query().await;
    assert_eq!(two.session, one.session);
    two.release.send(()).unwrap();
    assert!(pending.await.unwrap().is_ok());
    assert_eq!(peer.cancelled.load(Ordering::Acquire), 0);
    database.shutdown().await;
}

#[tokio::test]
async fn query_timer_starts_after_checkout_even_when_the_wait_exceeds_its_limit() {
    let mut peer = Peer::new().await;
    let mut target = peer.target(1);
    target.query_timeout_ms = Some(100);
    let database = Arc::new(Database::new(None, Some(target)));
    let capacity = database.primary.slots[0].hold_capacity();
    let pending = query(&database, false);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!pending.is_finished());
    assert_eq!(peer.accepted.load(Ordering::Acquire), 0);
    drop(capacity);
    peer.query().await.release.send(()).unwrap();
    assert!(pending.await.unwrap().is_ok());
    database.shutdown().await;
}

#[tokio::test]
async fn duplicate_replica_addresses_keep_distinct_source_budgets() {
    let mut peer = Peer::new().await;
    let mut first = peer.target(1);
    first.source = PoolSource::Replica { index: 0 };
    let mut second = first.clone();
    second.source = PoolSource::Replica { index: 1 };
    let online = OnlineUsers::new(Some(PostgresConfig {
        targets: vec![first, second],
    }));
    assert!(!Arc::ptr_eq(&online.slots[0], &online.slots[1]));
    let one = {
        let pool = Arc::clone(&online.slots[0]);
        tokio::spawn(async move { pool.acquire(true).await })
    };
    let two = {
        let pool = Arc::clone(&online.slots[1]);
        tokio::spawn(async move { pool.acquire(true).await })
    };
    let one = one.await.unwrap().unwrap();
    let two = two.await.unwrap().unwrap();
    assert_eq!(peer.accepted.load(Ordering::Acquire), 2);
    one.release();
    two.release();
    peer.no_query().await;
    online.shutdown().await;
}

#[tokio::test]
async fn selected_replica_saturation_never_checks_out_primary() {
    let mut replica = Peer::new().await;
    let primary = Peer::new().await;
    let mut read = replica.target(1);
    read.source = PoolSource::Replica { index: 0 };
    read.connect_timeout_ms = Some(50);
    let database = Arc::new(Database::new(
        Some(PostgresConfig {
            targets: vec![read],
        }),
        Some(primary.target(1)),
    ));
    let capacity = database.reads.slots[0].hold_capacity();
    assert!(matches!(
        query(&database, true).await.unwrap(),
        Err(QueryError::CheckoutTimeout)
    ));
    assert_eq!(replica.accepted.load(Ordering::Acquire), 0);
    assert_eq!(primary.accepted.load(Ordering::Acquire), 0);
    replica.no_query().await;
    drop(capacity);
    database.shutdown().await;
}

#[tokio::test]
async fn retirement_keeps_capacity_until_the_aborted_driver_future_is_dropped() {
    struct MarkDropped(Arc<AtomicBool>);
    impl Drop for MarkDropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let peer = Peer::new().await;
    let mut target = peer.target(1);
    target.pool.max_uses = Some(1);
    let database = Database::new(None, Some(target));
    let pool = &database.primary.slots[0];
    let mut lease = pool.acquire(false).await.unwrap();
    let dropped = Arc::new(AtomicBool::new(false));
    let marker = MarkDropped(Arc::clone(&dropped));
    let (entered, started) = oneshot::channel();
    let controlled = tokio::spawn(async move {
        let _marker = marker;
        entered.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    started.await.unwrap();
    let original = lease.connection_mut().driver.replace(controlled).unwrap();
    original.abort();
    let _ = original.await;
    lease.release();
    // On this current-thread runtime no disposal task has been polled yet.
    assert!(!dropped.load(Ordering::Acquire));
    assert_eq!(pool.total_count(), 1);
    let later = pool.acquire(false).await.unwrap();
    assert!(dropped.load(Ordering::Acquire));
    assert_eq!(pool.total_count(), 1);
    later.release();
    database.shutdown().await;
    assert_eq!(pool.total_count(), 0);
}

#[tokio::test]
async fn timer_tasks_are_owned_cancelled_on_checkout_and_drained_on_retirement() {
    let peer = Peer::new().await;
    let mut target = peer.target(1);
    target.pool.idle_timeout_ms = Some(60_000);
    target.pool.max_lifetime_ms = Some(60_000);
    let database = Database::new(None, Some(target));
    let pool = &database.primary.slots[0];
    for _ in 0..100 {
        let lease = pool.acquire(false).await.unwrap();
        assert_eq!(
            pool.active_timer_count(),
            1,
            "only the physical lifetime timer remains while borrowed"
        );
        lease.release();
        assert_eq!(
            pool.active_timer_count(),
            2,
            "one idle timer and one lifetime timer per physical connection"
        );
    }
    assert_eq!(peer.accepted.load(Ordering::Acquire), 1);
    database.shutdown().await;
    assert_eq!(pool.active_timer_count(), 0);
    assert_eq!(pool.total_count(), 0);
}

#[tokio::test]
async fn dropping_the_pool_owner_aborts_owned_timer_tasks() {
    let peer = Peer::new().await;
    let mut target = peer.target(1);
    target.pool.idle_timeout_ms = Some(60_000);
    target.pool.max_lifetime_ms = Some(60_000);
    let database = Database::new(None, Some(target));
    let lease = database.primary.slots[0].acquire(false).await.unwrap();
    lease.release();
    let active = database.primary.slots[0].timer_counter();
    assert_eq!(active.load(Ordering::Acquire), 2);
    drop(database);
    tokio::time::timeout(Duration::from_secs(1), async {
        while active.load(Ordering::Acquire) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn unchanged_v1_config_keeps_one_count_connection_and_separate_explicit_primary() {
    let raw = include_str!("../tests/fixtures/local.json");
    assert!(!raw.contains("\"source\""));
    assert!(!raw.contains("\"pool\""));
    let mut config: PostgresConfig = serde_json::from_str(raw).unwrap();
    assert!(matches!(config.targets[0].source, PoolSource::Unspecified));
    assert_eq!(config.targets[0].pool.max, 1);
    assert!(matches!(
        config.targets[0].pool.held,
        Some(PoolSetting::Legacy)
    ));
    let mut peer = Peer::new().await;
    config.targets[0].port = peer.port;
    let primary = config.targets[0].clone();
    let online = Arc::new(OnlineUsers::new(Some(config)));
    let mut pending = Vec::new();
    for _ in 0..16 {
        let online = Arc::clone(&online);
        pending.push(tokio::spawn(
            async move { online.count_at(UNIX_EPOCH).await },
        ));
    }
    for _ in 0..16 {
        let count = peer.query().await;
        assert_eq!(count.session, 0);
        assert!(count.read_only);
        count.release.send(()).unwrap();
    }
    for result in pending {
        assert_eq!(result.await.unwrap(), Ok(0.0));
    }
    assert_eq!(peer.accepted.load(Ordering::Acquire), 1);
    let database = Arc::new(online.database(Some(primary)));
    assert!(!Arc::ptr_eq(&online.slots[0], &database.primary.slots[0]));
    let write = query(&database, false);
    let writable = peer.query().await;
    assert_eq!(writable.session, 1);
    assert!(!writable.read_only);
    writable.release.send(()).unwrap();
    assert!(write.await.unwrap().is_ok());
    database.shutdown().await;
    online.shutdown().await;
}

#[tokio::test]
async fn unspecified_legacy_source_never_redirects_explicit_primary_to_a_replica() {
    let mut replica = Peer::new().await;
    let mut primary = Peer::new().await;
    let mut config: PostgresConfig =
        serde_json::from_str(include_str!("../tests/fixtures/local.json")).unwrap();
    let mut primary_target = config.targets[0].clone();
    primary_target.port = primary.port;
    config.targets[0].port = replica.port;
    let online = OnlineUsers::new(Some(config));
    let database = Arc::new(online.database(Some(primary_target)));
    let read = query(&database, true);
    let readonly = replica.query().await;
    assert!(readonly.read_only);
    assert_eq!(primary.accepted.load(Ordering::Acquire), 0);
    readonly.release.send(()).unwrap();
    assert!(read.await.unwrap().is_ok());
    let write = query(&database, false);
    let writable = primary.query().await;
    assert!(!writable.read_only);
    assert_eq!(replica.accepted.load(Ordering::Acquire), 1);
    writable.release.send(()).unwrap();
    assert!(write.await.unwrap().is_ok());
    database.shutdown().await;
}

#[tokio::test]
async fn legacy_and_held_count_waits_keep_the_operation_bound_instead_of_connect_deadline() {
    for hold in [PoolSetting::Legacy, PoolSetting::Max] {
        let mut peer = Peer::new().await;
        let mut target = peer.target(1);
        target.pool.held = Some(hold);
        target.connect_timeout_ms = Some(50);
        let online = Arc::new(OnlineUsers::new(Some(PostgresConfig {
            targets: vec![target],
        })));
        let capacity = online.slots[0].hold_capacity();
        let pending = {
            let online = Arc::clone(&online);
            tokio::spawn(async move { online.count_at(UNIX_EPOCH).await })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !pending.is_finished(),
            "legacy slot waiting must not consume the 50ms physical connect limit"
        );
        assert_eq!(peer.accepted.load(Ordering::Acquire), 0);
        drop(capacity);
        peer.query().await.release.send(()).unwrap();
        assert_eq!(pending.await.unwrap(), Ok(0.0));
        online.shutdown().await;
    }

    let peer = Peer::new().await;
    let mut target = peer.target(1);
    target.connect_timeout_ms = Some(50);
    let database = Arc::new(Database::new(None, Some(target)));
    let capacity = database.primary.slots[0].hold_capacity();
    assert!(matches!(
        query(&database, false).await.unwrap(),
        Err(QueryError::CheckoutTimeout)
    ));
    assert_eq!(peer.accepted.load(Ordering::Acquire), 0);
    drop(capacity);
    database.shutdown().await;
}
