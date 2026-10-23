use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct MockStore {
    users: Mutex<HashMap<String, Value>>,
    access: Mutex<Option<Value>>,
    app: Mutex<Option<Value>>,
    role_rows: Mutex<Vec<Value>>,
    assignment_rows: Mutex<Vec<Value>>,
    meta: Mutex<Value>,
    user_reads: AtomicUsize,
    app_reads: AtomicUsize,
    role_reads: AtomicUsize,
    updates: AtomicUsize,
    ip_writes: AtomicUsize,
    last_token_lookup: Mutex<Option<(String, String)>>,
}

#[async_trait]
impl Store for MockStore {
    async fn user_by_token(&self, token: &str) -> Result<Option<Value>, Failure> {
        self.user_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .users
            .lock()
            .await
            .values()
            .find(|u| u["token"] == token)
            .cloned())
    }
    async fn user_by_id(&self, id: &str) -> Result<Option<Value>, Failure> {
        self.user_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.users.lock().await.get(id).cloned())
    }
    async fn access_token_by_token(
        &self,
        lower: &str,
        exact: &str,
    ) -> Result<Option<Value>, Failure> {
        *self.last_token_lookup.lock().await = Some((lower.to_owned(), exact.to_owned()));
        Ok(self.access.lock().await.clone())
    }
    async fn update_last_used(&self, _: &str, _: i64) -> Result<(), Failure> {
        self.updates.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn app_by_id(&self, _: &str) -> Result<Option<Value>, Failure> {
        self.app_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.app.lock().await.clone())
    }
    async fn roles(&self) -> Result<Vec<Value>, Failure> {
        self.role_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.role_rows.lock().await.clone())
    }
    async fn meta(&self) -> Result<Value, Failure> {
        Ok(self.meta.lock().await.clone())
    }
    async fn assignments(&self, _: &str) -> Result<Vec<Value>, Failure> {
        Ok(self.assignment_rows.lock().await.clone())
    }
    async fn insert_ip(&self, _: i64, _: &str, _: &str) -> Result<(), Failure> {
        self.ip_writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn config() -> AuthConfig {
    AuthConfig {
        id_method: "aidx".into(),
        max_file_size: 30 * 1024 * 1024 + 1,
        default_policies: json!({
            "rateLimitFactor":1, "canSearchUsers":true, "canInvite":false,
            "maxFileSizeMb":40,"chatAvailability":"available",
            "uploadableFileTypes":["image/*","text/*"]
        }),
        meta: json!({"rootUserId":"root","policies":{},"enableIpLogging":true}),
        entity_layouts: HashMap::new(),
    }
}

fn user(token: &str) -> Value {
    json!({
        "id":"0000000100000000", "host":null, "token":token,
        "followersCount":3,"followingCount":4,"notesCount":5,
        "isSuspended":false,"isLocked":true,"isBot":false,"isCat":true,
        "isExplorable":true
    })
}

async fn service() -> (Arc<MockStore>, AuthService<MockStore>) {
    let store = Arc::new(MockStore::default());
    let config = config();
    *store.meta.lock().await = config.meta.clone();
    let service = AuthService::new(store.clone(), config, 0).unwrap();
    (store, service)
}

fn event(kind: &str, body: Value) -> Value {
    json!({"channel":"internal","message":{"type":kind,"body":body}})
}

#[tokio::test]
async fn native_auth_preserves_utf16_classification_and_negative_cache_expiry() {
    let (store, service) = service().await;
    assert!(service.authenticate(None, 0).await.unwrap().user.is_none());
    let token = "🦀🦀🦀🦀🦀🦀🦀🦀"; // 8 scalars, 16 UTF-16 units.
    assert!(matches!(
        service.authenticate(Some(token), 0).await,
        Err(AuthError::UserNotFound)
    ));
    let record = user(token);
    store
        .users
        .lock()
        .await
        .insert(record["id"].as_str().unwrap().into(), record.clone());
    assert!(matches!(
        service.authenticate(Some(token), USER_TTL).await,
        Err(AuthError::UserNotFound)
    ));
    assert_eq!(
        service
            .authenticate(Some(token), USER_TTL + 1)
            .await
            .unwrap()
            .user,
        Some(record)
    );
    assert_eq!(store.user_reads.load(Ordering::SeqCst), 2);
    assert!(store.last_token_lookup.lock().await.is_none());
}

#[tokio::test]
async fn app_auth_updates_async_and_returns_only_app_permissions_with_week_cache() {
    let (store, service) = service().await;
    let record = user("abcdefghijklmnop");
    store
        .users
        .lock()
        .await
        .insert(record["id"].as_str().unwrap().into(), record.clone());
    *store.access.lock().await = Some(json!({
        "id":"access", "userId":record["id"],"appId":"app",
        "permission":["write:notes"],"lastUsedAt":null,"token":"secret"
    }));
    *store.app.lock().await = Some(json!({"id":"app","permission":["read:account"]}));
    let context = service
        .authenticate(Some("ApP-TOKEN-LONGER-THAN-16"), 0)
        .await
        .unwrap();
    assert!(context.access_token_reduced);
    assert_eq!(context.user.as_ref(), Some(&record));
    assert_eq!(
        context.token,
        Some(json!({"id":"access","permission":["read:account"]}))
    );
    assert_eq!(
        *store.last_token_lookup.lock().await,
        Some((
            "app-token-longer-than-16".into(),
            "ApP-TOKEN-LONGER-THAN-16".into()
        ))
    );
    *store.app.lock().await = Some(json!({"id":"app","permission":[]}));
    assert_eq!(
        service
            .authenticate(Some("application-token"), APP_TTL)
            .await
            .unwrap()
            .token
            .as_ref()
            .unwrap()["permission"],
        json!(["read:account"])
    );
    assert_eq!(
        service
            .authenticate(Some("application-token"), APP_TTL + 1)
            .await
            .unwrap()
            .token
            .as_ref()
            .unwrap()["permission"],
        json!([])
    );
    tokio::task::yield_now().await;
    assert_eq!(store.updates.load(Ordering::SeqCst), 3);
    assert_eq!(store.app_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn miauth_keeps_the_full_access_token_snapshot_and_distinguishes_missing_tokens() {
    let (store, service) = service().await;
    assert!(matches!(
        service.authenticate(Some("not-a-valid-token"), 0).await,
        Err(AuthError::InvalidSignature)
    ));
    let record = user("abcdefghijklmnop");
    store
        .users
        .lock()
        .await
        .insert(record["id"].as_str().unwrap().into(), record.clone());
    let access =
        json!({"id":"access","userId":record["id"],"appId":null,"lastUsedAt":12,"permission":[]});
    *store.access.lock().await = Some(access.clone());
    let context = service
        .authenticate(Some("mi-auth-token-long"), 1)
        .await
        .unwrap();
    assert!(!context.access_token_reduced);
    assert_eq!(context.token.as_ref(), Some(&access));
}

#[tokio::test]
async fn role_assignments_expire_at_equality_and_conditional_roles_keep_legacy_order() {
    let (store, service) = service().await;
    let record = user("abcdefghijklmnop");
    let id = record["id"].as_str().unwrap();
    store.users.lock().await.insert(id.into(), record.clone());
    *store.role_rows.lock().await = vec![
        json!({"id":"assigned","target":"manual","policies":{}}),
        json!({"id":"conditional","target":"conditional","condFormula":{"type":"isLocal"},"policies":{}}),
    ];
    *store.assignment_rows.lock().await = vec![
        json!({"id":"a","roleId":"assigned","expiresAt":100}),
        json!({"id":"b","roleId":"conditional","expiresAt":null}),
    ];
    let ids = |roles: Vec<SafeJson>| {
        roles
            .into_iter()
            .map(|r| r["id"].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(service.roles_for(id, 99).await.unwrap()),
        json!(["assigned", "conditional", "conditional"])
            .as_array()
            .unwrap()
            .clone()
    );
    assert_eq!(
        ids(service.roles_for(id, 100).await.unwrap()),
        json!(["conditional", "conditional"])
            .as_array()
            .unwrap()
            .clone()
    );
    assert_eq!(store.role_reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn policy_priority_defaults_boolean_and_file_constraints_match_typescript() {
    let (store, service) = service().await;
    let id = "id";
    *store.role_rows.lock().await = vec![
        json!({"id":"a","target":"manual","policies":{
            "rateLimitFactor":{"priority":0,"useDefault":false,"value":100},
            "canInvite":{"priority":1,"useDefault":false,"value":true},
            "uploadableFileTypes":{"priority":0,"useDefault":false,"value":[" image/png ","","\u{feff}text/plain","image/png"]},
            "chatAvailability":{"priority":0,"useDefault":false,"value":"readonly"}
        }}),
        json!({"id":"b","target":"manual","policies":{
            "rateLimitFactor":{"priority":2,"useDefault":false,"value":0.5},
            "canInvite":{"priority":2,"useDefault":true,"value":true},
            "maxFileSizeMb":{"priority":2,"useDefault":false,"value":999},
            "chatAvailability":{"priority":0,"useDefault":false,"value":"unavailable"}
        }}),
    ];
    *store.assignment_rows.lock().await = vec![
        json!({"id":"a","roleId":"a","expiresAt":null}),
        json!({"id":"b","roleId":"b","expiresAt":null}),
    ];
    let policies = service.policies_for(Some(id), 0).await.unwrap();
    assert_eq!(policies["rateLimitFactor"], 0.5);
    assert_eq!(policies["canInvite"], false);
    assert_eq!(policies["maxFileSizeMb"], 30.0);
    assert_eq!(policies["chatAvailability"], "readonly");
    // Role b uses default file types, appended in first occurrence order.
    assert_eq!(
        policies["uploadableFileTypes"],
        json!(["image/png", "text/plain", "image/*", "text/*"])
    );
    service.apply_event(&event("metaUpdated",json!({"after":{"policies":{"rateLimitFactor":0,"canInvite":true,"extra":"anonymous-only"}}})),1).await.unwrap();
    assert_eq!(
        service.policies_for(None, 1).await.unwrap()["extra"],
        "anonymous-only"
    );
    assert!(
        service
            .policies_for(Some(id), 1)
            .await
            .unwrap()
            .get("extra")
            .is_none()
    );
}

#[tokio::test]
async fn user_events_refresh_suspension_and_regeneration_removes_the_old_native_token() {
    let (store, service) = service().await;
    let token = "abcdefghijklmnop";
    let mut record = user(token);
    let id = record["id"].as_str().unwrap().to_owned();
    store.users.lock().await.insert(id.clone(), record.clone());
    service.authenticate(Some(token), 0).await.unwrap();
    record["isSuspended"] = json!(true);
    store.users.lock().await.insert(id.clone(), record.clone());
    assert_eq!(
        service
            .authenticate(Some(token), 1)
            .await
            .unwrap()
            .user
            .as_ref()
            .unwrap()["isSuspended"],
        false
    );
    service
        .apply_event(&event("userChangeSuspendedState", json!({"id":id})), 2)
        .await
        .unwrap();
    assert_eq!(
        service
            .authenticate(Some(token), 3)
            .await
            .unwrap()
            .user
            .as_ref()
            .unwrap()["isSuspended"],
        true
    );
    let new_token = "ponmlkjihgfedcba";
    record["token"] = json!(new_token);
    store.users.lock().await.insert(id.clone(), record);
    service
        .apply_event(
            &event(
                "userTokenRegenerated",
                json!({"id":id,"oldToken":token,"newToken":new_token}),
            ),
            4,
        )
        .await
        .unwrap();
    assert!(matches!(
        service.authenticate(Some(token), 5).await,
        Err(AuthError::UserNotFound)
    ));
    assert!(
        service
            .authenticate(Some(new_token), 5)
            .await
            .unwrap()
            .user
            .is_some()
    );
}

#[tokio::test]
async fn follow_mutates_the_shared_user_snapshot_after_a_refresh_event_without_extending_ttl() {
    let (store, service) = service().await;
    let token = "abcdefghijklmnop";
    let record = user(token);
    let id = record["id"].as_str().unwrap();
    store.users.lock().await.insert(id.into(), record.clone());
    service
        .apply_event(&event("localUserUpdated", json!({"id":id})), 1)
        .await
        .unwrap();
    service
        .apply_event(
            &event("follow", json!({"followerId":id,"followeeId":"other"})),
            2,
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .authenticate(Some(token), 3)
            .await
            .unwrap()
            .user
            .as_ref()
            .unwrap()["followingCount"],
        5.0
    );
    assert_eq!(
        service
            .authenticate(Some(token), USER_TTL + 2)
            .await
            .unwrap()
            .user
            .as_ref()
            .unwrap()["followingCount"],
        4
    );
}

#[tokio::test]
async fn role_events_update_cached_rows_and_assignment_dates_without_refetching() {
    let (store, service) = service().await;
    let id = "id";
    assert!(service.roles_for(id, 0).await.unwrap().is_empty());
    service
        .apply_event(
            &event(
                "roleCreated",
                json!({"id":"role","target":"manual","policies":{}}),
            ),
            1,
        )
        .await
        .unwrap();
    service.apply_event(&event("userRoleAssigned",json!({"id":"a","userId":id,"roleId":"role","expiresAt":"1970-01-01T00:00:01.000Z"})),2).await.unwrap();
    assert_eq!(service.roles_for(id, 999).await.unwrap().len(), 1);
    assert!(service.roles_for(id, 1000).await.unwrap().is_empty());
    service
        .apply_event(
            &event(
                "roleUpdated",
                json!({"id":"role","target":"manual","isAdministrator":true,"policies":{}}),
            ),
            3,
        )
        .await
        .unwrap();
    assert_eq!(
        service.roles_for(id, 3).await.unwrap()[0]["isAdministrator"],
        true
    );
    service
        .apply_event(
            &event("userRoleUnassigned", json!({"id":"a","userId":id})),
            4,
        )
        .await
        .unwrap();
    assert!(service.roles_for(id, 4).await.unwrap().is_empty());
    assert_eq!(store.role_reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ip_logging_deduplicates_and_pubsub_gap_clears_cached_privileges() {
    let (store, service) = service().await;
    service.log_ip("user", "127.0.0.1", 1).await.unwrap();
    service.log_ip("user", "127.0.0.1", 2).await.unwrap();
    service.log_ip("other", "127.0.0.1", 3).await.unwrap();
    service.clear_ip_history().await;
    service
        .log_ip("user", "127.0.0.1", 60 * 60 * 1000)
        .await
        .unwrap();
    assert_eq!(store.ip_writes.load(Ordering::SeqCst), 3);
    let record = user("abcdefghijklmnop");
    store
        .users
        .lock()
        .await
        .insert(record["id"].as_str().unwrap().into(), record.clone());
    service
        .authenticate(Some("abcdefghijklmnop"), 0)
        .await
        .unwrap();
    store.users.lock().await.clear();
    *store.meta.lock().await =
        json!({"rootUserId":"new-root","policies":{},"enableIpLogging":false});
    service.invalidate_all().await.unwrap();
    assert!(matches!(
        service.authenticate(Some("abcdefghijklmnop"), 1).await,
        Err(AuthError::UserNotFound)
    ));
    assert_eq!(service.root_user_id().await.as_deref(), Some("new-root"));
    service.log_ip("new", "::1", 1).await.unwrap();
    assert_eq!(store.ip_writes.load(Ordering::SeqCst), 3);
}

#[test]
fn every_condition_and_id_encoding_obeys_boundary_comparisons() {
    let user = user("abcdefghijklmnop");
    let assigned = vec![json!({"id":"a"})];
    for (formula, expected) in [
        (json!({"type":"and","values":[]}), true),
        (json!({"type":"or","values":[]}), false),
        (json!({"type":"not","value":{"type":"isBot"}}), true),
        (json!({"type":"roleAssignedTo","roleId":"a"}), true),
        (json!({"type":"isLocal"}), true),
        (json!({"type":"isRemote"}), false),
        (json!({"type":"isSuspended"}), false),
        (json!({"type":"isLocked"}), true),
        (json!({"type":"isBot"}), false),
        (json!({"type":"isCat"}), true),
        (json!({"type":"isExplorable"}), true),
        (json!({"type":"followersLessThanOrEq","value":3}), true),
        (json!({"type":"followersMoreThanOrEq","value":4}), false),
        (json!({"type":"followingLessThanOrEq","value":4}), true),
        (json!({"type":"followingMoreThanOrEq","value":4}), true),
        (json!({"type":"notesLessThanOrEq","value":4}), false),
        (json!({"type":"notesMoreThanOrEq","value":5}), true),
        (json!({"type":"createdLessThan","sec":1}), true),
        (json!({"type":"createdMoreThan","sec":1}), false),
        (json!({"type":"unknown"}), false),
    ] {
        assert_eq!(
            evaluate_condition(&user, &assigned, &formula, "aidx", 946_684_800_001),
            expected,
            "{formula}"
        );
    }
    for (method, id, date) in [
        ("aid", "0000000100", 946_684_800_001.0),
        ("aidx", "0000000100000000", 946_684_800_001.0),
        ("meid", "800000000001000000000000", 1.0),
        ("meidg", "g00000000001000000000000", 1.0),
        ("objectid", "000000010000000000000000", 1000.0),
        ("ulid", "00000000010000000000000000", 1.0),
    ] {
        assert_eq!(parse_id_date(method, id), Some(date), "{method}");
    }
    assert_eq!(parse_id_date("aid", "1z!"), Some(946_684_800_071.0));
    assert_eq!(parse_id_date("aid", "!"), None);
    assert_eq!(parse_id_date("unknown", "1"), None);
}

#[test]
fn iso_dates_cover_negative_epoch_leap_years_and_malformed_inputs() {
    assert_eq!(role::parse_iso_date("1970-01-01T00:00:00.000Z"), Some(0));
    assert_eq!(role::parse_iso_date("1969-12-31T23:59:59.999Z"), Some(-1));
    assert_eq!(
        role::parse_iso_date("2000-02-29T00:00:00.000Z"),
        Some(951_782_400_000)
    );
    assert_eq!(role::parse_iso_date("1900-02-29T00:00:00.000Z"), None);
    assert_eq!(
        role::parse_iso_date("+010000-01-01T00:00:00.000Z"),
        Some(253_402_300_800_000)
    );
    assert_eq!(role::parse_iso_date("1970-01-01T00:00:00.00🦀"), None);
}

#[test]
fn differential_corpus_matches_actual_baseline_typescript_role_methods() {
    fn equivalent(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equivalent(a, b))
            }
            (Value::Object(a), Value::Object(b)) => {
                a.keys().eq(b.keys())
                    && a.iter()
                        .all(|(key, a)| b.get(key).is_some_and(|b| equivalent(a, b)))
            }
            _ => a == b,
        }
    }
    let fixture: Value =
        serde_json::from_str(include_str!("../tests/fixtures/role-reference.json")).unwrap();
    for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let actual = match case["kind"].as_str().unwrap() {
            "date" => json!(parse_id_date(
                case["method"].as_str().unwrap(),
                case["id"].as_str().unwrap()
            )),
            "condition" => json!(evaluate_condition(
                &case["user"],
                case["roles"].as_array().unwrap(),
                &case["formula"],
                case["method"].as_str().unwrap(),
                case["now"].as_i64().unwrap()
            )),
            "policies" => role::aggregate_policies(
                case["base"].as_object().unwrap(),
                case["roles"].as_array().unwrap(),
                case["maxFileSize"].as_u64().unwrap(),
            )
            .unwrap()
            .into_json(),
            _ => panic!("unknown reference case"),
        };
        assert!(
            equivalent(&actual, &case["expected"]),
            "reference case {index}: actual {actual}; expected {}",
            case["expected"]
        );
    }
}

#[test]
fn deep_auth_snapshots_and_configuration_clone_and_drop_iteratively() {
    let mut nested = Value::Bool(true);
    for _ in 0..10_000 {
        nested = Value::Array(vec![nested]);
    }
    let original = SafeJson::new(nested);
    let copied = original.clone();
    let mut value = &*copied;
    for _ in 0..10_000 {
        value = &value[0];
    }
    assert_eq!(value, &Value::Bool(true));
    let mut config = config();
    config.meta = Value::Object(Map::from_iter([("nested".into(), clone_json(&original))]));
    let copied_config = config.clone();
    let context = Context {
        user: Some(clone_json(&original)),
        token: None,
        access_token_reduced: false,
    };
    let copied_context = context.clone();
    drop(copied_context);
    drop(context);
    drop(copied_config);
    drop(config);
    drop(copied);
    drop(original);
}

#[test]
fn policy_values_use_js_numeric_coercion_and_reject_noniterable_file_types() {
    for (value, expected) in [
        (json!([true]), None),
        (json!([["0b10"]]), Some(2.0)),
        (json!("0o10"), Some(8.0)),
        (json!("0x10000000000000000"), Some(18446744073709551616.0)),
        (json!("inf"), None),
        (json!([]), Some(0.0)),
    ] {
        let base = Map::from_iter([("rateLimitFactor".into(), json!(1))]);
        let roles = vec![
            json!({"policies":{"rateLimitFactor":{"priority":2.0,"useDefault":false,"value":value}}}),
        ];
        let actual = role::aggregate_policies(&base, &roles, 1).unwrap();
        assert_eq!(actual["rateLimitFactor"].as_f64(), expected);
    }
    let base = Map::from_iter([("rateLimitFactor".into(), json!(1))]);
    let roles = vec![json!({"policies":{"rateLimitFactor":{"priority":2,"useDefault":false}}})];
    assert!(role::aggregate_policies(&base, &roles, 1).unwrap()["rateLimitFactor"].is_null());
    let base = Map::from_iter([("uploadableFileTypes".into(), json!(17))]);
    assert!(role::aggregate_policies::<Value>(&base, &[], 1).is_err());
}

fn assert_js_number(actual: f64, expected: f64) {
    if expected.is_nan() {
        assert!(actual.is_nan());
    } else {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }
}

#[tokio::test]
async fn policy_snapshot_preserves_nonfinite_factors_before_json_projection() {
    let (_, service) = service().await;
    for (at, (stored, expected)) in [
        (json!("Infinity"), f64::INFINITY),
        (json!("1e309"), f64::INFINITY),
        (json!("-Infinity"), f64::NEG_INFINITY),
        (json!("-1e309"), f64::NEG_INFINITY),
        (json!("NaN"), f64::NAN),
        (json!("invalid"), f64::NAN),
        (json!("-0"), -0.0),
        (json!(-0.0), -0.0),
        (json!(0), 0.0),
        (json!(0.5), 0.5),
        (json!("2.5"), 2.5),
        (json!("1e-309"), 1e-309),
        (json!(null), 0.0),
        (json!(false), 0.0),
        (json!(["Infinity"]), f64::INFINITY),
    ]
    .into_iter()
    .enumerate()
    {
        service
            .apply_event(
                &event(
                    "metaUpdated",
                    json!({"after":{"policies":{"rateLimitFactor":stored}}}),
                ),
                at as i64,
            )
            .await
            .unwrap();
        let snapshot = service.policies_for(Some("user"), at as i64).await.unwrap();
        let factor = snapshot.numeric_aggregate("rateLimitFactor").unwrap();
        assert_js_number(factor, expected);
        assert_eq!(factor > 0.0, expected > 0.0);
        assert_eq!(
            snapshot.is_truthy("rateLimitFactor"),
            expected != 0.0 && !expected.is_nan()
        );
        if expected.is_finite() {
            assert_js_number(snapshot["rateLimitFactor"].as_f64().unwrap(), expected);
        } else {
            assert!(snapshot["rateLimitFactor"].is_null());
        }
    }
}

#[test]
fn numerical_policy_priority_and_signed_zero_follow_math_max() {
    let base = Map::from_iter([("rateLimitFactor".into(), json!(1))]);
    let policy = |priority: u8, value: Value| json!({"policies":{"rateLimitFactor":{"priority":priority,"useDefault":false,"value":value}}});
    let snapshot = role::aggregate_policies(
        &base,
        &[policy(0, json!("NaN")), policy(2, json!("Infinity"))],
        1,
    )
    .unwrap();
    assert_js_number(
        snapshot.numeric_aggregate("rateLimitFactor").unwrap(),
        f64::INFINITY,
    );
    for values in [vec![json!("-0"), json!(0)], vec![json!(0), json!("-0")]] {
        let roles: Vec<_> = values.into_iter().map(|value| policy(2, value)).collect();
        let snapshot = role::aggregate_policies(&base, &roles, 1).unwrap();
        assert_js_number(snapshot.numeric_aggregate("rateLimitFactor").unwrap(), 0.0);
    }
    let roles = [json!({"policies":{"rateLimitFactor":{"priority":2,"useDefault":false}}})];
    assert!(
        role::aggregate_policies(&base, &roles, 1)
            .unwrap()
            .numeric_aggregate("rateLimitFactor")
            .unwrap()
            .is_nan()
    );
    let base = Map::from_iter([
        ("rateLimitFactor".into(), json!("-Infinity")),
        ("mentionLimit".into(), json!("Infinity")),
        ("maxFileSizeMb".into(), json!("-0")),
    ]);
    let snapshot = role::aggregate_policies::<Value>(&base, &[], 0).unwrap();
    assert_js_number(
        snapshot.numeric_aggregate("mentionLimit").unwrap(),
        f64::INFINITY,
    );
    assert_js_number(snapshot.numeric_aggregate("maxFileSizeMb").unwrap(), -0.0);
    assert!(snapshot.is_truthy("mentionLimit"));
    assert!(snapshot.is_truthy("rateLimitFactor"));
    assert!(!snapshot.is_truthy("maxFileSizeMb"));
}

#[tokio::test]
async fn nonfinite_factor_does_not_skip_other_policy_aggregation_errors() {
    let (_, service) = service().await;
    service
        .apply_event(
            &event(
                "metaUpdated",
                json!({"after":{"policies":{
                    "rateLimitFactor":"Infinity","uploadableFileTypes":17
                }}}),
            ),
            1,
        )
        .await
        .unwrap();
    assert!(service.policies_for(Some("user"), 1).await.is_err());
    let anonymous = service.policies_for(None, 1).await.unwrap();
    // The anonymous branch deliberately returns raw base values without the
    // numeric aggregation/coercion or the file-types aggregation exception.
    assert_eq!(anonymous["rateLimitFactor"], "Infinity");
    assert_eq!(anonymous.numeric_aggregate("rateLimitFactor"), None);
    assert_eq!(anonymous["uploadableFileTypes"], 17);
    assert!(anonymous.is_truthy("rateLimitFactor"));
}

#[test]
fn policy_numeric_coercion_preserves_stored_object_and_array_primitive_errors() {
    for value in [
        json!({"toString":1}),
        json!({"toString":null}),
        json!([{"toString":1}]),
        json!([0,{"toString":1}]),
        json!([[null,[{"toString":false}]],0]),
    ] {
        let base = Map::from_iter([("rateLimitFactor".into(), value)]);
        assert!(role::aggregate_policies::<Value>(&base, &[], 1).is_err());
    }
    for value in [
        json!({}),
        json!({"valueOf":1}),
        json!({"nested":{"toString":1}}),
        json!({"__proto__":{"toString":1}}),
        json!({"constructor":{"prototype":{"toString":1}}}),
    ] {
        let base = Map::from_iter([("rateLimitFactor".into(), value)]);
        let snapshot = role::aggregate_policies::<Value>(&base, &[], 1).unwrap();
        assert!(
            snapshot
                .numeric_aggregate("rateLimitFactor")
                .unwrap()
                .is_nan()
        );
    }
    for (value, expected) in [(json!([-0.0]), 0.0), (json!([["-0"]]), -0.0)] {
        let base = Map::from_iter([("rateLimitFactor".into(), value)]);
        let snapshot = role::aggregate_policies::<Value>(&base, &[], 1).unwrap();
        assert_js_number(
            snapshot.numeric_aggregate("rateLimitFactor").unwrap(),
            expected,
        );
    }
}

#[test]
fn policy_math_max_coerces_later_selected_values_even_after_nan() {
    let base = Map::from_iter([("rateLimitFactor".into(), json!(1))]);
    let policy = |priority, value| json!({"policies":{"rateLimitFactor":{"priority":priority,"useDefault":false,"value":value}}});
    let roles = [policy(2, json!("NaN")), policy(2, json!({"toString":1}))];
    assert!(role::aggregate_policies(&base, &roles, 1).is_err());
    let roles = [
        policy(2, json!("Infinity")),
        policy(0, json!({"toString":1})),
    ];
    let snapshot = role::aggregate_policies(&base, &roles, 1).unwrap();
    assert_js_number(
        snapshot.numeric_aggregate("rateLimitFactor").unwrap(),
        f64::INFINITY,
    );
}
