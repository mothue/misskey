use axum::{
    body::Body,
    http::{HeaderValue, Response, StatusCode},
};
use misskey_api_validation::Value;
use serde::Serialize;

/// Fixed API gate failures are defined once; callers select the failure rather
/// than rebuilding a response object or copying its wire identifiers.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ApiFailure {
    Internal,
    Unavailable,
    Authentication,
    InvalidCast,
    InvalidParam,
    UnknownEndpoint,
    AccessDenied,
    RateLimitExceeded,
    CredentialRequired,
    AccountSuspended,
    AccountMoved,
    ModeratorRequired,
    AdministratorRequired,
    RequiredRole,
    AppPermissionDenied,
    NoSuchAntenna,
    NoSuchClip,
    NoSuchPost,
    NoSuchFlash,
}

/// Captured from Bun 1.4.4 Date.toISOString for invalid stored dates.
pub(crate) struct LegacyErrorDef {
    pub message: &'static str,
    pub code: &'static str,
}
pub(crate) const INVALID_DATE: LegacyErrorDef = LegacyErrorDef {
    message: "Invalid Date",
    code: "RangeError",
};

// Captured from the pinned TS production Clip.pack. PostgreSQL infinity is a
// primitive, rather than a hydrated Date; differential tests guard this text.
pub(crate) const NON_DATE_CLIP_TIMESTAMP: LegacyErrorDef = LegacyErrorDef {
    message: "i.lastClippedAt.toISOString is not a function. (In 'i.lastClippedAt.toISOString()', 'i.lastClippedAt.toISOString' is undefined)",
    code: "TypeError",
};

// Actual pinned production GalleryPost.pack: the PG driver returns infinity
// as a primitive. Public HTTP fixtures guard the minifier-specific wording.
pub(crate) const NON_DATE_UPDATED_AT: LegacyErrorDef = LegacyErrorDef {
    message: "i.updatedAt.toISOString is not a function. (In 'i.updatedAt.toISOString()', 'i.updatedAt.toISOString' is undefined)",
    code: "TypeError",
};

// Actual UserEntityService filter callback in the pinned production build;
// null is also covered by public HTTP, undefined by the Bun property oracle.
pub(crate) const NULL_DECORATION_ID: LegacyErrorDef = LegacyErrorDef {
    message: "null is not an object (evaluating 't.id')",
    code: "TypeError",
};
pub(crate) const UNDEFINED_DECORATION_ID: LegacyErrorDef = LegacyErrorDef {
    message: "undefined is not an object (evaluating 't.id')",
    code: "TypeError",
};

struct ApiErrorPayload<'a> {
    message: &'a str,
    code: &'a str,
    id: &'a str,
    kind: &'a str,
    status: Option<u16>,
    info: Option<&'a Value>,
}

impl ApiFailure {
    fn payload(self) -> ApiErrorPayload<'static> {
        let (message, code, id, kind, status) = match self {
            Self::Internal => (
                "Internal error occurred. Please contact us if the error persists.",
                "INTERNAL_ERROR",
                "5d37dbcb-891e-41ca-a3d6-e690c97775ac",
                "server",
                Some(500),
            ),
            Self::Unavailable => (
                "Rust backend unavailable.",
                "RUST_BACKEND_UNAVAILABLE",
                "91600ba8-92ad-48fa-9afd-d6c615f06946",
                "server",
                Some(503),
            ),
            Self::Authentication => (
                "Authentication failed. Please ensure your token is correct.",
                "AUTHENTICATION_FAILED",
                "b0a7f5f8-dc2f-4171-b91f-de88ad238e14",
                "client",
                Some(401),
            ),
            Self::InvalidCast | Self::InvalidParam => (
                "Invalid param.",
                "INVALID_PARAM",
                if matches!(self, Self::InvalidCast) {
                    "0b5f1631-7c1a-41a6-b399-cce335f34d85"
                } else {
                    "3d81ceae-475f-4600-b2a8-2bc116157532"
                },
                "client",
                None,
            ),
            Self::UnknownEndpoint => (
                "Unknown API endpoint.",
                "UNKNOWN_API_ENDPOINT",
                "2ca3b769-540a-4f08-9dd5-b5a825b6d0f1",
                "client",
                Some(404),
            ),
            Self::AccessDenied => (
                "Access denied.",
                "ACCESS_DENIED",
                "56f35758-7dd5-468b-8439-5d6fb8ec9b8e",
                "client",
                None,
            ),
            Self::RateLimitExceeded => (
                "Rate limit exceeded. Please try again later.",
                "RATE_LIMIT_EXCEEDED",
                "d5826d14-3982-4d2e-8011-b9e9f02499ef",
                "client",
                Some(429),
            ),
            Self::CredentialRequired => (
                "Credential required.",
                "CREDENTIAL_REQUIRED",
                "1384574d-a912-4b81-8601-c7b1c4085df1",
                "client",
                Some(401),
            ),
            Self::AccountSuspended => (
                "Your account has been suspended.",
                "YOUR_ACCOUNT_SUSPENDED",
                "a8c724b3-6e9c-4b46-b1a8-bc3ed6258370",
                "permission",
                None,
            ),
            Self::AccountMoved => (
                "You have moved your account.",
                "YOUR_ACCOUNT_MOVED",
                "56f20ec9-fd06-4fa5-841b-edd6d7d4fa31",
                "permission",
                None,
            ),
            Self::ModeratorRequired => (
                "You are not assigned to a moderator role.",
                "ROLE_PERMISSION_DENIED",
                "d33d5333-db36-423d-a8f9-1a2b9549da41",
                "permission",
                None,
            ),
            Self::AdministratorRequired => (
                "You are not assigned to an administrator role.",
                "ROLE_PERMISSION_DENIED",
                "c3d38592-54c0-429d-be96-5636b0431a61",
                "permission",
                None,
            ),
            Self::RequiredRole => (
                "You are not assigned to a required role.",
                "ROLE_PERMISSION_DENIED",
                "7f86f06f-7e15-4057-8561-f4b6d4ac755a",
                "permission",
                None,
            ),
            Self::NoSuchAntenna => (
                "No such antenna.",
                "NO_SUCH_ANTENNA",
                "c06569fb-b025-4f23-b22d-1fcd20d2816b",
                "client",
                None,
            ),
            Self::NoSuchClip => (
                "No such clip.",
                "NO_SUCH_CLIP",
                "c3c5fe33-d62c-44d2-9ea5-d997703f5c20",
                "client",
                None,
            ),
            Self::NoSuchPost => (
                "No such post.",
                "NO_SUCH_POST",
                "1137bf14-c5b0-4604-85bb-5b5371b1cd45",
                "client",
                None,
            ),
            Self::NoSuchFlash => (
                "No such flash.",
                "NO_SUCH_FLASH",
                "f0d34a1a-d29a-401d-90ba-1982122b5630",
                "client",
                None,
            ),
            Self::AppPermissionDenied => (
                "Your app does not have the necessary permissions to use this endpoint.",
                "PERMISSION_DENIED",
                "1370e5b7-d4eb-4566-bb1d-7748ee6a1838",
                "permission",
                None,
            ),
        };
        ApiErrorPayload {
            message,
            code,
            id,
            kind,
            status,
            info: None,
        }
    }
}

impl ApiErrorPayload<'_> {
    fn status(&self) -> u16 {
        self.status.unwrap_or(match self.kind {
            "client" => 400,
            "permission" => 403,
            _ => 500,
        })
    }

    fn value(&self) -> Value {
        // Value retains JS member ordering, lone surrogates, undefined omission
        // and nonfinite-number serialization for callback error information.
        let mut error = Value::object([
            ("message", Value::string(self.message)),
            ("code", Value::string(self.code)),
            ("id", Value::string(self.id)),
            ("kind", Value::string(self.kind)),
        ]);
        if let Some(info) = self.info.filter(|info| info.truthy()) {
            error.set("info", info.clone());
        }
        Value::object([("error", error)])
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FrameworkErrorPayload<'a> {
    status_code: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'a str>,
    error: &'a str,
    message: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NotFoundPayload {
    message: String,
    error: &'static str,
    status_code: u16,
}

pub fn plain(status: u16, content_type: Option<&str>, body: impl Into<Body>) -> Response<Body> {
    let mut response = Response::new(body.into());
    *response.status_mut() =
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if let Some(kind) = content_type {
        response.headers_mut().insert(
            "content-type",
            HeaderValue::from_str(kind).expect("constant content type"),
        );
    }
    response
}
pub fn json(status: u16, value: serde_json::Value) -> Response<Body> {
    plain(
        status,
        Some("application/json; charset=utf-8"),
        value.to_string(),
    )
}
pub fn framework(status: u16, code: &str, message: &str) -> Response<Body> {
    framework_with_code(status, Some(code), message, "Internal Server Error")
}
pub(crate) fn framework_with_code(
    status: u16,
    code: Option<&str>,
    message: &str,
    fallback: &str,
) -> Response<Body> {
    let payload = FrameworkErrorPayload {
        status_code: status,
        code,
        error: StatusCode::from_u16(status)
            .ok()
            .and_then(|status| status.canonical_reason())
            .unwrap_or(fallback),
        message,
    };
    serialized(status, &payload)
}
fn serialized(status: u16, payload: &impl Serialize) -> Response<Body> {
    match serde_json::to_string(payload) {
        Ok(text) => plain(status, Some("application/json; charset=utf-8"), text),
        Err(_) => internal(),
    }
}
pub(crate) fn not_found(method: &str, path: &str) -> Response<Body> {
    serialized(
        404,
        &NotFoundPayload {
            message: format!("Route {method}:{path} not found"),
            error: "Not Found",
            status_code: 404,
        },
    )
}
pub(crate) fn fixed(error: ApiFailure, info: Option<&Value>) -> Response<Body> {
    let mut payload = error.payload();
    if matches!(error, ApiFailure::UnknownEndpoint) {
        // The API wildcard's legacy response does not challenge authentication.
        return plain(
            404,
            Some("application/json; charset=utf-8"),
            payload.value().stringify().expect("API error object"),
        );
    }
    payload.info = info;
    api_error_payload(payload)
}
pub fn internal() -> Response<Body> {
    fixed(ApiFailure::Internal, None)
}
pub fn unavailable() -> Response<Body> {
    fixed(ApiFailure::Unavailable, None)
}
pub fn authentication() -> Response<Body> {
    let mut response = fixed(ApiFailure::Authentication, None);
    response.headers_mut().insert("www-authenticate", HeaderValue::from_static("Bearer realm=\"Misskey\", error=\"invalid_token\", error_description=\"Authentication failed. Please ensure your token is correct.\""));
    response
}
pub fn invalid(param: &str, reason: &str, cast: bool) -> Response<Body> {
    let info = Value::object([
        ("param", Value::string(param)),
        ("reason", Value::string(reason)),
    ]);
    fixed(
        if cast {
            ApiFailure::InvalidCast
        } else {
            ApiFailure::InvalidParam
        },
        Some(&info),
    )
}
pub fn api_error(
    message: &str,
    code: &str,
    id: &str,
    kind: &str,
    explicit: Option<u16>,
    info: Option<&Value>,
) -> Response<Body> {
    api_error_payload(ApiErrorPayload {
        message,
        code,
        id,
        kind,
        status: explicit,
        info,
    })
}
fn api_error_payload(payload: ApiErrorPayload<'_>) -> Response<Body> {
    let status = payload.status();
    let ApiErrorPayload {
        message,
        code,
        kind,
        info,
        ..
    } = &payload;
    let mut response = plain(
        status,
        Some("application/json; charset=utf-8"),
        payload.value().stringify().expect("API error object"),
    );
    let authenticate = if status == 401 {
        Some("Bearer realm=\"Misskey\"".into())
    } else if *code == "RATE_LIMIT_EXCEEDED" {
        None
    } else if *kind == "client" {
        Some(format!(
            "Bearer realm=\"Misskey\", error=\"invalid_request\", error_description=\"{message}\""
        ))
    } else if *kind == "permission" && *code == "PERMISSION_DENIED" {
        Some(format!(
            "Bearer realm=\"Misskey\", error=\"insufficient_scope\", error_description=\"{message}\""
        ))
    } else {
        None
    };
    if let Some(value) = authenticate.and_then(|h| HeaderValue::from_str(&h).ok()) {
        response.headers_mut().insert("www-authenticate", value);
    }
    if *code == "RATE_LIMIT_EXCEEDED"
        && let Some(reset) = info
            .and_then(|v| v.get("resetMs"))
            .and_then(Value::as_number)
    {
        let now = crate::now_ms() as f64;
        let seconds = ((reset - now) / 1000.).ceil().max(0.);
        if let Ok(value) = HeaderValue::from_str(&format!("{seconds:.0}")) {
            response.headers_mut().insert("retry-after", value);
        }
    }
    response
}
pub fn value(value: &Value) -> Response<Body> {
    match value {
        Value::Undefined | Value::Null => plain(204, None, Body::empty()),
        Value::String(_) => plain(
            200,
            Some("text/plain; charset=utf-8"),
            value.stringify().unwrap_or_default(),
        ),
        _ => plain(
            200,
            Some("application/json; charset=utf-8"),
            value.stringify().unwrap_or_default(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn text(reply: Response<Body>) -> String {
        String::from_utf8(
            to_bytes(reply.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn gate_failures_keep_status_identifiers_and_challenges() {
        let cases = [
            (
                ApiFailure::Internal,
                500,
                "INTERNAL_ERROR",
                "5d37dbcb-891e-41ca-a3d6-e690c97775ac",
                None,
            ),
            (
                ApiFailure::Unavailable,
                503,
                "RUST_BACKEND_UNAVAILABLE",
                "91600ba8-92ad-48fa-9afd-d6c615f06946",
                None,
            ),
            (
                ApiFailure::AccessDenied,
                400,
                "ACCESS_DENIED",
                "56f35758-7dd5-468b-8439-5d6fb8ec9b8e",
                Some("invalid_request"),
            ),
            (
                ApiFailure::CredentialRequired,
                401,
                "CREDENTIAL_REQUIRED",
                "1384574d-a912-4b81-8601-c7b1c4085df1",
                Some("Bearer realm=\"Misskey\""),
            ),
            (
                ApiFailure::AccountSuspended,
                403,
                "YOUR_ACCOUNT_SUSPENDED",
                "a8c724b3-6e9c-4b46-b1a8-bc3ed6258370",
                None,
            ),
            (
                ApiFailure::AccountMoved,
                403,
                "YOUR_ACCOUNT_MOVED",
                "56f20ec9-fd06-4fa5-841b-edd6d7d4fa31",
                None,
            ),
            (
                ApiFailure::ModeratorRequired,
                403,
                "ROLE_PERMISSION_DENIED",
                "d33d5333-db36-423d-a8f9-1a2b9549da41",
                None,
            ),
            (
                ApiFailure::AdministratorRequired,
                403,
                "ROLE_PERMISSION_DENIED",
                "c3d38592-54c0-429d-be96-5636b0431a61",
                None,
            ),
            (
                ApiFailure::RequiredRole,
                403,
                "ROLE_PERMISSION_DENIED",
                "7f86f06f-7e15-4057-8561-f4b6d4ac755a",
                None,
            ),
            (
                ApiFailure::AppPermissionDenied,
                403,
                "PERMISSION_DENIED",
                "1370e5b7-d4eb-4566-bb1d-7748ee6a1838",
                Some("insufficient_scope"),
            ),
            (
                ApiFailure::UnknownEndpoint,
                404,
                "UNKNOWN_API_ENDPOINT",
                "2ca3b769-540a-4f08-9dd5-b5a825b6d0f1",
                None,
            ),
        ];
        for (failure, status, code, id, challenge) in cases {
            let reply = fixed(failure, None);
            assert_eq!(reply.status(), status);
            assert_eq!(
                reply.headers()["content-type"],
                "application/json; charset=utf-8"
            );
            match challenge {
                Some(challenge) => assert!(
                    reply.headers()["www-authenticate"]
                        .to_str()
                        .unwrap()
                        .contains(challenge)
                ),
                None => assert!(!reply.headers().contains_key("www-authenticate")),
            }
            let body: serde_json::Value = serde_json::from_str(&text(reply).await).unwrap();
            assert_eq!(body["error"]["code"], code);
            assert_eq!(body["error"]["id"], id);
            assert!(body["error"].get("info").is_none());
        }
    }

    #[tokio::test]
    async fn no_such_clip_matches_captured_typescript_show_response() {
        // Fresh clips/show TS HTTP baseline: unknown-anon and private-anon.
        let reply = fixed(ApiFailure::NoSuchClip, None);
        assert_eq!(reply.status(), 400);
        assert_eq!(
            reply.headers()["content-type"],
            "application/json; charset=utf-8"
        );
        assert_eq!(
            reply.headers()["www-authenticate"],
            "Bearer realm=\"Misskey\", error=\"invalid_request\", error_description=\"No such clip.\""
        );
        assert_eq!(
            text(reply).await,
            "{\"error\":{\"message\":\"No such clip.\",\"code\":\"NO_SUCH_CLIP\",\"id\":\"c3c5fe33-d62c-44d2-9ea5-d997703f5c20\",\"kind\":\"client\"}}"
        );
    }

    #[tokio::test]
    async fn error_info_keeps_javascript_omission_order_and_null_rules() {
        for info in [
            Value::Undefined,
            Value::Null,
            Value::Boolean(false),
            Value::Number(0.),
            Value::Number(f64::NAN),
            Value::string(""),
        ] {
            assert_eq!(
                text(api_error("m", "C", "id", "server", None, Some(&info))).await,
                "{\"error\":{\"message\":\"m\",\"code\":\"C\",\"id\":\"id\",\"kind\":\"server\"}}"
            );
        }
        let info = Value::object([
            ("missing", Value::Undefined),
            ("null", Value::Null),
            ("number", Value::Number(f64::INFINITY)),
            ("surrogate", Value::String(vec![0xd800])),
            (
                "array",
                Value::Array(vec![Value::Undefined, Value::Number(f64::NAN)]),
            ),
        ]);
        assert_eq!(
            text(api_error("m", "C", "id", "server", None, Some(&info))).await,
            "{\"error\":{\"message\":\"m\",\"code\":\"C\",\"id\":\"id\",\"kind\":\"server\",\"info\":{\"null\":null,\"number\":null,\"surrogate\":\"\\ud800\",\"array\":[null,null]}}}"
        );
        assert_eq!(
            text(fixed(ApiFailure::UnknownEndpoint, None)).await,
            "{\"error\":{\"message\":\"Unknown API endpoint.\",\"code\":\"UNKNOWN_API_ENDPOINT\",\"id\":\"2ca3b769-540a-4f08-9dd5-b5a825b6d0f1\",\"kind\":\"client\"}}"
        );
    }

    #[tokio::test]
    async fn framework_code_omission_and_global_not_found_are_preserved() {
        let reply = framework_with_code(418, None, "teapot", "Error");
        assert_eq!(reply.status(), 418);
        assert_eq!(
            text(reply).await,
            "{\"statusCode\":418,\"error\":\"I'm a teapot\",\"message\":\"teapot\"}"
        );
        assert_eq!(
            text(framework(400, "FST_ERR", "invalid")).await,
            "{\"statusCode\":400,\"code\":\"FST_ERR\",\"error\":\"Bad Request\",\"message\":\"invalid\"}"
        );
        let reply = not_found("POST", "/api/missing?x=1");
        assert_eq!(reply.status(), 404);
        assert!(!reply.headers().contains_key("www-authenticate"));
        assert_eq!(
            text(reply).await,
            "{\"message\":\"Route POST:/api/missing?x=1 not found\",\"error\":\"Not Found\",\"statusCode\":404}"
        );
    }

    #[tokio::test]
    async fn authentication_validation_and_retry_headers_keep_their_contract() {
        let reply = authentication();
        assert_eq!(reply.status(), 401);
        assert_eq!(
            reply.headers()["www-authenticate"],
            "Bearer realm=\"Misskey\", error=\"invalid_token\", error_description=\"Authentication failed. Please ensure your token is correct.\""
        );
        for (cast, id) in [
            (true, "0b5f1631-7c1a-41a6-b399-cce335f34d85"),
            (false, "3d81ceae-475f-4600-b2a8-2bc116157532"),
        ] {
            let reply = invalid("limit", "must be integer", cast);
            assert_eq!(reply.status(), 400);
            let body: serde_json::Value = serde_json::from_str(&text(reply).await).unwrap();
            assert_eq!(body["error"]["id"], id);
            assert_eq!(body["error"]["info"]["param"], "limit");
            assert_eq!(body["error"]["info"]["reason"], "must be integer");
        }
        let info = Value::object([("resetMs", Value::Number(0.))]);
        let reply = fixed(ApiFailure::RateLimitExceeded, Some(&info));
        assert_eq!(reply.status(), 429);
        assert_eq!(reply.headers()["retry-after"], "0");
        assert!(!reply.headers().contains_key("www-authenticate"));
        let reply = api_error("m", "C", "id", "custom", Some(422), None);
        assert_eq!(reply.status(), 422);
        assert!(text(reply).await.contains("\"kind\":\"custom\""));
    }

    #[test]
    fn failure_diagnostics_contain_only_the_sanitized_stage() {
        let failure = crate::Failure::new(crate::FailureStage::CallbackTransport);
        assert_eq!(failure.stage(), crate::FailureStage::CallbackTransport);
        assert_eq!(
            format!("{failure}"),
            "Rust backend unavailable at CallbackTransport"
        );
        assert_eq!(
            format!("{failure:?}"),
            "Failure { stage: CallbackTransport }"
        );
    }
}
