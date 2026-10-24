//! Complete native read operations. No operation delegates back to TypeScript.
use crate::Runtime;
use misskey_api_validation::Value;
use misskey_backend_api_auth::Context;
use misskey_backend_postgres::{Database, QueryError};
use tokio_postgres::types::ToSql;

mod discovery;
mod id;
mod registry_apps;
mod roles_admin;
use id::{iso_date, parse_id_date};

pub(super) struct Config {
    pub id_method: String,
    pub default_policies: Value,
}

impl Config {
    pub fn new(id_method: &str, default_policies: &serde_json::Value) -> Self {
        Self {
            id_method: id_method.to_lowercase(),
            default_policies: Value::from_json(default_policies),
        }
    }
}

#[derive(Debug)]
pub(super) enum Error {
    Internal,
    Legacy {
        message: String,
        code: &'static str,
        id: String,
    },
    Api {
        message: &'static str,
        code: &'static str,
        id: &'static str,
        kind: &'static str,
        status: Option<u16>,
        info: Option<Value>,
    },
}

impl Error {
    pub(super) fn legacy(message: String, code: &'static str) -> Self {
        let Ok(id) = correlation_id() else {
            return Self::Internal;
        };
        Self::Legacy { message, code, id }
    }

    fn query(error: QueryError) -> Self {
        let Some(legacy) = error.legacy_error() else {
            // Infrastructure-specific error detail is best effort. Retain the
            // strict outer INTERNAL_ERROR/500 contract without inventing facts.
            return Self::Internal;
        };
        Self::legacy(legacy.message, legacy.name)
    }

    pub fn response(self) -> axum::http::Response<axum::body::Body> {
        match self {
            Self::Internal => crate::response::internal(),
            Self::Legacy { message, code, id } => {
                // PostgreSQL messages can contain user-supplied data. Only the
                // correlation ID and error class go to stderr; never SQL,
                // parameters, credentials, server detail or the raw error.
                tracing::error!(event_name = "api.endpoint.failed", error.id = %id, error.code = code, "endpoint failed");
                let info = Value::object([(
                    "e",
                    Value::object([
                        ("message", Value::string(&message)),
                        ("code", Value::string(code)),
                        ("id", Value::string(&id)),
                    ]),
                )]);
                crate::response::api_error(
                    "Internal error occurred. Please contact us if the error persists.",
                    "INTERNAL_ERROR",
                    "5d37dbcb-891e-41ca-a3d6-e690c97775ac",
                    "server",
                    Some(500),
                    Some(&info),
                )
            }
            Self::Api {
                message,
                code,
                id,
                kind,
                status,
                info,
            } => crate::response::api_error(message, code, id, kind, status, info.as_ref()),
        }
    }
}

fn correlation_id() -> Result<String, Error> {
    use std::fmt::Write;
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| Error::Internal)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut id = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            id.push('-');
        }
        write!(id, "{byte:02x}").map_err(|_| Error::Internal)?;
    }
    Ok(id)
}

/// JSON text keeps PostgreSQL's selected property order and JavaScript Number
/// decoding. Callers explicitly choose the same read/primary path as the TS query.
pub(super) async fn rows(
    database: &Database,
    read: bool,
    sql: &str,
    params: &[&(dyn ToSql + Sync)],
) -> Result<Vec<Value>, Error> {
    database
        .query_typed(read, sql, params)
        .await
        .map_err(Error::query)?
        .into_iter()
        .map(|row| {
            let text: String = row.try_get(0).map_err(|_| Error::Internal)?;
            Value::parse_json(&text).map_err(|_| Error::Internal)
        })
        .collect()
}

/// pg encodes lone UTF-16 surrogates as U+FFFD when binding JS strings. JS-side
/// comparisons must instead retain the original Value::String code units.
pub(super) fn text(value: &Value, key: &str) -> Result<String, Error> {
    match value.get(key) {
        Some(Value::String(units)) => Ok(String::from_utf16_lossy(units)),
        _ => Err(Error::Internal),
    }
}
pub(super) fn number(value: &Value, key: &str) -> Result<f64, Error> {
    value
        .get(key)
        .and_then(Value::as_number)
        .ok_or(Error::Internal)
}

// Activation is selected per complete native slice after its paired benchmark.
// Unsupported/held endpoints enter the normal TS path before any native work.
pub(super) const ACTIVE_READ_ENDPOINTS: &[&str] = &[
    "emoji",
    "emojis",
    "hashtags/list",
    "hashtags/search",
    "hashtags/show",
    "username/available",
    "retention",
    "i/registry/get",
    "i/registry/get-all",
    "i/registry/get-detail",
    "i/registry/keys",
    "i/registry/keys-with-type",
    "i/registry/scopes-with-domain",
    "app/show",
    "my/apps",
    "i/apps",
    "i/authorized-apps",
    "roles/list",
    "roles/show",
    "admin/roles/list",
    "admin/roles/show",
    "admin/relays/list",
    "admin/get-index-stats",
    "admin/get-table-stats",
];

pub(super) async fn execute(
    runtime: &Runtime,
    name: &str,
    params: &Value,
    auth: &Context,
) -> Option<Result<Value, Error>> {
    if !ACTIVE_READ_ENDPOINTS.contains(&name) {
        return None;
    }
    if let Some(result) = discovery::execute(runtime, name, params, auth).await {
        return Some(result);
    }
    if let Some(result) = registry_apps::execute(runtime, name, params, auth).await {
        return Some(result);
    }
    roles_admin::execute(runtime, name, params, auth).await
}

#[cfg(test)]
mod query_error_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn business_id_method_is_normalized_like_the_existing_id_service() {
        for method in ["aid", "aidx", "meid", "meidg", "ulid", "objectid"] {
            let config = Config::new(&method.to_uppercase(), &serde_json::json!({}));
            assert_eq!(config.id_method, method);
        }
    }
}
