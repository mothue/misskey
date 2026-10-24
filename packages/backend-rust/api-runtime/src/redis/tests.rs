use super::*;

#[test]
fn unrelated_host_stream_payloads_do_not_invalidate_auth_subscription() {
    for payload in [
        br#"{"channel":"notesStream","message":{"id":"note","text":"ordinary post"}}"#.as_slice(),
        br#"{"channel":"broadcast","message":null}"#.as_slice(),
        br#"{"channel":"userStream:x","message":[1,2]}"#.as_slice(),
    ] {
        assert!(decode_event(payload).unwrap().is_none());
    }
    assert!(decode_event(br#"{"channel":"internal","message":null}"#).is_err());
}

#[test]
fn irrelevant_channels_skip_surrogates_and_deep_business_values() {
    for payload in [
        br#"{"channel":"notesStream","message":"\ud800"}"#.as_slice(),
        br#"{"channel":"notesStream","message":{"\ud800":"\udfff"}}"#.as_slice(),
        br#"{"channel":"\ud800","message":{"type":false}}"#.as_slice(),
        br#"{"channel":"notesStream","\ud800":"irrelevant","message":1e999}"#.as_slice(),
    ] {
        assert!(decode_event(payload).unwrap().is_none());
    }
    let depth = 20_000;
    let mut payload = b"{\"channel\":\"notesStream\",\"message\":".to_vec();
    payload.extend(std::iter::repeat_n(b'[', depth));
    payload.extend_from_slice(br#""\ud800""#);
    payload.extend(std::iter::repeat_n(b']', depth));
    payload.push(b'}');
    assert!(decode_event(&payload).unwrap().is_none());
    payload.push(b'}');
    assert!(decode_event(&payload).is_err());
}

#[test]
fn host_envelopes_keep_last_duplicate_channel_and_message() {
    for payload in [
        br#"{"channel":"internal","message":null,"channel":"notesStream"}"#.as_slice(),
        br#"{"channel":null,"channel":"notesStream","message":"\ud800"}"#.as_slice(),
        br#"{"channel":"internal","chann\u0065l":"notesStream","message":[]}"#.as_slice(),
    ] {
        assert!(decode_event(payload).unwrap().is_none());
    }
    for payload in [
        br#"{"channel":"notesStream","channel":"internal","message":{"type":"metaUpdated"}}"#.as_slice(),
        br#"{"channel":"\ud800","message":null,"channel":"internal","message":{"type":"metaUpdated"}}"#.as_slice(),
        br#"{"channel":null,"chann\u0065l":"internal","message":"\ud800","mess\u0061ge":{"type":"metaUpdated"},"\ud800":null}"#.as_slice(),
    ] {
        let event = decode_event(payload).unwrap().unwrap();
        assert_eq!(event["channel"], "internal");
        assert_eq!(event["message"]["type"], "metaUpdated");
    }
    assert!(
        decode_event(br#"{"channel":"notesStream","channel":"internal","message":null}"#).is_err()
    );
    assert!(
        decode_event(br#"{"channel":"internal","message":{"type":"metaUpdated"},"message":null}"#)
            .is_err()
    );
}

#[test]
fn malformed_json_and_invalid_auth_envelopes_fail_closed() {
    for payload in [
        br#"{"channel":"notesStream","message":[1,]}"#.as_slice(),
        br#"{"channel":"notesStream","message":"\uZZZZ"}"#.as_slice(),
        br#"{"channel":"notesStream"} garbage"#.as_slice(),
        br#"[]"#.as_slice(),
        br#"{}"#.as_slice(),
        br#"{"channel":null}"#.as_slice(),
        br#"{"channel":false}"#.as_slice(),
        br#"{"channel":42}"#.as_slice(),
        br#"{"channel":[]}"#.as_slice(),
        br#"{"channel":{}}"#.as_slice(),
        br#"{"channel":"internal"}"#.as_slice(),
        br#"{"channel":"internal","message":{}}"#.as_slice(),
        br#"{"channel":"internal","message":{"type":null}}"#.as_slice(),
        br#"{"channel":"internal","message":{"type":"metaUpdated","body":"\ud800"}}"#.as_slice(),
    ] {
        assert!(decode_event(payload).is_err());
    }
}
use misskey_backend_api_auth::{AuthConfig, Failure as AuthFailure};
use std::collections::HashMap;

fn equivalent(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get(key).is_some_and(|b| equivalent(a, b)))
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equivalent(a, b))
        }
        _ => a == b,
    }
}

#[test]
fn javascript_number_and_parse_int_boundaries_are_preserved() {
    for (value, expected) in [
        (0.0, "0"),
        (-0.0, "0"),
        (1e21, "1e+21"),
        (1e-7, "1e-7"),
        (f64::INFINITY, "Infinity"),
    ] {
        assert_eq!(js_number(value), expected);
    }
    assert_eq!(
        parse_int_reply(&RedisValue::Array(vec![RedisValue::BulkString(
            b"1700000000000000".to_vec()
        )])),
        1_700_000_000_000_000.0
    );
    assert_eq!(
        parse_int_reply(&RedisValue::BulkString(b" -12.75".to_vec())),
        -12.0
    );
    assert!(parse_int_reply(&RedisValue::Array(vec![])).is_nan());
    assert!(parse_int_reply(&RedisValue::Nil).is_nan());
}

#[test]
fn limiter_projection_preserves_nonfinite_nulls_and_redacted_failure_stages() {
    let info = LimitInfo {
        remaining: 0.,
        reset: f64::NAN,
        reset_ms: f64::NEG_INFINITY,
        total: f64::INFINITY,
    };
    assert_eq!(
        info.to_json(),
        json!({"remaining":0.,"reset":null,"resetMs":null,"total":null})
    );
    let mut config = config(0);
    config.username = Some("sensitive-user".into());
    config.password = Some("sensitive-password".into());
    let Err(failure) = client(&config) else {
        panic!("invalid Redis configuration accepted");
    };
    assert_eq!(failure.stage, FailureStage::Configuration);
    assert_eq!(
        format!("{failure}"),
        "Rust Redis unavailable at Configuration"
    );
    assert_eq!(format!("{failure:?}"), "Failure { stage: Configuration }");
}

#[test]
fn unsupported_or_partial_tls_configuration_is_rejected_without_downgrade() {
    let mut config = config(1);
    config.tls = Some(RedisTls {
        ca: vec![],
        cert: Some("invalid".into()),
        key: None,
        insecure: false,
    });
    assert!(client(&config).is_err());
    config.path = Some("/tmp/redis-auth-test.sock".into());
    assert!(client(&config).is_err());
}

#[tokio::test]
async fn deep_event_deserialization_and_auth_refresh_do_not_silently_skip_privilege_changes() {
    let store = Arc::new(EventStore {
        meta: tokio::sync::RwLock::new(json!({})),
    });
    let auth = AuthService::new(
        store,
        AuthConfig {
            id_method: "aidx".into(),
            max_file_size: 1,
            default_policies: json!({}),
            meta: json!({}),
            entity_layouts: HashMap::new(),
        },
        0,
    )
    .unwrap();
    let depth = 300;
    let mut bytes=b"{\"channel\":\"internal\",\"message\":{\"type\":\"metaUpdated\",\"body\":{\"after\":{\"rootUserId\":\"changed\",\"policies\":{\"extra\":".to_vec();
    bytes.extend(std::iter::repeat_n(b'[', depth));
    bytes.extend_from_slice(b"true");
    bytes.extend(std::iter::repeat_n(b']', depth));
    bytes.extend_from_slice(b"}}}}}");
    let event = decode_event(&bytes).unwrap().unwrap();
    auth.apply_event(&event, 1).await.unwrap();
    assert_eq!(auth.root_user_id().await.as_deref(), Some("changed"));
    let policies = auth.policies_for(None, 1).await.unwrap();
    let mut value = &policies.as_json()["extra"];
    for _ in 0..depth {
        value = &value[0];
    }
    assert_eq!(value, &Value::Bool(true));
    bytes.extend_from_slice(b"garbage");
    assert!(decode_event(&bytes).is_err());
    assert!(decode_event(br#"{"channel":"internal","message":null}"#).is_err());
}

fn config(port: u16) -> RedisConfig {
    RedisConfig {
        host: "127.0.0.1".into(),
        port,
        path: None,
        db: 15,
        username: None,
        password: None,
        key_prefix: String::new(),
        connect_timeout_ms: 1000,
        tls: None,
    }
}

fn dedicated_client() -> Client {
    let address = std::env::var("MISSKEY_RUST_REDIS_TEST_URL")
        .expect("explicit dedicated Redis URL required");
    Client::open(address).unwrap_or_else(|_| panic!("invalid dedicated Redis fixture"))
}

#[tokio::test]
#[ignore = "requires explicit dedicated Redis fixture; no FLUSHDB or shared keys"]
async fn typed_limits_use_endpoint_fallback_and_preserve_empty_or_shared_keys() {
    let client = dedicated_client();
    let connection = client.get_multiplexed_async_connection().await.unwrap();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut config = config(1);
    config.key_prefix = format!("typed-limit:{unique}:");
    let (stop, _) = watch::channel(false);
    let state = RedisState {
        client: client.clone(),
        config,
        connection: Mutex::new(Some(connection.clone())),
        ready: Arc::new(AtomicBool::new(true)),
        stopped: AtomicBool::new(false),
        stop,
        tasks: Mutex::new(Vec::new()),
        clock: Clock::new(),
    };
    assert!(
        state
            .limit(&RateLimit::default(), "disabled", "actor", 1.)
            .await
            .unwrap()
            .is_none()
    );
    for (override_key, expected_key) in [
        (None, "endpoint/name"),
        (Some(""), ""),
        (Some("shared"), "shared"),
    ] {
        let limit = RateLimit {
            key: override_key.map(str::to_owned),
            duration: Some(60_000.),
            max: Some(1.),
            min_interval: None,
        };
        assert!(
            state
                .limit(&limit, "endpoint/name", "actor", 1.)
                .await
                .unwrap()
                .is_none()
        );
        let blocked = state
            .limit(&limit, "endpoint/name", "actor", 1.)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(blocked.remaining, 0.);
        assert_eq!(blocked.total, 1.);
        let key = format!("{}limit:actor:{expected_key}", state.config.key_prefix);
        let mut connection = connection.clone();
        let count = redis::cmd("ZCARD")
            .arg(&key)
            .query_async::<i64>(&mut connection)
            .await
            .unwrap();
        assert_eq!(count, 1);
        redis::cmd("DEL")
            .arg(&key)
            .query_async::<i64>(&mut connection)
            .await
            .unwrap();
    }
    state.shutdown().await;
}

#[tokio::test]
#[ignore = "requires explicit dedicated Redis fixture; no FLUSHDB or shared keys"]
async fn live_redis_matches_existing_ratelimiter_including_fractional_errors() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/redis-reference.json")).unwrap();
    let client = dedicated_client();
    let mut connection = client
        .get_multiplexed_async_connection()
        .await
        .expect("dedicated Redis connection");
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    for scenario in fixture["scenarios"].as_array().unwrap() {
        let key = format!(
            "limit:rust-native-test:{unique}:{}",
            scenario["name"].as_str().unwrap()
        );
        if scenario["wrongType"] == true {
            redis::cmd("SET")
                .arg(&key)
                .arg("wrong-type")
                .query_async::<()>(&mut connection)
                .await
                .unwrap();
        }
        let duration = scenario["duration"].as_f64().unwrap();
        let max = scenario["max"].as_f64().unwrap_or(f64::INFINITY);
        for step in scenario["steps"].as_array().unwrap() {
            let actual = check_at(
                &mut connection,
                &key,
                duration,
                max,
                step["now"].as_f64().unwrap(),
            )
            .await
            .unwrap();
            assert!(
                equivalent(&actual.to_json(), &step["info"]),
                "scenario {}: actual{actual:?}, expected{}",
                scenario["name"],
                step["info"]
            );
            let members: Result<Vec<String>, _> = redis::cmd("ZRANGE")
                .arg(&key)
                .arg(0)
                .arg(-1)
                .arg("WITHSCORES")
                .query_async(&mut connection)
                .await;
            let actual_members = members.unwrap_or_else(|_| vec!["WRONGTYPE".into()]);
            assert_eq!(
                json!(actual_members),
                step["members"],
                "scenario {}",
                scenario["name"]
            );
            let ttl: i64 = redis::cmd("PTTL")
                .arg(&key)
                .query_async(&mut connection)
                .await
                .unwrap();
            let kind = if ttl == -1 {
                "persistent"
            } else if ttl == -2 {
                "missing"
            } else {
                "expiring"
            };
            assert_eq!(
                kind,
                step["ttlKind"].as_str().unwrap(),
                "scenario {}",
                scenario["name"]
            );
        }
        redis::cmd("DEL")
            .arg(&key)
            .query_async::<i64>(&mut connection)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires explicit dedicated Redis fixture; no FLUSHDB or shared keys"]
async fn limiter_serializes_concurrent_requests_and_keeps_exact_window_expiry() {
    let client = dedicated_client();
    let connection = client
        .get_multiplexed_async_connection()
        .await
        .expect("dedicated Redis connection");
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let key = format!("limit:rust-concurrent-test:{unique}");
    let mut tasks = Vec::new();
    for index in 0..10 {
        let mut connection = connection.clone();
        let key = key.clone();
        tasks.push(tokio::spawn(async move {
            check_at(
                &mut connection,
                &key,
                1000.0,
                3.0,
                1_700_000_000_000_000.0 + f64::from(index),
            )
            .await
            .unwrap()
        }));
    }
    let mut permitted = 0;
    for task in tasks {
        if task.await.unwrap().remaining > 0.0 {
            permitted += 1;
        }
    }
    assert_eq!(permitted, 3);
    let mut connection = connection;
    let count: i64 = redis::cmd("ZCARD")
        .arg(&key)
        .query_async(&mut connection)
        .await
        .unwrap();
    assert_eq!(count, 3);
    let now = 1_700_000_001_000_010.0;
    let expired = check_at(&mut connection, &key, 1000.0, 3.0, now)
        .await
        .unwrap();
    assert_eq!(expired.remaining, 3.0);
    redis::cmd("DEL")
        .arg(&key)
        .query_async::<i64>(&mut connection)
        .await
        .unwrap();
}

struct EventStore {
    meta: tokio::sync::RwLock<Value>,
}
#[async_trait::async_trait]
impl Store for EventStore {
    async fn user_by_token(&self, _: &str) -> Result<Option<Value>, AuthFailure> {
        Ok(None)
    }
    async fn user_by_id(&self, _: &str) -> Result<Option<Value>, AuthFailure> {
        Ok(None)
    }
    async fn access_token_by_token(&self, _: &str, _: &str) -> Result<Option<Value>, AuthFailure> {
        Ok(None)
    }
    async fn update_last_used(&self, _: &str, _: i64) -> Result<(), AuthFailure> {
        Ok(())
    }
    async fn app_by_id(&self, _: &str) -> Result<Option<Value>, AuthFailure> {
        Ok(None)
    }
    async fn roles(&self) -> Result<Vec<Value>, AuthFailure> {
        Ok(Vec::new())
    }
    async fn meta(&self) -> Result<Value, AuthFailure> {
        Ok(self.meta.read().await.clone())
    }
    async fn assignments(&self, _: &str) -> Result<Vec<Value>, AuthFailure> {
        Ok(Vec::new())
    }
    async fn insert_ip(&self, _: i64, _: &str, _: &str) -> Result<(), AuthFailure> {
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires explicit dedicated Redis fixture; proxy cuts only this test's subscription"]
async fn pubsub_disconnect_closes_admission_then_refreshes_and_recovers() {
    use tokio::net::{TcpListener, TcpStream};
    let client = dedicated_client();
    let info = client.get_connection_info();
    let ConnectionAddr::Tcp(host, port) = &info.addr else {
        panic!("dedicated plain TCP Redis required for isolated proxy test")
    };
    let target = (host.clone(), *port);
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let (cut, _) = watch::channel(false);
    let mut signal = cut.subscribe();
    let drivers = Arc::new(Mutex::new(Vec::new()));
    let proxy_drivers = drivers.clone();
    let proxy = tokio::spawn(async move {
        loop {
            let (mut incoming, _) = listener.accept().await.unwrap();
            let mut outgoing = TcpStream::connect(&target).await.unwrap();
            let mut connection_cut = signal.clone();
            let driver = tokio::spawn(async move {
                tokio::select! {
                    _ = connection_cut.changed() => {},
                    _ = tokio::io::copy_bidirectional(&mut incoming,&mut outgoing)=>{},
                }
            });
            proxy_drivers.lock().await.push(driver);
            let _ = signal.borrow_and_update();
        }
    });
    let mut command_config = config(*port);
    command_config.host = host.clone();
    command_config.db = info.redis.db;
    command_config.password = info.redis.password.clone();
    command_config.username = info.redis.username.clone();
    let mut subscription_config = command_config.clone();
    subscription_config.host = "127.0.0.1".into();
    subscription_config.port = proxy_port;
    let initial = json!({"rootUserId":"initial","policies":{},"enableIpLogging":false});
    let store = Arc::new(EventStore {
        meta: tokio::sync::RwLock::new(initial.clone()),
    });
    let auth = Arc::new(
        AuthService::new(
            store.clone(),
            AuthConfig {
                id_method: "aidx".into(),
                max_file_size: 1,
                default_policies: json!({}),
                meta: initial,
                entity_layouts: HashMap::new(),
            },
            crate::now_ms(),
        )
        .unwrap(),
    );
    let channel = format!(
        "rust-native-events:{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let state = RedisState::new_with_auth(
        command_config,
        subscription_config,
        channel.clone(),
        auth.clone(),
    )
    .await
    .unwrap();
    assert!(state.ready());
    let mut publisher = client.get_multiplexed_async_connection().await.unwrap();
    let payload=json!({"channel":"internal","message":{"type":"metaUpdated","body":{"after":{"rootUserId":"event"}}}}).to_string();
    redis::cmd("PUBLISH")
        .arg(&channel)
        .arg(&payload)
        .query_async::<i64>(&mut publisher)
        .await
        .unwrap();
    for _ in 0..100 {
        if auth.root_user_id().await.as_deref() == Some("event") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(auth.root_user_id().await.as_deref(), Some("event"));
    // Valid unrelated host traffic must leave the same subscription able to
    // process the next privilege event, including JS-only string payloads.
    let mut deep = b"{\"channel\":\"notesStream\",\"message\":".to_vec();
    deep.extend(std::iter::repeat_n(b'[', 1000));
    deep.extend_from_slice(br#""\ud800""#);
    deep.extend(std::iter::repeat_n(b']', 1000));
    deep.push(b'}');
    for payload in [
        br#"{"channel":"notesStream","message":"\ud800"}"#.as_slice(),
        br#"{"channel":"internal","channel":"notesStream","message":null}"#.as_slice(),
        deep.as_slice(),
        br#"{"channel":"internal","message":{"type":"metaUpdated","body":{"after":{"rootUserId":"after-irrelevant"}}}}"#.as_slice(),
    ] {
        redis::cmd("PUBLISH").arg(&channel).arg(payload)
            .query_async::<i64>(&mut publisher).await.unwrap();
    }
    for _ in 0..100 {
        if auth.root_user_id().await.as_deref() == Some("after-irrelevant") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        auth.root_user_id().await.as_deref(),
        Some("after-irrelevant")
    );
    assert!(state.ready());
    *store.meta.write().await =
        json!({"rootUserId":"after-gap","policies":{},"enableIpLogging":false});
    cut.send_replace(true);
    for _ in 0..100 {
        if !state.ready() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!state.ready());
    for _ in 0..400 {
        if state.ready() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(state.ready());
    assert_eq!(auth.root_user_id().await.as_deref(), Some("after-gap"));
    *store.meta.write().await =
        json!({"rootUserId":"after-malformed","policies":{},"enableIpLogging":false});
    redis::cmd("PUBLISH")
        .arg(&channel)
        .arg("{")
        .query_async::<i64>(&mut publisher)
        .await
        .unwrap();
    for _ in 0..100 {
        if !state.ready() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!state.ready());
    for _ in 0..400 {
        if state.ready() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(state.ready());
    assert_eq!(
        auth.root_user_id().await.as_deref(),
        Some("after-malformed")
    );
    state.shutdown().await;
    assert!(!state.ready());
    assert!(
        state
            .limit(
                &RateLimit {
                    key: Some("test".into()),
                    min_interval: Some(1.),
                    ..Default::default()
                },
                "endpoint",
                "actor",
                1.0
            )
            .await
            .is_err()
    );
    proxy.abort();
    let _ = proxy.await;
    for driver in drivers.lock().await.drain(..) {
        driver.abort();
        let _ = driver.await;
    }
}
