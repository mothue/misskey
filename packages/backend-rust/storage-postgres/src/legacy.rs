//! Explicit projection of retained facts into the measured Bun/pg/TypeORM
//! business-error contract. Unknown source shapes deliberately remain absent.

use crate::identity::typed_ip;
use crate::{
    ConnectStage, FaultKind, FaultPhase, IoDirection, IoOutcome, QueryError, QueryFault, SslReply,
    TlsConnectState,
};
use openssl::{nid::Nid, x509::X509Ref};
use std::{
    fmt,
    fmt::Write,
    net::{IpAddr, SocketAddr},
};

/// Public fields are available only through the explicit legacy projection.
/// Formatting and Error::source never disclose the retained public message.
#[derive(Clone, PartialEq, Eq)]
pub struct LegacyError {
    pub name: &'static str,
    pub message: String,
}

impl fmt::Debug for LegacyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LegacyError")
    }
}
impl fmt::Display for LegacyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Rust backend unavailable")
    }
}
impl std::error::Error for LegacyError {}

fn projected(name: &'static str, message: impl Into<String>) -> LegacyError {
    LegacyError {
        name,
        message: message.into(),
    }
}

pub(crate) fn query_error(error: &QueryError) -> Option<LegacyError> {
    match error {
        QueryError::Server { message } => Some(projected("QueryFailedError", message.clone())),
        QueryError::QueryTimeout => Some(projected("QueryFailedError", "Query read timeout")),
        QueryError::Fault(fault) => fault_error(fault),
        QueryError::Unavailable => None,
    }
}

fn fault_error(fault: &QueryFault) -> Option<LegacyError> {
    // PostgresQueryRunner awaits checkout before entering its query catch.
    // The Rust-only session SET has no corresponding measured pg operation.
    let name = match fault.phase() {
        FaultPhase::Connect => "Error",
        FaultPhase::Query => "QueryFailedError",
        FaultPhase::SessionSetup => return None,
    };
    if fault.kind() == FaultKind::Server {
        return Some(projected(
            if fault.phase() == FaultPhase::Connect {
                "error"
            } else {
                name
            },
            fault.server_message()?,
        ));
    }
    if fault.phase() == FaultPhase::Query {
        return query_transport_message(fault).map(|message| projected(name, message));
    }
    if fault.kind() == FaultKind::ConnectTimeout {
        return Some(projected(
            name,
            "Connection terminated due to connection timeout",
        ));
    }
    if let Some(code) = fault.resolver_eai() {
        let host = fault.resolver_host()?;
        return resolver_code(code)
            .map(|code| projected("DNSException", format!("getaddrinfo {code} {host}")));
    }
    if fault.connect_stage() == Some(ConnectStage::SocketConnect) {
        // The count is the actual selected/deduplicated TCP-attempt count,
        // not the raw resolver answer count. Startup/TLS cannot exhaust it.
        if fault.tcp_attempts_exhausted() == Some(true)
            && fault.tcp_attempt_count().is_some_and(|count| count > 1)
        {
            return Some(projected("AggregateError", ""));
        }
        if fault.tcp_attempt_count() == Some(1) {
            let code = socket_error_code(fault.raw_os_error()?)?;
            let address = fault.attempted_addr()?;
            // Bun ExceptionWithHostPort leaves IPv6 unbracketed and omits
            // a zero port. This uses the actual socket errno and endpoint.
            let endpoint = tcp_endpoint(fault.connect_host(), address);
            return Some(projected(name, format!("connect {code} {endpoint}")));
        }
        return None;
    }
    match fault.ssl_reply() {
        Some(SslReply::Declined) => {
            return Some(projected(
                name,
                "The server does not support SSL connections",
            ));
        }
        Some(SslReply::ServerError | SslReply::Other(_)) => {
            return Some(projected(
                name,
                "There was an error establishing an SSL connection",
            ));
        }
        Some(SslReply::Accepted) | None => {}
    }
    if fault.tls_connect_state() == Some(TlsConnectState::Failed) {
        if fault.tls_preverify_ok() == Some(false) {
            let code = fault.tls_verify_code()?;
            if matches!(
                code,
                openssl_sys::X509_V_ERR_HOSTNAME_MISMATCH
                    | openssl_sys::X509_V_ERR_IP_ADDRESS_MISMATCH
            ) {
                let message =
                    hostname_message(fault.tls_servername()?, fault.tls_verify_certificate()?)?;
                return Some(projected(name, message));
            }
            if code == openssl_sys::X509_V_ERR_UNABLE_TO_GET_ISSUER_CERT_LOCALLY {
                // Bun's rejecting verify callback keeps walking. With one
                // non-self-issued leaf and no issuer, BoringSSL internal_verify
                // reaches n == 0 and reports UNABLE_TO_VERIFY_LEAF_SIGNATURE.
                // OpenSSL's false callback has stopped earlier at local issuer.
                let depth = fault.tls_verify_depth()?;
                let chain_len = fault.tls_verify_chain_len()?;
                if depth == 0 && chain_len == 1 {
                    let certificate = fault.tls_verify_certificate()?;
                    if certificate.issued(certificate) == openssl::x509::X509VerifyResult::OK {
                        return None;
                    }
                    return Some(projected(name, "unable to verify the first certificate"));
                }
                if depth == 0 || chain_len <= 1 {
                    return None;
                }
            }
            return certificate_message(code).map(|message| projected(name, message));
        }
        // This is an actual EOF while the original delegated TLS future was
        // running, not merely a TLS factory being created or an SSL 'S'.
        if fault.tls_verify_code().is_none() && read_eof(fault) {
            return Some(projected(name, "Socket is closed"));
        }
        return None;
    }
    // After a completed handshake, pg's ordinary startup stream listeners
    // handle EOF/errno just as they do on a plaintext socket.
    query_transport_message(fault).map(|message| projected(name, message))
}

fn tcp_endpoint(host: Option<&str>, address: SocketAddr) -> String {
    let ip = host
        .filter(|host| typed_ip(host) == Some(address.ip()))
        .map(str::to_owned)
        .unwrap_or_else(|| address.ip().to_string());
    if address.port() == 0 {
        ip
    } else {
        format!("{ip}:{}", address.port())
    }
}

fn read_eof(fault: &QueryFault) -> bool {
    fault.last_io_direction() == Some(IoDirection::Read)
        && fault.last_io_outcome() == Some(IoOutcome::Eof)
}

fn query_transport_message(fault: &QueryFault) -> Option<String> {
    if read_eof(fault) {
        return Some("Connection terminated unexpectedly".to_owned());
    }
    if fault.last_io_outcome() == Some(IoOutcome::Error) {
        let operation = match fault.last_io_direction()? {
            IoDirection::Read => "read",
            IoDirection::Write | IoDirection::Flush => "write",
            IoDirection::Shutdown => return None,
        };
        let code = socket_error_code(fault.last_io_errno()?)?;
        return Some(format!("{operation} {code}"));
    }
    None
}

// These statuses come from the original Linux libc getaddrinfo result, never
// io::Error's errno. Bun 38afee6c8 c_ares.rs::init_eai provides this conversion.
// EAI_AGAIN is runtime-correlated; NONAME/NODATA are source-backed mappings.
#[cfg(target_os = "linux")]
fn resolver_code(code: i32) -> Option<&'static str> {
    // EAI_ADDRFAMILY and GNU's getaddrinfo_a extensions are defined by
    // Linux netdb.h but are not exported by libc 0.2.191. Keep their EAI
    // domain explicit rather than sending these values through io::Error.
    #[cfg(target_env = "gnu")]
    match code {
        -105 => return Some("EBADSTR"),           // EAI_IDN_ENCODE
        -103 => return Some("ENOTFOUND"),         // EAI_ALLDONE
        -100 => return Some("ETIMEOUT"),          // EAI_INPROGRESS
        -101 | -102 => return Some("ECANCELLED"), // CANCELED / NOTCANCELED
        _ => {}
    }
    match code {
        0 => None,
        libc::EAI_AGAIN => Some("EAI_AGAIN"),
        libc::EAI_NONAME | libc::EAI_NODATA => Some("ENOTFOUND"),
        -9 | libc::EAI_FAMILY => Some("EBADFAMILY"), // EAI_ADDRFAMILY
        libc::EAI_BADFLAGS => Some("EBADFLAGS"),
        libc::EAI_FAIL => Some("EBADRESP"),
        libc::EAI_MEMORY => Some("ENOMEM"),
        libc::EAI_SERVICE => Some("ESERVICE"),
        libc::EAI_SOCKTYPE => Some("ECONNREFUSED"),
        libc::EAI_SYSTEM => Some("ESERVFAIL"),
        // This is Bun init_eai's explicit fallback for an actual nonzero
        // EAI status, not a guessed status for an opaque resolver error.
        _ => Some("ENOTIMP"),
    }
}
#[cfg(not(target_os = "linux"))]
fn resolver_code(_: i32) -> Option<&'static str> {
    None
}

// Linux socket errno names are the names Bun's libuv-compatible
// util.getSystemErrorName exposes. This deliberately accepts actual errno
// only; io::ErrorKind and diagnostic text do not invent a platform code.
#[cfg(target_os = "linux")]
fn socket_error_code(code: i32) -> Option<&'static str> {
    macro_rules! codes {
        ($($code:ident),+ $(,)?) => { match code { $(libc::$code => Some(stringify!($code)),)+ _ => None } };
    }
    codes!(
        E2BIG,
        EACCES,
        EADDRINUSE,
        EADDRNOTAVAIL,
        EAFNOSUPPORT,
        EAGAIN,
        EALREADY,
        EBADF,
        EBUSY,
        ECANCELED,
        ECONNABORTED,
        ECONNREFUSED,
        ECONNRESET,
        EDESTADDRREQ,
        EEXIST,
        EFAULT,
        EHOSTUNREACH,
        EINTR,
        EINVAL,
        EIO,
        EISCONN,
        EISDIR,
        ELOOP,
        EMFILE,
        EMSGSIZE,
        ENAMETOOLONG,
        ENETDOWN,
        ENETUNREACH,
        ENFILE,
        ENOBUFS,
        ENODEV,
        ENOENT,
        ENOMEM,
        ENONET,
        ENOSPC,
        ENOSYS,
        ENOTCONN,
        ENOTDIR,
        ENOTEMPTY,
        ENOTSOCK,
        ENOTSUP,
        EPERM,
        EPIPE,
        EPROTO,
        EPROTONOSUPPORT,
        EPROTOTYPE,
        EROFS,
        ESHUTDOWN,
        ESPIPE,
        ESRCH,
        ETIMEDOUT,
        ETXTBSY,
        EXDEV,
        EFBIG,
        ENOPROTOOPT,
        ERANGE,
        ENXIO,
        EMLINK,
        EHOSTDOWN,
        EREMOTEIO,
        ENOTTY,
        EILSEQ,
        EOVERFLOW,
        ESOCKTNOSUPPORT,
        ENODATA,
        EUNATCH,
        ENOEXEC
    )
}
#[cfg(not(target_os = "linux"))]
fn socket_error_code(_: i32) -> Option<&'static str> {
    None
}

// Bun pins BoringSSL 41bf9b59c2ebf277a7aa427e1ecad5cc80dd4d4f;
// crypto/x509/x509_txt.cc supplies these messages. Match the native
// OpenSSL symbolic status, whose numeric INVALID_CA/INVALID_CALL values
// differ from BoringSSL, rather than treating the two enums as identical.
fn certificate_message(code: i32) -> Option<&'static str> {
    use openssl_sys::*;
    Some(match code {
        X509_V_ERR_UNABLE_TO_GET_ISSUER_CERT => "unable to get issuer certificate",
        X509_V_ERR_UNABLE_TO_GET_CRL => "unable to get certificate CRL",
        X509_V_ERR_UNABLE_TO_DECRYPT_CERT_SIGNATURE => "unable to decrypt certificate's signature",
        X509_V_ERR_UNABLE_TO_DECRYPT_CRL_SIGNATURE => "unable to decrypt CRL's signature",
        X509_V_ERR_UNABLE_TO_DECODE_ISSUER_PUBLIC_KEY => "unable to decode issuer public key",
        X509_V_ERR_CERT_SIGNATURE_FAILURE => "certificate signature failure",
        X509_V_ERR_CRL_SIGNATURE_FAILURE => "CRL signature failure",
        X509_V_ERR_CERT_NOT_YET_VALID => "certificate is not yet valid",
        X509_V_ERR_CERT_HAS_EXPIRED => "certificate has expired",
        X509_V_ERR_CRL_NOT_YET_VALID => "CRL is not yet valid",
        X509_V_ERR_CRL_HAS_EXPIRED => "CRL has expired",
        X509_V_ERR_ERROR_IN_CERT_NOT_BEFORE_FIELD => {
            "format error in certificate's notBefore field"
        }
        X509_V_ERR_ERROR_IN_CERT_NOT_AFTER_FIELD => "format error in certificate's notAfter field",
        X509_V_ERR_ERROR_IN_CRL_LAST_UPDATE_FIELD => "format error in CRL's lastUpdate field",
        X509_V_ERR_ERROR_IN_CRL_NEXT_UPDATE_FIELD => "format error in CRL's nextUpdate field",
        X509_V_ERR_OUT_OF_MEM => "out of memory",
        X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT => "self signed certificate",
        X509_V_ERR_SELF_SIGNED_CERT_IN_CHAIN => "self signed certificate in certificate chain",
        X509_V_ERR_UNABLE_TO_GET_ISSUER_CERT_LOCALLY => "unable to get local issuer certificate",
        X509_V_ERR_UNABLE_TO_VERIFY_LEAF_SIGNATURE => "unable to verify the first certificate",
        X509_V_ERR_CERT_CHAIN_TOO_LONG => "certificate chain too long",
        X509_V_ERR_CERT_REVOKED => "certificate revoked",
        X509_V_ERR_INVALID_CA => "invalid CA certificate",
        X509_V_ERR_INVALID_NON_CA => "invalid non-CA certificate (has CA markings)",
        X509_V_ERR_PATH_LENGTH_EXCEEDED => "path length constraint exceeded",
        X509_V_ERR_PROXY_PATH_LENGTH_EXCEEDED => "proxy path length constraint exceeded",
        X509_V_ERR_PROXY_CERTIFICATES_NOT_ALLOWED => {
            "proxy certificates not allowed, please set the appropriate flag"
        }
        X509_V_ERR_INVALID_PURPOSE => "unsupported certificate purpose",
        X509_V_ERR_CERT_UNTRUSTED => "certificate not trusted",
        X509_V_ERR_CERT_REJECTED => "certificate rejected",
        X509_V_ERR_APPLICATION_VERIFICATION => "application verification failure",
        X509_V_ERR_SUBJECT_ISSUER_MISMATCH => "subject issuer mismatch",
        X509_V_ERR_AKID_SKID_MISMATCH => "authority and subject key identifier mismatch",
        X509_V_ERR_AKID_ISSUER_SERIAL_MISMATCH => "authority and issuer serial number mismatch",
        X509_V_ERR_KEYUSAGE_NO_CERTSIGN => "key usage does not include certificate signing",
        X509_V_ERR_UNABLE_TO_GET_CRL_ISSUER => "unable to get CRL issuer certificate",
        X509_V_ERR_UNHANDLED_CRITICAL_EXTENSION => "unhandled critical extension",
        X509_V_ERR_KEYUSAGE_NO_CRL_SIGN => "key usage does not include CRL signing",
        X509_V_ERR_KEYUSAGE_NO_DIGITAL_SIGNATURE => "key usage does not include digital signature",
        X509_V_ERR_UNHANDLED_CRITICAL_CRL_EXTENSION => "unhandled critical CRL extension",
        X509_V_ERR_INVALID_EXTENSION => "invalid or inconsistent certificate extension",
        X509_V_ERR_INVALID_POLICY_EXTENSION => {
            "invalid or inconsistent certificate policy extension"
        }
        X509_V_ERR_NO_EXPLICIT_POLICY => "no explicit policy",
        X509_V_ERR_DIFFERENT_CRL_SCOPE => "Different CRL scope",
        X509_V_ERR_UNSUPPORTED_EXTENSION_FEATURE => "Unsupported extension feature",
        X509_V_ERR_UNNESTED_RESOURCE => "RFC 3779 resource not subset of parent's resources",
        X509_V_ERR_PERMITTED_VIOLATION => "permitted subtree violation",
        X509_V_ERR_EXCLUDED_VIOLATION => "excluded subtree violation",
        X509_V_ERR_SUBTREE_MINMAX => "name constraints minimum and maximum not supported",
        X509_V_ERR_UNSUPPORTED_CONSTRAINT_TYPE => "unsupported name constraint type",
        X509_V_ERR_UNSUPPORTED_CONSTRAINT_SYNTAX => "unsupported or invalid name constraint syntax",
        X509_V_ERR_UNSUPPORTED_NAME_SYNTAX => "unsupported or invalid name syntax",
        X509_V_ERR_CRL_PATH_VALIDATION_ERROR => "CRL path validation error",
        X509_V_ERR_EMAIL_MISMATCH => "Email address mismatch",
        X509_V_ERR_INVALID_CALL => "Invalid certificate verification context",
        X509_V_ERR_STORE_LOOKUP => "Issuer certificate lookup error",
        _ => return None,
    })
}

/// Pinned Bun tls.ts::checkServerIdentity and ncrypto.cpp::PrintGeneralName
/// define these templates and SAN presentation. Only typed DNS/IP/email/URI
/// SANs are supported. Directory/other-name encodings require bindings that
/// expose their typed ASN.1 values and RFC2253 presentation; they remain
/// explicit gaps instead of an incomplete or guessed public message.
fn hostname_message(servername: &str, certificate: &X509Ref) -> Option<String> {
    let host = servername.strip_suffix('.').unwrap_or(servername);
    // Bun converts non-ASCII names using nontransitional UTS #46 with
    // CheckHyphens/VerifyDnsLength disabled, preserving the original spelling
    // in the error. An invalid conversion is an empty comparison hostname.
    let ascii = if servername.is_ascii() {
        servername.to_owned()
    } else {
        idna::domain_to_ascii(servername).unwrap_or_default()
    };
    let ascii = ascii.strip_suffix('.').unwrap_or(&ascii);
    let mut dns = Vec::new();
    let mut ips = Vec::new();
    let mut rendered = Vec::new();
    let mut complete_rendering = true;
    if let Some(names) = certificate.subject_alt_names() {
        for name in names {
            if let Some(value) = name.dnsname() {
                dns.push(value.to_owned());
                rendered.push(format!("DNS:{}", alt_name(value)));
            } else if let Some(value) = name.ipaddress() {
                let address = match value {
                    [a, b, c, d] => IpAddr::from([*a, *b, *c, *d]),
                    value if value.len() == 16 => IpAddr::from(<[u8; 16]>::try_from(value).ok()?),
                    _ => return None,
                };
                ips.push(address);
                rendered.push(format!("IP Address:{}", san_ip(address)));
            } else if let Some(value) = name.email() {
                rendered.push(format!("email:{}", alt_name(value)));
            } else if let Some(value) = name.uri() {
                rendered.push(format!("URI:{}", alt_name(value)));
            } else {
                // Non-DNS SANs do not suppress CN fallback and do not enter
                // an IP-host message. Their rendering is needed only when
                // an actual DNS SAN makes the complete SAN list public.
                complete_rendering = false;
            }
        }
    }
    let reason = if let Some(ip) = typed_ip(host) {
        if ips.contains(&ip) {
            return None;
        }
        format!(
            "IP: {host} is not in the cert's list: {}",
            ips.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else if !dns.is_empty() {
        if !complete_rendering {
            return None;
        }
        if dns.iter().any(|pattern| dns_matches(ascii, pattern)) {
            return None;
        }
        format!(
            "Host: {host}. is not in the cert's altnames: {}",
            rendered.join(", ")
        )
    } else {
        let names = certificate
            .subject_name()
            .entries_by_nid(Nid::COMMONNAME)
            .map(|entry| entry.data().to_string().ok())
            .collect::<Option<Vec<_>>>()?;
        if names.iter().any(|pattern| dns_matches(ascii, pattern)) {
            return None;
        }
        if names.is_empty() || (names.len() == 1 && names[0].is_empty()) {
            "Cert does not contain a DNS name".to_owned()
        } else {
            format!("Host: {host}. is not cert's CN: {}", names.join(","))
        }
    };
    Some(format!(
        "Hostname/IP does not match certificate's altnames: {reason}"
    ))
}

fn san_ip(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => ip
            .segments()
            .iter()
            .map(|part| format!("{part:X}"))
            .collect::<Vec<_>>()
            .join(":"),
    }
}

fn alt_name(value: &str) -> String {
    if value
        .bytes()
        .all(|byte| (b' '..=b'~').contains(&byte) && !b"\"\\,'".contains(&byte))
    {
        return value.to_owned();
    }
    let mut rendered = String::from("\"");
    for byte in value.bytes() {
        match byte {
            b'\\' => rendered.push_str("\\\\"),
            b'\"' => rendered.push_str("\\\""),
            b' '..=b'~' if byte != b',' => rendered.push(char::from(byte)),
            byte => {
                let _ = write!(rendered, "\\u00{byte:02x}");
            }
        }
    }
    rendered.push('\"');
    rendered
}

fn dns_matches(host: &str, pattern: &str) -> bool {
    if !host
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return false;
    }
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    let pattern = pattern
        .strip_suffix('.')
        .unwrap_or(pattern)
        .to_ascii_lowercase();
    let host: Vec<_> = host.split('.').collect();
    let pattern: Vec<_> = pattern.split('.').collect();
    if host.len() != pattern.len()
        || pattern
            .iter()
            .any(|part| part.is_empty() || part.bytes().any(|byte| !(0x21..=0x7f).contains(&byte)))
    {
        return false;
    }
    if host[1..] != pattern[1..] {
        return false;
    }
    let label = pattern[0];
    if !label.contains('*') || label.contains("xn--") {
        return host[0] == label;
    }
    let Some((prefix, suffix)) = label.split_once('*') else {
        return false;
    };
    !suffix.contains('*')
        && pattern.len() > 2
        && !host[0].is_empty()
        && prefix.len() + suffix.len() <= host[0].len()
        && host[0].starts_with(prefix)
        && host[0].ends_with(suffix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Database, PostgresConfig, Target, Tls, TlsVersion};
    use openssl::{
        asn1::Asn1Time,
        ec::{EcGroup, EcKey},
        hash::MessageDigest,
        pkey::PKey,
        x509::{X509, X509NameBuilder, extension::SubjectAlternativeName},
    };
    use std::io;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn assert_projection(error: &QueryError, name: &str, message: &str) {
        let legacy = query_error(error).expect("typed fault must have a verified projection");
        assert_eq!(legacy.name, name);
        assert_eq!(legacy.message, message);
        assert_eq!(format!("{legacy:?}"), "LegacyError");
        assert_eq!(legacy.to_string(), "Rust backend unavailable");
        assert!(std::error::Error::source(&legacy).is_none());
    }

    fn target(port: u16) -> Target {
        let mut config: PostgresConfig =
            serde_json::from_str(include_str!("../tests/fixtures/local.json")).unwrap();
        let mut target = config.targets.remove(0);
        target.host = "127.0.0.1".into();
        target.port = port;
        target.connect_timeout_ms = Some(300);
        target
    }

    async fn startup(socket: &mut tokio::net::TcpStream) {
        let length = socket.read_u32().await.unwrap() as usize;
        let mut body = vec![0; length - 4];
        socket.read_exact(&mut body).await.unwrap();
    }

    #[test]
    fn only_the_explicit_projection_exposes_a_public_message() {
        assert_projection(
            &QueryError::Server {
                message: "private server input".into(),
            },
            "QueryFailedError",
            "private server input",
        );
        assert_projection(
            &QueryError::QueryTimeout,
            "QueryFailedError",
            "Query read timeout",
        );
        assert!(query_error(&QueryError::Unavailable).is_none());
        let unknown = QueryError::Fault(QueryFault::resolver(io::Error::other(
            "getaddrinfo EAI_AGAIN private.fake.invalid",
        )));
        assert!(
            query_error(&unknown).is_none(),
            "diagnostic text cannot establish an EAI status"
        );
        let unknown = QueryError::Fault(QueryFault::socket(io::Error::other("read ECONNRESET")));
        assert!(
            query_error(&unknown).is_none(),
            "diagnostic text cannot establish an IO event"
        );
    }

    #[tokio::test]
    async fn real_numeric_tcp_refusal_uses_the_actual_endpoint() {
        // Keep a socket bound without listening, so another process cannot
        // acquire this ephemeral address between selection and the real dial.
        let socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        socket
            .bind(&std::net::SocketAddr::from(([127, 0, 0, 1], 0)).into())
            .unwrap();
        let address = socket.local_addr().unwrap().as_socket().unwrap();
        let database = Database::new(None, Some(target(address.port())));
        let error = database
            .query_typed(false, "SELECT 1", &[])
            .await
            .unwrap_err();
        assert_projection(&error, "Error", &format!("connect ECONNREFUSED {address}"));
        assert!(matches!(
            database.query(false, "SELECT 1", &[]).await,
            Err(crate::Failure)
        ));
        database.shutdown().await;
    }

    #[tokio::test]
    async fn actual_startup_server_rejection_is_not_query_wrapped() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let database = Database::new(None, Some(target(listener.local_addr().unwrap().port())));
        let peer = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            startup(&mut socket).await;
            let fields = b"SFATAL\0C57P03\0Mactual startup message\0Dprivate detail\0\0";
            socket.write_u8(b'E').await.unwrap();
            socket.write_u32((fields.len() + 4) as u32).await.unwrap();
            socket.write_all(fields).await.unwrap();
        });
        let error = database
            .query_typed(false, "SELECT 1", &[])
            .await
            .unwrap_err();
        assert_projection(&error, "error", "actual startup message");
        peer.await.unwrap();
        database.shutdown().await;
    }

    #[tokio::test]
    async fn original_ssl_reply_and_tls_fin_have_distinct_legacy_errors() {
        for (reply, message) in [
            (Some(b'N'), "The server does not support SSL connections"),
            (
                Some(b'E'),
                "There was an error establishing an SSL connection",
            ),
            (
                Some(b'?'),
                "There was an error establishing an SSL connection",
            ),
            (None, "Connection terminated unexpectedly"),
            (Some(b'S'), "Socket is closed"),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let mut target = target(listener.local_addr().unwrap().port());
            target.tls = Some(Tls {
                reject_unauthorized: true,
                ca: vec![],
                cert: None,
                key: None,
                passphrase: None,
                servername: "localhost".into(),
                sni: true,
                min_version: TlsVersion::V12,
                max_version: TlsVersion::V13,
                ciphers: "DEFAULT".into(),
            });
            let database = Database::new(None, Some(target));
            let peer = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 8];
                socket.read_exact(&mut request).await.unwrap();
                assert_eq!(request, [0, 0, 0, 8, 4, 210, 22, 47]);
                if let Some(reply) = reply {
                    socket.write_all(&[reply]).await.unwrap();
                }
                if reply == Some(b'S') {
                    let mut header = [0; 5];
                    socket.read_exact(&mut header).await.unwrap();
                    let mut hello = vec![0; u16::from_be_bytes([header[3], header[4]]) as usize];
                    socket.read_exact(&mut hello).await.unwrap();
                }
                socket.shutdown().await.unwrap();
            });
            let error = database
                .query_typed(false, "SELECT 1", &[])
                .await
                .unwrap_err();
            assert_projection(&error, "Error", message);
            peer.await.unwrap();
            database.shutdown().await;
        }
    }

    #[tokio::test]
    async fn query_fin_is_wrapped_and_a_rust_only_session_failure_remains_unknown() {
        for read_only in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let target = target(listener.local_addr().unwrap().port());
            let database = if read_only {
                Database::new(
                    Some(PostgresConfig {
                        targets: vec![target],
                    }),
                    None,
                )
            } else {
                Database::new(None, Some(target))
            };
            let peer = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                startup(&mut socket).await;
                socket
                    .write_all(b"R\0\0\0\x08\0\0\0\0Z\0\0\0\x05I")
                    .await
                    .unwrap();
                // Drain the original operation's full frame(s) before FIN,
                // so unread client bytes cannot turn fixture close into RST.
                loop {
                    let kind = socket.read_u8().await.unwrap();
                    let length = socket.read_u32().await.unwrap() as usize;
                    let mut body = vec![0; length - 4];
                    socket.read_exact(&mut body).await.unwrap();
                    if kind == b'Q' || kind == b'S' {
                        break;
                    }
                }
                socket.shutdown().await.unwrap();
            });
            let error = database
                .query_typed(read_only, "SELECT 1", &[])
                .await
                .unwrap_err();
            if read_only {
                assert!(query_error(&error).is_none());
            } else {
                assert_projection(
                    &error,
                    "QueryFailedError",
                    "Connection terminated unexpectedly",
                );
            }
            peer.await.unwrap();
            database.shutdown().await;
        }
    }

    fn certificate(common_names: &[&str], sans: &[(&str, &str)]) -> X509 {
        certificate_with_key(common_names, sans, false).0
    }

    fn certificate_with_key(
        common_names: &[&str],
        sans: &[(&str, &str)],
        expired: bool,
    ) -> (X509, PKey<openssl::pkey::Private>) {
        signed_certificate(common_names, sans, expired, None)
    }

    fn signed_certificate(
        common_names: &[&str],
        sans: &[(&str, &str)],
        expired: bool,
        issuer: Option<(&X509Ref, &openssl::pkey::PKeyRef<openssl::pkey::Private>)>,
    ) -> (X509, PKey<openssl::pkey::Private>) {
        let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
        let key = PKey::from_ec_key(EcKey::generate(&group).unwrap()).unwrap();
        let mut name = X509NameBuilder::new().unwrap();
        for common_name in common_names {
            name.append_entry_by_nid(Nid::COMMONNAME, common_name)
                .unwrap();
        }
        let name = name.build();
        let mut certificate = X509::builder().unwrap();
        certificate.set_version(2).unwrap();
        certificate.set_subject_name(&name).unwrap();
        certificate
            .set_issuer_name(
                issuer
                    .map(|(certificate, _)| certificate.subject_name())
                    .unwrap_or(&name),
            )
            .unwrap();
        certificate.set_pubkey(&key).unwrap();
        let not_before = if expired {
            Asn1Time::from_unix(946684800)
        } else {
            Asn1Time::days_from_now(0)
        }
        .unwrap();
        let not_after = if expired {
            Asn1Time::from_unix(978307200)
        } else {
            Asn1Time::days_from_now(1)
        }
        .unwrap();
        certificate.set_not_before(&not_before).unwrap();
        certificate.set_not_after(&not_after).unwrap();
        if !sans.is_empty() {
            let mut names = SubjectAlternativeName::new();
            for (kind, value) in sans {
                match *kind {
                    "dns" => {
                        names.dns(value);
                    }
                    "ip" => {
                        names.ip(value);
                    }
                    "uri" => {
                        names.uri(value);
                    }
                    "email" => {
                        names.email(value);
                    }
                    _ => panic!("unsupported fixture SAN"),
                }
            }
            let extension = names
                .build(&certificate.x509v3_context(None, None))
                .unwrap();
            certificate.append_extension(extension).unwrap();
        }
        certificate
            .sign(
                issuer.map(|(_, key)| key).unwrap_or(&key),
                MessageDigest::sha256(),
            )
            .unwrap();
        (certificate.build(), key)
    }

    #[test]
    fn hostname_errors_use_the_actual_cert_names_and_bun_presentation() {
        let certificate = certificate(
            &["ignored.example"],
            &[
                ("dns", "wrong.example.invalid"),
                ("ip", "2001:db8::1"),
                ("uri", "https://wrong.example.invalid"),
            ],
        );
        assert_eq!(
            hostname_message("localhost.", &certificate).unwrap(),
            "Hostname/IP does not match certificate's altnames: Host: localhost. is not in the cert's altnames: DNS:wrong.example.invalid, IP Address:2001:DB8:0:0:0:0:0:1, URI:https://wrong.example.invalid"
        );
        assert_eq!(
            hostname_message("127.0.0.2", &certificate).unwrap(),
            "Hostname/IP does not match certificate's altnames: IP: 127.0.0.2 is not in the cert's list: 2001:db8::1"
        );
        assert!(hostname_message("wrong.example.invalid", &certificate).is_none());
        assert!(hostname_message("2001:db8::1", &certificate).is_none());
        assert!(
            hostname_message("例.example", &certificate)
                .unwrap()
                .contains("Host: 例.example.")
        );
        let certificate = self::certificate(&["first.example", "second.example"], &[]);
        assert_eq!(
            hostname_message("different.example", &certificate).unwrap(),
            "Hostname/IP does not match certificate's altnames: Host: different.example. is not cert's CN: first.example,second.example"
        );
        assert!(hostname_message("second.example", &certificate).is_none());
        let certificate = self::certificate(&[], &[("email", "user@example.invalid")]);
        assert_eq!(
            hostname_message("localhost", &certificate).unwrap(),
            "Hostname/IP does not match certificate's altnames: Cert does not contain a DNS name"
        );
    }

    #[test]
    fn supported_certificate_matching_and_escaping_follow_pinned_bun_rules() {
        assert!(dns_matches("API.Example.COM", "*.example.com"));
        assert!(dns_matches("api.example.com", "a*i.example.com"));
        assert!(!dns_matches("api.example.com", "*.*.com"));
        assert!(!dns_matches("example.com", "*.com"));
        assert!(!dns_matches("api.example.com", "xn--*.example.com"));
        assert!(!dns_matches("localhost/.example.com", "*.example.com"));
        assert_eq!(alt_name("plain.example"), "plain.example");
        assert_eq!(alt_name("a,b\"\\'\n"), "\"a\\u002cb\\\"\\\\'\\u000a\"");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn resolver_statuses_use_the_eai_domain_without_reinterpreting_errno() {
        assert_eq!(resolver_code(libc::EAI_AGAIN), Some("EAI_AGAIN"));
        assert_eq!(resolver_code(libc::EAI_NONAME), Some("ENOTFOUND"));
        assert_eq!(resolver_code(libc::EAI_NODATA), Some("ENOTFOUND"));
        assert_eq!(resolver_code(libc::EAI_SYSTEM), Some("ESERVFAIL"));
        assert_eq!(resolver_code(0), None);
        assert_eq!(resolver_code(i32::MIN), Some("ENOTIMP"));
    }
    #[test]
    fn symbolic_certificate_errors_do_not_assume_identical_engine_numbers() {
        assert_eq!(
            certificate_message(openssl_sys::X509_V_ERR_CERT_HAS_EXPIRED),
            Some("certificate has expired")
        );
        assert_eq!(
            certificate_message(openssl_sys::X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT),
            Some("self signed certificate")
        );
        assert_eq!(
            certificate_message(openssl_sys::X509_V_ERR_INVALID_CA),
            Some("invalid CA certificate")
        );
        assert_eq!(
            certificate_message(openssl_sys::X509_V_ERR_CERT_NOT_YET_VALID),
            Some("certificate is not yet valid")
        );
        assert_eq!(certificate_message(0), None);
        assert_eq!(certificate_message(i32::MIN), None);
    }

    #[tokio::test]
    async fn real_expired_and_self_signed_certificates_retain_their_distinct_messages() {
        use openssl::ssl::{SslAcceptor, SslMethod};
        use std::io::{Read, Write};
        for case in ["self-signed", "expired", "omitted-issuer"] {
            let expired = case == "expired";
            let (issuer, issuer_key) = certificate_with_key(&["omitted private CA"], &[], false);
            let (certificate, key) = signed_certificate(
                &["localhost"],
                &[("dns", "localhost")],
                expired,
                (case == "omitted-issuer").then_some((issuer.as_ref(), issuer_key.as_ref())),
            );
            let mut acceptor = SslAcceptor::mozilla_intermediate(SslMethod::tls()).unwrap();
            acceptor.set_certificate(&certificate).unwrap();
            acceptor.set_private_key(&key).unwrap();
            let acceptor = acceptor.build();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let mut target = target(listener.local_addr().unwrap().port());
            target.tls = Some(Tls {
                reject_unauthorized: true,
                ca: if expired {
                    vec![String::from_utf8(certificate.to_pem().unwrap()).unwrap()]
                } else {
                    vec![]
                },
                cert: None,
                key: None,
                passphrase: None,
                servername: "localhost".into(),
                sni: true,
                min_version: TlsVersion::V12,
                max_version: TlsVersion::V13,
                ciphers: "DEFAULT".into(),
            });
            let peer = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let mut request = [0; 8];
                socket.read_exact(&mut request).unwrap();
                assert_eq!(request, [0, 0, 0, 8, 4, 210, 22, 47]);
                socket.write_all(b"S").unwrap();
                assert!(acceptor.accept(socket).is_err());
            });
            let database = Database::new(None, Some(target));
            let error = database
                .query_typed(false, "SELECT 1", &[])
                .await
                .unwrap_err();
            assert_projection(
                &error,
                "Error",
                match case {
                    "expired" => "certificate has expired",
                    "omitted-issuer" => "unable to verify the first certificate",
                    _ => "self signed certificate",
                },
            );
            if case == "omitted-issuer" {
                let QueryError::Fault(fault) = &error else {
                    panic!("expected concrete verification fault")
                };
                assert_eq!(
                    fault.tls_verify_code(),
                    Some(openssl_sys::X509_V_ERR_UNABLE_TO_GET_ISSUER_CERT_LOCALLY)
                );
                assert_eq!(fault.tls_verify_depth(), Some(0));
                assert_eq!(fault.tls_verify_chain_len(), Some(1));
            }
            database.shutdown().await;
            peer.join().unwrap();
        }
    }

    #[test]
    fn idna_and_scoped_ipv6_match_the_typed_certificate_without_changing_public_spelling() {
        let certificate = certificate(&[], &[("dns", "xn--r8jz45g.example"), ("ip", "fe80::1")]);
        assert!(hostname_message("例え.example", &certificate).is_none());
        assert!(hostname_message("fe80::1%eth0", &certificate).is_none());
        assert!(
            hostname_message("fe80::2%eth0", &certificate)
                .unwrap()
                .contains("IP: fe80::2%eth0 is not in the cert's list: fe80::1")
        );
        assert!(
            hostname_message("別.example", &certificate)
                .unwrap()
                .contains("Host: 別.example.")
        );
        assert!(typed_ip("fe80::1%zone_unsupported").is_none());
        assert!(typed_ip("127.0.0.1%zone").is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_errno_and_remaining_eai_codes_follow_the_pinned_bun_source_domains() {
        assert_eq!(socket_error_code(libc::EHOSTUNREACH), Some("EHOSTUNREACH"));
        assert_eq!(socket_error_code(libc::ETIMEDOUT), Some("ETIMEDOUT"));
        assert_eq!(socket_error_code(libc::EPIPE), Some("EPIPE"));
        assert_eq!(socket_error_code(libc::EOPNOTSUPP), Some("ENOTSUP"));
        assert_eq!(socket_error_code(libc::EINPROGRESS), None);
        assert_eq!(socket_error_code(libc::ENETRESET), None);
        assert_eq!(socket_error_code(i32::MIN), None);
        assert_eq!(resolver_code(libc::EAI_FAIL), Some("EBADRESP"));
        assert_eq!(resolver_code(libc::EAI_SOCKTYPE), Some("ECONNREFUSED"));
        assert_eq!(resolver_code(libc::EAI_FAMILY), Some("EBADFAMILY"));
        assert_eq!(resolver_code(libc::EAI_OVERFLOW), Some("ENOTIMP"));
        #[cfg(target_env = "gnu")]
        {
            assert_eq!(resolver_code(-100), Some("ETIMEOUT"));
            assert_eq!(resolver_code(-105), Some("EBADSTR"));
        }
    }
    #[test]
    fn numeric_endpoint_text_preserves_the_original_expanded_and_scoped_spelling() {
        let address: SocketAddr = "[2001:db8::1]:5432".parse().unwrap();
        assert_eq!(
            tcp_endpoint(Some("2001:0DB8:0:0:0:0:0:1"), address),
            "2001:0DB8:0:0:0:0:0:1:5432"
        );
        let address: SocketAddr = "[fe80::1%3]:5432".parse().unwrap();
        assert_eq!(
            tcp_endpoint(Some("fe80::1%eth0"), address),
            "fe80::1%eth0:5432"
        );
        assert_eq!(
            tcp_endpoint(Some("database.example"), address),
            "fe80::1:5432"
        );
        assert_eq!(
            tcp_endpoint(Some("127.0.0.2"), "127.0.0.1:5432".parse().unwrap()),
            "127.0.0.1:5432"
        );
    }
}
