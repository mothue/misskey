//! Typed source retention. Driver diagnostics are never public error strings.

use crate::Failure;
use std::{error::Error, fmt, io, sync::Arc};

/// The operation that actually observed the failure. `Connect` includes the
/// driver's DNS, TCP, TLS and PostgreSQL startup sequence: those subphases are
/// not all exposed by tokio-postgres's public interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultPhase {
    Connect,
    SessionSetup,
    Query,
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
}

impl QueryFault {
    pub(crate) fn postgres(phase: FaultPhase, error: tokio_postgres::Error) -> Self {
        Self::shared_postgres(phase, Arc::new(error))
    }

    pub(crate) fn shared_postgres(phase: FaultPhase, error: Arc<tokio_postgres::Error>) -> Self {
        Self {
            phase,
            source: Source::Postgres(error),
        }
    }

    pub(crate) fn tls_configuration(error: openssl::error::ErrorStack) -> Self {
        Self {
            phase: FaultPhase::Connect,
            source: Source::TlsConfiguration(error),
        }
    }

    pub(crate) fn configuration(_: Failure) -> Self {
        Self {
            phase: FaultPhase::Connect,
            source: Source::Configuration,
        }
    }

    pub(crate) fn connect_timeout() -> Self {
        Self {
            phase: FaultPhase::Connect,
            source: Source::ConnectTimeout,
        }
    }

    pub fn phase(&self) -> FaultPhase {
        self.phase
    }

    pub fn kind(&self) -> FaultKind {
        match &self.source {
            Source::ConnectTimeout => FaultKind::ConnectTimeout,
            Source::Configuration => FaultKind::Configuration,
            Source::TlsConfiguration(_) => FaultKind::Tls,
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
        self.find_source::<io::Error>().map(io::Error::kind)
    }

    pub fn raw_os_error(&self) -> Option<i32> {
        self.find_source::<io::Error>()
            .and_then(io::Error::raw_os_error)
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

    fn find_source<T: Error + 'static>(&self) -> Option<&T> {
        let mut error: &(dyn Error + 'static) = match &self.source {
            Source::Postgres(error) => error.as_ref(),
            Source::TlsConfiguration(error) => error,
            Source::ConnectTimeout | Source::Configuration => return None,
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
