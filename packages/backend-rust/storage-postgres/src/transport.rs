//! Observations from the original resolution and native connection attempts.
//! Native PostgreSQL sockets, TLS negotiation and cancellation remain owned by
//! tokio-postgres. No lookup or connection is made to reconstruct a failure.

use crate::{ConnectStage, FaultPhase, QueryFault};
use openssl::x509::{X509, X509StoreContextRef};
use std::{
    fmt,
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio_postgres::{Client, Config, Socket, config::Host, tls::MakeTlsConnect};

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
}

#[derive(Clone)]
pub(crate) struct AttemptObservation(Arc<Mutex<AttemptState>>);
impl AttemptObservation {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(AttemptState {
            stage: ConnectStage::AddressAttempt,
            servername: None,
            verification: None,
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
        };
        if let Some(attempt) = &state.attempt {
            snapshot.attempt_index = Some(attempt.index);
            snapshot.attempted_addr = Some(attempt.addr);
            if let Ok(attempt) = attempt.observation.0.lock() {
                snapshot.stage = attempt.stage;
                snapshot.servername = attempt.servername.clone();
                snapshot.verification = attempt.verification.clone();
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
}
impl fmt::Debug for TransportContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TransportContext")
    }
}

pub(crate) async fn connect<T, F>(
    config: &Config,
    resolver: &dyn Resolver,
    context: &ConnectContext,
    make_tls: F,
) -> Result<(Client, tokio_postgres::Connection<Socket, T::Stream>), QueryFault>
where
    T: MakeTlsConnect<Socket>,
    F: Fn(Option<AttemptObservation>) -> T,
{
    let Some(Host::Tcp(host)) = config.get_hosts().first() else {
        // Unix socket handling, including its hostname/TLS restrictions and
        // native cancellation SocketConfig, stays in the original driver.
        return config
            .connect(make_tls(None))
            .await
            .map_err(|error| QueryFault::postgres(FaultPhase::Connect, error));
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
    let mut last_error = None;
    for (index, address) in addresses.into_iter().enumerate() {
        // The native driver uses addr.ip() plus the configured port, dropping
        // resolver scope/flow metadata. Preserve that exact attempted route.
        let address = SocketAddr::new(address.ip(), port);
        let observation = context.attempt(index, address);
        let mut attempt = config.clone();
        attempt.hostaddr(address.ip());
        // Numeric hostaddr is parsed locally by Tokio, without another system
        // name lookup. Native sockets, options, TLS and startup stay unchanged.
        match attempt.connect(make_tls(Some(observation.clone()))).await {
            Ok(connection) => {
                observation.established();
                return Ok(connection);
            }
            Err(error) => {
                last_error = Some(
                    QueryFault::postgres(FaultPhase::Connect, error)
                        .with_context(context.snapshot()),
                );
            }
        }
    }
    Err(last_error.unwrap_or_else(|| QueryFault::no_addresses().with_context(context.snapshot())))
}
