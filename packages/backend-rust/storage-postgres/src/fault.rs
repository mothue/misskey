//! Typed source retention. Driver diagnostics are never public error strings.

use crate::{Failure, transport::TransportContext};
use std::{error::Error, fmt, io, net::SocketAddr, sync::Arc};

/// The operation that actually observed the failure. `Connect` includes the
/// driver's DNS, TCP, TLS and PostgreSQL startup sequence: those subphases are
/// not all exposed by tokio-postgres's public interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultPhase {
    Connect,
    SessionSetup,
    Query,
}

/// Last event observed during this connection attempt, without inferring a
/// Node failure class or formatting a diagnostic string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectStage {
    Resolve,
    AddressAttempt,
    SocketConnect,
    SocketOptions,
    SocketReady,
    TlsFactory,
    SslRequest,
    TlsConnectFuture,
    TlsVerification,
    TlsConnectComplete,
    Established,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SslReply {
    Accepted,
    Declined,
    ServerError,
    Other(u8),
}
/// Lifecycle of the delegated TLS connector future, including fallible local
/// SSL setup before SSL_connect. Started/Failed do not prove handshake entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsConnectState {
    Started,
    Succeeded,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoDirection {
    Read,
    Write,
    Flush,
    Shutdown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoOutcome {
    Eof,
    ZeroWrite,
    Error,
}

/// Classification uses concrete source types, never diagnostic-string parsing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultKind {
    Server,
    Io,
    Tls,
    Closed,
    ConnectTimeout,
    Configuration,
    Unknown,
}

enum Source {
    Postgres(Arc<tokio_postgres::Error>),
    Resolver(io::Error),
    Socket(io::Error),
    SocketTimeout,
    TlsFactory(Box<dyn Error + Send + Sync>),
    NoAddresses,
    TlsConfiguration(openssl::error::ErrorStack),
    ConnectTimeout,
    Configuration,
}

/// Retains the actual source without formatting it or exposing its error chain.
/// Machine-readable observations are available for compatibility research; they
/// do not establish the legacy Node message, name or code by themselves.
pub struct QueryFault {
    phase: FaultPhase,
    source: Source,
    context: Option<Box<TransportContext>>,
}

impl QueryFault {
    pub(crate) fn postgres(phase: FaultPhase, error: tokio_postgres::Error) -> Self {
        Self::shared_postgres(phase, Arc::new(error))
    }

    pub(crate) fn shared_postgres(phase: FaultPhase, error: Arc<tokio_postgres::Error>) -> Self {
        Self {
            phase,
            source: Source::Postgres(error),
            context: None,
        }
    }

    pub(crate) fn tls_configuration(error: openssl::error::ErrorStack) -> Self {
        Self {
            phase: FaultPhase::Connect,
            source: Source::TlsConfiguration(error),
            context: None,
        }
    }

    pub(crate) fn configuration(_: Failure) -> Self {
        Self {
            phase: FaultPhase::Connect,
            source: Source::Configuration,
            context: None,
        }
    }

    pub(crate) fn connect_timeout() -> Self {
        Self {
            phase: FaultPhase::Connect,
            source: Source::ConnectTimeout,
            context: None,
        }
    }

    pub(crate) fn resolver(error: io::Error) -> Self {
        Self {
            phase: FaultPhase::Connect,
            source: Source::Resolver(error),
            context: None,
        }
    }

    pub(crate) fn no_addresses() -> Self {
        Self {
            phase: FaultPhase::Connect,
            source: Source::NoAddresses,
            context: None,
        }
    }

    pub(crate) fn socket(error: io::Error) -> Self {
        Self {
            phase: FaultPhase::Connect,
            source: Source::Socket(error),
            context: None,
        }
    }
    pub(crate) fn socket_timeout() -> Self {
        Self {
            phase: FaultPhase::Connect,
            source: Source::SocketTimeout,
            context: None,
        }
    }
    pub(crate) fn tls_factory(error: Box<dyn Error + Send + Sync>) -> Self {
        Self {
            phase: FaultPhase::Connect,
            source: Source::TlsFactory(error),
            context: None,
        }
    }

    pub(crate) fn with_context(mut self, context: Option<TransportContext>) -> Self {
        self.context = context.map(Box::new);
        self
    }

    pub fn phase(&self) -> FaultPhase {
        self.phase
    }

    pub fn kind(&self) -> FaultKind {
        match &self.source {
            Source::ConnectTimeout => FaultKind::ConnectTimeout,
            Source::Configuration => FaultKind::Configuration,
            Source::TlsConfiguration(_) | Source::TlsFactory(_) => FaultKind::Tls,
            Source::Resolver(_)
            | Source::Socket(_)
            | Source::SocketTimeout
            | Source::NoAddresses => FaultKind::Io,
            Source::Postgres(error) => {
                if error.as_db_error().is_some() {
                    FaultKind::Server
                } else if self.find_source::<openssl::ssl::Error>().is_some()
                    || self.find_source::<openssl::error::ErrorStack>().is_some()
                {
                    FaultKind::Tls
                } else if self.io_kind().is_some() {
                    FaultKind::Io
                } else if error.is_closed() {
                    FaultKind::Closed
                } else {
                    FaultKind::Unknown
                }
            }
        }
    }

    pub fn io_kind(&self) -> Option<io::ErrorKind> {
        if matches!(self.source, Source::SocketTimeout) {
            return Some(io::ErrorKind::TimedOut);
        }
        if matches!(self.source, Source::NoAddresses) {
            // This is the driver's existing classification for an empty
            // resolution result, not a claimed platform EAI/OS error code.
            return Some(io::ErrorKind::InvalidInput);
        }
        self.find_source::<io::Error>().map(io::Error::kind)
    }

    pub fn raw_os_error(&self) -> Option<i32> {
        self.find_source::<io::Error>()
            .and_then(io::Error::raw_os_error)
            .or_else(|| self.resolver_errno())
    }

    fn resolver_facts(&self) -> Option<&crate::resolver::ResolverFacts> {
        match &self.source {
            Source::Resolver(error) => crate::resolver::facts(error),
            _ => None,
        }
    }
    pub fn resolver_eai(&self) -> Option<i32> {
        self.resolver_facts().map(|f| f.eai)
    }
    pub fn resolver_errno(&self) -> Option<i32> {
        self.resolver_facts().and_then(|f| f.errno)
    }
    pub fn resolver_domain(&self) -> Option<&'static str> {
        self.resolver_facts().map(|_| "getaddrinfo")
    }
    pub fn resolver_flags(&self) -> Option<i32> {
        self.resolver_facts().map(|f| f.flags)
    }
    pub fn resolver_family(&self) -> Option<i32> {
        self.resolver_facts().map(|f| f.family)
    }
    pub fn resolver_socket_type(&self) -> Option<i32> {
        self.resolver_facts().map(|f| f.socket_type)
    }
    pub fn resolver_protocol(&self) -> Option<i32> {
        self.resolver_facts().map(|f| f.protocol)
    }
    pub fn resolver_service(&self) -> Option<&'static str> {
        self.resolver_facts().map(|f| f.service)
    }
    pub fn tcp_attempt_count(&self) -> Option<usize> {
        self.context
            .as_deref()
            .map(TransportContext::tcp_attempt_count)
    }
    pub fn tcp_attempts_exhausted(&self) -> Option<bool> {
        self.context
            .as_deref()
            .map(TransportContext::tcp_attempts_exhausted)
    }
    /// Original failed dials in actual attempt order; never exposes an error string.
    pub fn tcp_failures(
        &self,
    ) -> Option<impl Iterator<Item = (SocketAddr, Option<io::ErrorKind>, Option<i32>)> + '_> {
        self.context.as_deref().map(|context| {
            context
                .tcp_failures()
                .iter()
                .map(|failure| (failure.address, failure.kind, failure.errno))
        })
    }
    pub(crate) fn tls_verify_certificate(&self) -> Option<&openssl::x509::X509Ref> {
        self.context
            .as_deref()
            .and_then(TransportContext::tls_verify_certificate)
    }

    pub fn server_code(&self) -> Option<&str> {
        self.find_source::<tokio_postgres::error::DbError>()
            .map(|error| error.code().code())
    }

    /// The protocol message only; SQL, parameters and detail/hint are excluded.
    /// A caller must validate the baseline before using this as public output.
    pub fn server_message(&self) -> Option<&str> {
        self.find_source::<tokio_postgres::error::DbError>()
            .map(tokio_postgres::error::DbError::message)
    }

    pub fn connect_stage(&self) -> Option<ConnectStage> {
        self.context.as_deref().map(TransportContext::connect_stage)
    }
    pub fn connect_host(&self) -> Option<&str> {
        self.context
            .as_deref()
            .and_then(TransportContext::connect_host)
    }
    pub fn tls_verify_chain_len(&self) -> Option<usize> {
        self.context
            .as_deref()
            .and_then(TransportContext::tls_verify_chain_len)
    }

    pub fn resolver_host(&self) -> Option<&str> {
        self.context
            .as_deref()
            .and_then(TransportContext::resolver_host)
    }
    pub fn resolver_address_count(&self) -> Option<usize> {
        self.context
            .as_deref()
            .and_then(TransportContext::resolver_address_count)
    }
    pub fn attempt_index(&self) -> Option<usize> {
        self.context
            .as_deref()
            .and_then(TransportContext::attempt_index)
    }
    pub fn attempted_addr(&self) -> Option<SocketAddr> {
        self.context
            .as_deref()
            .and_then(TransportContext::attempted_addr)
    }
    pub fn tls_servername(&self) -> Option<&str> {
        self.context
            .as_deref()
            .and_then(TransportContext::tls_servername)
    }
    pub fn tls_verify_code(&self) -> Option<i32> {
        self.context
            .as_deref()
            .and_then(TransportContext::tls_verify_code)
    }
    pub fn tls_verify_depth(&self) -> Option<u32> {
        self.context
            .as_deref()
            .and_then(TransportContext::tls_verify_depth)
    }
    pub fn tls_preverify_ok(&self) -> Option<bool> {
        self.context
            .as_deref()
            .and_then(TransportContext::tls_preverify_ok)
    }

    pub fn ssl_reply(&self) -> Option<SslReply> {
        self.context
            .as_deref()
            .and_then(TransportContext::ssl_reply)
    }
    pub fn tls_connect_state(&self) -> Option<TlsConnectState> {
        self.context
            .as_deref()
            .and_then(TransportContext::tls_connect_state)
    }
    pub fn last_io_direction(&self) -> Option<IoDirection> {
        self.context
            .as_deref()
            .and_then(TransportContext::last_io_direction)
    }
    pub fn last_io_outcome(&self) -> Option<IoOutcome> {
        self.context
            .as_deref()
            .and_then(TransportContext::last_io_outcome)
    }
    pub fn last_io_kind(&self) -> Option<io::ErrorKind> {
        self.context
            .as_deref()
            .and_then(TransportContext::last_io_kind)
    }
    pub fn last_io_errno(&self) -> Option<i32> {
        self.context
            .as_deref()
            .and_then(TransportContext::last_io_errno)
    }
    pub fn socket_bytes_read(&self) -> Option<u64> {
        self.context
            .as_deref()
            .map(TransportContext::socket_bytes_read)
    }
    pub fn socket_bytes_written(&self) -> Option<u64> {
        self.context
            .as_deref()
            .map(TransportContext::socket_bytes_written)
    }

    fn find_source<T: Error + 'static>(&self) -> Option<&T> {
        let mut error: &(dyn Error + 'static) = match &self.source {
            Source::Postgres(error) => error.as_ref(),
            Source::TlsConfiguration(error) => error,
            Source::Resolver(error) => error,
            Source::Socket(error) => error,
            Source::TlsFactory(error) => error.as_ref(),
            Source::ConnectTimeout
            | Source::Configuration
            | Source::NoAddresses
            | Source::SocketTimeout => return None,
        };
        // Known driver chains are short; avoid unbounded traversal if an
        // upstream/custom serialization source ever contains a cycle.
        for _ in 0..16 {
            if let Some(error) = error.downcast_ref::<T>() {
                return Some(error);
            }
            error = error.source()?;
        }
        None
    }
}

impl fmt::Debug for QueryFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("QueryFault")
    }
}
impl fmt::Display for QueryFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Rust backend unavailable")
    }
}
// In particular, do not expose the driver's source through Error::source().
impl Error for QueryFault {}
