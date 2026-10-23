//! The online-user operation and its bounded, read-only PostgreSQL resources.
//!
//! Configuration is normalized by the existing `pg` client without connecting,
//! then supplied over the private bootstrap pipe. Never format it or DB errors.
use misskey_backend_core::{online_users_count, online_users_cutoff};
use openssl::{
    pkey::PKey,
    ssl::{SslConnector, SslMethod, SslVerifyMode, SslVersion},
    x509::{X509, store::X509StoreBuilder},
};
use postgres_openssl::MakeTlsConnector;
use serde::Deserialize;
use std::{
    fmt,
    future::Future,
    sync::{
        Arc, Mutex as SyncMutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{Mutex, watch},
    task::{AbortHandle, JoinHandle},
    time::{Instant, timeout_at},
};
use tokio_postgres::{
    Client, Config, NoTls,
    config::{ChannelBinding, SslMode, SslNegotiation},
    tls::MakeTlsConnect,
};

const OPERATION_LIMIT: Duration = Duration::from_secs(4);

mod fault;
mod transport;
mod wire;
pub use fault::{
    ConnectStage, FaultKind, FaultPhase, IoDirection, IoOutcome, QueryFault, SslReply,
    TlsConnectState,
};
#[cfg(test)]
use transport::TransportContext;
use transport::{AttemptObservation, CancelRoute, ConnectContext, Resolver, SystemResolver};

/// Deliberately hides connection strings, SQL errors and TLS details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure;
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Rust backend unavailable")
    }
}
impl std::error::Error for Failure {}

/// Native business queries retain protocol messages and concrete fault sources
/// before the existing opaque API discards them. Only verified legacy messages
/// are exposed as such; Debug/Display never expose source diagnostics.
pub enum QueryError {
    Server { message: String },
    QueryTimeout,
    Fault(QueryFault),
    Unavailable,
}
impl QueryError {
    fn from_postgres(error: tokio_postgres::Error) -> Self {
        match error.as_db_error() {
            Some(error) => Self::Server {
                message: error.message().to_owned(),
            },
            None => Self::Fault(QueryFault::postgres(FaultPhase::Query, error)),
        }
    }

    pub fn legacy_message(&self) -> Option<&str> {
        match self {
            Self::Server { message } => Some(message),
            // node-postgres emits this exact Error.message for query_timeout;
            // TypeORM wraps the query's rejection as QueryFailedError.
            Self::QueryTimeout => Some("Query read timeout"),
            Self::Fault(_) | Self::Unavailable => None,
        }
    }
}
impl fmt::Debug for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Server { .. } => "QueryError::Server",
            Self::QueryTimeout => "QueryError::QueryTimeout",
            Self::Fault(_) => "QueryError::Fault",
            Self::Unavailable => "QueryError::Unavailable",
        })
    }
}
impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Rust backend unavailable")
    }
}
impl std::error::Error for QueryError {}
impl From<Failure> for QueryError {
    fn from(_: Failure) -> Self {
        Self::Unavailable
    }
}
impl From<QueryFault> for QueryError {
    fn from(fault: QueryFault) -> Self {
        Self::Fault(fault)
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostgresConfig {
    pub targets: Vec<Target>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub database: String,
    pub options: Option<String>,
    pub application_name: Option<String>,
    pub statement_timeout_ms: Option<u32>,
    pub lock_timeout_ms: Option<u32>,
    pub idle_in_transaction_session_timeout_ms: Option<u32>,
    pub connect_timeout_ms: Option<u32>,
    pub query_timeout_ms: Option<u32>,
    pub keep_alive: bool,
    pub keep_alive_initial_delay_ms: Option<u32>,
    pub channel_binding: bool,
    pub ssl_negotiation: Negotiation,
    pub tls: Option<Tls>,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Negotiation {
    Postgres,
    Direct,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Tls {
    pub reject_unauthorized: bool,
    pub ca: Vec<String>,
    pub cert: Option<String>,
    pub key: Option<String>,
    pub passphrase: Option<String>,
    pub servername: String,
    pub sni: bool,
    pub min_version: TlsVersion,
    pub max_version: TlsVersion,
    pub ciphers: String,
}
#[derive(Clone, Copy, Deserialize)]
pub enum TlsVersion {
    #[serde(rename = "TLSv1")]
    V1,
    #[serde(rename = "TLSv1.1")]
    V11,
    #[serde(rename = "TLSv1.2")]
    V12,
    #[serde(rename = "TLSv1.3")]
    V13,
}
impl TlsVersion {
    fn openssl(self) -> SslVersion {
        match self {
            Self::V1 => SslVersion::TLS1,
            Self::V11 => SslVersion::TLS1_1,
            Self::V12 => SslVersion::TLS1_2,
            Self::V13 => SslVersion::TLS1_3,
        }
    }
}

type Cancellations = Arc<SyncMutex<Vec<JoinHandle<()>>>>;
type DriverFault = Arc<SyncMutex<Option<Arc<tokio_postgres::Error>>>>;
struct Connection {
    client: Client,
    driver: JoinHandle<()>,
    driver_fault: DriverFault,
    transport_context: ConnectContext,
    cancel_route: CancelRoute,
    in_flight: AtomicBool,
    cancel_tls: Option<NamedConnector>,
    cancellations: Cancellations,
}
impl Connection {
    fn fault(&self, phase: FaultPhase, error: tokio_postgres::Error) -> QueryFault {
        if error.is_closed()
            && let Ok(fault) = self.driver_fault.lock()
            && let Some(error) = fault.as_ref()
        {
            return QueryFault::shared_postgres(phase, Arc::clone(error))
                .with_context(self.transport_context.snapshot());
        }
        QueryFault::postgres(phase, error).with_context(self.transport_context.snapshot())
    }

    #[cfg(test)]
    fn transport_context(&self) -> Option<TransportContext> {
        self.transport_context.snapshot()
    }
}

fn spawn_driver<F>(driver: F) -> (JoinHandle<()>, DriverFault)
where
    F: Future<Output = Result<(), tokio_postgres::Error>> + Send + 'static,
{
    let fault = Arc::new(SyncMutex::new(None));
    let capture = Arc::clone(&fault);
    let task = tokio::spawn(async move {
        // Keep the driver alive until the result is retained. Dropping it
        // releases pending response channels, which report only Closed.
        let mut driver = std::pin::pin!(driver);
        std::future::poll_fn(|cx| {
            // The driver's terminal poll also closes its request receiver.
            // Hold the source lock for that synchronous poll, so another
            // runtime thread observing Closed cannot outrun source capture.
            let mut capture = capture.lock().ok();
            match driver.as_mut().poll(cx) {
                std::task::Poll::Pending => std::task::Poll::Pending,
                std::task::Poll::Ready(result) => {
                    if let Err(error) = result
                        && let Some(capture) = capture.as_mut()
                    {
                        **capture = Some(Arc::new(error));
                    }
                    std::task::Poll::Ready(())
                }
            }
        })
        .await;
    });
    (task, fault)
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.driver.abort();
        if !self.in_flight.load(Ordering::Acquire) {
            return;
        }
        // Closing TCP does not interrupt a PostgreSQL lock wait. Send the
        // protocol cancellation capability, never a replacement SQL request.
        let token = self.client.cancel_token();
        let tls = self.cancel_tls.clone();
        let route = self.cancel_route.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let task = runtime.spawn(async move {
                let _ = tokio::time::timeout(Duration::from_millis(500), async {
                    match tls {
                        Some(tls) => {
                            let _ = route
                                .cancel(&token, |observation| tls.with_observation(observation))
                                .await;
                        }
                        None => {
                            let _ = route.cancel(&token, |_| NoTls).await;
                        }
                    }
                })
                .await;
            });
            if let Ok(mut pending) = self.cancellations.lock() {
                pending.retain(|task| !task.is_finished());
                pending.push(task);
            }
        }
    }
}
struct Slot {
    target: Target,
    connection: Mutex<Option<Connection>>,
    driver: SyncMutex<Option<AbortHandle>>,
}

/// One connection per configured read target, established only on first use.
/// A failed operation never retries. A later operation may connect again.
pub struct OnlineUsers {
    slots: Vec<Slot>,
    stopped: AtomicBool,
    stop_signal: watch::Sender<bool>,
    cancellations: Cancellations,
    read_only: bool,
    resolver: Arc<dyn Resolver>,
}
impl OnlineUsers {
    pub fn new(config: Option<PostgresConfig>) -> Self {
        Self::with_access(config, true)
    }

    fn with_access(config: Option<PostgresConfig>, read_only: bool) -> Self {
        Self::with_access_and_resolver(config, read_only, Arc::new(SystemResolver))
    }

    #[cfg(test)]
    pub(crate) fn new_with_resolver(
        config: Option<PostgresConfig>,
        resolver: Arc<dyn Resolver>,
    ) -> Self {
        Self::with_access_and_resolver(config, true, resolver)
    }

    fn with_access_and_resolver(
        config: Option<PostgresConfig>,
        read_only: bool,
        resolver: Arc<dyn Resolver>,
    ) -> Self {
        Self {
            slots: config.map_or_else(Vec::new, |c| {
                c.targets
                    .into_iter()
                    .map(|target| Slot {
                        target,
                        connection: Mutex::new(None),
                        driver: SyncMutex::new(None),
                    })
                    .collect()
            }),
            stopped: AtomicBool::new(false),
            stop_signal: watch::channel(false).0,
            cancellations: Arc::new(SyncMutex::new(Vec::new())),
            read_only,
            resolver,
        }
    }

    pub async fn count_at(&self, now: SystemTime) -> Result<f64, Failure> {
        // Subscribe before checking stopped, so shutdown cannot fall into the
        // interval between that check and registering a cancellation waiter.
        let mut stop = self.stop_signal.subscribe();
        if self.stopped.load(Ordering::Acquire) || self.slots.is_empty() {
            return Err(Failure);
        }
        // Equivalent to TypeORM's per-read random slave selection. No master
        // fallback is added when the selected slave fails.
        let slot = &self.slots[rand::random_range(0..self.slots.len())];
        let deadline = Instant::now() + OPERATION_LIMIT;
        let cutoff = system_time(online_users_cutoff(now).map_err(|_| Failure)?)?;
        let mut stored = timeout_at(deadline, slot.connection.lock())
            .await
            .map_err(|_| Failure)?;
        if self.stopped.load(Ordering::Acquire) {
            return Err(Failure);
        }
        // Ownership while in flight is intentional. Timeout, cancellation, SQL
        // failure or disconnect drops this resource and aborts its IO driver.
        let mut connection = stored.take();
        let operation = timeout_at(deadline, async {
            // Do not accumulate cancellation sockets when a peer is slow or
            // unavailable. Each previous cancellation has its own 500ms bound;
            // the wait remains inside this operation's total four seconds.
            loop {
                let pending = {
                    let mut tasks = self.cancellations.lock().map_err(|_| Failure)?;
                    tasks.retain(|task| !task.is_finished());
                    !tasks.is_empty()
                };
                if !pending {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            if connection.as_ref().is_none_or(|c| c.client.is_closed()) {
                connection = None;
                connection =
                    Some(connect(&slot.target, &self.cancellations, self.resolver.as_ref()).await?);
                *slot.driver.lock().map_err(|_| Failure)? =
                    connection.as_ref().map(|c| c.driver.abort_handle());
            }
            if self.stopped.load(Ordering::Acquire) {
                return Err(Failure);
            }
            let connection = connection.as_ref().ok_or(Failure)?;
            let query = async {
                connection.in_flight.store(true, Ordering::Release);
                let response = connection
                    .client
                    .query_one(
                        "SELECT COUNT(*) FROM \"user\" WHERE \"lastActiveDate\" > $1",
                        &[&cutoff],
                    )
                    .await;
                if response.is_ok()
                    || response
                        .as_ref()
                        .err()
                        .is_some_and(|error| error.as_db_error().is_some())
                {
                    connection.in_flight.store(false, Ordering::Release);
                }
                let row = response.map_err(|_| Failure)?;
                let count: i64 = row.try_get(0).map_err(|_| Failure)?;
                online_users_count(count).map_err(|_| Failure)
            };
            match positive_duration(slot.target.query_timeout_ms) {
                Some(limit) => tokio::time::timeout(limit, query)
                    .await
                    .map_err(|_| Failure)?,
                None => query.await,
            }
        });
        let result = tokio::select! {
            biased;
            _ = stop.changed() => return Err(Failure),
            result = operation => result.map_err(|_| Failure)?,
        };
        if result.is_ok() && !self.stopped.load(Ordering::Acquire) {
            *stored = connection;
        }
        result
    }

    pub async fn shutdown(&self) {
        self.stopped.store(true, Ordering::Release);
        self.stop_signal.send_replace(true);
        // Concurrent queries have a shorter-than-parent deadline, but shutdown
        // itself does not wait that long: abort anything still holding a slot.
        // The parent process also bounds its complete child shutdown.
        for slot in &self.slots {
            if let Ok(mut driver) = slot.driver.lock()
                && let Some(driver) = driver.take()
            {
                driver.abort();
            }
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        for slot in &self.slots {
            if let Ok(mut connection) = timeout_at(deadline, slot.connection.lock()).await {
                *connection = None;
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
}

/// API infrastructure uses the same TLS/configuration and cancellation ownership
/// as the original count adapter. Reads retain replica selection; writes always
/// use the explicitly configured primary, with no retry or read fallback.
pub struct Database {
    reads: OnlineUsers,
    primary: OnlineUsers,
}
impl Database {
    pub fn new(reads: Option<PostgresConfig>, primary: Option<Target>) -> Self {
        Self {
            reads: OnlineUsers::new(reads),
            primary: OnlineUsers::with_access(
                primary.map(|target| PostgresConfig {
                    targets: vec![target],
                }),
                false,
            ),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_with_resolver(
        reads: Option<PostgresConfig>,
        primary: Option<Target>,
        resolver: Arc<dyn Resolver>,
    ) -> Self {
        Self {
            reads: OnlineUsers::with_access_and_resolver(reads, true, Arc::clone(&resolver)),
            primary: OnlineUsers::with_access_and_resolver(
                primary.map(|target| PostgresConfig {
                    targets: vec![target],
                }),
                false,
                resolver,
            ),
        }
    }

    pub async fn query(
        &self,
        read: bool,
        sql: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Result<Vec<tokio_postgres::Row>, Failure> {
        self.query_typed(read, sql, params)
            .await
            .map_err(|_| Failure)
    }

    /// Adds business-error information without changing the opaque query API
    /// used by authentication, nor connection selection/cancellation behavior.
    pub async fn query_typed(
        &self,
        read: bool,
        sql: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Result<Vec<tokio_postgres::Row>, QueryError> {
        let pool = if read { &self.reads } else { &self.primary };
        let mut stop = pool.stop_signal.subscribe();
        if pool.stopped.load(Ordering::Acquire) || pool.slots.is_empty() {
            return Err(QueryError::Unavailable);
        }
        let slot = &pool.slots[rand::random_range(0..pool.slots.len())];
        // Connection checkout is interruptible by shutdown. No arbitrary
        // five-second RPC deadline is imposed on existing API operations.
        let operation = async {
            let mut stored = slot.connection.lock().await;
            if pool.stopped.load(Ordering::Acquire) {
                return Err(QueryError::Unavailable);
            }
            let mut connection = stored.take();
            if connection.as_ref().is_none_or(|c| c.client.is_closed()) {
                connection = Some(
                    connect_with_access(
                        &slot.target,
                        &pool.cancellations,
                        pool.read_only,
                        pool.resolver.as_ref(),
                    )
                    .await?,
                );
                *slot.driver.lock().map_err(|_| QueryError::Unavailable)? =
                    connection.as_ref().map(|c| c.driver.abort_handle());
            }
            let current = connection.as_ref().ok_or(QueryError::Unavailable)?;
            if pool.stopped.load(Ordering::Acquire) {
                return Err(QueryError::Unavailable);
            }
            current.in_flight.store(true, Ordering::Release);
            let execute = current.client.query(sql, params);
            let result = match positive_duration(slot.target.query_timeout_ms) {
                Some(limit) => tokio::time::timeout(limit, execute)
                    .await
                    .map_err(|_| QueryError::QueryTimeout)?,
                None => execute.await,
            };
            if result.is_ok()
                || result
                    .as_ref()
                    .err()
                    .is_some_and(|error| error.as_db_error().is_some())
            {
                current.in_flight.store(false, Ordering::Release);
            }
            let rows = result.map_err(|error| {
                if error.as_db_error().is_some() {
                    QueryError::from_postgres(error)
                } else {
                    current.fault(FaultPhase::Query, error).into()
                }
            })?;
            if !pool.stopped.load(Ordering::Acquire) {
                *stored = connection;
            }
            Ok(rows)
        };
        tokio::select! {
            biased;
            _ = stop.changed() => Err(QueryError::Unavailable),
            result = operation => result,
        }
    }

    pub async fn shutdown(&self) {
        tokio::join!(self.reads.shutdown(), self.primary.shutdown());
    }
}
fn system_time(millis: i64) -> Result<SystemTime, Failure> {
    let delta = Duration::from_millis(millis.unsigned_abs());
    if millis >= 0 {
        UNIX_EPOCH.checked_add(delta)
    } else {
        UNIX_EPOCH.checked_sub(delta)
    }
    .ok_or(Failure)
}
fn positive_duration(millis: Option<u32>) -> Option<Duration> {
    millis
        .filter(|ms| *ms > 0)
        .map(|ms| Duration::from_millis(ms.into()))
}

fn config(target: &Target) -> Result<Config, Failure> {
    if target.host.is_empty()
        || target.port == 0
        || target.user.is_empty()
        || target.database.is_empty()
    {
        return Err(Failure);
    }
    let mut config = Config::new();
    config
        .host(&target.host)
        .port(target.port)
        .user(&target.user)
        .dbname(&target.database);
    if let Some(password) = &target.password {
        config.password(password);
    }
    if let Some(name) = &target.application_name {
        config.application_name(name);
    }
    if let Some(limit) = positive_duration(target.connect_timeout_ms) {
        config.connect_timeout(limit);
    }
    config.keepalives(target.keep_alive);
    if let Some(ms) = target.keep_alive_initial_delay_ms {
        config.keepalives_idle(Duration::from_millis(ms.into()));
    }
    config.channel_binding(if target.channel_binding {
        ChannelBinding::Prefer
    } else {
        ChannelBinding::Disable
    });
    let mut options = target.options.clone().unwrap_or_default();
    for (name, value) in [
        ("statement_timeout", target.statement_timeout_ms),
        ("lock_timeout", target.lock_timeout_ms),
        (
            "idle_in_transaction_session_timeout",
            target.idle_in_transaction_session_timeout_ms,
        ),
    ] {
        if let Some(ms) = value {
            options.push_str(&format!(" -c {name}={ms}"));
        }
    }
    if !options.is_empty() {
        config.options(options);
    }
    config.ssl_mode(if target.tls.is_some() {
        SslMode::Require
    } else {
        SslMode::Disable
    });
    config.ssl_negotiation(match target.ssl_negotiation {
        Negotiation::Postgres => SslNegotiation::Postgres,
        Negotiation::Direct => SslNegotiation::Direct,
    });
    if target.tls.is_none() && matches!(target.ssl_negotiation, Negotiation::Direct) {
        return Err(Failure);
    }
    Ok(config)
}
async fn connect(
    target: &Target,
    cancellations: &Cancellations,
    resolver: &dyn Resolver,
) -> Result<Connection, Failure> {
    connect_with_access(target, cancellations, true, resolver)
        .await
        .map_err(|_| Failure)
}
async fn connect_with_access(
    target: &Target,
    cancellations: &Cancellations,
    read_only: bool,
    resolver: &dyn Resolver,
) -> Result<Connection, QueryFault> {
    let config = config(target).map_err(QueryFault::configuration)?;
    let context = ConnectContext::default();
    let establish = async {
        if let Some(tls) = &target.tls {
            let connector = tls_connector(tls, target.ssl_negotiation)?;
            let transport::Connected {
                client,
                driver,
                cancel_route,
            } = transport::connect(&config, resolver, &context, |observation| {
                connector.with_observation(observation)
            })
            .await?;
            let (driver, driver_fault) = spawn_driver(driver);
            Ok(Connection {
                client,
                driver,
                driver_fault,
                transport_context: context.clone(),
                cancel_route,
                in_flight: AtomicBool::new(false),
                cancel_tls: Some(connector),
                cancellations: Arc::clone(cancellations),
            })
        } else {
            let transport::Connected {
                client,
                driver,
                cancel_route,
            } = transport::connect(&config, resolver, &context, |_| NoTls).await?;
            let (driver, driver_fault) = spawn_driver(driver);
            Ok(Connection {
                client,
                driver,
                driver_fault,
                transport_context: context.clone(),
                cancel_route,
                in_flight: AtomicBool::new(false),
                cancel_tls: None,
                cancellations: Arc::clone(cancellations),
            })
        }
    };
    let connection = match positive_duration(target.connect_timeout_ms) {
        Some(limit) => tokio::time::timeout(limit, establish)
            .await
            .map_err(|_| QueryFault::connect_timeout().with_context(context.snapshot()))??,
        None => establish.await?,
    };
    // Session-scoped and applied before the first business query. This does
    // not create roles, grant privileges, or change the database schema.
    if read_only {
        connection
            .client
            .batch_execute("SET default_transaction_read_only = on")
            .await
            .map_err(|error| connection.fault(FaultPhase::SessionSetup, error))?;
    }
    Ok(connection)
}

#[derive(Clone)]
struct NamedConnector {
    inner: MakeTlsConnector,
    servername: String,
    verify: bool,
    sni: bool,
    observe_verify: bool,
    observation: Option<AttemptObservation>,
}
impl NamedConnector {
    fn with_observation(&self, observation: Option<AttemptObservation>) -> Self {
        Self {
            observation,
            ..self.clone()
        }
    }
}
impl<S> MakeTlsConnect<S> for NamedConnector
where
    MakeTlsConnector: MakeTlsConnect<S>,
{
    type Stream = <MakeTlsConnector as MakeTlsConnect<S>>::Stream;
    type TlsConnect = wire::ObservedTls<<MakeTlsConnector as MakeTlsConnect<S>>::TlsConnect>;
    type Error = <MakeTlsConnector as MakeTlsConnect<S>>::Error;
    fn make_tls_connect(&mut self, _: &str) -> Result<Self::TlsConnect, Self::Error> {
        // Each establishment attempt supplies its own observer. Cancellation
        // uses the original connector and gets fresh, unrelated state.
        let observation = self
            .observation
            .clone()
            .unwrap_or_else(AttemptObservation::new);
        observation.tls_factory(&self.servername);
        let mut inner = self.inner.clone();
        let (verify, sni, observe_verify) = (self.verify, self.sni, self.observe_verify);
        let verify_observation = observation.clone();
        inner.set_callback(move |config, _| {
            config.set_verify_hostname(verify);
            config.set_use_server_name_indication(sni);
            if verify && observe_verify {
                let observation = verify_observation.clone();
                config.set_verify_callback(SslVerifyMode::PEER, move |preverify_ok, context| {
                    observation.verify(preverify_ok, context);
                    preverify_ok
                });
            }
            Ok(())
        });
        let tls = <MakeTlsConnector as MakeTlsConnect<S>>::make_tls_connect(
            &mut inner,
            &self.servername,
        )?;
        Ok(wire::ObservedTls::new(tls, observation))
    }
}
fn tls_connector(tls: &Tls, negotiation: Negotiation) -> Result<NamedConnector, QueryFault> {
    let mut builder =
        SslConnector::builder(SslMethod::tls()).map_err(QueryFault::tls_configuration)?;
    // Replace, rather than extend, OpenSSL roots: explicit pg `ca` replaces
    // Node's defaults, while the TS normalizer supplies actual defaults otherwise.
    let mut store = X509StoreBuilder::new().map_err(QueryFault::tls_configuration)?;
    for pem in &tls.ca {
        for cert in X509::stack_from_pem(pem.as_bytes()).map_err(QueryFault::tls_configuration)? {
            store
                .add_cert(cert)
                .map_err(QueryFault::tls_configuration)?;
        }
    }
    builder.set_cert_store(store.build());
    builder.set_verify(if tls.reject_unauthorized {
        SslVerifyMode::PEER
    } else {
        SslVerifyMode::NONE
    });
    builder
        .set_min_proto_version(Some(tls.min_version.openssl()))
        .map_err(QueryFault::tls_configuration)?;
    builder
        .set_max_proto_version(Some(tls.max_version.openssl()))
        .map_err(QueryFault::tls_configuration)?;
    let (modern, legacy): (Vec<_>, Vec<_>) = tls
        .ciphers
        .split(':')
        .partition(|cipher| cipher.starts_with("TLS_"));
    // Node leaves TLS1.3 defaults in place when its explicit cipher string
    // contains no TLS1.3 names; only a nonempty list replaces those defaults.
    if !modern.is_empty() {
        builder
            .set_ciphersuites(&modern.join(":"))
            .map_err(QueryFault::tls_configuration)?;
    }
    if matches!(negotiation, Negotiation::Direct) {
        builder
            .set_alpn_protos(b"\x0apostgresql")
            .map_err(QueryFault::tls_configuration)?;
    }
    // An empty legacy list is valid for TLS1.3-only configurations. OpenSSL
    // returns an error for it, so disable older protocol versions explicitly.
    if legacy.is_empty() {
        builder
            .set_min_proto_version(Some(SslVersion::TLS1_3))
            .map_err(QueryFault::tls_configuration)?;
    } else {
        builder
            .set_cipher_list(&legacy.join(":"))
            .map_err(QueryFault::tls_configuration)?;
    }
    if let Some(pem) = &tls.cert {
        let mut certificates = X509::stack_from_pem(pem.as_bytes())
            .map_err(QueryFault::tls_configuration)?
            .into_iter();
        let certificate = certificates
            .next()
            .ok_or_else(|| QueryFault::configuration(Failure))?;
        builder
            .set_certificate(&certificate)
            .map_err(QueryFault::tls_configuration)?;
        for cert in certificates {
            builder
                .add_extra_chain_cert(cert)
                .map_err(QueryFault::tls_configuration)?;
        }
    }
    if let Some(pem) = &tls.key {
        let key = if let Some(passphrase) = &tls.passphrase {
            PKey::private_key_from_pem_passphrase(pem.as_bytes(), passphrase.as_bytes())
        } else {
            PKey::private_key_from_pem(pem.as_bytes())
        }
        .map_err(QueryFault::tls_configuration)?;
        builder
            .set_private_key(&key)
            .map_err(QueryFault::tls_configuration)?;
    }
    if tls.cert.is_some() != tls.key.is_some() {
        return Err(QueryFault::configuration(Failure));
    }
    if tls.cert.is_some() {
        builder
            .check_private_key()
            .map_err(QueryFault::tls_configuration)?;
    }
    Ok(NamedConnector {
        inner: MakeTlsConnector::new(builder.build()),
        servername: tls.servername.clone(),
        verify: tls.reject_unauthorized,
        sni: tls.sni,
        observe_verify: true,
        observation: None,
    })
}

#[cfg(test)]
fn tls_connector_without_observation(
    tls: &Tls,
    negotiation: Negotiation,
) -> Result<NamedConnector, QueryFault> {
    let mut connector = tls_connector(tls, negotiation)?;
    connector.observe_verify = false;
    Ok(connector)
}

#[cfg(test)]
mod tests;
