use axum::{
    body::Body,
    http::{HeaderValue, Response, StatusCode},
};
use misskey_api_validation::Value;
use serde_json::json;

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
    json(
        status,
        json!({"statusCode":status,"code":code,"error":StatusCode::from_u16(status).ok().and_then(|s|s.canonical_reason()).unwrap_or("Internal Server Error"),"message":message}),
    )
}
pub fn internal() -> Response<Body> {
    api_error(
        "Internal error occurred. Please contact us if the error persists.",
        "INTERNAL_ERROR",
        "5d37dbcb-891e-41ca-a3d6-e690c97775ac",
        "server",
        Some(500),
        None,
    )
}
pub fn unavailable() -> Response<Body> {
    api_error(
        "Rust backend unavailable.",
        "RUST_BACKEND_UNAVAILABLE",
        "91600ba8-92ad-48fa-9afd-d6c615f06946",
        "server",
        Some(503),
        None,
    )
}
pub fn authentication() -> Response<Body> {
    let message = "Authentication failed. Please ensure your token is correct.";
    let mut response = api_error(
        message,
        "AUTHENTICATION_FAILED",
        "b0a7f5f8-dc2f-4171-b91f-de88ad238e14",
        "client",
        Some(401),
        None,
    );
    response.headers_mut().insert("www-authenticate", HeaderValue::from_static("Bearer realm=\"Misskey\", error=\"invalid_token\", error_description=\"Authentication failed. Please ensure your token is correct.\""));
    response
}
pub fn invalid(param: &str, reason: &str, cast: bool) -> Response<Body> {
    let info = Value::object([
        ("param", Value::string(param)),
        ("reason", Value::string(reason)),
    ]);
    api_error(
        "Invalid param.",
        "INVALID_PARAM",
        if cast {
            "0b5f1631-7c1a-41a6-b399-cce335f34d85"
        } else {
            "3d81ceae-475f-4600-b2a8-2bc116157532"
        },
        "client",
        None,
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
    let status = explicit.unwrap_or(if kind == "client" {
        400
    } else if kind == "permission" {
        403
    } else {
        500
    });
    let mut error = Value::object([
        ("message", Value::string(message)),
        ("code", Value::string(code)),
        ("id", Value::string(id)),
        ("kind", Value::string(kind)),
    ]);
    if let Some(info) = info.filter(|info| info.truthy()) {
        error.set("info", info.clone());
    }
    let value = Value::object([("error", error)]);
    let mut response = plain(
        status,
        Some("application/json; charset=utf-8"),
        value.stringify().unwrap_or_default(),
    );
    let authenticate = if status == 401 {
        Some("Bearer realm=\"Misskey\"".into())
    } else if code == "RATE_LIMIT_EXCEEDED" {
        None
    } else if kind == "client" {
        Some(format!(
            "Bearer realm=\"Misskey\", error=\"invalid_request\", error_description=\"{message}\""
        ))
    } else if kind == "permission" && code == "PERMISSION_DENIED" {
        Some(format!(
            "Bearer realm=\"Misskey\", error=\"insufficient_scope\", error_description=\"{message}\""
        ))
    } else {
        None
    };
    if let Some(value) = authenticate.and_then(|h| HeaderValue::from_str(&h).ok()) {
        response.headers_mut().insert("www-authenticate", value);
    }
    if code == "RATE_LIMIT_EXCEEDED"
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
