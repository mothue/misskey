//! Native API routing, access policy, rate limiting, validation and responses.
//! Unported business callbacks alone cross the temporary authenticated executor.
use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use misskey_api_validation::{Schema, Value};
use misskey_backend_api_auth::{
    AuthConfig, AuthError, AuthService, Context, Store, postgres::PostgresAuthStore,
};
use misskey_backend_core::{Catalog, Endpoint as CatalogEndpoint, Property, ping_at};
use misskey_backend_postgres::{Database, OnlineUsers, PostgresConfig, Target};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub mod redis;
pub mod request;
mod response;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug)]
pub struct Failure;
impl From<()> for Failure {
    fn from(_: ()) -> Self {
        Self
    }
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Rust backend unavailable")
    }
}
impl std::error::Error for Failure {}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiBootstrap {
    callback: CallbackConfig,
    endpoints: Vec<EndpointDefinition>,
    max_file_size: u64,
    enable_ip_rate_limit: bool,
    environment: String,
    upload_directory: String,
    host: String,
    postgres_primary: Option<Target>,
    redis: redis::RedisConfig,
    redis_pubsub: redis::RedisConfig,
    auth: AuthConfig,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallbackConfig {
    url: String,
    token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EndpointDefinition {
    name: String,
    meta: Json,
    params: Json,
}
struct Endpoint {
    name: String,
    meta: Json,
    schema: Schema,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestContext {
    v: u8,
    request_id: String,
    path: String,
    method: String,
    ip: String,
    headers: std::collections::HashMap<String, Json>,
}
impl RequestContext {
    fn header(&self, key: &str) -> Option<&str> {
        self.headers.get(key).and_then(Json::as_str)
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Invocation<'a> {
    v: u8,
    request_id: &'a str,
    kind: &'a str,
    endpoint: &'a str,
    params: &'a Value,
    user: &'a Option<Json>,
    token: &'a Option<Json>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<&'a request::Upload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    route_params: Option<Json>,
}
#[derive(Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum CallbackResult {
    #[serde(rename = "value")]
    Value { v: u8, value: Value },
    #[serde(rename = "json", rename_all = "camelCase")]
    Json { v: u8, result_json: Option<String> },
    #[serde(rename = "bytes")]
    Bytes { v: u8, base64: String },
    #[serde(rename = "api-error", rename_all = "camelCase")]
    ApiError {
        v: u8,
        message: String,
        code: String,
        id: String,
        error_kind: String,
        http_status_code: Option<u16>,
        info: Value,
    },
    #[serde(rename = "authentication-error")]
    Authentication { v: u8 },
    #[serde(rename = "framework-error")]
    FrameworkError {
        v: u8,
        status: u16,
        headers: std::collections::HashMap<String, String>,
        message: String,
        code: Option<String>,
    },
    #[serde(rename = "http", rename_all = "camelCase")]
    Http {
        v: u8,
        status: u16,
        headers: std::collections::HashMap<String, String>,
        body_json: Option<String>,
    },
}

pub struct Runtime {
    endpoints: Vec<Endpoint>,
    catalog: Catalog,
    callback: CallbackConfig,
    client: reqwest::Client,
    auth: Arc<AuthService<PostgresAuthStore>>,
    database: Arc<Database>,
    redis: redis::RedisState,
    online_users: Arc<OnlineUsers>,
    max_file_size: u64,
    upload_directory: String,
    enable_ip_rate_limit: bool,
    production: bool,
}
impl Runtime {
    pub async fn new(
        config: ApiBootstrap,
        reads: Option<PostgresConfig>,
        online_users: Arc<OnlineUsers>,
    ) -> Result<Self, Failure> {
        let callback_url = reqwest::Url::parse(&config.callback.url).map_err(|_| ())?;
        if callback_url.scheme() != "http"
            || callback_url.host_str() != Some("127.0.0.1")
            || callback_url.port().is_none()
            || callback_url.path() != "/execute"
            || !callback_url.username().is_empty()
            || callback_url.password().is_some()
            || callback_url.query().is_some()
            || callback_url.fragment().is_some()
            || config.callback.token.len() != 64
            || !config.callback.token.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(Failure);
        }
        let mut endpoints = Vec::new();
        let mut definitions = Vec::new();
        let mut names = std::collections::HashSet::new();
        for definition in config.endpoints {
            if !names.insert(definition.name.clone()) || !definition.meta.is_object() {
                return Err(Failure);
            }
            let properties = definition
                .params
                .get("properties")
                .and_then(Json::as_object)
                .map(|p| {
                    p.iter()
                        .map(|(key, value)| Property {
                            name: key.encode_utf16().collect(),
                            kind: value.get("type").and_then(Json::as_str).map(str::to_owned),
                        })
                        .collect()
                })
                .unwrap_or_default();
            definitions.push(CatalogEndpoint {
                name: definition.name.encode_utf16().collect(),
                properties,
            });
            endpoints.push(Endpoint {
                name: definition.name,
                meta: definition.meta,
                schema: Schema::compile(definition.params).map_err(|_| ())?,
            });
        }
        let database = Arc::new(Database::new(reads, config.postgres_primary));
        let store = Arc::new(
            PostgresAuthStore::new(database.clone(), config.auth.entity_layouts.clone())
                .map_err(|_| ())?,
        );
        let auth = Arc::new(AuthService::new(store, config.auth, now_ms()).map_err(|_| ())?);
        let redis =
            redis::RedisState::new(config.redis, config.redis_pubsub, config.host, auth.clone())
                .await
                .map_err(|_| ())?;
        Ok(Self {
            endpoints,
            catalog: Catalog::new(definitions),
            callback: config.callback,
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| ())?,
            auth,
            database,
            redis,
            online_users,
            max_file_size: config.max_file_size,
            upload_directory: config.upload_directory,
            enable_ip_rate_limit: config.enable_ip_rate_limit,
            production: config.environment == "production",
        })
    }
    pub async fn shutdown(&self) {
        self.redis.shutdown().await;
        self.database.shutdown().await;
    }

    pub async fn handle(&self, headers: &HeaderMap, body: Body) -> Response<Body> {
        let context = headers
            .get("x-misskey-api-context")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| STANDARD.decode(s).ok())
            .and_then(|b| serde_json::from_slice::<RequestContext>(&b).ok());
        let Some(context) = context.filter(|c| {
            c.v == 1
                && c.request_id.len() == 64
                && c.request_id.bytes().all(|b| b.is_ascii_hexdigit())
        }) else {
            return response::plain(400, None, Body::empty());
        };
        let mut reply = self.process(&context, body).await;
        if context.method == "HEAD" {
            *reply.body_mut() = Body::empty();
        }
        reply
    }
    async fn process(&self, context: &RequestContext, body: Body) -> Response<Body> {
        let path = context.path.split('?').next().unwrap_or("");
        let name = path.strip_prefix("/api/").unwrap_or("");
        let session = request::miauth_session_encoded(name);
        let name = request::decode_path(name);
        let endpoint = self.endpoints.iter().find(|e| e.name == name);
        let special = matches!(
            name.as_str(),
            "signup"
                | "signin-flow"
                | "signin-with-passkey"
                | "signup-pending"
                | "v1/instance/peers"
                | "clear-browser-cache"
        ) || session.is_some();
        let known = route_is_known(endpoint.is_some(), special, &name, &context.method);
        // The old wildcard is GET/HEAD only. Other unknown methods use Fastify's
        // global not-found response and do not inherit the /api CORS/cache hooks.
        if !known && !matches!(context.method.as_str(), "GET" | "HEAD" | "OPTIONS") {
            return unmatched_route(context, body, self.max_file_size, &self.upload_directory)
                .await;
        }
        if context.method == "OPTIONS" {
            return request::preflight(context);
        }
        if endpoint.is_some_and(|e| context.method == "GET" && !flag(&e.meta, "allowGet")) {
            let mut reply = response::plain(405, None, Body::empty());
            reply
                .headers_mut()
                .insert("access-control-allow-origin", HeaderValue::from_static("*"));
            reply.headers_mut().insert(
                "cache-control",
                HeaderValue::from_static("private, max-age=0, must-revalidate"),
            );
            return reply;
        }
        let file_required = endpoint.is_some_and(|e| flag(&e.meta, "requireFile"));
        let parsed = request::parse(
            context,
            body,
            file_required,
            self.max_file_size,
            &self.upload_directory,
        )
        .await;
        let mut reply = match parsed {
            Err(reply) => *reply,
            Ok(mut input) => {
                if let Some(endpoint) = endpoint {
                    if context.method == "GET" && !flag(&endpoint.meta, "allowGet") {
                        response::plain(405, None, Body::empty())
                    } else if file_required && input.upload.is_none() {
                        response::plain(400, None, Body::empty())
                    } else {
                        self.execute(context, endpoint, &mut input).await
                    }
                } else if name == "clear-browser-cache" {
                    let mut reply = response::plain(
                        if matches!(context.method.as_str(), "GET" | "POST") {
                            204
                        } else {
                            405
                        },
                        None,
                        Body::empty(),
                    );
                    if reply.status().as_u16() == 204 {
                        reply.headers_mut().insert("clear-site-data",HeaderValue::from_static("\"cache\", \"prefetchCache\", \"prerenderCache\", \"executionContexts\""));
                    }
                    reply
                } else if special && request::special_method(&name, &context.method) {
                    self.callback(Invocation {
                        v: 1,
                        request_id: &context.request_id,
                        kind: "special",
                        endpoint: &name,
                        params: &input.params,
                        user: &None,
                        token: &None,
                        file: None,
                        route_params: session.map(|session| json!({"session":session})),
                    })
                    .await
                } else if matches!(context.method.as_str(), "GET" | "HEAD") {
                    response::json(
                        404,
                        json!({"error":{"message":"Unknown API endpoint.","code":"UNKNOWN_API_ENDPOINT","id":"2ca3b769-540a-4f08-9dd5-b5a825b6d0f1","kind":"client"}}),
                    )
                } else {
                    response::json(
                        404,
                        json!({"message":format!("Route {}:{} not found",context.method,context.path),"error":"Not Found","statusCode":404}),
                    )
                }
            }
        };
        reply
            .headers_mut()
            .entry("access-control-allow-origin")
            .or_insert(HeaderValue::from_static("*"));
        reply
            .headers_mut()
            .entry("cache-control")
            .or_insert(HeaderValue::from_static(
                "private, max-age=0, must-revalidate",
            ));
        reply
    }
    async fn execute(
        &self,
        context: &RequestContext,
        endpoint: &Endpoint,
        input: &mut request::Input,
    ) -> Response<Body> {
        let token_value = match effective_token(context, &input.params) {
            Ok(token) => token,
            Err(reply) => return *reply,
        };
        let token = token_value.as_str();
        if matches!(&token_value, Value::String(_)) && token.is_none() {
            return response::authentication();
        }
        if requires_auth_cache(&token_value, &endpoint.meta) && !self.redis.ready() {
            return response::unavailable();
        }
        let auth = match self.auth.authenticate(token.as_deref(), now_ms()).await {
            Ok(auth) => auth,
            Err(AuthError::UserNotFound | AuthError::InvalidSignature) => {
                return response::authentication();
            }
            Err(AuthError::Storage) => return response::internal(),
        };
        if let Some(user) = &auth.user
            && let Some(user_id) = user.get("id").and_then(Json::as_str)
        {
            let auth = self.auth.clone();
            let user_id = user_id.to_owned();
            let ip = context.ip.clone();
            tokio::spawn(async move {
                let _ = auth.log_ip(&user_id, &ip, now_ms()).await;
            });
        }
        if let Err(reply) = self.authorize(context, endpoint, &auth).await {
            return *reply;
        }
        if (context.method == "GET" || flag(&endpoint.meta, "requireFile"))
            && let Err(error) = endpoint.schema.cast_query(&mut input.params)
        {
            return response::invalid(&error.param, &error.reason, true);
        }
        if let Err(error) = endpoint.schema.validate(&mut input.params) {
            return response::invalid(&error.schema_path, &error.message, false);
        }
        let mut reply = match endpoint.name.as_str() {
            "ping" => match ping_at(SystemTime::now()) {
                Ok(pong) => response::json(200, json!({"pong":pong})),
                Err(_) => response::internal(),
            },
            "test" => response::value(&input.params),
            "endpoint" => {
                let description = input
                    .params
                    .get("endpoint")
                    .and_then(|v| {
                        if let Value::String(name) = v {
                            Some(self.catalog.lookup(name))
                        } else {
                            None
                        }
                    })
                    .flatten();
                match description {
                    None => response::value(&Value::Null),
                    Some(params) => {
                        let params = Value::Array(
                            params
                                .into_iter()
                                .map(|(name, kind)| {
                                    Value::object([
                                        ("name", Value::String(name)),
                                        ("type", Value::string(&kind)),
                                    ])
                                })
                                .collect(),
                        );
                        response::value(&Value::object([("params", params)]))
                    }
                }
            }
            "endpoints" => response::value(&Value::Array(
                self.catalog
                    .names()
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            )),
            "get-online-users-count" => match self.online_users.count_at(SystemTime::now()).await {
                Ok(count) => response::value(&Value::object([("count", Value::Number(count))])),
                Err(_) => response::unavailable(),
            },
            _ => {
                self.callback(Invocation {
                    v: 1,
                    request_id: &context.request_id,
                    kind: "endpoint",
                    endpoint: &endpoint.name,
                    params: &input.params,
                    user: &auth.user,
                    token: &auth.token,
                    file: input.upload.as_ref(),
                    route_params: None,
                })
                .await
            }
        };
        if reply.status().is_success()
            && context.method == "GET"
            && !token_value.truthy()
            && auth.user.is_none()
            && let Some(seconds) = endpoint
                .meta
                .get("cacheSec")
                .and_then(Json::as_f64)
                .filter(|s| *s != 0.)
            && let Ok(value) = HeaderValue::from_str(&format!("public, max-age={seconds}"))
        {
            reply.headers_mut().insert("cache-control", value);
        }
        reply
    }
    async fn authorize(
        &self,
        context: &RequestContext,
        endpoint: &Endpoint,
        auth: &Context,
    ) -> Result<(), Box<Response<Body>>> {
        let meta = &endpoint.meta;
        let user = auth.user.as_ref();
        let token = auth.token.as_ref();
        let user_id = user.and_then(|u| u.get("id")).and_then(Json::as_str);
        let fail = |message: &str, code: &str, id: &str, kind: &str, status: Option<u16>| {
            Box::new(response::api_error(message, code, id, kind, status, None))
        };
        if flag(meta, "secure") && !(user.is_some() && token.is_none()) {
            return Err(fail(
                "Access denied.",
                "ACCESS_DENIED",
                "56f35758-7dd5-468b-8439-5d6fb8ec9b8e",
                "client",
                None,
            ));
        }
        if let Some(limit) = meta.get("limit") {
            let actor = if let Some(id) = user_id {
                Some(id.to_owned())
            } else if self.enable_ip_rate_limit {
                request::ip_hash(&context.ip)
            } else {
                None
            };
            let factor = if user_id.is_some() {
                self.auth
                    .policies_for(user_id, now_ms())
                    .await
                    .map_err(|_| response::internal())?
                    .numeric_aggregate("rateLimitFactor")
                    .unwrap_or(f64::NAN)
            } else {
                1.
            };
            if let Some(actor) = actor
                && factor > 0.
                && self.production
            {
                let mut limit = limit.clone();
                if limit.get("key").is_none_or(Json::is_null) {
                    limit["key"] = json!(endpoint.name);
                }
                if let Some(info) = self
                    .redis
                    .limit(&limit, &actor, factor)
                    .await
                    .map_err(|_| response::internal())?
                {
                    return Err(Box::new(response::api_error(
                        "Rate limit exceeded. Please try again later.",
                        "RATE_LIMIT_EXCEEDED",
                        "d5826d14-3982-4d2e-8011-b9e9f02499ef",
                        "client",
                        Some(429),
                        Some(&Value::from_json(&info)),
                    )));
                }
            }
        }
        let required = flag(meta, "requireCredential")
            || flag(meta, "requireModerator")
            || flag(meta, "requireAdmin");
        if required {
            if user.is_none() {
                return Err(fail(
                    "Credential required.",
                    "CREDENTIAL_REQUIRED",
                    "1384574d-a912-4b81-8601-c7b1c4085df1",
                    "client",
                    Some(401),
                ));
            }
            if user.is_some_and(|u| u.get("isSuspended").and_then(Json::as_bool) == Some(true)) {
                return Err(fail(
                    "Your account has been suspended.",
                    "YOUR_ACCOUNT_SUSPENDED",
                    "a8c724b3-6e9c-4b46-b1a8-bc3ed6258370",
                    "permission",
                    None,
                ));
            }
        }
        if flag(meta, "prohibitMoved")
            && user
                .and_then(|u| u.get("movedToUri"))
                .is_some_and(json_truthy)
        {
            return Err(fail(
                "You have moved your account.",
                "YOUR_ACCOUNT_MOVED",
                "56f20ec9-fd06-4fa5-841b-edd6d7d4fa31",
                "permission",
                None,
            ));
        }
        let root = self.auth.root_user_id().await;
        if (flag(meta, "requireModerator") || flag(meta, "requireAdmin"))
            && !is_root_user(user_id, root.as_deref())
        {
            let roles = self
                .auth
                .roles_for(user_id.ok_or_else(response::internal)?, now_ms())
                .await
                .map_err(|_| response::internal())?;
            if flag(meta, "requireModerator")
                && !roles
                    .iter()
                    .any(|r| flag(r, "isModerator") || flag(r, "isAdministrator"))
            {
                return Err(fail(
                    "You are not assigned to a moderator role.",
                    "ROLE_PERMISSION_DENIED",
                    "d33d5333-db36-423d-a8f9-1a2b9549da41",
                    "permission",
                    None,
                ));
            }
            if flag(meta, "requireAdmin") && !roles.iter().any(|r| flag(r, "isAdministrator")) {
                return Err(fail(
                    "You are not assigned to an administrator role.",
                    "ROLE_PERMISSION_DENIED",
                    "c3d38592-54c0-429d-be96-5636b0431a61",
                    "permission",
                    None,
                ));
            }
        }
        if let Some(policy) = meta.get("requiredRolePolicy").and_then(Json::as_str) {
            authorize_required_policy(&self.auth, user_id, root.as_deref(), policy).await?;
        }
        if let Some(token) = token {
            let kind = meta.get("kind").and_then(Json::as_str);
            if (kind.is_some()
                && !token
                    .get("permission")
                    .and_then(Json::as_array)
                    .is_some_and(|p| p.iter().any(|v| v.as_str() == kind)))
                || (kind.is_none() && required)
            {
                return Err(fail(
                    "Your app does not have the necessary permissions to use this endpoint.",
                    "PERMISSION_DENIED",
                    "1370e5b7-d4eb-4566-bb1d-7748ee6a1838",
                    "permission",
                    None,
                ));
            }
        }
        Ok(())
    }
    async fn callback(&self, frame: Invocation<'_>) -> Response<Body> {
        let result = async {
            let mut bytes = Vec::new();
            let mut serializer = serde_json::Serializer::new(&mut bytes);
            frame
                .serialize(serde_stacker::Serializer::new(&mut serializer))
                .map_err(|_| ())?;
            let response = self
                .client
                .post(&self.callback.url)
                .bearer_auth(&self.callback.token)
                .header("content-type", "application/json")
                .body(bytes)
                .send()
                .await
                .map_err(|_| ())?;
            if !response.status().is_success() {
                return Err(());
            }
            let bytes = response.bytes().await.map_err(|_| ())?;
            let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
            deserializer.disable_recursion_limit();
            let result =
                CallbackResult::deserialize(serde_stacker::Deserializer::new(&mut deserializer))
                    .map_err(|_| ())?;
            deserializer.end().map_err(|_| ())?;
            Ok(result)
        }
        .await;
        match result {
            Ok(CallbackResult::Value { v: 1, value }) => response::value(&value),
            Ok(CallbackResult::Json {
                v: 1,
                result_json: Some(text),
            }) => {
                if Value::parse_json(&text).is_ok() {
                    response::plain(200, Some("application/json; charset=utf-8"), text)
                } else {
                    response::internal()
                }
            }
            Ok(CallbackResult::Json {
                v: 1,
                result_json: None,
            }) => response::plain(200, Some("application/json; charset=utf-8"), Body::empty()),
            Ok(CallbackResult::Bytes { v: 1, base64 }) => match STANDARD.decode(base64) {
                Ok(bytes) => response::plain(200, Some("application/octet-stream"), bytes),
                Err(_) => response::internal(),
            },
            Ok(CallbackResult::ApiError {
                v: 1,
                message,
                code,
                id,
                error_kind,
                http_status_code,
                info,
            }) => response::api_error(
                &message,
                &code,
                &id,
                &error_kind,
                http_status_code,
                Some(&info),
            ),
            Ok(CallbackResult::Authentication { v: 1 }) => response::internal(),
            Ok(CallbackResult::FrameworkError {
                v: 1,
                status,
                headers,
                message,
                code,
            }) if (400..600).contains(&status) => {
                let mut value = serde_json::Map::new();
                value.insert("statusCode".into(), json!(status));
                if let Some(code) = code {
                    value.insert("code".into(), json!(code));
                }
                value.insert(
                    "error".into(),
                    json!(
                        axum::http::StatusCode::from_u16(status)
                            .ok()
                            .and_then(|v| v.canonical_reason())
                            .unwrap_or("Error")
                    ),
                );
                value.insert("message".into(), json!(message));
                apply_headers(response::json(status, Json::Object(value)), headers)
            }
            Ok(CallbackResult::Http {
                v: 1,
                status,
                headers,
                body_json,
            }) if (200..600).contains(&status) => {
                if headers
                    .get("content-type")
                    .is_some_and(|h| h.starts_with("application/json"))
                    && body_json
                        .as_ref()
                        .is_some_and(|s| Value::parse_json(s).is_err())
                {
                    return response::internal();
                }
                let mut reply = response::plain(
                    status,
                    body_json
                        .as_ref()
                        .map(|_| "application/json; charset=utf-8"),
                    body_json.unwrap_or_default(),
                );
                if matches!(status, 204 | 304) {
                    *reply.body_mut() = Body::empty();
                }
                apply_headers(reply, headers)
            }
            _ => response::internal(),
        }
    }
}
fn apply_headers(
    mut reply: Response<Body>,
    headers: std::collections::HashMap<String, String>,
) -> Response<Body> {
    for (key, value) in headers {
        let (Ok(key), Ok(value)) = (
            axum::http::HeaderName::try_from(key),
            HeaderValue::from_str(&value),
        ) else {
            return response::internal();
        };
        if matches!(
            key.as_str(),
            "connection" | "keep-alive" | "transfer-encoding" | "upgrade" | "content-length"
        ) {
            return response::internal();
        }
        reply.headers_mut().insert(key, value);
    }
    reply
}
fn flag(meta: &Json, key: &str) -> bool {
    meta.get(key).and_then(Json::as_bool) == Some(true)
}
fn json_truthy(value: &Json) -> bool {
    !value.is_null()
        && value != &Json::Bool(false)
        && value.as_f64() != Some(0.)
        && value.as_str() != Some("")
}

// Header precedence is applied before token-type validation, including an empty
// Bearer token. Invalid input completes before authentication or any callback.
fn effective_token(context: &RequestContext, params: &Value) -> Result<Value, Box<Response<Body>>> {
    let token = context
        .header("authorization")
        .and_then(|header| header.strip_prefix("Bearer "))
        .map(Value::string)
        .unwrap_or_else(|| params.get("i").cloned().unwrap_or(Value::Undefined));
    if matches!(&token, Value::Undefined | Value::Null | Value::String(_)) {
        Ok(token)
    } else {
        Err(Box::new(response::plain(400, None, Body::empty())))
    }
}

fn requires_auth_cache(token: &Value, meta: &Json) -> bool {
    // Anonymous policy checks also depend on Meta invalidations. Do not use a
    // stale permissive base policy while the direct Redis subscription is down.
    token.truthy()
        || meta
            .get("requiredRolePolicy")
            .and_then(Json::as_str)
            .is_some()
}

fn is_root_user(user_id: Option<&str>, root_id: Option<&str>) -> bool {
    user_id.is_some() && user_id == root_id
}

async fn authorize_required_policy<S: Store>(
    auth: &AuthService<S>,
    user_id: Option<&str>,
    root_id: Option<&str>,
    policy: &str,
) -> Result<(), Box<Response<Body>>> {
    if is_root_user(user_id, root_id) {
        return Ok(());
    }
    let roles = match user_id {
        Some(id) => auth
            .roles_for(id, now_ms())
            .await
            .map_err(|_| response::internal())?,
        None => Vec::new(),
    };
    let policies = auth
        .policies_for(user_id, now_ms())
        .await
        .map_err(|_| response::internal())?;
    if !policies.is_truthy(policy) && !roles.iter().any(|role| flag(role, "isAdministrator")) {
        return Err(Box::new(response::api_error(
            "You are not assigned to a required role.",
            "ROLE_PERMISSION_DENIED",
            "7f86f06f-7e15-4057-8561-f4b6d4ac755a",
            "permission",
            None,
            None,
        )));
    }
    Ok(())
}

#[cfg(test)]
mod compatibility_tests;

fn route_is_known(registry: bool, special: bool, name: &str, method: &str) -> bool {
    registry || name == "clear-browser-cache" || (special && request::special_method(name, method))
}

async fn unmatched_route(
    context: &RequestContext,
    body: Body,
    max_file_size: u64,
    upload_directory: &str,
) -> Response<Body> {
    // Fastify parses an unmatched non-GET request before its global 404 handler.
    // Neither parser failures nor that handler inherit the API response hooks.
    match request::parse(context, body, false, max_file_size, upload_directory).await {
        Err(reply) => *reply,
        Ok(_) => response::json(
            404,
            json!({"message":format!("Route {}:{} not found",context.method,context.path),"error":"Not Found","statusCode":404}),
        ),
    }
}
