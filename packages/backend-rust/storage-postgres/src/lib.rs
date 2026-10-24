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
    sync::watch,
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
mod identity;
mod legacy;
mod pool;
pub use legacy::LegacyError;
mod resolver;
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
    CheckoutTimeout,
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

    /// Explicit public business projection; opaque auth/count callers stay opaque.
    pub fn legacy_error(&self) -> Option<LegacyError> {
        legacy::query_error(self)
    }

    pub fn legacy_message(&self) -> Option<&str> {
        match self {
            Self::Server { message } => Some(message),
            // node-postgres emits this exact Error.message for query_timeout;
            // TypeORM wraps the query's rejection as QueryFailedError.
            Self::QueryTimeout => Some("Query read timeout"),
            Self::CheckoutTimeout | Self::Fault(_) | Self::Unavailable => None,
        }
    }
}
impl fmt::Debug for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Server { .. } => "QueryError::Server",
            Self::QueryTimeout => "QueryError::QueryTimeout",
            Self::CheckoutTimeout => "QueryError::CheckoutTimeout",
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

#[derive(Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum PoolSource {
    #[default]
    Unspecified,
    Primary,
    Replica {
        index: usize,
    },
}

/// Canonical pg-pool comparison/timer semantics, resolved without connecting.
/// A held exotic configuration retains the original single-connection adapter.
#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PoolSettings {
    pub max: usize,
    pub min: usize,
    pub idle_timeout_ms: Option<u32>,
    pub max_uses: Option<u64>,
    pub max_lifetime_ms: Option<u32>,
    pub held: Option<PoolSetting>,
}
#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PoolSetting {
    Legacy,
    Max,
    Min,
    IdleTimeoutMillis,
    MaxUses,
    MaxLifetimeSeconds,
}
#[derive(Clone, Copy)]
enum CheckoutPolicy {
    LegacySlot,
    PgPool,
}
impl Default for PoolSettings {
    fn default() -> Self {
        Self {
            max: 1,
            min: 0,
            idle_timeout_ms: None,
            max_uses: None,
            max_lifetime_ms: None,
            held: Some(PoolSetting::Legacy),
        }
    }
}
impl PoolSettings {
    fn checkout_policy(&self) -> CheckoutPolicy {
        match self.held {
            Some(_) => CheckoutPolicy::LegacySlot,
            None => CheckoutPolicy::PgPool,
        }
    }

    fn valid(&self) -> bool {
        self.max > 0
            && self.min <= self.max
            && self.max_uses != Some(0)
            && self.idle_timeout_ms != Some(0)
            && self.max_lifetime_ms != Some(0)
            && (self.held.is_none()
                || (self.max == 1
                    && self.min == 0
                    && self.idle_timeout_ms.is_none()
                    && self.max_uses.is_none()
                    && self.max_lifetime_ms.is_none()))
    }
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Target {
    #[serde(default)]
    pub source: PoolSource,
    #[serde(default)]
    pub pool: PoolSettings,
    pub host: String,
    pub port: u16,
    #[serde(default = "default_auto_select_family")]
    pub auto_select_family: bool,
    #[serde(default = "default_attempt_timeout")]
    pub auto_select_family_attempt_timeout_ms: u32,
    #[serde(default)]
    pub dns_result_order: DnsResultOrder,
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
fn default_auto_select_family() -> bool {
    true
}
fn default_attempt_timeout() -> u32 {
    500
}

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DnsResultOrder {
    #[default]
    Verbatim,
    Ipv4first,
    Ipv6first,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Negotiation {
    Postgres,
    Direct,
}
#[derive(Clone, Deserialize, PartialEq, Eq)]
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
#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
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
    driver: Option<JoinHandle<()>>,
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
impl Connection {
    fn start_cancellation(&self) -> Option<JoinHandle<()>> {
        if !self.in_flight.swap(false, Ordering::AcqRel) {
            return None;
        }
        let token = self.client.cancel_token();
        let tls = self.cancel_tls.clone();
        let route = self.cancel_route.clone();
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        Some(runtime.spawn(async move {
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
        }))
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(driver) = self.driver.take() {
            driver.abort();
        }
        if let Some(task) = self.start_cancellation()
            && let Ok(mut pending) = self.cancellations.lock()
        {
            pending.retain(|task| !task.is_finished());
            pending.push(task);
        }
    }
}

/// Complete count operation over shared source pools. A failed SQL operation
/// is never retried; a later independent operation may establish a connection.
pub struct OnlineUsers {
    slots: Vec<Arc<pool::TargetPool>>,
    read_only: bool,
    resolver: Arc<dyn Resolver>,
}
impl OnlineUsers {
    pub fn new(config: Option<PostgresConfig>) -> Self {
        Self::with_access_and_resolver(config, true, Arc::new(SystemResolver))
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
            slots: config.map_or_else(Vec::new, |config| {
                config
                    .targets
                    .into_iter()
                    .map(|target| pool::TargetPool::new(target, Arc::clone(&resolver)))
                    .collect()
            }),
            read_only,
            resolver,
        }
    }

    /// Count, authentication and native operations use the same source budget.
    /// With no replicas, read-only and writable sessions share primary capacity.
    pub fn database(&self, primary: Option<Target>) -> Database {
        let primary = primary.map(|mut target| {
            if let Some(existing) = self
                .slots
                .iter()
                // Legacy v1 descriptors carry no source identity. They may
                // describe replicas, even when their credentials match the
                // primary. Only explicit primary identity permits sharing.
                .find(|pool| {
                    target.source == PoolSource::Primary
                        && pool.target.source == PoolSource::Primary
                })
            {
                if existing.target == target {
                    return Arc::clone(existing);
                }
                // Conflicting descriptors must not create a second source budget.
                target.pool.max = 0;
            }
            pool::TargetPool::new(target, Arc::clone(&self.resolver))
        });
        Database {
            reads: Self {
                slots: self.slots.clone(),
                read_only: true,
                resolver: Arc::clone(&self.resolver),
            },
            primary: Self {
                slots: primary.into_iter().collect(),
                read_only: false,
                resolver: Arc::clone(&self.resolver),
            },
        }
    }

    pub async fn count_at(&self, now: SystemTime) -> Result<f64, Failure> {
        let deadline = Instant::now() + OPERATION_LIMIT;
        let cutoff = system_time(online_users_cutoff(now).map_err(|_| Failure)?)?;
        let rows = timeout_at(
            deadline,
            self.query_typed(
                "SELECT COUNT(*) FROM \"user\" WHERE \"lastActiveDate\" > $1",
                &[&cutoff],
            ),
        )
        .await
        .map_err(|_| Failure)?
        .map_err(|_| Failure)?;
        let count: i64 = rows
            .first()
            .ok_or(Failure)?
            .try_get(0)
            .map_err(|_| Failure)?;
        online_users_count(count).map_err(|_| Failure)
    }

    async fn query_typed(
        &self,
        sql: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Result<Vec<tokio_postgres::Row>, QueryError> {
        if self.slots.is_empty() {
            return Err(QueryError::Unavailable);
        }
        // Select once, before waiting. Saturation or failure never selects again.
        let pool = &self.slots[rand::random_range(0..self.slots.len())];
        let mut stop = pool.subscribe();
        if pool.is_stopped() {
            return Err(QueryError::Unavailable);
        }
        let operation = async {
            let lease = pool.acquire(self.read_only).await?;
            let current = lease.connection();
            current.in_flight.store(true, Ordering::Release);
            let execute = current.client.query(sql, params);
            let result = match positive_duration(pool.target.query_timeout_ms) {
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
            match result {
                Ok(rows) => {
                    lease.release();
                    Ok(rows)
                }
                Err(error) if error.as_db_error().is_some() => {
                    let error = QueryError::from_postgres(error);
                    // ReadyForQuery completed the operation. A SQL rejection
                    // does not poison or cancel this healthy pool session.
                    lease.release();
                    Err(error)
                }
                Err(error) => Err(current.fault(FaultPhase::Query, error).into()),
            }
        };
        tokio::select! {
            biased;
            _ = stop.changed() => Err(QueryError::Unavailable),
            result = operation => result,
        }
    }

    pub async fn shutdown(&self) {
        // Every source begins stopping together; disposal has one shared bound.
        let futures = self.slots.iter().map(|pool| pool.shutdown());
        let mut tasks = Vec::new();
        for future in futures {
            tasks.push(Box::pin(future));
        }
        std::future::poll_fn(|cx| {
            tasks.retain_mut(|task| task.as_mut().poll(cx).is_pending());
            if tasks.is_empty() {
                std::task::Poll::Ready(())
            } else {
                std::task::Poll::Pending
            }
        })
        .await;
    }
}

/// Shared PostgreSQL resources; read routing and explicit-primary operations
/// retain their existing call sites and opaque authentication error boundary.
pub struct Database {
    reads: OnlineUsers,
    primary: OnlineUsers,
}
impl Database {
    pub fn new(reads: Option<PostgresConfig>, primary: Option<Target>) -> Self {
        OnlineUsers::new(reads).database(primary)
    }

    #[cfg(test)]
    pub(crate) fn new_with_resolver(
        reads: Option<PostgresConfig>,
        primary: Option<Target>,
        resolver: Arc<dyn Resolver>,
    ) -> Self {
        OnlineUsers::with_access_and_resolver(reads, true, resolver).database(primary)
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

    pub async fn query_typed(
        &self,
        read: bool,
        sql: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Result<Vec<tokio_postgres::Row>, QueryError> {
        let pool = if read { &self.reads } else { &self.primary };
        pool.query_typed(sql, params).await
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
#[cfg(test)]
async fn connect_with_access(
    target: &Target,
    cancellations: &Cancellations,
    read_only: bool,
    resolver: &dyn Resolver,
) -> Result<Connection, QueryFault> {
    let connection = establish_connection(target, cancellations, resolver).await?;
    if read_only {
        set_read_only(&connection).await?;
    }
    Ok(connection)
}

async fn establish_connection(
    target: &Target,
    cancellations: &Cancellations,
    resolver: &dyn Resolver,
) -> Result<Connection, QueryFault> {
    let config = config(target).map_err(QueryFault::configuration)?;
    let context = ConnectContext::for_target(target);
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
                driver: Some(driver),
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
                driver: Some(driver),
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
    Ok(connection)
}

async fn set_read_only(connection: &Connection) -> Result<(), QueryFault> {
    connection
        .client
        .batch_execute("SET default_transaction_read_only = on")
        .await
        .map_err(|error| connection.fault(FaultPhase::SessionSetup, error))
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
        // Bun verifies UTS #46 names and zone-free IP identities while sending
        // the original servername bytes as SNI. Keep those two roles separate.
        let original_name = self
            .servername
            .strip_suffix('.')
            .unwrap_or(&self.servername);
        let verify_name =
            idna::domain_to_ascii(original_name).unwrap_or_else(|_| original_name.to_owned());
        let verify_ip = identity::typed_ip(original_name);
        inner.set_callback(move |config, _| {
            config.set_verify_hostname(false);
            config.set_use_server_name_indication(sni);
            if verify {
                config
                    .param_mut()
                    .set_hostflags(openssl::x509::verify::X509CheckFlags::NO_PARTIAL_WILDCARDS);
                if let Some(ip) = verify_ip {
                    config.param_mut().set_ip(ip)?;
                } else {
                    config.param_mut().set_host(&verify_name)?;
                }
            }
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
mod pool_tests;
#[cfg(test)]
mod tests;
