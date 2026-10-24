use super::*;
use async_trait::async_trait;
use axum::body::to_bytes;
use misskey_backend_api_auth::Failure as AuthFailure;
use std::sync::atomic::{AtomicUsize, Ordering};

fn context(authorization: Option<&str>) -> RequestContext {
    RequestContext {
        v: 1,
        request_id: "test".into(),
        path: "/api/test".into(),
        method: "POST".into(),
        ip: "127.0.0.1".into(),
        headers: authorization
            .map(|value| [("authorization".into(), json!(value))].into())
            .unwrap_or_default(),
    }
}

#[tokio::test]
async fn invalid_effective_tokens_finish_with_empty_400_and_header_precedence() {
    for token in [
        Value::Boolean(false),
        Value::Boolean(true),
        Value::Number(0.),
        Value::Number(1.),
        Value::Array(Vec::new()),
        Value::object([] as [(&str, Value); 0]),
    ] {
        let params = Value::object([("i", token)]);
        for header in [None, Some("Basic ignored"), Some("bearer ignored")] {
            let reply = effective_token(&context(header), &params).unwrap_err();
            assert_eq!(reply.status(), 400);
            assert!(reply.headers().is_empty());
            assert!(to_bytes(reply.into_body(), 1).await.unwrap().is_empty());
        }
        for header in ["Bearer valid", "Bearer "] {
            let value = effective_token(&context(Some(header)), &params).unwrap();
            assert_eq!(value.to_utf8().as_deref(), Some(&header[7..]));
        }
    }
    for token in [
        Value::Undefined,
        Value::Null,
        Value::string(""),
        Value::string("token"),
    ] {
        let params = Value::object([("i", token.clone())]);
        assert_eq!(effective_token(&context(None), &params).unwrap(), token);
    }
    assert_eq!(
        effective_token(&context(None), &Value::object([] as [(&str, Value); 0])).unwrap(),
        Value::Undefined
    );
}

#[test]
fn anonymous_policy_checks_require_live_invalidation_and_never_count_as_root() {
    let public_meta = EndpointMeta::default();
    let policy_meta = EndpointMeta {
        required_role_policy: Some("canSearchUsers".into()),
        ..Default::default()
    };
    for token in [Value::Undefined, Value::Null, Value::string("")] {
        assert!(!requires_auth_cache(&token, &public_meta));
        assert!(requires_auth_cache(&token, &policy_meta));
    }
    assert!(requires_auth_cache(&Value::string("token"), &public_meta));
    assert!(!is_root_user(None, None));
    assert!(!is_root_user(None, Some("root")));
    assert!(!is_root_user(Some("regular"), None));
    assert!(!is_root_user(Some("regular"), Some("root")));
    assert!(is_root_user(Some("root"), Some("root")));
}

#[derive(Default)]
struct PolicyStore {
    roles: Vec<Json>,
    role_reads: AtomicUsize,
    assignment_reads: AtomicUsize,
}
#[async_trait]
impl Store for PolicyStore {
    async fn user_by_token(&self, _: &str) -> Result<Option<Json>, AuthFailure> {
        panic!("unexpected token lookup")
    }
    async fn user_by_id(&self, _: &str) -> Result<Option<Json>, AuthFailure> {
        panic!("unexpected conditional-role user lookup")
    }
    async fn access_token_by_token(&self, _: &str, _: &str) -> Result<Option<Json>, AuthFailure> {
        panic!("unexpected access-token lookup")
    }
    async fn update_last_used(&self, _: &str, _: i64) -> Result<(), AuthFailure> {
        panic!("unexpected last-used write")
    }
    async fn app_by_id(&self, _: &str) -> Result<Option<Json>, AuthFailure> {
        panic!("unexpected app lookup")
    }
    async fn roles(&self) -> Result<Vec<Json>, AuthFailure> {
        self.role_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.roles.clone())
    }
    async fn meta(&self) -> Result<Json, AuthFailure> {
        panic!("unexpected Meta refresh")
    }
    async fn assignments(&self, _: &str) -> Result<Vec<Json>, AuthFailure> {
        self.assignment_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .roles
            .iter()
            .map(|role| json!({"roleId":role["id"],"expiresAt":null}))
            .collect())
    }
    async fn insert_ip(&self, _: i64, _: &str, _: &str) -> Result<(), AuthFailure> {
        panic!("unexpected IP write")
    }
}
fn auth_service(
    can_search: bool,
    uploadable: Json,
    roles: Vec<Json>,
) -> (Arc<PolicyStore>, AuthService<PolicyStore>) {
    let store = Arc::new(PolicyStore {
        roles,
        ..Default::default()
    });
    let service = AuthService::new(
        store.clone(),
        AuthConfig {
            id_method: "aidx".into(),
            max_file_size: 30 * 1024 * 1024,
            default_policies: json!({"canSearchUsers":can_search,"uploadableFileTypes":uploadable}),
            meta: json!({"rootUserId":"root","policies":{}}),
            entity_layouts: Default::default(),
        },
        now_ms(),
    )
    .unwrap();
    (store, service)
}
async fn error_json(reply: Response<Body>) -> Json {
    serde_json::from_slice(&to_bytes(reply.into_body(), 8192).await.unwrap()).unwrap()
}

#[tokio::test]
async fn anonymous_policy_uses_base_and_meta_without_loading_user_roles() {
    for root in [None, Some("root")] {
        let (store, auth) = auth_service(true, json!(["image/*"]), Vec::new());
        authorize_required_policy(&auth, None, root, "canSearchUsers")
            .await
            .unwrap();
        auth.apply_event(&json!({"channel":"internal","message":{"type":"metaUpdated","body":{"after":{"policies":{"canSearchUsers":false}}}}}), now_ms()).await.unwrap();
        let reply = authorize_required_policy(&auth, None, root, "canSearchUsers")
            .await
            .unwrap_err();
        assert_eq!(reply.status(), 403);
        assert!(!reply.headers().contains_key("www-authenticate"));
        assert_eq!(
            error_json(*reply).await,
            json!({"error":{
                "message":"You are not assigned to a required role.", "code":"ROLE_PERMISSION_DENIED",
                "id":"7f86f06f-7e15-4057-8561-f4b6d4ac755a", "kind":"permission"
            }})
        );
        assert_eq!(store.role_reads.load(Ordering::SeqCst), 0);
        assert_eq!(store.assignment_reads.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn authenticated_policy_and_actual_root_and_administrator_bypasses() {
    let (store, auth) = auth_service(false, json!(["image/*"]), Vec::new());
    let denied = authorize_required_policy(&auth, Some("regular"), Some("root"), "canSearchUsers")
        .await
        .unwrap_err();
    assert_eq!(denied.status(), 403);
    assert_eq!(store.role_reads.load(Ordering::SeqCst), 1);
    authorize_required_policy(&auth, Some("root"), Some("root"), "canSearchUsers")
        .await
        .unwrap();
    assert_eq!(store.assignment_reads.load(Ordering::SeqCst), 1);
    let (_, auth) = auth_service(
        false,
        json!(["image/*"]),
        vec![json!({"id":"admin-role","target":"manual","isAdministrator":true,"policies":{}})],
    );
    authorize_required_policy(&auth, Some("regular"), Some("root"), "canSearchUsers")
        .await
        .unwrap();
}

#[tokio::test]
async fn malformed_accepted_policies_fail_with_fixed_internal_error_and_strings_stay_valid() {
    for uploadable in [json!(17), json!([false]), json!({})] {
        let (_, auth) = auth_service(true, uploadable, Vec::new());
        let reply =
            authorize_required_policy(&auth, Some("regular"), Some("root"), "canSearchUsers")
                .await
                .unwrap_err();
        assert_eq!(reply.status(), 500);
        assert!(!reply.headers().contains_key("www-authenticate"));
        assert_eq!(
            error_json(*reply).await,
            json!({"error":{
                "message":"Internal error occurred. Please contact us if the error persists.",
                "code":"INTERNAL_ERROR", "id":"5d37dbcb-891e-41ca-a3d6-e690c97775ac", "kind":"server"
            }})
        );
    }
    for uploadable in [json!(["image/*"]), json!("xy")] {
        let (_, auth) = auth_service(true, uploadable, Vec::new());
        authorize_required_policy(&auth, Some("regular"), Some("root"), "canSearchUsers")
            .await
            .unwrap();
    }
}

#[test]
fn special_routes_match_their_methods_without_restricting_registry_routes() {
    for method in ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"] {
        assert!(route_is_known(true, false, "test", method));
        assert!(route_is_known(false, true, "clear-browser-cache", method));
        assert!(!route_is_known(false, false, "unknown", method));
        for name in [
            "signup",
            "signin-flow",
            "signin-with-passkey",
            "signup-pending",
            "miauth/probe/check",
        ] {
            assert_eq!(
                route_is_known(false, true, name, method),
                method == "POST",
                "{method} {name}"
            );
        }
        assert_eq!(
            route_is_known(false, true, "v1/instance/peers", method),
            matches!(method, "GET" | "HEAD")
        );
    }
}

#[tokio::test]
async fn unmatched_routes_parse_bodies_before_global_404_without_api_headers() {
    for (content_type, body, status, code) in [
        ("application/json", "{}", 404, None),
        (
            "application/json",
            "{",
            400,
            Some("FST_ERR_CTP_INVALID_JSON_BODY"),
        ),
        (
            "application/json",
            "",
            400,
            Some("FST_ERR_CTP_EMPTY_JSON_BODY"),
        ),
        ("multipart/form-data; boundary=probe", "x", 404, None),
    ] {
        let mut context = context(None);
        context.method = "PUT".into();
        context.path = "/api/signup".into();
        context
            .headers
            .insert("content-type".into(), json!(content_type));
        let reply = unmatched_route(&context, Body::from(body), 64, "/unused").await;
        assert_eq!(reply.status(), status);
        assert!(!reply.headers().contains_key("access-control-allow-origin"));
        assert!(!reply.headers().contains_key("cache-control"));
        let json = error_json(reply).await;
        if let Some(code) = code {
            assert_eq!(json["code"], code);
        } else {
            assert_eq!(
                json,
                json!({"message":"Route PUT:/api/signup not found","error":"Not Found","statusCode":404})
            );
        }
    }
}
