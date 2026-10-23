use super::*;
use std::sync::Arc;

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
