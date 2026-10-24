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

mod meta;
pub mod redis;
pub mod request;
mod response;
use meta::EndpointMeta;
use response::ApiFailure;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureStage {
    CallbackConfig,
    EndpointRegistry,
    EndpointMetadata,
    EndpointSchema,
    AuthStore,
    AuthService,
    Redis,
    HttpClient,
    InvocationEncode,
    CallbackTransport,
    CallbackStatus,
    CallbackBody,
    CallbackDecode,
    CallbackResult,
    CallbackHeaders,
    Authentication,
    IpLog,
    RateLimitPolicy,
    RateLimit,
    RoleLookup,
    RequiredPolicy,
}

/// Retain a useful failure location without retaining credentials, request data
/// or transport errors that may embed the authenticated callback URL.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Failure {
    stage: FailureStage,
}
impl Failure {
    pub const fn new(stage: FailureStage) -> Self {
        Self { stage }
    }

    pub const fn stage(&self) -> FailureStage {
        self.stage
    }
}
impl std::fmt::Debug for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Failure")
            .field("stage", &self.stage)
            .finish()
    }
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rust backend unavailable at {:?}", self.stage)
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
    meta: EndpointMeta,
    schema: Schema,
}

struct Route<'a> {
    name: String,
    endpoint: Option<&'a Endpoint>,
    special: bool,
    session: Option<String>,
}
impl Route<'_> {
    fn file_required(&self) -> bool {
        self.endpoint
            .is_some_and(|endpoint| endpoint.meta.require_file)
    }

    fn is_known(&self, method: &str) -> bool {
        route_is_known(self.endpoint.is_some(), self.special, &self.name, method)
    }
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
        let callback_url = reqwest::Url::parse(&config.callback.url)
            .map_err(|_| Failure::new(FailureStage::CallbackConfig))?;
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
            return Err(Failure::new(FailureStage::CallbackConfig));
        }
        let mut endpoints = Vec::new();
        let mut definitions = Vec::new();
        let mut names = std::collections::HashSet::new();
        for definition in config.endpoints {
            if !names.insert(definition.name.clone()) || !definition.meta.is_object() {
                return Err(Failure::new(FailureStage::EndpointRegistry));
            }
            let meta = serde_json::from_value(definition.meta)
                .map_err(|_| Failure::new(FailureStage::EndpointMetadata))?;
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
                meta,
                schema: Schema::compile(definition.params)
                    .map_err(|_| Failure::new(FailureStage::EndpointSchema))?,
            });
        }
        let database = Arc::new(Database::new(reads, config.postgres_primary));
        let store = Arc::new(
            PostgresAuthStore::new(database.clone(), config.auth.entity_layouts.clone())
                .map_err(|_| Failure::new(FailureStage::AuthStore))?,
        );
        let auth = Arc::new(
            AuthService::new(store, config.auth, now_ms())
                .map_err(|_| Failure::new(FailureStage::AuthService))?,
        );
        let redis =
            redis::RedisState::new(config.redis, config.redis_pubsub, config.host, auth.clone())
                .await
                .map_err(|_| Failure::new(FailureStage::Redis))?;
        Ok(Self {
            endpoints,
            catalog: Catalog::new(definitions),
            callback: config.callback,
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| Failure::new(FailureStage::HttpClient))?,
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
        let route = self.resolve_route(context);
        // The old wildcard is GET/HEAD only. Other unknown methods use Fastify's
        // global not-found response and do not inherit the /api CORS/cache hooks.
        if !route.is_known(&context.method)
            && !matches!(context.method.as_str(), "GET" | "HEAD" | "OPTIONS")
        {
            return unmatched_route(context, body, self.max_file_size, &self.upload_directory)
                .await;
        }
        if context.method == "OPTIONS" {
            return request::preflight(context);
        }
        if route
            .endpoint
            .is_some_and(|endpoint| context.method == "GET" && !endpoint.meta.allow_get)
        {
            return api_response_headers(response::plain(405, None, Body::empty()));
        }
        let reply = match request::parse(
            context,
            body,
            route.file_required(),
            self.max_file_size,
            &self.upload_directory,
        )
        .await
        {
            Err(reply) => *reply,
            Ok(mut input) => self.dispatch_route(context, route, &mut input).await,
        };
        api_response_headers(reply)
    }

    fn resolve_route(&self, context: &RequestContext) -> Route<'_> {
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
        Route {
            name,
            endpoint,
            special,
            session,
        }
    }

    async fn dispatch_route(
        &self,
        context: &RequestContext,
        route: Route<'_>,
        input: &mut request::Input,
    ) -> Response<Body> {
        if let Some(endpoint) = route.endpoint {
            return if route.file_required() && input.upload.is_none() {
                response::plain(400, None, Body::empty())
            } else {
                self.execute(context, endpoint, input).await
            };
        }
        if route.name == "clear-browser-cache" {
            return clear_browser_cache(&context.method);
        }
        if route.special && request::special_method(&route.name, &context.method) {
            return self
                .callback(Invocation {
                    v: 1,
                    request_id: &context.request_id,
                    kind: "special",
                    endpoint: &route.name,
                    params: &input.params,
                    user: &None,
                    token: &None,
                    file: None,
                    route_params: route.session.map(|session| json!({"session":session})),
                })
                .await;
        }
        if matches!(context.method.as_str(), "GET" | "HEAD") {
            response::fixed(ApiFailure::UnknownEndpoint, None)
        } else {
            response::not_found(&context.method, &context.path)
        }
    }
    async fn execute(
        &self,
        context: &RequestContext,
        endpoint: &Endpoint,
        input: &mut request::Input,
    ) -> Response<Body> {
        let (token_value, auth) = match self
            .authenticate_request(context, endpoint, &input.params)
            .await
        {
            Ok(auth) => auth,
            Err(reply) => return *reply,
        };
        if let Err(reply) = self.authorize(context, endpoint, &auth).await {
            return *reply;
        }
        if let Err(reply) = validate_input(context, endpoint, input) {
            return *reply;
        }
        let mut reply = self
            .dispatch_endpoint(context, endpoint, input, &auth)
            .await;
        cache_public_response(&mut reply, context, endpoint, &token_value, &auth);
        reply
    }

    async fn authenticate_request(
        &self,
        context: &RequestContext,
        endpoint: &Endpoint,
        params: &Value,
    ) -> Result<(Value, Context), Box<Response<Body>>> {
        let token_value = effective_token(context, params)?;
        let token = token_value.to_utf8();
        if matches!(&token_value, Value::String(_)) && token.is_none() {
            return Err(Box::new(response::authentication()));
        }
        if requires_auth_cache(&token_value, &endpoint.meta) && !self.redis.ready() {
            return Err(Box::new(response::unavailable()));
        }
        let auth = match self.auth.authenticate(token.as_deref(), now_ms()).await {
            Ok(auth) => auth,
            Err(AuthError::UserNotFound | AuthError::InvalidSignature) => {
                return Err(Box::new(response::authentication()));
            }
            Err(AuthError::Storage) => {
                return Err(Box::new(failure_response(Failure::new(
                    FailureStage::Authentication,
                ))));
            }
        };
        self.record_ip(context, &auth);
        Ok((token_value, auth))
    }

    fn record_ip(&self, context: &RequestContext, auth: &Context) {
        if let Some(user) = &auth.user
            && let Some(user_id) = user.get("id").and_then(Json::as_str)
        {
            let auth = self.auth.clone();
            let user_id = user_id.to_owned();
            let ip = context.ip.clone();
            tokio::spawn(async move {
                if auth.log_ip(&user_id, &ip, now_ms()).await.is_err() {
                    eprintln!("Native API failure: {}", Failure::new(FailureStage::IpLog));
                }
            });
        }
    }

    async fn dispatch_endpoint(
        &self,
        context: &RequestContext,
        endpoint: &Endpoint,
        input: &mut request::Input,
        auth: &Context,
    ) -> Response<Body> {
        match endpoint.name.as_str() {
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
        }
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
        let fail = |error| Box::new(response::fixed(error, None));
        if meta.secure && !(user.is_some() && token.is_none()) {
            return Err(fail(ApiFailure::AccessDenied));
        }
        self.enforce_rate_limit(context, endpoint, user_id).await?;
        let required = meta.credential_required();
        if required {
            if user.is_none() {
                return Err(fail(ApiFailure::CredentialRequired));
            }
            if user.is_some_and(|u| u.get("isSuspended").and_then(Json::as_bool) == Some(true)) {
                return Err(fail(ApiFailure::AccountSuspended));
            }
        }
        if meta.prohibit_moved
            && user
                .and_then(|u| u.get("movedToUri"))
                .is_some_and(json_truthy)
        {
            return Err(fail(ApiFailure::AccountMoved));
        }
        let root = self.auth.root_user_id().await;
        self.enforce_role_requirements(meta, user_id, root.as_deref())
            .await?;
        if let Some(policy) = meta.required_role_policy.as_deref() {
            authorize_required_policy(&self.auth, user_id, root.as_deref(), policy).await?;
        }
        if let Some(token) = token {
            let kind = meta.kind.as_deref();
            if (kind.is_some()
                && !token
                    .get("permission")
                    .and_then(Json::as_array)
                    .is_some_and(|p| p.iter().any(|v| v.as_str() == kind)))
                || (kind.is_none() && required)
            {
                return Err(fail(ApiFailure::AppPermissionDenied));
            }
        }
        Ok(())
    }

    async fn enforce_rate_limit(
        &self,
        context: &RequestContext,
        endpoint: &Endpoint,
        user_id: Option<&str>,
    ) -> Result<(), Box<Response<Body>>> {
        if let Some(limit) = &endpoint.meta.limit {
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
                    .map_err(|_| failure_response(Failure::new(FailureStage::RateLimitPolicy)))?
                    .numeric_aggregate("rateLimitFactor")
                    .unwrap_or(f64::NAN)
            } else {
                1.
            };
            if let Some(actor) = actor
                && factor > 0.
                && self.production
                && let Some(info) = self
                    .redis
                    .limit(limit, &endpoint.name, &actor, factor)
                    .await
                    .map_err(|_| failure_response(Failure::new(FailureStage::RateLimit)))?
            {
                return Err(Box::new(response::fixed(
                    ApiFailure::RateLimitExceeded,
                    Some(&Value::from_json(&info.to_json())),
                )));
            }
        }
        Ok(())
    }

    async fn enforce_role_requirements(
        &self,
        meta: &EndpointMeta,
        user_id: Option<&str>,
        root: Option<&str>,
    ) -> Result<(), Box<Response<Body>>> {
        if (meta.require_moderator || meta.require_admin) && !is_root_user(user_id, root) {
            let roles = self
                .auth
                .roles_for(
                    user_id
                        .ok_or_else(|| failure_response(Failure::new(FailureStage::RoleLookup)))?,
                    now_ms(),
                )
                .await
                .map_err(|_| failure_response(Failure::new(FailureStage::RoleLookup)))?;
            if meta.require_moderator
                && !roles
                    .iter()
                    .any(|r| flag(r, "isModerator") || flag(r, "isAdministrator"))
            {
                return Err(Box::new(response::fixed(
                    ApiFailure::ModeratorRequired,
                    None,
                )));
            }
            if meta.require_admin && !roles.iter().any(|r| flag(r, "isAdministrator")) {
                return Err(Box::new(response::fixed(
                    ApiFailure::AdministratorRequired,
                    None,
                )));
            }
        }
        Ok(())
    }
    async fn invoke_callback(&self, frame: Invocation<'_>) -> Result<CallbackResult, Failure> {
        let mut bytes = Vec::new();
        let mut serializer = serde_json::Serializer::new(&mut bytes);
        frame
            .serialize(serde_stacker::Serializer::new(&mut serializer))
            .map_err(|_| Failure::new(FailureStage::InvocationEncode))?;
        let response = self
            .client
            .post(&self.callback.url)
            .bearer_auth(&self.callback.token)
            .header("content-type", "application/json")
            .body(bytes)
            .send()
            .await
            .map_err(|_| Failure::new(FailureStage::CallbackTransport))?;
        if !response.status().is_success() {
            return Err(Failure::new(FailureStage::CallbackStatus));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| Failure::new(FailureStage::CallbackBody))?;
        let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
        deserializer.disable_recursion_limit();
        let result =
            CallbackResult::deserialize(serde_stacker::Deserializer::new(&mut deserializer))
                .map_err(|_| Failure::new(FailureStage::CallbackDecode))?;
        deserializer
            .end()
            .map_err(|_| Failure::new(FailureStage::CallbackDecode))?;
        Ok(result)
    }

    async fn callback(&self, frame: Invocation<'_>) -> Response<Body> {
        match self.invoke_callback(frame).await {
            Ok(CallbackResult::Value { v: 1, value }) => response::value(&value),
            Ok(CallbackResult::Json {
                v: 1,
                result_json: Some(text),
            }) => {
                if Value::parse_json(&text).is_ok() {
                    response::plain(200, Some("application/json; charset=utf-8"), text)
                } else {
                    failure_response(Failure::new(FailureStage::CallbackResult))
                }
            }
            Ok(CallbackResult::Json {
                v: 1,
                result_json: None,
            }) => response::plain(200, Some("application/json; charset=utf-8"), Body::empty()),
            Ok(CallbackResult::Bytes { v: 1, base64 }) => match STANDARD.decode(base64) {
                Ok(bytes) => response::plain(200, Some("application/octet-stream"), bytes),
                Err(_) => failure_response(Failure::new(FailureStage::CallbackResult)),
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
            Ok(CallbackResult::Authentication { v: 1 }) => {
                failure_response(Failure::new(FailureStage::CallbackResult))
            }
            Ok(CallbackResult::FrameworkError {
                v: 1,
                status,
                headers,
                message,
                code,
            }) if (400..600).contains(&status) => apply_headers(
                response::framework_with_code(status, code.as_deref(), &message, "Error"),
                headers,
            ),
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
                    return failure_response(Failure::new(FailureStage::CallbackResult));
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
            Err(failure) => failure_response(failure),
            _ => failure_response(Failure::new(FailureStage::CallbackResult)),
        }
    }
}
fn failure_response(failure: Failure) -> Response<Body> {
    eprintln!("Native API failure: {failure}");
    response::internal()
}

fn validate_input(
    context: &RequestContext,
    endpoint: &Endpoint,
    input: &mut request::Input,
) -> Result<(), Box<Response<Body>>> {
    if (context.method == "GET" || endpoint.meta.require_file)
        && let Err(error) = endpoint.schema.cast_query(&mut input.params)
    {
        return Err(Box::new(response::invalid(
            &error.param,
            &error.reason,
            true,
        )));
    }
    endpoint
        .schema
        .validate(&mut input.params)
        .map_err(|error| Box::new(response::invalid(&error.schema_path, &error.message, false)))
}

fn cache_public_response(
    reply: &mut Response<Body>,
    context: &RequestContext,
    endpoint: &Endpoint,
    token: &Value,
    auth: &Context,
) {
    if reply.status().is_success()
        && context.method == "GET"
        && !token.truthy()
        && auth.user.is_none()
        && let Some(seconds) = endpoint.meta.cache_sec.filter(|seconds| *seconds != 0.)
        && let Ok(value) = HeaderValue::from_str(&format!("public, max-age={seconds}"))
    {
        reply.headers_mut().insert("cache-control", value);
    }
}

fn api_response_headers(mut reply: Response<Body>) -> Response<Body> {
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

fn clear_browser_cache(method: &str) -> Response<Body> {
    let allowed = matches!(method, "GET" | "POST");
    let mut reply = response::plain(if allowed { 204 } else { 405 }, None, Body::empty());
    if allowed {
        reply.headers_mut().insert(
            "clear-site-data",
            HeaderValue::from_static(
                "\"cache\", \"prefetchCache\", \"prerenderCache\", \"executionContexts\"",
            ),
        );
    }
    reply
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
            return failure_response(Failure::new(FailureStage::CallbackHeaders));
        };
        if matches!(
            key.as_str(),
            "connection" | "keep-alive" | "transfer-encoding" | "upgrade" | "content-length"
        ) {
            return failure_response(Failure::new(FailureStage::CallbackHeaders));
        }
        reply.headers_mut().insert(key, value);
    }
    reply
}
fn flag(record: &Json, key: &str) -> bool {
    record.get(key).and_then(Json::as_bool) == Some(true)
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

fn requires_auth_cache(token: &Value, meta: &EndpointMeta) -> bool {
    // Anonymous policy checks also depend on Meta invalidations. Do not use a
    // stale permissive base policy while the direct Redis subscription is down.
    token.truthy() || meta.required_role_policy.is_some()
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
            .map_err(|_| failure_response(Failure::new(FailureStage::RoleLookup)))?,
        None => Vec::new(),
    };
    let policies = auth
        .policies_for(user_id, now_ms())
        .await
        .map_err(|_| failure_response(Failure::new(FailureStage::RequiredPolicy)))?;
    if !policies.is_truthy(policy) && !roles.iter().any(|role| flag(role, "isAdministrator")) {
        return Err(Box::new(response::fixed(ApiFailure::RequiredRole, None)));
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
        Ok(_) => response::not_found(&context.method, &context.path),
    }
}
