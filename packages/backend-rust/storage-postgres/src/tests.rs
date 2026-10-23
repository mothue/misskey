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
    admin
        .batch_execute(&format!("DROP SCHEMA {schema} CASCADE"))
        .await
        .unwrap();
    drop(admin);
    driver.abort();
}
