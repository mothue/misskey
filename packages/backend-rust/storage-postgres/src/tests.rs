use super::*;
use std::sync::Arc;

struct FixtureResolver {
    calls: SyncMutex<Vec<(String, u16)>>,
    result: ResolverResult,
}

enum ResolverResult {
    Addresses(Vec<std::net::SocketAddr>),
    OsError(i32),
    Pending {
        entered: tokio::sync::Notify,
        dropped: Arc<AtomicBool>,
    },
}

impl FixtureResolver {
    fn addresses(addresses: Vec<std::net::SocketAddr>) -> Arc<Self> {
        Arc::new(Self {
            calls: SyncMutex::new(Vec::new()),
            result: ResolverResult::Addresses(addresses),
        })
    }

    fn calls(&self) -> Vec<(String, u16)> {
        self.calls.lock().unwrap().clone()
    }
}

impl transport::Resolver for FixtureResolver {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> transport::ResolveFuture<'a> {
        Box::pin(async move {
            self.calls.lock().unwrap().push((host.to_owned(), port));
            match &self.result {
                ResolverResult::Addresses(addresses) => Ok(addresses.clone()),
                ResolverResult::OsError(code) => Err(std::io::Error::from_raw_os_error(*code)),
                ResolverResult::Pending { entered, dropped } => {
                    struct MarkDropped(Arc<AtomicBool>);
                    impl Drop for MarkDropped {
                        fn drop(&mut self) {
                            self.0.store(true, Ordering::Release);
                        }
                    }
                    let _guard = MarkDropped(Arc::clone(dropped));
                    entered.notify_one();
                    std::future::pending().await
                }
            }
        })
    }
}

fn resolved_target(port: u16) -> Target {
    let mut target = target_on_port(port);
    target.host = "private-resolver.transport.test".into();
    target
}

fn refused_tcp_address() -> (socket2::Socket, std::net::SocketAddr) {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )
    .unwrap();
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
    socket.bind(&address.into()).unwrap();
    let address = socket.local_addr().unwrap().as_socket().unwrap();
    (socket, address)
}

async fn startup_ready(socket: &mut tokio::net::TcpStream) {
    use tokio::io::AsyncWriteExt;
    read_startup(socket).await;
    socket
        .write_all(b"R\0\0\0\x08\0\0\0\0K\0\0\0\x0c\0\0\0\x11\0\0\0\x17Z\0\0\0\x05I")
        .await
        .unwrap();
}

async fn empty_query(socket: &mut tokio::net::TcpStream) {
    use tokio::io::AsyncWriteExt;
    while read_frontend(socket).await.0 != b'S' {}
    socket
        .write_all(b"1\0\0\0\x04t\0\0\0\x06\0\0n\0\0\0\x04Z\0\0\0\x05I")
        .await
        .unwrap();
    while read_frontend(socket).await.0 != b'S' {}
    socket
        .write_all(b"2\0\0\0\x04C\0\0\0\x0dSELECT 0\0Z\0\0\0\x05I")
        .await
        .unwrap();
}

fn fixture() -> PostgresConfig {
    serde_json::from_str(include_str!("../tests/fixtures/local.json")).unwrap()
}

#[test]
fn options_and_explicit_connection_settings_survive_normalization() {
    let mut target = fixture().targets.remove(0);
    target.options = Some("-c search_path=read_scope -c timezone=UTC".into());
    target.lock_timeout_ms = Some(45);
    target.query_timeout_ms = Some(120);
    target.channel_binding = true;
    target.keep_alive = true;
    target.keep_alive_initial_delay_ms = Some(2000);
    let config = config(&target).unwrap();
    assert_eq!(
        config.get_options(),
        Some(
            "-c search_path=read_scope -c timezone=UTC -c statement_timeout=10000 -c lock_timeout=45"
        )
    );
    assert_eq!(config.get_ssl_mode(), SslMode::Disable);
    assert_eq!(config.get_channel_binding(), ChannelBinding::Prefer);
    assert!(config.get_keepalives());
    assert_eq!(config.get_keepalives_idle(), Duration::from_secs(2));
}

#[test]
fn deserialization_does_not_accept_unknown_connection_or_tls_settings() {
    let mut wire: serde_json::Value =
        serde_json::from_str(include_str!("../tests/fixtures/local.json")).unwrap();
    wire["targets"][0]["customPasswordResolver"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<PostgresConfig>(wire).is_err());
    assert_eq!(Failure.to_string(), "Rust backend unavailable");
    assert_eq!(format!("{Failure:?}"), "Failure");
}

#[test]
fn typed_query_errors_expose_only_explicit_legacy_messages_and_redact_formatting() {
    let error = QueryError::Server {
        message: "private server input".into(),
    };
    assert_eq!(error.legacy_message(), Some("private server input"));
    assert_eq!(format!("{error:?}"), "QueryError::Server");
    assert_eq!(error.to_string(), "Rust backend unavailable");
    assert_eq!(
        QueryError::QueryTimeout.legacy_message(),
        Some("Query read timeout")
    );
    assert_eq!(QueryError::Unavailable.legacy_message(), None);
}

fn target_on_port(port: u16) -> Target {
    let mut target = fixture().targets.remove(0);
    target.host = "127.0.0.1".into();
    target.port = port;
    target
}

fn retained_fault(error: &QueryError) -> &QueryFault {
    match error {
        QueryError::Fault(fault) => fault,
        other => panic!("expected retained fault, got {other:?}"),
    }
}

async fn read_startup(socket: &mut tokio::net::TcpStream) {
    use tokio::io::AsyncReadExt;
    let length = socket.read_u32().await.unwrap() as usize;
    let mut startup = vec![0; length - 4];
    socket.read_exact(&mut startup).await.unwrap();
}

async fn read_frontend(socket: &mut tokio::net::TcpStream) -> (u8, Vec<u8>) {
    use tokio::io::AsyncReadExt;
    let kind = socket.read_u8().await.unwrap();
    let length = socket.read_u32().await.unwrap() as usize;
    let mut payload = vec![0; length - 4];
    socket.read_exact(&mut payload).await.unwrap();
    (kind, payload)
}

async fn write_error(socket: &mut tokio::net::TcpStream) {
    use tokio::io::AsyncWriteExt;
    let payload = b"SFATAL\0C28000\0Mfixture startup rejection\0Dprivate detail\0\0";
    socket.write_u8(b'E').await.unwrap();
    socket.write_u32((payload.len() + 4) as u32).await.unwrap();
    socket.write_all(payload).await.unwrap();
}

#[tokio::test]
async fn typed_connect_refusal_retains_actual_io_context_and_opaque_callers() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let database = Database::new(None, Some(target_on_port(port)));
    let error = database
        .query_typed(false, "private SQL", &[])
        .await
        .unwrap_err();
    let fault = retained_fault(&error);
    assert_eq!(fault.phase(), FaultPhase::Connect);
    assert_eq!(fault.kind(), FaultKind::Io);
    assert_eq!(fault.io_kind(), Some(std::io::ErrorKind::ConnectionRefused));
    assert_eq!(fault.connect_stage(), Some(ConnectStage::SocketConnect));
    assert!(fault.raw_os_error().is_some());
    assert_eq!(error.legacy_message(), None);
    assert_eq!(format!("{error:?}"), "QueryError::Fault");
    assert_eq!(format!("{fault:?}"), "QueryFault");
    assert_eq!(error.to_string(), "Rust backend unavailable");
    assert!(std::error::Error::source(&error).is_none());
    assert!(std::error::Error::source(fault).is_none());
    assert!(matches!(
        database.query(false, "private SQL", &[]).await,
        Err(Failure)
    ));
    database.shutdown().await;
}

#[tokio::test]
async fn typed_connect_deadline_retains_its_phase_and_closes_the_socket() {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut target = target_on_port(listener.local_addr().unwrap().port());
    target.connect_timeout_ms = Some(50);
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), socket.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert!(!bytes.is_empty());
    });
    let database = Database::new(None, Some(target));
    let error = database
        .query_typed(false, "SELECT 1", &[])
        .await
        .unwrap_err();
    let fault = retained_fault(&error);
    assert_eq!(fault.phase(), FaultPhase::Connect);
    assert_eq!(fault.kind(), FaultKind::ConnectTimeout);
    assert_eq!(fault.io_kind(), None);
    assert_eq!(error.legacy_message(), None);
    peer.await.unwrap();
    database.shutdown().await;
}

#[tokio::test]
async fn startup_server_error_is_retained_before_query_wrapping() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = target_on_port(listener.local_addr().unwrap().port());
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_startup(&mut socket).await;
        write_error(&mut socket).await;
    });
    let database = Database::new(None, Some(target));
    let error = database
        .query_typed(false, "private SQL", &[])
        .await
        .unwrap_err();
    let fault = retained_fault(&error);
    assert_eq!(fault.phase(), FaultPhase::Connect);
    assert_eq!(fault.kind(), FaultKind::Server);
    assert_eq!(fault.server_code(), Some("28000"));
    assert_eq!(fault.server_message(), Some("fixture startup rejection"));
    assert_eq!(error.legacy_message(), None);
    assert_eq!(format!("{fault:?}"), "QueryFault");
    peer.await.unwrap();
    database.shutdown().await;
}

#[tokio::test]
async fn startup_disconnect_retains_closed_source_without_a_query_message() {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = target_on_port(listener.local_addr().unwrap().port());
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_startup(&mut socket).await;
        socket.shutdown().await.unwrap();
    });
    let database = Database::new(None, Some(target));
    let error = database
        .query_typed(false, "SELECT 1", &[])
        .await
        .unwrap_err();
    let fault = retained_fault(&error);
    assert_eq!(fault.phase(), FaultPhase::Connect);
    assert_eq!(fault.kind(), FaultKind::Closed);
    assert_eq!(error.legacy_message(), None);
    peer.await.unwrap();
    database.shutdown().await;
}

#[tokio::test]
async fn session_setup_error_retains_its_distinct_phase() {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = target_on_port(listener.local_addr().unwrap().port());
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_startup(&mut socket).await;
        socket
            .write_all(b"R\0\0\0\x08\0\0\0\0Z\0\0\0\x05I")
            .await
            .unwrap();
        let (kind, payload) = read_frontend(&mut socket).await;
        assert_eq!(kind, b'Q');
        assert_eq!(payload, b"SET default_transaction_read_only = on\0");
        write_error(&mut socket).await;
    });
    let database = Database::new(
        Some(PostgresConfig {
            targets: vec![target],
        }),
        None,
    );
    let error = database
        .query_typed(true, "private SQL", &[])
        .await
        .unwrap_err();
    let fault = retained_fault(&error);
    assert_eq!(fault.phase(), FaultPhase::SessionSetup);
    assert_eq!(fault.kind(), FaultKind::Server);
    assert_eq!(fault.server_code(), Some("28000"));
    peer.await.unwrap();
    database.shutdown().await;
}

#[tokio::test]
async fn mid_query_disconnect_retains_the_driver_error_and_discards_connection() {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = target_on_port(listener.local_addr().unwrap().port());
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_startup(&mut socket).await;
        socket
            .write_all(b"R\0\0\0\x08\0\0\0\0Z\0\0\0\x05I")
            .await
            .unwrap();
        let (kind, _) = read_frontend(&mut socket).await;
        assert_eq!(kind, b'P');
        // A truncated frame forces a concrete driver IO error. The query's
        // response channel otherwise reports only that the connection closed.
        socket.write_all(b"E\0\0").await.unwrap();
        socket.shutdown().await.unwrap();
    });
    let database = Database::new(None, Some(target));
    let error = database
        .query_typed(false, "SELECT 1", &[])
        .await
        .unwrap_err();
    let fault = retained_fault(&error);
    assert_eq!(fault.phase(), FaultPhase::Query);
    assert_eq!(fault.kind(), FaultKind::Io);
    assert!(
        fault.io_kind().is_some(),
        "concrete driver IO source survives Closed"
    );
    assert!(database.primary.slots[0].connection.lock().await.is_none());
    peer.await.unwrap();
    database.shutdown().await;
}

#[tokio::test]
async fn typed_query_deadline_keeps_legacy_error_and_cancels_the_same_connection() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut target = target_on_port(listener.local_addr().unwrap().port());
    target.query_timeout_ms = Some(50);
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_startup(&mut socket).await;
        // AuthenticationOk, BackendKeyData for this connection, ReadyForQuery.
        socket
            .write_all(b"R\0\0\0\x08\0\0\0\0K\0\0\0\x0c\0\0\0\x11\0\0\0\x17Z\0\0\0\x05I")
            .await
            .unwrap();
        assert_eq!(read_frontend(&mut socket).await.0, b'P');
        let mut pending_query = Vec::new();
        tokio::time::timeout(
            Duration::from_secs(1),
            socket.read_to_end(&mut pending_query),
        )
        .await
        .unwrap()
        .unwrap();
        let (mut cancellation, _) = tokio::time::timeout(Duration::from_secs(1), listener.accept())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cancellation.read_u32().await.unwrap(), 16);
        assert_eq!(cancellation.read_u32().await.unwrap(), 80877102);
        assert_eq!(cancellation.read_u32().await.unwrap(), 17);
        assert_eq!(cancellation.read_u32().await.unwrap(), 23);
        let mut remaining = Vec::new();
        cancellation.read_to_end(&mut remaining).await.unwrap();
        assert!(remaining.is_empty());
    });
    let database = Database::new(None, Some(target));
    let error = database
        .query_typed(false, "SELECT 1", &[])
        .await
        .unwrap_err();
    assert!(matches!(error, QueryError::QueryTimeout));
    assert_eq!(error.legacy_message(), Some("Query read timeout"));
    assert!(database.primary.slots[0].connection.lock().await.is_none());
    database.shutdown().await;
    peer.await.unwrap();
    assert!(database.primary.cancellations.lock().unwrap().is_empty());
}

#[tokio::test]
async fn selected_read_failure_never_opens_the_primary_connection() {
    let refused = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let read = target_on_port(refused.local_addr().unwrap().port());
    drop(refused);
    let primary = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let database = Database::new(
        Some(PostgresConfig {
            targets: vec![read],
        }),
        Some(target_on_port(primary.local_addr().unwrap().port())),
    );
    let error = database
        .query_typed(true, "SELECT 1", &[])
        .await
        .unwrap_err();
    assert_eq!(
        retained_fault(&error).io_kind(),
        Some(std::io::ErrorKind::ConnectionRefused)
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(25), primary.accept())
            .await
            .is_err()
    );
    database.shutdown().await;
}

#[tokio::test]
async fn typed_shutdown_interrupts_startup_and_remains_unavailable() {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut target = target_on_port(listener.local_addr().unwrap().port());
    target.connect_timeout_ms = None;
    let database = Arc::new(Database::new(None, Some(target)));
    let pending = {
        let database = Arc::clone(&database);
        tokio::spawn(async move { database.query_typed(false, "SELECT 1", &[]).await })
    };
    let (mut socket, _) = listener.accept().await.unwrap();
    read_startup(&mut socket).await;
    database.shutdown().await;
    assert!(matches!(
        pending.await.unwrap(),
        Err(QueryError::Unavailable)
    ));
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), socket.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        database.query_typed(false, "SELECT 1", &[]).await,
        Err(QueryError::Unavailable)
    ));
}

#[tokio::test]
async fn a_later_query_connects_again_after_a_failed_startup() {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = target_on_port(listener.local_addr().unwrap().port());
    let peer = tokio::spawn(async move {
        let (mut failed, _) = listener.accept().await.unwrap();
        read_startup(&mut failed).await;
        write_error(&mut failed).await;
        drop(failed);
        let (mut socket, _) = listener.accept().await.unwrap();
        read_startup(&mut socket).await;
        socket
            .write_all(b"R\0\0\0\x08\0\0\0\0Z\0\0\0\x05I")
            .await
            .unwrap();
        loop {
            if read_frontend(&mut socket).await.0 == b'S' {
                break;
            }
        }
        // ParseComplete, empty ParameterDescription, NoData, ReadyForQuery.
        socket
            .write_all(b"1\0\0\0\x04t\0\0\0\x06\0\0n\0\0\0\x04Z\0\0\0\x05I")
            .await
            .unwrap();
        loop {
            if read_frontend(&mut socket).await.0 == b'S' {
                break;
            }
        }
        // BindComplete, CommandComplete, ReadyForQuery: an empty result set.
        socket
            .write_all(b"2\0\0\0\x04C\0\0\0\x0dSELECT 0\0Z\0\0\0\x05I")
            .await
            .unwrap();
        // Keep the recovered driver alive until normal shutdown closes it.
        use tokio::io::AsyncReadExt;
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), socket.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
    });
    let database = Database::new(None, Some(target));
    let error = database
        .query_typed(false, "SELECT 1", &[])
        .await
        .unwrap_err();
    assert_eq!(retained_fault(&error).phase(), FaultPhase::Connect);
    assert!(
        database
            .query_typed(false, "SELECT 1", &[])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(database.primary.slots[0].connection.lock().await.is_some());
    database.shutdown().await;
    peer.await.unwrap();
}

#[test]
fn tls_configuration_faults_retain_classification_but_redact_source_text() {
    let error = X509::from_pem(b"private invalid PEM").unwrap_err();
    let fault = QueryFault::tls_configuration(error);
    assert_eq!(fault.phase(), FaultPhase::Connect);
    assert_eq!(fault.kind(), FaultKind::Tls);
    assert_eq!(fault.server_message(), None);
    assert_eq!(format!("{fault:?}"), "QueryFault");
    assert_eq!(fault.to_string(), "Rust backend unavailable");
    assert!(std::error::Error::source(&fault).is_none());
}

#[tokio::test]
async fn typed_query_error_extracts_only_the_postgres_protocol_message() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (client_socket, mut server_socket) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        let startup_length = server_socket.read_u32().await.unwrap() as usize;
        let mut startup = vec![0; startup_length - 4];
        server_socket.read_exact(&mut startup).await.unwrap();
        // AuthenticationOk, then ReadyForQuery. No real PostgreSQL is needed.
        server_socket
            .write_all(b"R\0\0\0\x08\0\0\0\0Z\0\0\0\x05I")
            .await
            .unwrap();
        loop {
            let kind = match server_socket.read_u8().await {
                Ok(kind) => kind,
                Err(_) => return,
            };
            let length = server_socket.read_u32().await.unwrap() as usize;
            let mut payload = vec![0; length - 4];
            server_socket.read_exact(&mut payload).await.unwrap();
            if kind == b'P' {
                let payload = b"SERROR\0C2201X\0MOFFSET must not be negative\0Dprivate server detail and parameters\0\0";
                server_socket.write_u8(b'E').await.unwrap();
                server_socket
                    .write_u32((payload.len() + 4) as u32)
                    .await
                    .unwrap();
                server_socket.write_all(payload).await.unwrap();
            } else if kind == b'S' {
                server_socket.write_all(b"Z\0\0\0\x05I").await.unwrap();
            }
        }
    });
    let (client, driver) = Config::new()
        .user("fixture")
        .dbname("fixture")
        .ssl_mode(SslMode::Disable)
        .connect_raw(client_socket, NoTls)
        .await
        .unwrap();
    let driver = tokio::spawn(async move {
        let _ = driver.await;
    });
    let error = client.query("SELECT 1 OFFSET -1", &[]).await.unwrap_err();
    let error = QueryError::from_postgres(error);
    assert_eq!(error.legacy_message(), Some("OFFSET must not be negative"));
    assert_eq!(format!("{error:?}"), "QueryError::Server");
    assert!(!format!("{error:?}").contains("private"));
    drop(client);
    driver.abort();
    server.abort();
}

#[tokio::test]
async fn absent_configuration_and_stopped_adapters_are_unavailable() {
    let adapter = OnlineUsers::new(None);
    assert_eq!(adapter.count_at(UNIX_EPOCH).await, Err(Failure));
    adapter.shutdown().await;
    adapter.shutdown().await;
    assert_eq!(adapter.count_at(UNIX_EPOCH).await, Err(Failure));
    assert_eq!(
        OnlineUsers::new(Some(PostgresConfig { targets: vec![] }))
            .count_at(UNIX_EPOCH)
            .await,
        Err(Failure)
    );
}

#[tokio::test(start_paused = true)]
async fn operation_deadline_includes_waiting_for_a_shared_connection() {
    let adapter = OnlineUsers::new(Some(fixture()));
    let _held = adapter.slots[0].connection.lock().await;
    let started = Instant::now();
    assert_eq!(adapter.count_at(UNIX_EPOCH).await, Err(Failure));
    assert_eq!(Instant::now() - started, OPERATION_LIMIT);
    let started = Instant::now();
    adapter.shutdown().await;
    assert!(Instant::now() - started <= Duration::from_secs(1));
}

#[tokio::test]
async fn a_configured_connect_deadline_closes_a_stalled_handshake() {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut wire = fixture();
    wire.targets[0].port = listener.local_addr().unwrap().port();
    wire.targets[0].connect_timeout_ms = Some(50);
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), socket.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert!(!bytes.is_empty());
    });
    let started = Instant::now();
    let adapter = OnlineUsers::new(Some(wire));
    assert_eq!(adapter.count_at(UNIX_EPOCH).await, Err(Failure));
    assert!(started.elapsed() < Duration::from_secs(1));
    peer.await.unwrap();
    adapter.shutdown().await;
}

#[tokio::test]
async fn shutdown_interrupts_connection_establishment_before_a_driver_exists() {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut wire = fixture();
    wire.targets[0].port = listener.local_addr().unwrap().port();
    wire.targets[0].connect_timeout_ms = None;
    let adapter = Arc::new(OnlineUsers::new(Some(wire)));
    let pending = {
        let adapter = Arc::clone(&adapter);
        tokio::spawn(async move { adapter.count_at(UNIX_EPOCH).await })
    };
    let (mut peer, _) = listener.accept().await.unwrap();
    let started = Instant::now();
    adapter.shutdown().await;
    assert_eq!(pending.await.unwrap(), Err(Failure));
    assert!(started.elapsed() < Duration::from_secs(1));
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), peer.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
#[ignore = "requires explicit MISSKEY_POSTGRES_TEST_CONFIG for dedicated online-adapter database"]
async fn dedicated_postgres_lifecycle() {
    let path =
        std::env::var_os("MISSKEY_POSTGRES_TEST_CONFIG").expect("dedicated config path required");
    let mut fixture: PostgresConfig =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(fixture.targets.len(), 1);
    let target = &mut fixture.targets[0];
    assert_eq!(
        target.database, "online-adapter",
        "test must use its dedicated database"
    );
    assert!(
        target.tls.is_none(),
        "plaintext lifecycle fixture only; TLS is tested separately"
    );
    let schema = format!("online_adapter_{}", std::process::id());
    let app = schema.clone();
    let (admin, driver) = config(target).unwrap().connect(NoTls).await.unwrap();
    let driver = tokio::spawn(async move {
        let _ = driver.await;
    });
    admin.batch_execute(&format!("CREATE SCHEMA {schema}; CREATE TABLE {schema}.\"user\" (id integer, \"lastActiveDate\" timestamptz);")).await.unwrap();
    target.options = Some(format!(
        "-c search_path={schema} -c timezone=Pacific/Honolulu"
    ));
    target.application_name = Some(app.clone());
    target.query_timeout_ms = Some(150);
    let adapter = Arc::new(OnlineUsers::new(Some(fixture.clone())));
    let now = UNIX_EPOCH + Duration::from_millis(1_700_000_000_123);
    assert_eq!(adapter.count_at(now).await, Ok(0.0));
    let cutoff = system_time(online_users_cutoff(now).unwrap()).unwrap();
    admin.execute(&format!("INSERT INTO {schema}.\"user\" VALUES (0,NULL),(1,$1::timestamptz-interval '1 millisecond'),(2,$1),(3,$1::timestamptz+interval '1 microsecond'),(4,$1::timestamptz+interval '1 millisecond'),(5,$1::timestamptz+interval '10 minute'),(6,$1::timestamptz+interval '1 day')"), &[&cutoff]).await.unwrap();
    assert_eq!(adapter.count_at(now).await, Ok(4.0));
    let mut concurrent = Vec::new();
    for _ in 0..16 {
        let adapter = Arc::clone(&adapter);
        concurrent.push(tokio::spawn(async move { adapter.count_at(now).await }));
    }
    for count in concurrent {
        assert_eq!(count.await.unwrap(), Ok(4.0));
    }
    let sessions = || async {
        // pg_stat_activity caches its snapshot for the observing transaction.
        // Lock/deadline checks below intentionally keep that transaction open.
        admin
            .execute("SELECT pg_stat_clear_snapshot()", &[])
            .await
            .unwrap();
        admin
            .query_one(
                "SELECT COUNT(*) FROM pg_stat_activity WHERE application_name=$1",
                &[&app],
            )
            .await
            .unwrap()
            .get::<_, i64>(0)
    };
    assert_eq!(
        sessions().await,
        1,
        "one connection is shared by concurrent operations"
    );
    let pid: i32;
    {
        let stored = adapter.slots[0].connection.lock().await;
        let client = &stored.as_ref().unwrap().client;
        pid = client
            .query_one("SELECT pg_backend_pid()", &[])
            .await
            .unwrap()
            .get(0);
        assert_eq!(
            client
                .query_one("SHOW default_transaction_read_only", &[])
                .await
                .unwrap()
                .get::<_, String>(0),
            "on"
        );
        let error = client
            .execute("INSERT INTO \"user\" VALUES (8,NULL)", &[])
            .await
            .unwrap_err();
        assert_eq!(error.code().map(|code| code.code()), Some("25006"));
    }
    admin
        .query_one("SELECT pg_terminate_backend($1)", &[&pid])
        .await
        .unwrap();
    for _ in 0..100 {
        if adapter.slots[0]
            .connection
            .lock()
            .await
            .as_ref()
            .unwrap()
            .client
            .is_closed()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        adapter.count_at(now).await,
        Ok(4.0),
        "a subsequent operation can establish a new connection"
    );
    admin
        .batch_execute(&format!(
            "BEGIN; LOCK TABLE {schema}.\"user\" IN ACCESS EXCLUSIVE MODE;"
        ))
        .await
        .unwrap();
    let started = Instant::now();
    assert_eq!(adapter.count_at(now).await, Err(Failure));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "configured query deadline is respected"
    );
    for _ in 0..100 {
        if sessions().await == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        sessions().await,
        0,
        "timed-out query and connection are discarded"
    );
    admin.batch_execute("ROLLBACK").await.unwrap();
    assert_eq!(adapter.count_at(now).await, Ok(4.0));
    admin
        .batch_execute(&format!(
            "BEGIN; LOCK TABLE {schema}.\"user\" IN ACCESS EXCLUSIVE MODE;"
        ))
        .await
        .unwrap();
    let pending = {
        let adapter = Arc::clone(&adapter);
        tokio::spawn(async move { adapter.count_at(now).await })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    for _ in 0..100 {
        if sessions().await == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(sessions().await, 0, "cancelled operation drops its driver");
    admin.batch_execute("ROLLBACK").await.unwrap();
    assert_eq!(adapter.count_at(now).await, Ok(4.0));
    admin
        .batch_execute(&format!(
            "BEGIN; LOCK TABLE {schema}.\"user\" IN ACCESS EXCLUSIVE MODE;"
        ))
        .await
        .unwrap();
    let pending = {
        let adapter = Arc::clone(&adapter);
        tokio::spawn(async move { adapter.count_at(now).await })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    let started = Instant::now();
    adapter.shutdown().await;
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(pending.await.unwrap(), Err(Failure));
    assert_eq!(adapter.count_at(now).await, Err(Failure));
    admin.batch_execute("ROLLBACK").await.unwrap();
    for _ in 0..100 {
        if sessions().await == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(sessions().await, 0, "shutdown leaves no adapter connection");
    // The API kernel shares the original TLS/cancellation implementation but
    // must route auth writes to the primary, never a read-only replica slot.
    let database = Arc::new(Database::new(
        Some(fixture.clone()),
        Some(fixture.targets[0].clone()),
    ));
    assert_eq!(
        database
            .query(true, "SHOW default_transaction_read_only", &[])
            .await
            .unwrap()[0]
            .get::<_, String>(0),
        "on"
    );
    assert_eq!(
        database
            .query(false, "SHOW default_transaction_read_only", &[])
            .await
            .unwrap()[0]
            .get::<_, String>(0),
        "off"
    );
    let offset = database
        .query_typed(true, "SELECT 1 OFFSET -1", &[])
        .await
        .unwrap_err();
    assert_eq!(offset.legacy_message(), Some("OFFSET must not be negative"));
    let nul = database
        .query_typed(true, "SELECT $1::text", &[&"\0"])
        .await
        .unwrap_err();
    assert_eq!(
        nul.legacy_message(),
        Some("invalid byte sequence for encoding \"UTF8\": 0x00")
    );
    // Existing callers deliberately retain the opaque Failure API.
    assert!(matches!(
        database.query(true, "SELECT 1 OFFSET -1", &[]).await,
        Err(Failure)
    ));
    assert!(
        database
            .query(true, "INSERT INTO \"user\" VALUES (99,NULL)", &[])
            .await
            .is_err()
    );
    database
        .query(false, "INSERT INTO \"user\" VALUES (99,NULL)", &[])
        .await
        .unwrap();
    assert_eq!(
        database
            .query(true, "SELECT count(*) FROM \"user\" WHERE id=99", &[])
            .await
            .unwrap()[0]
            .get::<_, i64>(0),
        1
    );
    admin
        .batch_execute(&format!(
            "BEGIN; LOCK TABLE {schema}.\"user\" IN ACCESS EXCLUSIVE MODE;"
        ))
        .await
        .unwrap();
    let timeout = database
        .query_typed(true, "SELECT * FROM \"user\"", &[])
        .await
        .unwrap_err();
    assert_eq!(timeout.legacy_message(), Some("Query read timeout"));
    for _ in 0..100 {
        let waiting:i64=admin.query_one("SELECT count(*) FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock'",&[&app]).await.unwrap().get(0);
        if waiting == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let waiting:i64=admin.query_one("SELECT count(*) FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock'",&[&app]).await.unwrap().get(0);
    assert_eq!(waiting, 0, "generic query timeout sends CancelRequest");
    admin.batch_execute("ROLLBACK").await.unwrap();
    database
        .query(true, "SELECT * FROM \"user\"", &[])
        .await
        .unwrap();
    database.shutdown().await;
    assert!(database.query(false, "SELECT 1", &[]).await.is_err());
    admin
        .batch_execute(&format!("DROP SCHEMA {schema} CASCADE"))
        .await
        .unwrap();
    drop(admin);
    driver.abort();
}

#[tokio::test]
async fn resolver_failure_retains_the_original_io_facts_without_connecting_or_mapping() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let resolver = Arc::new(FixtureResolver {
        calls: SyncMutex::new(Vec::new()),
        result: ResolverResult::OsError(13),
    });
    let mut target = resolved_target(port);
    target.user = "private-user".into();
    target.password = Some("private-password".into());
    target.options = Some("-c application_name=private-options".into());
    let database = Database::new_with_resolver(None, Some(target), resolver.clone());
    let error = database
        .query_typed(false, "private SQL", &[])
        .await
        .unwrap_err();
    let fault = retained_fault(&error);
    assert_eq!(fault.phase(), FaultPhase::Connect);
    assert_eq!(fault.kind(), FaultKind::Io);
    assert_eq!(
        fault.io_kind(),
        Some(std::io::Error::from_raw_os_error(13).kind())
    );
    assert_eq!(fault.raw_os_error(), Some(13));
    assert_eq!(fault.connect_stage(), Some(ConnectStage::Resolve));
    assert_eq!(
        fault.resolver_host(),
        Some("private-resolver.transport.test")
    );
    assert_eq!(fault.attempt_index(), None);
    assert_eq!(fault.attempted_addr(), None);
    assert_eq!(fault.tls_verify_code(), None);
    assert_eq!(fault.tls_preverify_ok(), None);
    assert_eq!(error.legacy_message(), None);
    assert_eq!(format!("{fault:?}"), "QueryFault");
    assert_eq!(fault.to_string(), "Rust backend unavailable");
    assert_eq!(format!("{error:?}"), "QueryError::Fault");
    assert!(std::error::Error::source(fault).is_none());
    assert!(std::error::Error::source(&error).is_none());
    assert_eq!(
        resolver.calls(),
        vec![("private-resolver.transport.test".into(), port)]
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(25), listener.accept())
            .await
            .is_err()
    );
    assert!(matches!(
        database.query(false, "private SQL", &[]).await,
        Err(Failure)
    ));
    assert_eq!(
        resolver.calls().len(),
        2,
        "only a later operation resolves again"
    );
    let online = OnlineUsers::new_with_resolver(
        Some(PostgresConfig {
            targets: vec![resolved_target(port)],
        }),
        resolver.clone(),
    );
    assert_eq!(online.count_at(UNIX_EPOCH).await, Err(Failure));
    assert_eq!(resolver.calls().len(), 3);
    online.shutdown().await;
    database.shutdown().await;
}

#[tokio::test]
async fn empty_resolver_result_has_no_attempt_or_invented_tls_or_errno_facts() {
    let resolver = FixtureResolver::addresses(Vec::new());
    let database = Database::new_with_resolver(None, Some(resolved_target(5432)), resolver.clone());
    let error = database
        .query_typed(false, "SELECT 1", &[])
        .await
        .unwrap_err();
    let fault = retained_fault(&error);
    assert_eq!(fault.connect_stage(), Some(ConnectStage::Resolve));
    assert_eq!(fault.resolver_address_count(), Some(0));
    assert_eq!(fault.attempt_index(), None);
    assert_eq!(fault.attempted_addr(), None);
    assert_eq!(fault.raw_os_error(), None);
    assert_eq!(fault.tls_verify_code(), None);
    assert_eq!(error.legacy_message(), None);
    assert_eq!(resolver.calls().len(), 1);
    database.shutdown().await;
}

#[tokio::test]
async fn numeric_host_never_calls_the_injected_resolver() {
    for bind in ["127.0.0.1:0", "[::1]:0"] {
        let listener = match tokio::net::TcpListener::bind(bind).await {
            Ok(listener) => listener,
            // IPv6 is optional on supported hosts; exercise it whenever this
            // machine can provide an actual IPv6 loopback listener.
            Err(_) if bind == "[::1]:0" => continue,
            Err(error) => panic!("IPv4 fixture bind failed: {error}"),
        };
        let addr = listener.local_addr().unwrap();
        let resolver = FixtureResolver::addresses(Vec::new());
        let peer = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_startup(&mut socket).await;
            write_error(&mut socket).await;
        });
        let mut target = target_on_port(addr.port());
        target.host = addr.ip().to_string();
        let database = Database::new_with_resolver(None, Some(target), resolver.clone());
        let error = database
            .query_typed(false, "SELECT 1", &[])
            .await
            .unwrap_err();
        let fault = retained_fault(&error);
        assert_eq!(fault.kind(), FaultKind::Server);
        assert_eq!(fault.attempted_addr(), Some(addr));
        assert_eq!(fault.attempt_index(), Some(0));
        assert_eq!(fault.resolver_address_count(), Some(1));
        assert_eq!(fault.resolver_host(), None);
        assert!(resolver.calls().is_empty());
        peer.await.unwrap();
        database.shutdown().await;
    }
}

#[tokio::test]
async fn duplicate_addresses_do_not_retry_startup_failure_and_a_later_operation_recovers() {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let resolver = FixtureResolver::addresses(vec![addr, addr, addr]);
    let peer = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        read_startup(&mut first).await;
        write_error(&mut first).await;
        drop(first);
        let (mut recovered, _) = listener.accept().await.unwrap();
        startup_ready(&mut recovered).await;
        empty_query(&mut recovered).await;
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), recovered.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(25), listener.accept())
                .await
                .is_err()
        );
    });
    let database =
        Database::new_with_resolver(None, Some(resolved_target(addr.port())), resolver.clone());
    let error = database
        .query_typed(false, "SELECT 1", &[])
        .await
        .unwrap_err();
    let fault = retained_fault(&error);
    assert_eq!(fault.server_code(), Some("28000"));
    assert_eq!(fault.attempt_index(), Some(0));
    assert_eq!(fault.attempted_addr(), Some(addr));
    assert_eq!(fault.resolver_address_count(), Some(3));
    assert_eq!(resolver.calls().len(), 1);
    assert!(database.primary.slots[0].connection.lock().await.is_none());
    assert!(
        database
            .query_typed(false, "SELECT 1", &[])
            .await
            .unwrap()
            .is_empty()
    );
    {
        let stored = database.primary.slots[0].connection.lock().await;
        let context = stored.as_ref().unwrap().transport_context().unwrap();
        assert_eq!(context.attempted_addr(), Some(addr));
        assert_eq!(context.attempt_index(), Some(0));
        assert_eq!(context.connect_stage(), ConnectStage::Established);
        assert_eq!(context.tls_verify_code(), None);
    }
    assert_eq!(
        resolver.calls().len(),
        2,
        "a later requested operation owns a new lookup"
    );
    database.shutdown().await;
    peer.await.unwrap();
}

#[tokio::test]
async fn first_tcp_startup_fatal_never_tries_a_healthy_second_ip_and_later_operation_recovers() {
    use tokio::io::AsyncReadExt;
    let first = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first_addr = first.local_addr().unwrap();
    let second =
        tokio::net::TcpListener::bind((std::net::Ipv4Addr::new(127, 0, 0, 2), first_addr.port()))
            .await
            .unwrap();
    let second_addr = second.local_addr().unwrap();
    let resolver = FixtureResolver::addresses(vec![first_addr, second_addr]);
    let second_accepts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let accepted = Arc::clone(&second_accepts);
    let (stop_second, stopped) = tokio::sync::oneshot::channel::<()>();
    let second_peer = tokio::spawn(async move {
        tokio::select! {
            _ = stopped => {}
            result = second.accept() => {
                let (mut socket, _) = result.unwrap();
                accepted.fetch_add(1, Ordering::AcqRel);
                startup_ready(&mut socket).await;
                empty_query(&mut socket).await;
                let mut bytes = Vec::new();
                let _ = tokio::time::timeout(Duration::from_secs(1), socket.read_to_end(&mut bytes)).await;
            }
        }
    });
    let peer = tokio::spawn(async move {
        let (mut socket, _) = first.accept().await.unwrap();
        read_startup(&mut socket).await;
        write_error(&mut socket).await;
        drop(socket);
        let (mut recovered, _) = first.accept().await.unwrap();
        startup_ready(&mut recovered).await;
        empty_query(&mut recovered).await;
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), recovered.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(25), first.accept())
                .await
                .is_err()
        );
    });
    let database = Database::new_with_resolver(
        None,
        Some(resolved_target(first_addr.port())),
        resolver.clone(),
    );
    let error = database
        .query_typed(false, "SELECT 1", &[])
        .await
        .unwrap_err();
    let fault = retained_fault(&error);
    assert_eq!(fault.phase(), FaultPhase::Connect);
    assert_eq!(fault.server_code(), Some("28000"));
    assert_eq!(fault.server_message(), Some("fixture startup rejection"));
    assert_eq!(fault.attempted_addr(), Some(first_addr));
    assert_eq!(fault.attempt_index(), Some(0));
    assert_eq!(fault.resolver_address_count(), Some(2));
    assert_eq!(fault.tls_verify_code(), None);
    assert_eq!(error.legacy_message(), None);
    assert_eq!(resolver.calls().len(), 1);
    assert!(database.primary.slots[0].connection.lock().await.is_none());
    assert_eq!(
        second_accepts.load(Ordering::Acquire),
        0,
        "TCP success commits the original operation to the first route"
    );
    assert!(
        database
            .query_typed(false, "SELECT 1", &[])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(resolver.calls().len(), 2);
    {
        let stored = database.primary.slots[0].connection.lock().await;
        let context = stored.as_ref().unwrap().transport_context().unwrap();
        assert_eq!(context.attempted_addr(), Some(first_addr));
        assert_eq!(context.attempt_index(), Some(0));
    }
    assert_eq!(
        second_accepts.load(Ordering::Acquire),
        0,
        "recovery is an independent request to the first route"
    );
    database.shutdown().await;
    stop_second.send(()).unwrap();
    second_peer.await.unwrap();
    peer.await.unwrap();
}

#[tokio::test]
async fn resolver_deadline_and_shutdown_drop_the_original_pending_lookup() {
    for shutdown in [false, true] {
        let dropped = Arc::new(AtomicBool::new(false));
        let resolver = Arc::new(FixtureResolver {
            calls: SyncMutex::new(Vec::new()),
            result: ResolverResult::Pending {
                entered: tokio::sync::Notify::new(),
                dropped: Arc::clone(&dropped),
            },
        });
        let mut target = resolved_target(5432);
        target.connect_timeout_ms = if shutdown { None } else { Some(50) };
        let database = Arc::new(Database::new_with_resolver(
            None,
            Some(target),
            resolver.clone(),
        ));
        let pending = {
            let database = Arc::clone(&database);
            tokio::spawn(async move { database.query_typed(false, "SELECT 1", &[]).await })
        };
        let ResolverResult::Pending { entered, .. } = &resolver.result else {
            unreachable!()
        };
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .unwrap();
        if shutdown {
            database.shutdown().await;
            assert!(matches!(
                pending.await.unwrap(),
                Err(QueryError::Unavailable)
            ));
        } else {
            let error = pending.await.unwrap().unwrap_err();
            let fault = retained_fault(&error);
            assert_eq!(fault.kind(), FaultKind::ConnectTimeout);
            assert_eq!(fault.connect_stage(), Some(ConnectStage::Resolve));
            assert_eq!(
                fault.resolver_host(),
                Some("private-resolver.transport.test")
            );
            assert_eq!(fault.attempted_addr(), None);
            assert_eq!(fault.tls_verify_code(), None);
            database.shutdown().await;
        }
        assert!(dropped.load(Ordering::Acquire));
        assert_eq!(resolver.calls().len(), 1);
        assert!(database.primary.slots[0].connection.lock().await.is_none());
    }
}

#[tokio::test]
async fn native_cancellation_uses_the_successful_ip_without_dns_retry_and_later_query_recovers() {
    use tokio::io::AsyncReadExt;
    let (_refused, first_addr) = refused_tcp_address();
    let selected =
        tokio::net::TcpListener::bind((std::net::Ipv4Addr::new(127, 0, 0, 2), first_addr.port()))
            .await
            .unwrap();
    let selected_addr = selected.local_addr().unwrap();
    let resolver = FixtureResolver::addresses(vec![first_addr, selected_addr]);
    let selected_peer = tokio::spawn(async move {
        let (mut query, _) = selected.accept().await.unwrap();
        startup_ready(&mut query).await;
        assert_eq!(read_frontend(&mut query).await.0, b'P');
        let mut pending_query = Vec::new();
        tokio::time::timeout(
            Duration::from_secs(1),
            query.read_to_end(&mut pending_query),
        )
        .await
        .unwrap()
        .unwrap();
        let (mut cancel, _) = tokio::time::timeout(Duration::from_secs(1), selected.accept())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cancel.read_u32().await.unwrap(), 16);
        assert_eq!(cancel.read_u32().await.unwrap(), 80877102);
        assert_eq!(cancel.read_u32().await.unwrap(), 17);
        assert_eq!(cancel.read_u32().await.unwrap(), 23);
        let mut trailing = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), cancel.read_to_end(&mut trailing))
            .await
            .unwrap()
            .unwrap();
        assert!(
            trailing.is_empty(),
            "cancel sends only its native capability"
        );
        let (mut recovered, _) = selected.accept().await.unwrap();
        startup_ready(&mut recovered).await;
        empty_query(&mut recovered).await;
        let mut trailing = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), recovered.read_to_end(&mut trailing))
            .await
            .unwrap()
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(25), selected.accept())
                .await
                .is_err()
        );
    });
    let mut target = resolved_target(first_addr.port());
    target.query_timeout_ms = Some(50);
    let database = Database::new_with_resolver(None, Some(target), resolver.clone());
    let error = database
        .query_typed(false, "SELECT 1", &[])
        .await
        .unwrap_err();
    assert!(matches!(error, QueryError::QueryTimeout));
    assert_eq!(
        resolver.calls().len(),
        1,
        "the cancellation socket never resolves"
    );
    assert!(database.primary.slots[0].connection.lock().await.is_none());
    assert!(
        database
            .query_typed(false, "SELECT 1", &[])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        resolver.calls().len(),
        2,
        "a later operation owns its own lookup"
    );
    {
        let stored = database.primary.slots[0].connection.lock().await;
        let context = stored.as_ref().unwrap().transport_context().unwrap();
        assert_eq!(context.attempted_addr(), Some(selected_addr));
        assert_eq!(context.attempt_index(), Some(1));
    }
    database.shutdown().await;
    selected_peer.await.unwrap();
    assert!(database.primary.cancellations.lock().unwrap().is_empty());
}

struct MemoryTlsFixture {
    ca: X509,
    certificate: X509,
    key: PKey<openssl::pkey::Private>,
}

impl MemoryTlsFixture {
    fn new() -> Self {
        use openssl::{
            asn1::Asn1Time,
            bn::BigNum,
            ec::{EcGroup, EcKey},
            hash::MessageDigest,
            nid::Nid,
            x509::{
                X509NameBuilder,
                extension::{BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectAlternativeName},
            },
        };
        let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
        let ca_key = PKey::from_ec_key(EcKey::generate(&group).unwrap()).unwrap();
        let key = PKey::from_ec_key(EcKey::generate(&group).unwrap()).unwrap();
        let name = |common_name: &str| {
            let mut name = X509NameBuilder::new().unwrap();
            name.append_entry_by_text("CN", common_name).unwrap();
            name.build()
        };
        let ca_name = name("transport fixture CA");
        let mut ca = X509::builder().unwrap();
        ca.set_version(2).unwrap();
        ca.set_serial_number(&BigNum::from_u32(1).unwrap().to_asn1_integer().unwrap())
            .unwrap();
        ca.set_subject_name(&ca_name).unwrap();
        ca.set_issuer_name(&ca_name).unwrap();
        ca.set_pubkey(&ca_key).unwrap();
        ca.set_not_before(Asn1Time::days_from_now(0).unwrap().as_ref())
            .unwrap();
        ca.set_not_after(Asn1Time::days_from_now(1).unwrap().as_ref())
            .unwrap();
        ca.append_extension(BasicConstraints::new().critical().ca().build().unwrap())
            .unwrap();
        ca.append_extension(
            KeyUsage::new()
                .critical()
                .key_cert_sign()
                .crl_sign()
                .build()
                .unwrap(),
        )
        .unwrap();
        ca.sign(&ca_key, MessageDigest::sha256()).unwrap();
        let ca = ca.build();
        let mut certificate = X509::builder().unwrap();
        certificate.set_version(2).unwrap();
        certificate
            .set_serial_number(&BigNum::from_u32(2).unwrap().to_asn1_integer().unwrap())
            .unwrap();
        certificate
            .set_subject_name(&name("transport.test"))
            .unwrap();
        certificate.set_issuer_name(ca.subject_name()).unwrap();
        certificate.set_pubkey(&key).unwrap();
        certificate
            .set_not_before(Asn1Time::days_from_now(0).unwrap().as_ref())
            .unwrap();
        certificate
            .set_not_after(Asn1Time::days_from_now(1).unwrap().as_ref())
            .unwrap();
        certificate
            .append_extension(BasicConstraints::new().critical().build().unwrap())
            .unwrap();
        certificate
            .append_extension(
                KeyUsage::new()
                    .critical()
                    .digital_signature()
                    .build()
                    .unwrap(),
            )
            .unwrap();
        certificate
            .append_extension(ExtendedKeyUsage::new().server_auth().build().unwrap())
            .unwrap();
        let sans = SubjectAlternativeName::new()
            .dns("transport.test")
            .ip("127.0.0.1")
            .build(&certificate.x509v3_context(Some(&ca), None))
            .unwrap();
        certificate.append_extension(sans).unwrap();
        certificate.sign(&ca_key, MessageDigest::sha256()).unwrap();
        Self {
            ca,
            certificate: certificate.build(),
            key,
        }
    }

    fn target(&self, port: u16) -> Target {
        let mut target = target_on_port(port);
        target.tls = Some(Tls {
            reject_unauthorized: true,
            ca: vec![String::from_utf8(self.ca.to_pem().unwrap()).unwrap()],
            cert: None,
            key: None,
            passphrase: None,
            servername: "transport.test".into(),
            sni: true,
            min_version: TlsVersion::V12,
            max_version: TlsVersion::V13,
            ciphers: "DEFAULT".into(),
        });
        target
    }
}

struct TlsPeerOutcome {
    handshake: bool,
    servername: Option<String>,
    alpn: Option<Vec<u8>>,
}

fn memory_tls_acceptor(
    fixture: &MemoryTlsFixture,
    negotiation: Negotiation,
) -> openssl::ssl::SslAcceptor {
    use openssl::ssl::{AlpnError, SslAcceptor};
    let mut acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls_server()).unwrap();
    acceptor.set_certificate(&fixture.certificate).unwrap();
    acceptor.set_private_key(&fixture.key).unwrap();
    acceptor.check_private_key().unwrap();
    if matches!(negotiation, Negotiation::Direct) {
        acceptor.set_alpn_select_callback(|_, offered| {
            openssl::ssl::select_next_proto(b"\x0apostgresql", offered)
                .ok_or(AlpnError::ALERT_FATAL)
        });
    }
    acceptor.build()
}

fn accept_fixture_socket(listener: &std::net::TcpListener) -> std::net::TcpStream {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let socket = loop {
        match listener.accept() {
            Ok((socket, _)) => break socket,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(_) => panic!("TLS fixture did not receive a connection"),
        }
    };
    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    socket
}

fn fixture_tls_accept(
    acceptor: &openssl::ssl::SslAcceptor,
    mut socket: std::net::TcpStream,
    negotiation: Negotiation,
) -> Result<openssl::ssl::SslStream<std::net::TcpStream>, ()> {
    use std::io::{Read, Write};
    if matches!(negotiation, Negotiation::Postgres) {
        let mut request = [0; 8];
        socket.read_exact(&mut request).unwrap();
        assert_eq!(request, [0, 0, 0, 8, 4, 210, 22, 47]);
        socket.write_all(b"S").unwrap();
    }
    acceptor.accept(socket).map_err(|_| ())
}

fn tls_fixture_listener(
    fixture: &MemoryTlsFixture,
    negotiation: Negotiation,
) -> (u16, JoinHandle<TlsPeerOutcome>) {
    use openssl::ssl::NameType;
    use std::io::{Read, Write};
    let acceptor = memory_tls_acceptor(fixture, negotiation);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let task = tokio::task::spawn_blocking(move || {
        let socket = accept_fixture_socket(&listener);
        let Ok(mut socket) = fixture_tls_accept(&acceptor, socket, negotiation) else {
            return TlsPeerOutcome {
                handshake: false,
                servername: None,
                alpn: None,
            };
        };
        let outcome = TlsPeerOutcome {
            handshake: true,
            servername: socket
                .ssl()
                .servername(NameType::HOST_NAME)
                .map(str::to_owned),
            alpn: socket.ssl().selected_alpn_protocol().map(<[u8]>::to_vec),
        };
        let mut length = [0; 4];
        socket.read_exact(&mut length).unwrap();
        let length = u32::from_be_bytes(length) as usize;
        assert!((8..=1024 * 1024).contains(&length));
        let mut startup = vec![0; length - 4];
        socket.read_exact(&mut startup).unwrap();
        socket
            .write_all(b"R\0\0\0\x08\0\0\0\0Z\0\0\0\x05I")
            .unwrap();
        let mut remaining = Vec::new();
        // A driver abort can close TLS without close_notify, as before this
        // unit. Either EOF or that close error ends the fixture ownership.
        let _ = socket.read_to_end(&mut remaining);
        outcome
    });
    (port, task)
}

#[tokio::test]
async fn tls_verification_observation_preserves_native_controls_and_sni_and_alpn() {
    let fixture = MemoryTlsFixture::new();
    for negotiation in [Negotiation::Postgres, Negotiation::Direct] {
        for case in [
            "trusted-dns",
            "trusted-ip",
            "untrusted",
            "wrong-dns",
            "wrong-ip",
            "verification-disabled",
            "sni-disabled",
        ] {
            for observed in [false, true] {
                let (port, peer) = tls_fixture_listener(&fixture, negotiation);
                let mut target = fixture.target(port);
                target.ssl_negotiation = negotiation;
                let tls = target.tls.as_mut().unwrap();
                match case {
                    "trusted-ip" => tls.servername = "127.0.0.1".into(),
                    "untrusted" => tls.ca.clear(),
                    "wrong-dns" => tls.servername = "wrong.transport.test".into(),
                    "wrong-ip" => tls.servername = "127.0.0.2".into(),
                    "verification-disabled" => {
                        tls.ca.clear();
                        tls.servername = "wrong.transport.test".into();
                        tls.reject_unauthorized = false;
                    }
                    "sni-disabled" => tls.sni = false,
                    _ => {}
                }
                let expected_success = !matches!(case, "untrusted" | "wrong-dns" | "wrong-ip");
                if observed {
                    let cancellations = Arc::new(SyncMutex::new(Vec::new()));
                    match connect_with_access(
                        &target,
                        &cancellations,
                        false,
                        &transport::SystemResolver,
                    )
                    .await
                    {
                        Ok(connection) => {
                            assert!(expected_success, "observed {case}");
                            let context = connection.transport_context().unwrap();
                            assert_eq!(context.connect_stage(), ConnectStage::Established);
                            assert_eq!(
                                context.tls_connect_state(),
                                Some(TlsConnectState::Succeeded)
                            );
                            assert_eq!(
                                context.ssl_reply(),
                                if matches!(negotiation, Negotiation::Postgres) {
                                    Some(SslReply::Accepted)
                                } else {
                                    None
                                }
                            );
                            assert_eq!(
                                context.attempted_addr(),
                                Some(([127, 0, 0, 1], port).into())
                            );
                            assert_eq!(
                                context.tls_servername(),
                                Some(target.tls.as_ref().unwrap().servername.as_str())
                            );
                            if case == "verification-disabled" {
                                assert_eq!(context.tls_verify_code(), None);
                                assert_eq!(context.tls_preverify_ok(), None);
                            } else {
                                assert_eq!(
                                    context.tls_verify_code(),
                                    Some(openssl::x509::X509VerifyResult::OK.as_raw())
                                );
                                assert_eq!(context.tls_preverify_ok(), Some(true));
                                assert_eq!(context.tls_verify_depth(), Some(0));
                            }
                            assert_eq!(format!("{context:?}"), "TransportContext");
                            drop(connection);
                        }
                        Err(fault) => {
                            assert!(!expected_success, "observed {case}");
                            assert_eq!(fault.kind(), FaultKind::Tls);
                            assert_eq!(fault.connect_stage(), Some(ConnectStage::TlsVerification));
                            assert_eq!(fault.tls_connect_state(), Some(TlsConnectState::Failed));
                            assert_eq!(
                                fault.ssl_reply(),
                                if matches!(negotiation, Negotiation::Postgres) {
                                    Some(SslReply::Accepted)
                                } else {
                                    None
                                }
                            );
                            assert!(fault.tls_verify_code().is_some_and(|code| code != 0));
                            assert_eq!(fault.tls_preverify_ok(), Some(false));
                            assert_eq!(fault.tls_verify_depth(), Some(0));
                            assert_eq!(
                                fault.tls_servername(),
                                Some(target.tls.as_ref().unwrap().servername.as_str())
                            );
                            let error = QueryError::Fault(fault);
                            assert_eq!(error.legacy_message(), None);
                            assert_eq!(format!("{error:?}"), "QueryError::Fault");
                            assert!(std::error::Error::source(&error).is_none());
                        }
                    }
                } else {
                    let connector = tls_connector_without_observation(
                        target.tls.as_ref().unwrap(),
                        negotiation,
                    )
                    .unwrap();
                    let result = config(&target).unwrap().connect(connector).await;
                    assert_eq!(result.is_ok(), expected_success, "native {case}");
                    if let Ok((client, driver)) = result {
                        let driver = tokio::spawn(async move {
                            let _ = driver.await;
                        });
                        drop(client);
                        driver.abort();
                    }
                }
                let outcome = peer.await.unwrap();
                assert_eq!(outcome.handshake, expected_success, "server {case}");
                if expected_success {
                    let tls = target.tls.as_ref().unwrap();
                    // OpenSSL omits the SNI extension for numeric IP names.
                    let expected_name =
                        if tls.sni && tls.servername.parse::<std::net::IpAddr>().is_err() {
                            Some(tls.servername.as_str())
                        } else {
                            None
                        };
                    assert_eq!(outcome.servername.as_deref(), expected_name);
                    assert_eq!(
                        outcome.alpn.as_deref(),
                        if matches!(negotiation, Negotiation::Direct) {
                            Some(b"postgresql".as_slice())
                        } else {
                            None
                        }
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn original_ssl_reply_and_tls_future_are_retained_without_invented_verification() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let fixture = MemoryTlsFixture::new();
    for reply in [Some(b'N'), Some(b'E'), Some(b'?'), Some(b'S'), None] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut target = fixture.target(listener.local_addr().unwrap().port());
        target.connect_timeout_ms = Some(250);
        let peer = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 8];
            socket.read_exact(&mut request).await.unwrap();
            if let Some(reply) = reply {
                socket.write_all(&[reply]).await.unwrap();
            }
            socket.shutdown().await.unwrap();
        });
        let database = Database::new(None, Some(target));
        let error = database
            .query_typed(false, "SELECT 1", &[])
            .await
            .unwrap_err();
        let fault = retained_fault(&error);
        assert_eq!(
            fault.connect_stage(),
            Some(if reply == Some(b'S') {
                ConnectStage::TlsConnectFuture
            } else {
                ConnectStage::SslRequest
            })
        );
        assert_eq!(
            fault.ssl_reply(),
            reply.map(|byte| match byte {
                b'S' => SslReply::Accepted,
                b'N' => SslReply::Declined,
                b'E' => SslReply::ServerError,
                other => SslReply::Other(other),
            })
        );
        assert_eq!(
            fault.tls_connect_state(),
            if reply == Some(b'S') {
                Some(TlsConnectState::Failed)
            } else {
                None
            }
        );
        assert_eq!(fault.tls_servername(), Some("transport.test"));
        assert_eq!(fault.tls_verify_code(), None);
        assert_eq!(fault.tls_verify_depth(), None);
        assert_eq!(fault.tls_preverify_ok(), None);
        assert_eq!(error.legacy_message(), None);
        peer.await.unwrap();
        database.shutdown().await;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn unix_target_keeps_native_connect_and_does_not_enter_the_tcp_resolver() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    struct Directory(std::path::PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let directory = Directory(std::env::temp_dir().join(format!(
        "misskey-transport-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    )));
    std::fs::create_dir(&directory.0).unwrap();
    let port = 5432;
    let listener =
        tokio::net::UnixListener::bind(directory.0.join(format!(".s.PGSQL.{port}"))).unwrap();
    let resolver = FixtureResolver::addresses(Vec::new());
    let mut target = target_on_port(port);
    target.host = directory.0.to_str().unwrap().into();
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let length = socket.read_u32().await.unwrap() as usize;
        let mut startup = vec![0; length - 4];
        socket.read_exact(&mut startup).await.unwrap();
        let payload = b"SFATAL\0C28000\0Munix startup rejection\0\0";
        socket.write_u8(b'E').await.unwrap();
        socket.write_u32((payload.len() + 4) as u32).await.unwrap();
        socket.write_all(payload).await.unwrap();
    });
    let database = Database::new_with_resolver(None, Some(target), resolver.clone());
    let error = database
        .query_typed(false, "SELECT 1", &[])
        .await
        .unwrap_err();
    let fault = retained_fault(&error);
    assert_eq!(fault.kind(), FaultKind::Server);
    assert_eq!(fault.server_message(), Some("unix startup rejection"));
    assert_eq!(fault.connect_stage(), None);
    assert_eq!(fault.attempted_addr(), None);
    assert_eq!(fault.resolver_host(), None);
    assert_eq!(fault.tls_verify_code(), None);
    assert_eq!(error.legacy_message(), None);
    assert!(resolver.calls().is_empty());
    peer.await.unwrap();
    database.shutdown().await;
}

#[tokio::test]
async fn read_only_session_setup_stays_outside_connect_and_query_deadlines() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut target = resolved_target(addr.port());
    target.connect_timeout_ms = Some(50);
    target.query_timeout_ms = Some(25);
    let resolver = FixtureResolver::addresses(vec![addr]);
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        startup_ready(&mut socket).await;
        let (kind, payload) = read_frontend(&mut socket).await;
        assert_eq!(kind, b'Q');
        assert_eq!(payload, b"SET default_transaction_read_only = on\0");
        tokio::time::sleep(Duration::from_millis(100)).await;
        socket
            .write_all(b"C\0\0\0\x08SET\0Z\0\0\0\x05I")
            .await
            .unwrap();
        empty_query(&mut socket).await;
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), socket.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
    });
    let database = Database::new_with_resolver(
        Some(PostgresConfig {
            targets: vec![target],
        }),
        None,
        resolver.clone(),
    );
    assert!(
        database
            .query_typed(true, "SELECT 1", &[])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(resolver.calls().len(), 1);
    database.shutdown().await;
    peer.await.unwrap();
}

#[tokio::test]
async fn verification_from_a_failed_startup_does_not_leak_into_a_later_independent_operation() {
    use std::io::{Read, Write};
    let fixture = MemoryTlsFixture::new();
    let acceptor = memory_tls_acceptor(&fixture, Negotiation::Postgres);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let resolver = FixtureResolver::addresses(vec![addr, addr]);
    let mut target = fixture.target(addr.port());
    target.host = "private-resolver.transport.test".into();
    let peer = tokio::task::spawn_blocking(move || {
        let socket = accept_fixture_socket(&listener);
        let mut socket = fixture_tls_accept(&acceptor, socket, Negotiation::Postgres).unwrap();
        let mut length = [0; 4];
        socket.read_exact(&mut length).unwrap();
        let mut startup = vec![0; u32::from_be_bytes(length) as usize - 4];
        socket.read_exact(&mut startup).unwrap();
        let payload = b"SFATAL\0C28000\0Mfirst verified startup rejected\0\0";
        socket.write_all(b"E").unwrap();
        socket
            .write_all(&((payload.len() + 4) as u32).to_be_bytes())
            .unwrap();
        socket.write_all(payload).unwrap();
        drop(socket);
        let mut socket = accept_fixture_socket(&listener);
        let mut request = [0; 8];
        socket.read_exact(&mut request).unwrap();
        assert_eq!(request, [0, 0, 0, 8, 4, 210, 22, 47]);
        socket.write_all(b"N").unwrap();
    });
    let database = Database::new_with_resolver(None, Some(target), resolver.clone());
    let error = database
        .query_typed(false, "SELECT 1", &[])
        .await
        .unwrap_err();
    let first_fault = retained_fault(&error);
    assert_eq!(first_fault.server_code(), Some("28000"));
    assert_eq!(first_fault.attempt_index(), Some(0));
    assert_eq!(first_fault.attempted_addr(), Some(addr));
    assert_eq!(first_fault.tls_verify_code(), Some(0));
    assert_eq!(
        first_fault.tls_connect_state(),
        Some(TlsConnectState::Succeeded)
    );
    assert_eq!(resolver.calls().len(), 1);
    let later_error = database
        .query_typed(false, "SELECT 1", &[])
        .await
        .unwrap_err();
    let fault = retained_fault(&later_error);
    assert_eq!(fault.attempt_index(), Some(0));
    assert_eq!(fault.attempted_addr(), Some(addr));
    assert_eq!(fault.connect_stage(), Some(ConnectStage::SslRequest));
    assert_eq!(fault.ssl_reply(), Some(SslReply::Declined));
    assert_eq!(fault.tls_connect_state(), None);
    assert_eq!(fault.tls_servername(), Some("transport.test"));
    assert_eq!(fault.tls_verify_code(), None);
    assert_eq!(fault.tls_verify_depth(), None);
    assert_eq!(fault.tls_preverify_ok(), None);
    assert_eq!(fault.server_code(), None);
    assert_eq!(later_error.legacy_message(), None);
    assert_eq!(resolver.calls().len(), 2);
    assert_eq!(
        first_fault.tls_verify_code(),
        Some(0),
        "a retained prior fault stays immutable"
    );
    peer.await.unwrap();
    database.shutdown().await;
}

#[tokio::test]
async fn native_drop_cancellation_keeps_verified_tls_and_its_original_deadline_and_resolver_route()
{
    use std::io::{Read, Write};
    let fixture = MemoryTlsFixture::new();
    let untrusted = MemoryTlsFixture::new();
    for negotiation in [Negotiation::Postgres, Negotiation::Direct] {
        for cancellation in ["trusted", "untrusted", "stalled"] {
            let acceptor = memory_tls_acceptor(&fixture, negotiation);
            let cancellation_acceptor = memory_tls_acceptor(
                if cancellation == "untrusted" {
                    &untrusted
                } else {
                    &fixture
                },
                negotiation,
            );
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            listener.set_nonblocking(true).unwrap();
            let resolver = FixtureResolver::addresses(vec![addr]);
            let mut target = fixture.target(addr.port());
            target.host = "private-resolver.transport.test".into();
            target.ssl_negotiation = negotiation;
            let peer = tokio::task::spawn_blocking(move || {
                let socket = accept_fixture_socket(&listener);
                let mut socket = fixture_tls_accept(&acceptor, socket, negotiation).unwrap();
                let mut length = [0; 4];
                socket.read_exact(&mut length).unwrap();
                let mut startup = vec![0; u32::from_be_bytes(length) as usize - 4];
                socket.read_exact(&mut startup).unwrap();
                socket
                    .write_all(b"R\0\0\0\x08\0\0\0\0K\0\0\0\x0c\0\0\0\x11\0\0\0\x17Z\0\0\0\x05I")
                    .unwrap();
                let mut remaining = Vec::new();
                let _ = socket.read_to_end(&mut remaining);
                drop(socket);
                let mut socket = accept_fixture_socket(&listener);
                if cancellation == "stalled" {
                    if matches!(negotiation, Negotiation::Postgres) {
                        let mut request = [0; 8];
                        socket.read_exact(&mut request).unwrap();
                        assert_eq!(request, [0, 0, 0, 8, 4, 210, 22, 47]);
                        socket.write_all(b"S").unwrap();
                    }
                    let mut handshake = Vec::new();
                    socket.read_to_end(&mut handshake).unwrap();
                    assert_eq!(
                        handshake.first(),
                        Some(&22),
                        "a TLS record starts the cancellation handshake"
                    );
                } else {
                    let result = fixture_tls_accept(&cancellation_acceptor, socket, negotiation);
                    if cancellation == "untrusted" {
                        assert!(
                            result.is_err(),
                            "cancellation preserves certificate verification"
                        );
                    } else {
                        let mut socket = result.unwrap();
                        let mut capability = [0; 16];
                        socket.read_exact(&mut capability).unwrap();
                        assert_eq!(
                            capability,
                            [0, 0, 0, 16, 4, 210, 22, 46, 0, 0, 0, 17, 0, 0, 0, 23]
                        );
                        let mut trailing = Vec::new();
                        let _ = socket.read_to_end(&mut trailing);
                        assert!(
                            trailing.is_empty(),
                            "cancellation sends no SQL or extra capability"
                        );
                    }
                }
                assert_eq!(
                    listener.accept().unwrap_err().kind(),
                    std::io::ErrorKind::WouldBlock
                );
            });
            let cancellations = Arc::new(SyncMutex::new(Vec::new()));
            let connection = connect_with_access(&target, &cancellations, false, resolver.as_ref())
                .await
                .unwrap();
            let context = connection.transport_context().unwrap();
            assert_eq!(context.tls_verify_code(), Some(0));
            assert_eq!(context.tls_preverify_ok(), Some(true));
            assert!(
                connection
                    .cancel_tls
                    .as_ref()
                    .unwrap()
                    .observation
                    .is_none(),
                "the cancellation factory owns no business attempt state"
            );
            connection.in_flight.store(true, Ordering::Release);
            let started = Instant::now();
            drop(connection);
            let tasks = std::mem::take(&mut *cancellations.lock().unwrap());
            assert_eq!(tasks.len(), 1);
            for task in tasks {
                tokio::time::timeout(Duration::from_secs(1), task)
                    .await
                    .unwrap()
                    .unwrap();
            }
            assert!(started.elapsed() < Duration::from_secs(1));
            if cancellation == "stalled" {
                assert!(started.elapsed() >= Duration::from_millis(450));
            }
            assert_eq!(resolver.calls().len(), 1);
            assert_eq!(context.tls_verify_code(), Some(0));
            assert_eq!(context.tls_preverify_ok(), Some(true));
            assert!(cancellations.lock().unwrap().is_empty());
            peer.await.unwrap();
        }
    }
}

#[derive(Default)]
struct WireAudit {
    reads: usize,
    writes: usize,
    vectored: usize,
    flushes: usize,
    shutdowns: usize,
    written: Vec<u8>,
}

enum WireRead {
    Pending,
    Bytes(Vec<u8>),
    Eof,
    Error(std::io::Error),
}
enum WireWrite {
    Pending,
    Count(usize),
    Error(std::io::Error),
}
enum WireFinish {
    Pending,
    Ok,
    Error(std::io::Error),
}

struct ScriptedWire {
    reads: std::collections::VecDeque<WireRead>,
    writes: std::collections::VecDeque<WireWrite>,
    flushes: std::collections::VecDeque<WireFinish>,
    shutdowns: std::collections::VecDeque<WireFinish>,
    audit: Arc<SyncMutex<WireAudit>>,
}
impl ScriptedWire {
    fn new(reads: Vec<WireRead>, writes: Vec<WireWrite>) -> Self {
        Self {
            reads: reads.into(),
            writes: writes.into(),
            flushes: Vec::new().into(),
            shutdowns: Vec::new().into(),
            audit: Arc::new(SyncMutex::new(WireAudit::default())),
        }
    }
}
impl tokio::io::AsyncRead for ScriptedWire {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.audit.lock().unwrap().reads += 1;
        if buf.remaining() == 0 {
            return std::task::Poll::Ready(Ok(()));
        }
        match self.reads.pop_front().expect("unexpected underlying read") {
            WireRead::Pending => {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
            WireRead::Bytes(bytes) => {
                let count = bytes.len().min(buf.remaining());
                buf.put_slice(&bytes[..count]);
                if count < bytes.len() {
                    self.reads
                        .push_front(WireRead::Bytes(bytes[count..].to_vec()));
                }
                std::task::Poll::Ready(Ok(()))
            }
            WireRead::Eof => std::task::Poll::Ready(Ok(())),
            WireRead::Error(error) => std::task::Poll::Ready(Err(error)),
        }
    }
}
impl tokio::io::AsyncWrite for ScriptedWire {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        self.audit.lock().unwrap().writes += 1;
        match self
            .writes
            .pop_front()
            .expect("unexpected underlying write")
        {
            WireWrite::Pending => {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
            WireWrite::Count(count) => {
                assert!(count <= bytes.len());
                self.audit
                    .lock()
                    .unwrap()
                    .written
                    .extend_from_slice(&bytes[..count]);
                std::task::Poll::Ready(Ok(count))
            }
            WireWrite::Error(error) => std::task::Poll::Ready(Err(error)),
        }
    }
    fn poll_write_vectored(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<std::io::Result<usize>> {
        self.audit.lock().unwrap().vectored += 1;
        match self
            .writes
            .pop_front()
            .expect("unexpected underlying vectored write")
        {
            WireWrite::Pending => {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
            WireWrite::Count(count) => {
                assert!(count <= bytes.iter().map(|slice| slice.len()).sum::<usize>());
                self.audit.lock().unwrap().written.extend(
                    bytes
                        .iter()
                        .flat_map(|slice| slice.iter().copied())
                        .take(count),
                );
                std::task::Poll::Ready(Ok(count))
            }
            WireWrite::Error(error) => std::task::Poll::Ready(Err(error)),
        }
    }
    fn is_write_vectored(&self) -> bool {
        true
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.audit.lock().unwrap().flushes += 1;
        match self
            .flushes
            .pop_front()
            .expect("unexpected underlying flush")
        {
            WireFinish::Pending => {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
            WireFinish::Ok => std::task::Poll::Ready(Ok(())),
            WireFinish::Error(error) => std::task::Poll::Ready(Err(error)),
        }
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.audit.lock().unwrap().shutdowns += 1;
        match self
            .shutdowns
            .pop_front()
            .expect("unexpected underlying shutdown")
        {
            WireFinish::Pending => {
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
            WireFinish::Ok => std::task::Poll::Ready(Ok(())),
            WireFinish::Error(error) => std::task::Poll::Ready(Err(error)),
        }
    }
}

fn observed_fixture(
    inner: ScriptedWire,
    mode: SslMode,
    negotiation: SslNegotiation,
) -> (
    wire::ObservedStream<ScriptedWire>,
    transport::ConnectContext,
    Arc<SyncMutex<WireAudit>>,
) {
    let (context, observation) =
        transport::ConnectContext::test_attempt(([127, 0, 0, 1], 5432).into());
    let audit = Arc::clone(&inner.audit);
    (
        wire::ObservedStream::new(inner, observation, mode, negotiation),
        context,
        audit,
    )
}

#[test]
fn observed_stream_preserves_partial_vectored_pending_and_prefilled_io_without_extra_reads() {
    use std::{
        io::IoSlice,
        pin::Pin,
        task::{Context, Poll, Waker},
    };
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    const REQUEST: &[u8] = &[0, 0, 0, 8, 4, 210, 22, 47];
    let inner = ScriptedWire::new(
        vec![
            WireRead::Pending,
            WireRead::Bytes(b"Nprivate reply bytes".to_vec()),
            WireRead::Bytes(b"S".to_vec()),
        ],
        vec![
            WireWrite::Pending,
            WireWrite::Count(3),
            WireWrite::Count(2),
            WireWrite::Count(3),
            WireWrite::Count(5),
        ],
    );
    let (mut stream, context, audit) =
        observed_fixture(inner, SslMode::Require, SslNegotiation::Postgres);
    let mut cx = Context::from_waker(Waker::noop());
    assert!(stream.is_write_vectored());
    assert!(
        Pin::new(&mut stream)
            .poll_write(&mut cx, REQUEST)
            .is_pending()
    );
    assert_eq!(context.snapshot().unwrap().socket_bytes_written(), 0);
    assert!(matches!(
        Pin::new(&mut stream).poll_write(&mut cx, &REQUEST[..5]),
        Poll::Ready(Ok(3))
    ));
    let middle = [IoSlice::new(&REQUEST[3..5]), IoSlice::new(&REQUEST[5..])];
    assert!(matches!(
        Pin::new(&mut stream).poll_write_vectored(&mut cx, &middle),
        Poll::Ready(Ok(2))
    ));
    let tail = [
        IoSlice::new(&REQUEST[5..6]),
        IoSlice::new(&[]),
        IoSlice::new(&REQUEST[6..]),
    ];
    assert!(matches!(
        Pin::new(&mut stream).poll_write_vectored(&mut cx, &tail),
        Poll::Ready(Ok(3))
    ));
    assert_eq!(
        audit.lock().unwrap().reads,
        0,
        "writes never cause an observer read"
    );
    let mut empty = [];
    assert!(matches!(
        Pin::new(&mut stream).poll_read(&mut cx, &mut ReadBuf::new(&mut empty)),
        Poll::Ready(Ok(()))
    ));
    assert_eq!(context.snapshot().unwrap().last_io_outcome(), None);
    let mut bytes = [0; 64];
    let mut buf = ReadBuf::new(&mut bytes);
    buf.put_slice(b"pre-filled");
    assert!(
        Pin::new(&mut stream)
            .poll_read(&mut cx, &mut buf)
            .is_pending()
    );
    assert_eq!(buf.filled(), b"pre-filled");
    assert_eq!(context.snapshot().unwrap().socket_bytes_read(), 0);
    assert!(matches!(
        Pin::new(&mut stream).poll_read(&mut cx, &mut buf),
        Poll::Ready(Ok(()))
    ));
    assert_eq!(buf.filled(), b"pre-filledNprivate reply bytes");
    let snapshot = context.snapshot().unwrap();
    assert_eq!(snapshot.ssl_reply(), Some(SslReply::Declined));
    assert_eq!(
        snapshot.socket_bytes_read(),
        b"Nprivate reply bytes".len() as u64
    );
    assert_eq!(snapshot.socket_bytes_written(), 8);
    assert_eq!(snapshot.connect_stage(), ConnectStage::SslRequest);
    assert!(matches!(
        Pin::new(&mut stream).poll_write(&mut cx, b"private SQL"),
        Poll::Ready(Ok(5))
    ));
    assert!(matches!(
        Pin::new(&mut stream).poll_read(&mut cx, &mut buf),
        Poll::Ready(Ok(()))
    ));
    assert_eq!(
        context.snapshot().unwrap().ssl_reply(),
        Some(SslReply::Declined),
        "later bytes cannot overwrite the original reply"
    );
    let audit = audit.lock().unwrap();
    assert_eq!(audit.reads, 4);
    assert_eq!(audit.writes, 3);
    assert_eq!(audit.vectored, 2);
    assert_eq!(audit.written, [REQUEST, b"priva"].concat());
    assert_eq!(format!("{stream:?}"), "ObservedStream");
    assert_eq!(format!("{snapshot:?}"), "TransportContext");
}

#[test]
fn observed_stream_records_only_an_original_complete_postgres_ssl_request_reply() {
    use std::{
        pin::Pin,
        task::{Context, Poll, Waker},
    };
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    const REQUEST: &[u8] = &[0, 0, 0, 8, 4, 210, 22, 47];
    for (byte, expected) in [
        (b'S', SslReply::Accepted),
        (b'N', SslReply::Declined),
        (b'E', SslReply::ServerError),
        (b'?', SslReply::Other(b'?')),
    ] {
        let inner = ScriptedWire::new(vec![WireRead::Bytes(vec![byte])], vec![WireWrite::Count(8)]);
        let (mut stream, context, audit) =
            observed_fixture(inner, SslMode::Require, SslNegotiation::Postgres);
        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(
            Pin::new(&mut stream).poll_write(&mut cx, REQUEST),
            Poll::Ready(Ok(8))
        ));
        let mut bytes = [0; 1];
        assert!(matches!(
            Pin::new(&mut stream).poll_read(&mut cx, &mut ReadBuf::new(&mut bytes)),
            Poll::Ready(Ok(()))
        ));
        assert_eq!(bytes, [byte]);
        assert_eq!(context.snapshot().unwrap().ssl_reply(), Some(expected));
        assert_eq!(audit.lock().unwrap().reads, 1);
    }
    for case in [
        "wrong-prefix",
        "read-before-write",
        "partial-then-read",
        "write-before-reply",
        "direct",
        "disabled",
    ] {
        let (mode, negotiation) = match case {
            "direct" => (SslMode::Require, SslNegotiation::Direct),
            "disabled" => (SslMode::Disable, SslNegotiation::Postgres),
            _ => (SslMode::Require, SslNegotiation::Postgres),
        };
        let writes = match case {
            "partial-then-read" => vec![WireWrite::Count(3)],
            "write-before-reply" => vec![WireWrite::Count(8), WireWrite::Count(1)],
            _ => vec![WireWrite::Count(8)],
        };
        let inner = ScriptedWire::new(vec![WireRead::Bytes(vec![b'S'])], writes);
        let (mut stream, context, audit) = observed_fixture(inner, mode, negotiation);
        let mut cx = Context::from_waker(Waker::noop());
        let mut bytes = [0; 1];
        if case == "read-before-write" {
            assert!(matches!(
                Pin::new(&mut stream).poll_read(&mut cx, &mut ReadBuf::new(&mut bytes)),
                Poll::Ready(Ok(()))
            ));
        }
        let request = if case == "wrong-prefix" {
            b"private!".as_slice()
        } else {
            REQUEST
        };
        let count = if case == "partial-then-read" { 3 } else { 8 };
        assert!(
            matches!(Pin::new(&mut stream).poll_write(&mut cx, &request[..count]), Poll::Ready(Ok(n)) if n == count)
        );
        if case == "write-before-reply" {
            assert!(matches!(
                Pin::new(&mut stream).poll_write(&mut cx, b"x"),
                Poll::Ready(Ok(1))
            ));
        }
        if case != "read-before-write" {
            assert!(matches!(
                Pin::new(&mut stream).poll_read(&mut cx, &mut ReadBuf::new(&mut bytes)),
                Poll::Ready(Ok(()))
            ));
        }
        assert_eq!(bytes, [b'S']);
        assert_eq!(context.snapshot().unwrap().ssl_reply(), None, "{case}");
        assert_eq!(audit.lock().unwrap().reads, 1);
    }
}

#[test]
fn observed_stream_forwards_exact_errors_and_terminal_io_without_fabricating_byte_events() {
    use std::{
        pin::Pin,
        task::{Context, Poll, Waker},
    };
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    #[derive(Debug)]
    struct ErrorMarker(Arc<()>);
    impl std::fmt::Display for ErrorMarker {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("private transport marker")
        }
    }
    impl std::error::Error for ErrorMarker {}
    let marker = Arc::new(());
    let original = std::io::Error::new(
        std::io::ErrorKind::ConnectionReset,
        ErrorMarker(Arc::clone(&marker)),
    );
    let mut inner = ScriptedWire::new(
        vec![WireRead::Eof, WireRead::Error(original)],
        vec![
            WireWrite::Count(0),
            WireWrite::Count(0),
            WireWrite::Error(std::io::Error::from_raw_os_error(5)),
        ],
    );
    inner.flushes = vec![
        WireFinish::Pending,
        WireFinish::Ok,
        WireFinish::Error(std::io::Error::from_raw_os_error(13)),
    ]
    .into();
    inner.shutdowns = vec![
        WireFinish::Pending,
        WireFinish::Ok,
        WireFinish::Error(std::io::Error::from_raw_os_error(22)),
    ]
    .into();
    let (mut stream, context, audit) =
        observed_fixture(inner, SslMode::Require, SslNegotiation::Postgres);
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(
        Pin::new(&mut stream).poll_write(&mut cx, &[]),
        Poll::Ready(Ok(0))
    ));
    assert_eq!(context.snapshot().unwrap().last_io_outcome(), None);
    assert!(matches!(
        Pin::new(&mut stream).poll_write(&mut cx, b"x"),
        Poll::Ready(Ok(0))
    ));
    let zero = context.snapshot().unwrap();
    assert_eq!(zero.last_io_direction(), Some(IoDirection::Write));
    assert_eq!(zero.last_io_outcome(), Some(IoOutcome::ZeroWrite));
    let mut bytes = [0; 1];
    assert!(matches!(
        Pin::new(&mut stream).poll_read(&mut cx, &mut ReadBuf::new(&mut bytes)),
        Poll::Ready(Ok(()))
    ));
    assert_eq!(
        context.snapshot().unwrap().last_io_direction(),
        Some(IoDirection::Read)
    );
    assert_eq!(
        context.snapshot().unwrap().last_io_outcome(),
        Some(IoOutcome::Eof)
    );
    let Poll::Ready(Err(error)) =
        Pin::new(&mut stream).poll_read(&mut cx, &mut ReadBuf::new(&mut bytes))
    else {
        panic!("read source must pass through")
    };
    assert!(Arc::ptr_eq(
        &error
            .get_ref()
            .unwrap()
            .downcast_ref::<ErrorMarker>()
            .unwrap()
            .0,
        &marker
    ));
    assert_eq!(
        context.snapshot().unwrap().last_io_kind(),
        Some(std::io::ErrorKind::ConnectionReset)
    );
    assert_eq!(context.snapshot().unwrap().last_io_errno(), None);
    let Poll::Ready(Err(error)) = Pin::new(&mut stream).poll_write(&mut cx, b"x") else {
        panic!("write source must pass through")
    };
    assert_eq!(error.raw_os_error(), Some(5));
    assert_eq!(
        context.snapshot().unwrap().last_io_direction(),
        Some(IoDirection::Write)
    );
    assert_eq!(context.snapshot().unwrap().last_io_errno(), Some(5));
    assert!(Pin::new(&mut stream).poll_flush(&mut cx).is_pending());
    assert!(matches!(
        Pin::new(&mut stream).poll_flush(&mut cx),
        Poll::Ready(Ok(()))
    ));
    let Poll::Ready(Err(error)) = Pin::new(&mut stream).poll_flush(&mut cx) else {
        panic!("flush source must pass through")
    };
    assert_eq!(error.raw_os_error(), Some(13));
    assert_eq!(
        context.snapshot().unwrap().last_io_direction(),
        Some(IoDirection::Flush)
    );
    assert!(Pin::new(&mut stream).poll_shutdown(&mut cx).is_pending());
    assert!(matches!(
        Pin::new(&mut stream).poll_shutdown(&mut cx),
        Poll::Ready(Ok(()))
    ));
    let Poll::Ready(Err(error)) = Pin::new(&mut stream).poll_shutdown(&mut cx) else {
        panic!("shutdown source must pass through")
    };
    assert_eq!(error.raw_os_error(), Some(22));
    let final_snapshot = context.snapshot().unwrap();
    assert_eq!(
        final_snapshot.last_io_direction(),
        Some(IoDirection::Shutdown)
    );
    assert_eq!(final_snapshot.last_io_outcome(), Some(IoOutcome::Error));
    assert_eq!(
        final_snapshot.last_io_kind(),
        Some(std::io::Error::from_raw_os_error(22).kind())
    );
    assert_eq!(final_snapshot.last_io_errno(), Some(22));
    assert_eq!(final_snapshot.socket_bytes_read(), 0);
    assert_eq!(final_snapshot.socket_bytes_written(), 0);
    assert_eq!(final_snapshot.ssl_reply(), None);
    assert_eq!(
        zero.last_io_outcome(),
        Some(IoOutcome::ZeroWrite),
        "earlier snapshots do not mutate"
    );
    let fault = QueryFault::socket(std::io::Error::from_raw_os_error(22))
        .with_context(Some(final_snapshot));
    assert_eq!(format!("{fault:?}"), "QueryFault");
    assert!(std::error::Error::source(&fault).is_none());
    assert_eq!(format!("{stream:?}"), "ObservedStream");
    let audit = audit.lock().unwrap();
    assert_eq!(
        (audit.reads, audit.writes, audit.flushes, audit.shutdowns),
        (2, 3, 3, 3)
    );
}

impl tokio_postgres::tls::TlsStream for ScriptedWire {
    fn channel_binding(&self) -> tokio_postgres::tls::ChannelBinding {
        tokio_postgres::tls::ChannelBinding::tls_server_end_point(vec![1, 2, 3])
    }
}

struct ScriptedTls {
    polls: Arc<std::sync::atomic::AtomicUsize>,
    error: Option<std::io::Error>,
}
impl tokio_postgres::tls::TlsConnect<ScriptedWire> for ScriptedTls {
    type Stream = ScriptedWire;
    type Error = std::io::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ScriptedWire, std::io::Error>> + Send>,
    >;
    fn connect(self, stream: ScriptedWire) -> Self::Future {
        let mut stream = Some(stream);
        let mut error = self.error;
        Box::pin(std::future::poll_fn(move |cx| {
            if self.polls.fetch_add(1, Ordering::AcqRel) == 0 {
                cx.waker().wake_by_ref();
                return std::task::Poll::Pending;
            }
            std::task::Poll::Ready(match error.take() {
                Some(error) => Err(error),
                None => Ok(stream.take().unwrap()),
            })
        }))
    }
}

#[test]
fn observed_tls_tracks_only_original_connector_future_polls_and_passes_its_result_unchanged() {
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    use tokio_postgres::tls::TlsConnect;
    for succeeds in [false, true] {
        let (context, observation) =
            transport::ConnectContext::test_attempt(([127, 0, 0, 1], 5432).into());
        let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let connector = ScriptedTls {
            polls: Arc::clone(&polls),
            error: (!succeeds).then(|| std::io::Error::from_raw_os_error(13)),
        };
        let stream = ScriptedWire::new(Vec::new(), Vec::new());
        let audit = Arc::clone(&stream.audit);
        let mut future = Box::pin(wire::ObservedTls::new(connector, observation).connect(stream));
        assert_eq!(
            context.snapshot().unwrap().tls_connect_state(),
            None,
            "creating the future is not polling it"
        );
        let mut cx = Context::from_waker(Waker::noop());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        let started = context.snapshot().unwrap();
        assert_eq!(started.tls_connect_state(), Some(TlsConnectState::Started));
        assert_eq!(started.connect_stage(), ConnectStage::TlsConnectFuture);
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(Ok(stream)) => {
                assert!(succeeds);
                assert!(
                    Arc::ptr_eq(&stream.audit, &audit),
                    "the original TLS stream is returned"
                );
                assert_eq!(
                    context.snapshot().unwrap().tls_connect_state(),
                    Some(TlsConnectState::Succeeded)
                );
                assert_eq!(
                    context.snapshot().unwrap().connect_stage(),
                    ConnectStage::TlsConnectComplete
                );
            }
            Poll::Ready(Err(error)) => {
                assert!(!succeeds);
                assert_eq!(error.raw_os_error(), Some(13));
                assert_eq!(
                    context.snapshot().unwrap().tls_connect_state(),
                    Some(TlsConnectState::Failed)
                );
                assert_eq!(
                    context.snapshot().unwrap().connect_stage(),
                    ConnectStage::TlsConnectFuture
                );
            }
            Poll::Pending => panic!("original connector completed on its second poll"),
        }
        assert_eq!(polls.load(Ordering::Acquire), 2);
        assert_eq!(
            started.tls_connect_state(),
            Some(TlsConnectState::Started),
            "retained facts are immutable snapshots"
        );
        assert_eq!(context.snapshot().unwrap().ssl_reply(), None);
        assert_eq!(context.snapshot().unwrap().last_io_outcome(), None);
        assert_eq!(audit.lock().unwrap().reads, 0);
    }
}

#[tokio::test]
async fn a_query_fault_snapshots_later_socket_io_and_stays_immutable_during_native_cancellation() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let resolver = FixtureResolver::addresses(vec![addr]);
    let peer = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        startup_ready(&mut socket).await;
        assert_eq!(read_frontend(&mut socket).await.0, b'P');
        socket.write_all(b"E\0\0").await.unwrap();
        socket.shutdown().await.unwrap();
        let (mut cancellation, _) = listener.accept().await.unwrap();
        let mut capability = [0; 16];
        cancellation.read_exact(&mut capability).await.unwrap();
        assert_eq!(
            capability,
            [0, 0, 0, 16, 4, 210, 22, 46, 0, 0, 0, 17, 0, 0, 0, 23]
        );
        let mut trailing = Vec::new();
        cancellation.read_to_end(&mut trailing).await.unwrap();
        assert!(trailing.is_empty());
    });
    let target = resolved_target(addr.port());
    let cancellations = Arc::new(SyncMutex::new(Vec::new()));
    let connection = connect_with_access(&target, &cancellations, false, resolver.as_ref())
        .await
        .unwrap();
    let established = connection.transport_context().unwrap();
    assert_eq!(established.last_io_outcome(), None);
    assert_eq!(established.ssl_reply(), None);
    assert_eq!(established.tls_connect_state(), None);
    let query_error = connection
        .client
        .query("private SQL", &[])
        .await
        .unwrap_err();
    let fault = connection.fault(FaultPhase::Query, query_error);
    assert_eq!(fault.phase(), FaultPhase::Query);
    assert_eq!(fault.attempted_addr(), Some(addr));
    assert_eq!(fault.last_io_direction(), Some(IoDirection::Read));
    assert_eq!(fault.last_io_outcome(), Some(IoOutcome::Eof));
    assert!(fault.socket_bytes_read().unwrap() > established.socket_bytes_read());
    assert!(fault.socket_bytes_written().unwrap() > established.socket_bytes_written());
    let before = (
        fault.socket_bytes_read(),
        fault.socket_bytes_written(),
        fault.last_io_outcome(),
    );
    let token = connection.client.cancel_token();
    tokio::time::timeout(
        Duration::from_secs(1),
        connection.cancel_route.cancel(&token, |_| NoTls),
    )
    .await
    .unwrap()
    .unwrap();
    let after = connection.transport_context().unwrap();
    assert_eq!(
        after.socket_bytes_read(),
        fault.socket_bytes_read().unwrap()
    );
    assert_eq!(
        after.socket_bytes_written(),
        fault.socket_bytes_written().unwrap()
    );
    assert_eq!(
        before,
        (
            fault.socket_bytes_read(),
            fault.socket_bytes_written(),
            fault.last_io_outcome()
        )
    );
    assert_eq!(resolver.calls().len(), 1);
    assert_eq!(
        established.last_io_outcome(),
        None,
        "pre-query snapshot remains unchanged"
    );
    assert_eq!(format!("{fault:?}"), "QueryFault");
    assert!(std::error::Error::source(&fault).is_none());
    drop(connection);
    peer.await.unwrap();
}

#[tokio::test]
async fn a_trusted_tls_startup_failure_retains_the_completed_original_connector_future() {
    use std::io::{Read, Write};
    let fixture = MemoryTlsFixture::new();
    let acceptor = memory_tls_acceptor(&fixture, Negotiation::Postgres);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let peer = tokio::task::spawn_blocking(move || {
        let socket = accept_fixture_socket(&listener);
        let mut socket = fixture_tls_accept(&acceptor, socket, Negotiation::Postgres).unwrap();
        let mut length = [0; 4];
        socket.read_exact(&mut length).unwrap();
        let mut startup = vec![0; u32::from_be_bytes(length) as usize - 4];
        socket.read_exact(&mut startup).unwrap();
        let payload = b"SFATAL\0C28000\0Mverified startup rejection\0\0";
        socket.write_all(b"E").unwrap();
        socket
            .write_all(&((payload.len() + 4) as u32).to_be_bytes())
            .unwrap();
        socket.write_all(payload).unwrap();
    });
    let database = Database::new(None, Some(fixture.target(port)));
    let error = database
        .query_typed(false, "private SQL", &[])
        .await
        .unwrap_err();
    let fault = retained_fault(&error);
    assert_eq!(fault.kind(), FaultKind::Server);
    assert_eq!(fault.server_code(), Some("28000"));
    assert_eq!(
        fault.connect_stage(),
        Some(ConnectStage::TlsConnectComplete)
    );
    assert_eq!(fault.ssl_reply(), Some(SslReply::Accepted));
    assert_eq!(fault.tls_connect_state(), Some(TlsConnectState::Succeeded));
    assert_eq!(fault.tls_verify_code(), Some(0));
    assert_eq!(error.legacy_message(), None);
    peer.await.unwrap();
    database.shutdown().await;
}

#[derive(Clone, Debug)]
struct InspectedSocket {
    nodelay: bool,
    keepalive: bool,
    #[cfg(target_os = "linux")]
    idle: Duration,
    #[cfg(target_os = "linux")]
    interval: Duration,
    #[cfg(target_os = "linux")]
    retries: u32,
    #[cfg(target_os = "linux")]
    user_timeout: Option<Duration>,
}
trait InspectWireSocket {
    fn inspect_socket(&self) -> Option<InspectedSocket>;
}
impl InspectWireSocket for wire::ObservedStream<tokio::net::TcpStream> {
    fn inspect_socket(&self) -> Option<InspectedSocket> {
        let stream = self.get_ref_for_test();
        let socket = socket2::SockRef::from(stream);
        Some(InspectedSocket {
            nodelay: stream.nodelay().unwrap(),
            keepalive: socket.keepalive().unwrap(),
            #[cfg(target_os = "linux")]
            idle: socket.tcp_keepalive_time().unwrap(),
            #[cfg(target_os = "linux")]
            interval: socket.tcp_keepalive_interval().unwrap(),
            #[cfg(target_os = "linux")]
            retries: socket.tcp_keepalive_retries().unwrap(),
            #[cfg(target_os = "linux")]
            user_timeout: socket.tcp_user_timeout().unwrap(),
        })
    }
}
impl InspectWireSocket for tokio_postgres::Socket {
    fn inspect_socket(&self) -> Option<InspectedSocket> {
        None
    }
}

struct InspectTlsFactory {
    inner: NamedConnector,
    sockets: Arc<SyncMutex<Vec<InspectedSocket>>>,
}
struct InspectTlsConnect<T> {
    inner: T,
    sockets: Arc<SyncMutex<Vec<InspectedSocket>>>,
}
impl<S> MakeTlsConnect<S> for InspectTlsFactory
where
    NamedConnector: MakeTlsConnect<S>,
    S: InspectWireSocket,
{
    type Stream = <NamedConnector as MakeTlsConnect<S>>::Stream;
    type TlsConnect = InspectTlsConnect<<NamedConnector as MakeTlsConnect<S>>::TlsConnect>;
    type Error = <NamedConnector as MakeTlsConnect<S>>::Error;
    fn make_tls_connect(&mut self, domain: &str) -> Result<Self::TlsConnect, Self::Error> {
        Ok(InspectTlsConnect {
            inner: <NamedConnector as MakeTlsConnect<S>>::make_tls_connect(
                &mut self.inner,
                domain,
            )?,
            sockets: Arc::clone(&self.sockets),
        })
    }
}
impl<S, T> tokio_postgres::tls::TlsConnect<S> for InspectTlsConnect<T>
where
    S: InspectWireSocket,
    T: tokio_postgres::tls::TlsConnect<S>,
{
    type Stream = T::Stream;
    type Error = T::Error;
    type Future = T::Future;
    fn connect(self, stream: S) -> Self::Future {
        if let Some(socket) = stream.inspect_socket() {
            self.sockets.lock().unwrap().push(socket);
        }
        self.inner.connect(stream)
    }
}

#[tokio::test]
async fn original_business_and_cancellation_sockets_keep_the_native_socket_options() {
    use std::io::{Read, Write};
    let fixture = MemoryTlsFixture::new();
    for negotiation in [Negotiation::Postgres, Negotiation::Direct] {
        for keepalive in [false, true] {
            let acceptor = memory_tls_acceptor(&fixture, negotiation);
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            listener.set_nonblocking(true).unwrap();
            let peer = tokio::task::spawn_blocking(move || {
                let business_socket = accept_fixture_socket(&listener);
                let mut business =
                    fixture_tls_accept(&acceptor, business_socket, negotiation).unwrap();
                let mut length = [0; 4];
                business.read_exact(&mut length).unwrap();
                let mut startup = vec![0; u32::from_be_bytes(length) as usize - 4];
                business.read_exact(&mut startup).unwrap();
                business
                    .write_all(b"R\0\0\0\x08\0\0\0\0K\0\0\0\x0c\0\0\0\x11\0\0\0\x17Z\0\0\0\x05I")
                    .unwrap();
                let cancel_socket = accept_fixture_socket(&listener);
                let mut cancel = fixture_tls_accept(&acceptor, cancel_socket, negotiation).unwrap();
                let mut capability = [0; 16];
                cancel.read_exact(&mut capability).unwrap();
                assert_eq!(
                    capability,
                    [0, 0, 0, 16, 4, 210, 22, 46, 0, 0, 0, 17, 0, 0, 0, 23]
                );
                let mut trailing = Vec::new();
                let _ = cancel.read_to_end(&mut trailing);
                assert!(trailing.is_empty());
                let _ = business.read_to_end(&mut trailing);
                assert_eq!(
                    listener.accept().unwrap_err().kind(),
                    std::io::ErrorKind::WouldBlock
                );
            });
            let mut target = fixture.target(addr.port());
            target.host = "private-resolver.transport.test".into();
            target.ssl_negotiation = negotiation;
            let mut settings = config(&target).unwrap();
            settings
                .keepalives(keepalive)
                .keepalives_idle(Duration::from_secs(7));
            #[cfg(target_os = "linux")]
            settings
                .keepalives_interval(Duration::from_secs(3))
                .keepalives_retries(4)
                .tcp_user_timeout(Duration::from_secs(6));
            let resolver = FixtureResolver::addresses(vec![addr]);
            let context = transport::ConnectContext::default();
            let sockets = Arc::new(SyncMutex::new(Vec::new()));
            let base = tls_connector(target.tls.as_ref().unwrap(), negotiation).unwrap();
            let make_tls = |observation| InspectTlsFactory {
                inner: base.with_observation(observation),
                sockets: Arc::clone(&sockets),
            };
            let connected = transport::connect(&settings, resolver.as_ref(), &context, make_tls)
                .await
                .unwrap();
            let driver = tokio::spawn(async move {
                let _ = connected.driver.await;
            });
            let before = context.snapshot().unwrap();
            let token = connected.client.cancel_token();
            tokio::time::timeout(
                Duration::from_secs(1),
                connected.cancel_route.cancel(&token, make_tls),
            )
            .await
            .unwrap()
            .unwrap();
            let after = context.snapshot().unwrap();
            assert_eq!(after.ssl_reply(), before.ssl_reply());
            assert_eq!(after.tls_connect_state(), before.tls_connect_state());
            assert_eq!(after.tls_verify_code(), before.tls_verify_code());
            assert_eq!(after.last_io_outcome(), before.last_io_outcome());
            assert_eq!(after.socket_bytes_written(), before.socket_bytes_written());
            assert_eq!(resolver.calls().len(), 1);
            {
                let sockets = sockets.lock().unwrap();
                assert_eq!(
                    sockets.len(),
                    2,
                    "inspect the actual original business and cancellation sockets"
                );
                for socket in sockets.iter() {
                    assert!(socket.nodelay);
                    assert_eq!(socket.keepalive, keepalive);
                    #[cfg(target_os = "linux")]
                    {
                        assert_eq!(socket.user_timeout, Some(Duration::from_secs(6)));
                        if keepalive {
                            assert_eq!(socket.idle, Duration::from_secs(7));
                            assert_eq!(socket.interval, Duration::from_secs(3));
                            assert_eq!(socket.retries, 4);
                        }
                    }
                }
            }
            drop(connected.client);
            driver.abort();
            peer.await.unwrap();
        }
    }
}

#[tokio::test]
async fn a_refused_tcp_candidate_falls_back_to_the_actual_healthy_second_ip() {
    use tokio::io::AsyncReadExt;
    let (_refused, first_addr) = refused_tcp_address();
    let second =
        tokio::net::TcpListener::bind((std::net::Ipv4Addr::new(127, 0, 0, 2), first_addr.port()))
            .await
            .unwrap();
    let second_addr = second.local_addr().unwrap();
    let resolver = FixtureResolver::addresses(vec![first_addr, second_addr]);
    let peer = tokio::spawn(async move {
        let (mut socket, _) = second.accept().await.unwrap();
        startup_ready(&mut socket).await;
        empty_query(&mut socket).await;
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), socket.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(25), second.accept())
                .await
                .is_err()
        );
    });
    let database = Database::new_with_resolver(
        None,
        Some(resolved_target(first_addr.port())),
        resolver.clone(),
    );
    assert!(
        database
            .query_typed(false, "SELECT 1", &[])
            .await
            .unwrap()
            .is_empty()
    );
    {
        let stored = database.primary.slots[0].connection.lock().await;
        let context = stored.as_ref().unwrap().transport_context().unwrap();
        assert_eq!(context.attempted_addr(), Some(second_addr));
        assert_eq!(context.attempt_index(), Some(1));
        assert_eq!(context.resolver_address_count(), Some(2));
    }
    assert_eq!(resolver.calls().len(), 1);
    database.shutdown().await;
    peer.await.unwrap();
}

fn ready_and_empty_query_sync<S: std::io::Read + std::io::Write>(socket: &mut S) {
    let mut length = [0; 4];
    socket.read_exact(&mut length).unwrap();
    let mut startup = vec![0; u32::from_be_bytes(length) as usize - 4];
    socket.read_exact(&mut startup).unwrap();
    socket
        .write_all(b"R\0\0\0\x08\0\0\0\0K\0\0\0\x0c\0\0\0\x11\0\0\0\x17Z\0\0\0\x05I")
        .unwrap();
    for response in [
        b"1\0\0\0\x04t\0\0\0\x06\0\0n\0\0\0\x04Z\0\0\0\x05I".as_slice(),
        b"2\0\0\0\x04C\0\0\0\x0dSELECT 0\0Z\0\0\0\x05I".as_slice(),
    ] {
        loop {
            let mut kind = [0];
            socket.read_exact(&mut kind).unwrap();
            socket.read_exact(&mut length).unwrap();
            let mut payload = vec![0; u32::from_be_bytes(length) as usize - 4];
            socket.read_exact(&mut payload).unwrap();
            if kind[0] == b'S' {
                break;
            }
        }
        socket.write_all(response).unwrap();
    }
}

#[tokio::test]
async fn first_tcp_ssl_decline_never_tries_a_healthy_second_ip_and_later_operation_recovers() {
    use std::io::{Read, Write};
    let fixture = MemoryTlsFixture::new();
    let acceptor = memory_tls_acceptor(&fixture, Negotiation::Postgres);
    let first = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let first_addr = first.local_addr().unwrap();
    let second =
        std::net::TcpListener::bind((std::net::Ipv4Addr::new(127, 0, 0, 2), first_addr.port()))
            .unwrap();
    let second_addr = second.local_addr().unwrap();
    first.set_nonblocking(true).unwrap();
    second.set_nonblocking(true).unwrap();
    let accepted = Arc::new([
        std::sync::atomic::AtomicUsize::new(0),
        std::sync::atomic::AtomicUsize::new(0),
    ]);
    let worker_accepted = Arc::clone(&accepted);
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let peer = tokio::task::spawn_blocking(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !worker_stop.load(Ordering::Acquire) {
            assert!(
                std::time::Instant::now() < deadline,
                "two-route TLS fixture is bounded"
            );
            for (route, listener) in [&first, &second].into_iter().enumerate() {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        socket.set_nonblocking(false).unwrap();
                        socket
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        socket
                            .set_write_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let previous = worker_accepted[route].fetch_add(1, Ordering::AcqRel);
                        if route == 0 && previous == 0 {
                            let mut request = [0; 8];
                            socket.read_exact(&mut request).unwrap();
                            assert_eq!(request, [0, 0, 0, 8, 4, 210, 22, 47]);
                            socket.write_all(b"N").unwrap();
                        } else {
                            let mut socket =
                                fixture_tls_accept(&acceptor, socket, Negotiation::Postgres)
                                    .unwrap();
                            ready_and_empty_query_sync(&mut socket);
                            let mut trailing = Vec::new();
                            let _ = socket.read_to_end(&mut trailing);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => panic!("two-route TLS fixture accept failed"),
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let resolver = FixtureResolver::addresses(vec![first_addr, second_addr]);
    let mut target = fixture.target(first_addr.port());
    target.host = "private-resolver.transport.test".into();
    let database = Database::new_with_resolver(None, Some(target), resolver.clone());
    let error = database
        .query_typed(false, "SELECT 1", &[])
        .await
        .unwrap_err();
    let fault = retained_fault(&error);
    assert_eq!(fault.phase(), FaultPhase::Connect);
    assert_eq!(fault.attempted_addr(), Some(first_addr));
    assert_eq!(fault.attempt_index(), Some(0));
    assert_eq!(fault.resolver_address_count(), Some(2));
    assert_eq!(fault.ssl_reply(), Some(SslReply::Declined));
    assert_eq!(fault.tls_connect_state(), None);
    assert_eq!(fault.tls_verify_code(), None);
    assert_eq!(error.legacy_message(), None);
    assert_eq!(resolver.calls().len(), 1);
    assert_eq!(accepted[0].load(Ordering::Acquire), 1);
    assert_eq!(accepted[1].load(Ordering::Acquire), 0);
    assert!(database.primary.slots[0].connection.lock().await.is_none());
    assert!(
        database
            .query_typed(false, "SELECT 1", &[])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(resolver.calls().len(), 2);
    {
        let stored = database.primary.slots[0].connection.lock().await;
        let context = stored.as_ref().unwrap().transport_context().unwrap();
        assert_eq!(context.attempted_addr(), Some(first_addr));
        assert_eq!(context.attempt_index(), Some(0));
        assert_eq!(context.ssl_reply(), Some(SslReply::Accepted));
        assert_eq!(
            context.tls_connect_state(),
            Some(TlsConnectState::Succeeded)
        );
        assert_eq!(context.tls_verify_code(), Some(0));
    }
    assert_eq!(
        fault.ssl_reply(),
        Some(SslReply::Declined),
        "the original failure remains immutable"
    );
    assert_eq!(accepted[0].load(Ordering::Acquire), 2);
    assert_eq!(accepted[1].load(Ordering::Acquire), 0);
    database.shutdown().await;
    stop.store(true, Ordering::Release);
    peer.await.unwrap();
}

#[tokio::test]
async fn all_refused_tcp_candidates_retain_the_final_actual_address_and_original_io_error() {
    let (_first, first_addr) = refused_tcp_address();
    let second = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )
    .unwrap();
    let second_addr = std::net::SocketAddr::from(([127, 0, 0, 2], first_addr.port()));
    second.bind(&second_addr.into()).unwrap();
    let resolver = FixtureResolver::addresses(vec![first_addr, second_addr]);
    let database = Database::new_with_resolver(
        None,
        Some(resolved_target(first_addr.port())),
        resolver.clone(),
    );
    let error = database
        .query_typed(false, "SELECT 1", &[])
        .await
        .unwrap_err();
    let fault = retained_fault(&error);
    assert_eq!(fault.phase(), FaultPhase::Connect);
    assert_eq!(fault.connect_stage(), Some(ConnectStage::SocketConnect));
    assert_eq!(fault.kind(), FaultKind::Io);
    assert_eq!(fault.io_kind(), Some(std::io::ErrorKind::ConnectionRefused));
    assert!(fault.raw_os_error().is_some());
    assert_eq!(fault.attempted_addr(), Some(second_addr));
    assert_eq!(fault.attempt_index(), Some(1));
    assert_eq!(fault.resolver_address_count(), Some(2));
    assert_eq!(fault.ssl_reply(), None);
    assert_eq!(fault.tls_connect_state(), None);
    assert_eq!(fault.tls_verify_code(), None);
    assert_eq!(error.legacy_message(), None);
    assert_eq!(resolver.calls().len(), 1);
    database.shutdown().await;
}
