//! Original resolution, TCP sockets and observations. PostgreSQL TLS negotiation,
//! startup, authentication and cancellation packets remain in tokio-postgres.

use crate::{
    ConnectStage, FaultPhase, IoDirection, IoOutcome, QueryFault, SslReply, TlsConnectState,
    wire::ObservedStream,
};
use openssl::x509::{X509, X509StoreContextRef};
use socket2::{SockRef, TcpKeepalive};
use std::{
    fmt,
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::net::TcpStream;
use tokio_postgres::{CancelToken, Client, Config, Socket, config::Host, tls::MakeTlsConnect};

pub(crate) type ResolveFuture<'a> =
    Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'a>>;

pub(crate) trait Resolver: Send + Sync {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> ResolveFuture<'a>;
}

pub(crate) struct SystemResolver;
impl Resolver for SystemResolver {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> ResolveFuture<'a> {
        Box::pin(async move { Ok(tokio::net::lookup_host((host, port)).await?.collect()) })
    }
}

#[derive(Clone)]
struct Verification {
    code: i32,
    depth: u32,
    preverify_ok: bool,
    // The actual certificate is private context for later compatibility work.
    // It is never formatted or exported as a public legacy message.
    _certificate: Option<X509>,
}

struct AttemptState {
    stage: ConnectStage,
    servername: Option<String>,
    verification: Option<Verification>,
    ssl_reply: Option<SslReply>,
    tls_connect: Option<TlsConnectState>,
    last_io: Option<IoObservation>,
    bytes_read: u64,
    bytes_written: u64,
}

#[derive(Clone, Copy)]
struct IoObservation {
    direction: IoDirection,
    outcome: IoOutcome,
    kind: Option<io::ErrorKind>,
    errno: Option<i32>,
}

#[derive(Clone)]
pub(crate) struct AttemptObservation(Arc<Mutex<AttemptState>>);
impl AttemptObservation {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(AttemptState {
            stage: ConnectStage::AddressAttempt,
            servername: None,
            verification: None,
            ssl_reply: None,
            tls_connect: None,
            last_io: None,
            bytes_read: 0,
            bytes_written: 0,
        })))
    }

    pub(crate) fn tls_factory(&self, servername: &str) {
        if let Ok(mut state) = self.0.lock() {
            state.stage = ConnectStage::TlsFactory;
            state.servername = Some(servername.to_owned());
        }
    }

    pub(crate) fn verify(&self, preverify_ok: bool, context: &X509StoreContextRef) {
        if let Ok(mut state) = self.0.lock() {
            state.stage = ConnectStage::TlsVerification;
            state.verification = Some(Verification {
                code: context.error().as_raw(),
                depth: context.error_depth(),
                preverify_ok,
                _certificate: context.current_cert().map(ToOwned::to_owned),
            });
        }
    }

    fn stage(&self, stage: ConnectStage) {
        if let Ok(mut state) = self.0.lock() {
            state.stage = stage;
        }
    }
    pub(crate) fn ssl_request_written(&self) {
        self.stage(ConnectStage::SslRequest);
    }
    pub(crate) fn ssl_reply(&self, reply: SslReply) {
        if let Ok(mut state) = self.0.lock() {
            state.ssl_reply = Some(reply);
        }
    }
    pub(crate) fn tls_connect_started(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.stage = ConnectStage::TlsConnectFuture;
            state.tls_connect = Some(TlsConnectState::Started);
        }
    }
    pub(crate) fn tls_connect_finished(&self, succeeded: bool) {
        if let Ok(mut state) = self.0.lock() {
            state.tls_connect = Some(if succeeded {
                TlsConnectState::Succeeded
            } else {
                TlsConnectState::Failed
            });
            if succeeded {
                state.stage = ConnectStage::TlsConnectComplete;
            }
        }
    }
    pub(crate) fn read(&self, count: usize, requested: bool) {
        if let Ok(mut state) = self.0.lock() {
            state.bytes_read = state.bytes_read.saturating_add(count as u64);
            if count == 0 && requested {
                state.last_io = Some(IoObservation {
                    direction: IoDirection::Read,
                    outcome: IoOutcome::Eof,
                    kind: None,
                    errno: None,
                });
            }
        }
    }
    pub(crate) fn written(&self, count: usize, requested: bool) {
        if let Ok(mut state) = self.0.lock() {
            state.bytes_written = state.bytes_written.saturating_add(count as u64);
            if count == 0 && requested {
                state.last_io = Some(IoObservation {
                    direction: IoDirection::Write,
                    outcome: IoOutcome::ZeroWrite,
                    kind: None,
                    errno: None,
                });
            }
        }
    }
    pub(crate) fn io_error(&self, direction: IoDirection, error: &io::Error) {
        if let Ok(mut state) = self.0.lock() {
            state.last_io = Some(IoObservation {
                direction,
                outcome: IoOutcome::Error,
                kind: Some(error.kind()),
                errno: error.raw_os_error(),
            });
        }
    }

    fn established(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.stage = ConnectStage::Established;
        }
    }
}

struct CurrentAttempt {
    index: usize,
    addr: SocketAddr,
    observation: AttemptObservation,
}
struct ContextState {
    host: Option<String>,
    address_count: Option<usize>,
    attempt: Option<CurrentAttempt>,
}

#[derive(Clone, Default)]
pub(crate) struct ConnectContext(Arc<Mutex<Option<ContextState>>>);
impl ConnectContext {
    #[cfg(test)]
    pub(crate) fn test_attempt(addr: SocketAddr) -> (Self, AttemptObservation) {
        let context = Self::default();
        context.resolved(1);
        let observation = context.attempt(0, addr);
        (context, observation)
    }

    fn resolving(&self, host: &str) {
        if let Ok(mut state) = self.0.lock() {
            *state = Some(ContextState {
                host: Some(host.to_owned()),
                address_count: None,
                attempt: None,
            });
        }
    }

    fn resolved(&self, count: usize) {
        if let Ok(mut state) = self.0.lock() {
            let state = state.get_or_insert_with(|| ContextState {
                host: None,
                address_count: None,
                attempt: None,
            });
            state.address_count = Some(count);
        }
    }

    fn attempt(&self, index: usize, addr: SocketAddr) -> AttemptObservation {
        let observation = AttemptObservation::new();
        if let Ok(mut state) = self.0.lock()
            && let Some(state) = state.as_mut()
        {
            state.attempt = Some(CurrentAttempt {
                index,
                addr,
                observation: observation.clone(),
            });
        }
        observation
    }

    pub(crate) fn snapshot(&self) -> Option<TransportContext> {
        let state = self.0.lock().ok()?;
        let state = state.as_ref()?;
        let mut snapshot = TransportContext {
            stage: ConnectStage::Resolve,
            host: state.host.clone(),
            address_count: state.address_count,
            attempt_index: None,
            attempted_addr: None,
            servername: None,
            verification: None,
            ssl_reply: None,
            tls_connect: None,
            last_io: None,
            bytes_read: 0,
            bytes_written: 0,
        };
        if let Some(attempt) = &state.attempt {
            snapshot.attempt_index = Some(attempt.index);
            snapshot.attempted_addr = Some(attempt.addr);
            if let Ok(attempt) = attempt.observation.0.lock() {
                snapshot.stage = attempt.stage;
                snapshot.servername = attempt.servername.clone();
                snapshot.verification = attempt.verification.clone();
                snapshot.ssl_reply = attempt.ssl_reply;
                snapshot.tls_connect = attempt.tls_connect;
                snapshot.last_io = attempt.last_io;
                snapshot.bytes_read = attempt.bytes_read;
                snapshot.bytes_written = attempt.bytes_written;
            }
        }
        Some(snapshot)
    }
}

/// Private source context; every field is an observation, never an inferred
/// Node error. Formatting deliberately omits hosts, addresses and certificates.
#[derive(Clone)]
pub(crate) struct TransportContext {
    stage: ConnectStage,
    host: Option<String>,
    address_count: Option<usize>,
    attempt_index: Option<usize>,
    attempted_addr: Option<SocketAddr>,
    servername: Option<String>,
    verification: Option<Verification>,
    ssl_reply: Option<SslReply>,
    tls_connect: Option<TlsConnectState>,
    last_io: Option<IoObservation>,
    bytes_read: u64,
    bytes_written: u64,
}
impl TransportContext {
    pub(crate) fn connect_stage(&self) -> ConnectStage {
        self.stage
    }
    pub(crate) fn resolver_host(&self) -> Option<&str> {
        self.host.as_deref()
    }
    pub(crate) fn resolver_address_count(&self) -> Option<usize> {
        self.address_count
    }
    pub(crate) fn attempt_index(&self) -> Option<usize> {
        self.attempt_index
    }
    pub(crate) fn attempted_addr(&self) -> Option<SocketAddr> {
        self.attempted_addr
    }
    pub(crate) fn tls_servername(&self) -> Option<&str> {
        self.servername.as_deref()
    }
    pub(crate) fn tls_verify_code(&self) -> Option<i32> {
        self.verification.as_ref().map(|verify| verify.code)
    }
    pub(crate) fn tls_verify_depth(&self) -> Option<u32> {
        self.verification.as_ref().map(|verify| verify.depth)
    }
    pub(crate) fn tls_preverify_ok(&self) -> Option<bool> {
        self.verification.as_ref().map(|verify| verify.preverify_ok)
    }
    pub(crate) fn ssl_reply(&self) -> Option<SslReply> {
        self.ssl_reply
    }
    pub(crate) fn tls_connect_state(&self) -> Option<TlsConnectState> {
        self.tls_connect
    }
    pub(crate) fn last_io_direction(&self) -> Option<IoDirection> {
        self.last_io.map(|event| event.direction)
    }
    pub(crate) fn last_io_outcome(&self) -> Option<IoOutcome> {
        self.last_io.map(|event| event.outcome)
    }
    pub(crate) fn last_io_kind(&self) -> Option<io::ErrorKind> {
        self.last_io.and_then(|event| event.kind)
    }
    pub(crate) fn last_io_errno(&self) -> Option<i32> {
        self.last_io.and_then(|event| event.errno)
    }
    pub(crate) fn socket_bytes_read(&self) -> u64 {
        self.bytes_read
    }
    pub(crate) fn socket_bytes_written(&self) -> u64 {
        self.bytes_written
    }
}
impl fmt::Debug for TransportContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TransportContext")
    }
}

pub(crate) type Driver = Pin<Box<dyn Future<Output = Result<(), tokio_postgres::Error>> + Send>>;

pub(crate) struct Connected {
    pub(crate) client: Client,
    pub(crate) driver: Driver,
    pub(crate) cancel_route: CancelRoute,
}

#[derive(Clone)]
struct KeepaliveSettings {
    idle: Duration,
    interval: Option<Duration>,
    retries: Option<u32>,
}
#[derive(Clone)]
struct SocketSettings {
    connect_timeout: Option<Duration>,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    tcp_user_timeout: Option<Duration>,
    keepalive: Option<KeepaliveSettings>,
}
impl SocketSettings {
    fn from_config(config: &Config) -> Self {
        Self {
            connect_timeout: config.get_connect_timeout().copied(),
            tcp_user_timeout: config.get_tcp_user_timeout().copied(),
            keepalive: config.get_keepalives().then(|| KeepaliveSettings {
                idle: config.get_keepalives_idle(),
                interval: config.get_keepalives_interval(),
                retries: config.get_keepalives_retries(),
            }),
        }
    }

    async fn open(
        &self,
        address: SocketAddr,
        observation: &AttemptObservation,
    ) -> Result<TcpStream, QueryFault> {
        observation.stage(ConnectStage::SocketConnect);
        let connect = TcpStream::connect(address);
        let stream = match self.connect_timeout {
            Some(limit) => tokio::time::timeout(limit, connect)
                .await
                .map_err(|_| QueryFault::socket_timeout())?
                .map_err(QueryFault::socket)?,
            None => connect.await.map_err(QueryFault::socket)?,
        };
        // Match tokio-postgres connect_socket's ordering and platform guards.
        // Disabled keepalive leaves the newly created socket's default intact.
        observation.stage(ConnectStage::SocketOptions);
        stream.set_nodelay(true).map_err(QueryFault::socket)?;
        let socket = SockRef::from(&stream);
        #[cfg(target_os = "linux")]
        if let Some(timeout) = self.tcp_user_timeout {
            socket
                .set_tcp_user_timeout(Some(timeout))
                .map_err(QueryFault::socket)?;
        }
        if let Some(keepalive) = &self.keepalive {
            let mut settings = TcpKeepalive::new().with_time(keepalive.idle);
            #[cfg(not(any(
                target_os = "aix",
                target_os = "redox",
                target_os = "solaris",
                target_os = "openbsd"
            )))]
            if let Some(interval) = keepalive.interval {
                settings = settings.with_interval(interval);
            }
            #[cfg(not(any(
                target_os = "aix",
                target_os = "redox",
                target_os = "solaris",
                target_os = "windows",
                target_os = "openbsd"
            )))]
            if let Some(retries) = keepalive.retries {
                settings = settings.with_retries(retries);
            }
            socket
                .set_tcp_keepalive(&settings)
                .map_err(QueryFault::socket)?;
        }
        observation.stage(ConnectStage::SocketReady);
        Ok(stream)
    }
}

#[derive(Clone)]
pub(crate) struct TcpRoute {
    address: SocketAddr,
    hostname: String,
    settings: SocketSettings,
    mode: tokio_postgres::config::SslMode,
    negotiation: tokio_postgres::config::SslNegotiation,
}
impl TcpRoute {
    async fn open(
        &self,
        observation: AttemptObservation,
    ) -> Result<ObservedStream<TcpStream>, QueryFault> {
        let stream = self.settings.open(self.address, &observation).await?;
        Ok(ObservedStream::new(
            stream,
            observation,
            self.mode,
            self.negotiation,
        ))
    }
}

#[derive(Clone)]
pub(crate) enum CancelRoute {
    Native,
    Tcp(TcpRoute),
}
impl CancelRoute {
    pub(crate) async fn cancel<T, F>(
        &self,
        token: &CancelToken,
        make_tls: F,
    ) -> Result<(), QueryFault>
    where
        T: MakeTlsConnect<Socket> + MakeTlsConnect<ObservedStream<TcpStream>>,
        F: Fn(Option<AttemptObservation>) -> T,
    {
        match self {
            Self::Native => token
                .cancel_query(make_tls(None))
                .await
                .map_err(|error| QueryFault::postgres(FaultPhase::Connect, error)),
            Self::Tcp(route) => {
                // Original native cancellation creates the TLS factory before
                // its socket. This independent observer never touches a live
                // or retained business fault. No DNS, retry or SQL is used.
                let observation = AttemptObservation::new();
                let mut factory = make_tls(Some(observation.clone()));
                let tls = <T as MakeTlsConnect<ObservedStream<TcpStream>>>::make_tls_connect(
                    &mut factory,
                    &route.hostname,
                )
                .map_err(|error| QueryFault::tls_factory(error.into()))?;
                let stream = route.open(observation).await?;
                token
                    .cancel_query_raw(stream, tls)
                    .await
                    .map_err(|error| QueryFault::postgres(FaultPhase::Connect, error))
            }
        }
    }
}

pub(crate) async fn connect<T, F>(
    config: &Config,
    resolver: &dyn Resolver,
    context: &ConnectContext,
    make_tls: F,
) -> Result<Connected, QueryFault>
where
    T: MakeTlsConnect<Socket> + MakeTlsConnect<ObservedStream<TcpStream>>,
    <T as MakeTlsConnect<Socket>>::Stream: Send + 'static,
    <T as MakeTlsConnect<ObservedStream<TcpStream>>>::Stream: Send + 'static,
    F: Fn(Option<AttemptObservation>) -> T,
{
    let Some(Host::Tcp(host)) = config.get_hosts().first() else {
        // Native Unix connect/cancel preserves private SocketConfig and the
        // driver's has_hostname=false restrictions. Raw APIs hardcode true.
        let (client, driver) = config
            .connect(make_tls(None))
            .await
            .map_err(|error| QueryFault::postgres(FaultPhase::Connect, error))?;
        return Ok(Connected {
            client,
            driver: Box::pin(driver),
            cancel_route: CancelRoute::Native,
        });
    };
    let port = config.get_ports().first().copied().unwrap_or(5432);
    let addresses = match host.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => {
            context.resolving(host);
            resolver
                .resolve(host, port)
                .await
                .map_err(|error| QueryFault::resolver(error).with_context(context.snapshot()))?
        }
    };
    context.resolved(addresses.len());
    let settings = SocketSettings::from_config(config);
    let mut last_error = None;
    for (index, address) in addresses.into_iter().enumerate() {
        // Preserve the original native addr.ip()+configured-port route,
        // including its removal of resolver IPv6 scope/flow metadata.
        let address = SocketAddr::new(address.ip(), port);
        let observation = context.attempt(index, address);
        let route = TcpRoute {
            address,
            hostname: host.clone(),
            settings: settings.clone(),
            mode: config.get_ssl_mode(),
            negotiation: config.get_ssl_negotiation(),
        };
        let attempt = async {
            let stream = route.open(observation.clone()).await?;
            let mut factory = make_tls(Some(observation.clone()));
            let tls = <T as MakeTlsConnect<ObservedStream<TcpStream>>>::make_tls_connect(
                &mut factory,
                host,
            )
            .map_err(|error| QueryFault::tls_factory(error.into()))?;
            // connect_raw retains the original SSLRequest/direct negotiation,
            // authentication, startup options and BackendKeyData handling.
            // Target does not configure target_session_attrs beyond Any.
            config
                .connect_raw(stream, tls)
                .await
                .map_err(|error| QueryFault::postgres(FaultPhase::Connect, error))
        }
        .await;
        match attempt {
            Ok((client, driver)) => {
                observation.established();
                return Ok(Connected {
                    client,
                    driver: Box::pin(driver),
                    cancel_route: CancelRoute::Tcp(route),
                });
            }
            Err(error) => last_error = Some(error.with_context(context.snapshot())),
        }
    }
    Err(last_error.unwrap_or_else(|| QueryFault::no_addresses().with_context(context.snapshot())))
}
