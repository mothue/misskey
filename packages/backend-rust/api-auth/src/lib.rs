//! Authentication and role context for the native API execution kernel.
//!
//! The only legacy boundary is a complete endpoint callback. Authentication,
//! cache lookup/invalidation, role conditions and policy aggregation live here.
//! Never log entity snapshots or format storage errors: they can contain tokens.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{collections::HashMap, fmt, sync::Arc};
use tokio::sync::{Mutex, RwLock};

pub mod postgres;
mod role;
mod safe_json;
pub use role::{evaluate_condition, parse_id_date};
pub use safe_json::{SafeJson, clone_json, drop_json};

/// One completed policy evaluation. JSON is only a projection: JSON.stringify
/// turns nonfinite JS numbers into null, which cannot drive numeric decisions.
/// Keep every numeric aggregation in binary64 form from the same evaluation.
/// JSON access requires an explicit call to [`Self::as_json`].
///
/// ```compile_fail
/// use misskey_backend_api_auth::PolicySnapshot;
///
/// fn policy_factor(policies: &PolicySnapshot) -> Option<f64> {
///     policies["rateLimitFactor"].as_f64()
/// }
/// ```
#[derive(Clone)]
pub struct PolicySnapshot {
    json: SafeJson,
    numeric: HashMap<String, f64>,
}
impl PolicySnapshot {
    fn raw(json: SafeJson) -> Self {
        Self {
            json,
            numeric: HashMap::new(),
        }
    }

    pub(crate) fn aggregated(json: Value, numeric: HashMap<String, f64>) -> Self {
        Self {
            json: SafeJson::new(json),
            numeric,
        }
    }

    /// A JS Math.max/Math.min result before JSON projection. This preserves
    /// infinities, NaN and signed zero without inferring them back from null.
    /// The anonymous branch returns unaggregated base policies, so has none.
    pub fn numeric_aggregate(&self, name: &str) -> Option<f64> {
        self.numeric.get(name).copied()
    }

    pub fn is_truthy(&self, name: &str) -> bool {
        if let Some(number) = self.numeric_aggregate(name) {
            number != 0.0 && !number.is_nan()
        } else {
            self.json.get(name).is_some_and(role::truthy)
        }
    }

    /// The JSON projection for serialization and reference comparisons.
    /// Use [`Self::numeric_aggregate`] for numeric policy decisions: nonfinite
    /// aggregates appear here as null and cannot be recovered from JSON.
    pub fn as_json(&self) -> &Value {
        &self.json
    }

    pub fn into_json(self) -> Value {
        self.json.into_value()
    }
}

const USER_TTL: i64 = 5 * 60 * 1000;
const APP_TTL: i64 = 7 * 24 * 60 * 60 * 1000;
const ROLE_TTL: i64 = 60 * 60 * 1000;

/// Deliberately carries neither a SQL error nor the failed authentication value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure;
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Rust backend unavailable")
    }
}
impl std::error::Error for Failure {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthError {
    UserNotFound,
    InvalidSignature,
    Storage,
}
impl AuthError {
    pub fn authentication_message(self) -> Option<&'static str> {
        match self {
            Self::UserNotFound => Some("user not found"),
            Self::InvalidSignature => Some("invalid signature"),
            Self::Storage => None,
        }
    }
}
impl From<Failure> for AuthError {
    fn from(_: Failure) -> Self {
        Self::Storage
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthConfig {
    pub id_method: String,
    pub max_file_size: u64,
    pub default_policies: Value,
    pub meta: Value,
    pub entity_layouts: HashMap<String, Vec<EntityColumn>>,
}
impl Clone for AuthConfig {
    fn clone(&self) -> Self {
        Self {
            id_method: self.id_method.clone(),
            max_file_size: self.max_file_size,
            default_policies: clone_json(&self.default_policies),
            meta: clone_json(&self.meta),
            entity_layouts: self.entity_layouts.clone(),
        }
    }
}
impl Drop for AuthConfig {
    fn drop(&mut self) {
        drop_json(std::mem::replace(&mut self.default_policies, Value::Null));
        drop_json(std::mem::replace(&mut self.meta, Value::Null));
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntityColumn {
    pub property_name: String,
    pub database_name: String,
    pub kind: ColumnKind,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
pub enum ColumnKind {
    #[serde(rename = "date")]
    Date,
    #[serde(rename = "simple-array")]
    SimpleArray,
    #[serde(rename = "json")]
    Json,
}

/// Records contain selected columns only, in metadata order. Dates are epoch
/// milliseconds and simple-array columns are already split, for TS hydration.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    pub user: Option<Value>,
    pub token: Option<Value>,
    pub access_token_reduced: bool,
}
impl Clone for Context {
    fn clone(&self) -> Self {
        Self {
            user: self.user.as_ref().map(clone_json),
            token: self.token.as_ref().map(clone_json),
            access_token_reduced: self.access_token_reduced,
        }
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        if let Some(value) = self.user.take() {
            drop_json(value);
        }
        if let Some(value) = self.token.take() {
            drop_json(value);
        }
    }
}

#[async_trait]
pub trait Store: Send + Sync + 'static {
    async fn user_by_token(&self, token: &str) -> Result<Option<Value>, Failure>;
    async fn user_by_id(&self, user_id: &str) -> Result<Option<Value>, Failure>;
    async fn access_token_by_token(
        &self,
        lower: &str,
        exact: &str,
    ) -> Result<Option<Value>, Failure>;
    async fn update_last_used(&self, token_id: &str, now_ms: i64) -> Result<(), Failure>;
    async fn app_by_id(&self, app_id: &str) -> Result<Option<Value>, Failure>;
    async fn roles(&self) -> Result<Vec<Value>, Failure>;
    async fn meta(&self) -> Result<Value, Failure>;
    async fn assignments(&self, user_id: &str) -> Result<Vec<Value>, Failure>;
    async fn insert_ip(&self, now_ms: i64, user_id: &str, ip: &str) -> Result<(), Failure>;
}

struct Entry<T> {
    at: i64,
    value: T,
}

struct Cache<T> {
    ttl: i64,
    entries: HashMap<String, Entry<T>>,
}
impl<T: Clone> Cache<T> {
    fn new(ttl: i64) -> Self {
        Self {
            ttl,
            entries: HashMap::new(),
        }
    }
    fn get(&mut self, key: &str, now: i64) -> Option<T> {
        let entry = self.entries.get(key)?;
        // MemoryKVCache.get uses `>`; the exact expiry instant still hits.
        if now.saturating_sub(entry.at) > self.ttl {
            self.entries.remove(key);
            None
        } else {
            Some(entry.value.clone())
        }
    }
    fn set(&mut self, key: impl Into<String>, value: T, now: i64) {
        self.entries.insert(key.into(), Entry { at: now, value });
    }
    fn remove(&mut self, key: &str) {
        self.entries.remove(key);
    }
}

type CachedUser = Arc<std::sync::RwLock<SafeJson>>;

fn snapshot(value: Option<CachedUser>) -> Result<Option<Value>, Failure> {
    value
        .map(|value| value.read().map(|v| clone_json(&v)).map_err(|_| Failure))
        .transpose()
}

fn cached_user(value: Value) -> CachedUser {
    Arc::new(std::sync::RwLock::new(SafeJson::new(value)))
}

pub struct AuthService<S: Store> {
    store: Arc<S>,
    config: AuthConfig,
    meta: RwLock<SafeJson>,
    native_users: Mutex<Cache<Option<CachedUser>>>,
    local_users: Mutex<Cache<Option<CachedUser>>>,
    // RoleService uses CacheService.findUserById, a distinct cache from auth.
    users: Mutex<Cache<Option<CachedUser>>>,
    apps: Mutex<Cache<SafeJson>>,
    roles: Mutex<Option<Entry<Vec<SafeJson>>>>,
    assigns: Mutex<Cache<Vec<SafeJson>>>,
    ip_history: Mutex<IpHistory>,
}

struct IpHistory {
    users: HashMap<String, Vec<String>>,
}

impl<S: Store> AuthService<S> {
    pub fn new(store: Arc<S>, mut config: AuthConfig, _now_ms: i64) -> Result<Self, Failure> {
        config.id_method = config.id_method.to_lowercase();
        if !config.meta.is_object()
            || !config.default_policies.is_object()
            || !matches!(
                config.id_method.as_str(),
                "aid" | "aidx" | "meid" | "meidg" | "ulid" | "objectid"
            )
        {
            return Err(Failure);
        }
        let meta = SafeJson::new(clone_json(&config.meta));
        Ok(Self {
            store,
            config,
            meta: RwLock::new(meta),
            native_users: Mutex::new(Cache::new(USER_TTL)),
            local_users: Mutex::new(Cache::new(USER_TTL)),
            users: Mutex::new(Cache::new(USER_TTL)),
            apps: Mutex::new(Cache::new(APP_TTL)),
            roles: Mutex::new(None),
            assigns: Mutex::new(Cache::new(USER_TTL)),
            ip_history: Mutex::new(IpHistory {
                users: HashMap::new(),
            }),
        })
    }

    pub async fn meta(&self) -> SafeJson {
        self.meta.read().await.clone()
    }

    pub async fn root_user_id(&self) -> Option<String> {
        self.meta
            .read()
            .await
            .get("rootUserId")
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    /// Run on the same three-minute cadence as MemoryKVCache's collector.
    /// Authentication requests themselves do not extend a cached value's age.
    pub async fn collect_garbage(&self, now_ms: i64) {
        fn gc<T>(cache: &mut Cache<T>, now_ms: i64) {
            cache
                .entries
                .retain(|_, entry| now_ms.saturating_sub(entry.at) < cache.ttl);
        }
        gc(&mut *self.native_users.lock().await, now_ms);
        gc(&mut *self.local_users.lock().await, now_ms);
        gc(&mut *self.users.lock().await, now_ms);
        gc(&mut *self.apps.lock().await, now_ms);
        gc(&mut *self.assigns.lock().await, now_ms);
    }

    /// Called by an hourly monotonic timer, matching ApiCallService.setInterval.
    /// Clock adjustments must not move the IP-deduplication clear boundary.
    pub async fn clear_ip_history(&self) {
        self.ip_history.lock().await.users.clear();
    }

    /// A missed pubsub interval invalidates every auth/role snapshot. The
    /// caller holds API admission closed until subscription + refresh finish.
    pub async fn invalidate_all(&self) -> Result<(), Failure> {
        self.native_users.lock().await.entries.clear();
        self.local_users.lock().await.entries.clear();
        self.users.lock().await.entries.clear();
        self.apps.lock().await.entries.clear();
        self.assigns.lock().await.entries.clear();
        *self.roles.lock().await = None;
        *self.meta.write().await = SafeJson::new(self.store.meta().await?);
        Ok(())
    }

    pub async fn authenticate(
        &self,
        token: Option<&str>,
        now_ms: i64,
    ) -> Result<Context, AuthError> {
        let Some(token) = token else {
            return Ok(Context {
                user: None,
                token: None,
                access_token_reduced: false,
            });
        };
        // JS token.length counts UTF-16 code units, not UTF-8 bytes or scalars.
        if token.encode_utf16().count() == 16 {
            let cached = self.native_users.lock().await.get(token, now_ms);
            let user = if let Some(cached) = cached {
                cached
            } else {
                let user = self.store.user_by_token(token).await?.map(cached_user);
                self.native_users
                    .lock()
                    .await
                    .set(token, user.clone(), now_ms);
                user
            };
            return snapshot(user)?.map_or(Err(AuthError::UserNotFound), |user| {
                Ok(Context {
                    user: Some(user),
                    token: None,
                    access_token_reduced: false,
                })
            });
        }

        let access_token = self
            .store
            .access_token_by_token(&token.to_lowercase(), token)
            .await?
            .ok_or(AuthError::InvalidSignature)?;
        let token_id = string_field(&access_token, "id")?;
        let store = self.store.clone();
        // The current implementation deliberately does not await this update.
        // A storage failure must not turn a successful lookup into auth denial.
        tokio::spawn(async move {
            let _ = store.update_last_used(&token_id, now_ms).await;
        });
        let user_id = string_field(&access_token, "userId")?;
        let cached = self.local_users.lock().await.get(&user_id, now_ms);
        let user = if let Some(cached) = cached {
            cached
        } else {
            let user = self.store.user_by_id(&user_id).await?.map(cached_user);
            self.local_users
                .lock()
                .await
                .set(user_id, user.clone(), now_ms);
            user
        };
        let user = snapshot(user)?;
        // MiAccessToken's app branch returns exactly id + app permissions.
        if let Some(app_id) = access_token.get("appId").and_then(Value::as_str)
            && !app_id.is_empty()
        {
            let cached = self.apps.lock().await.get(app_id, now_ms);
            let app = if let Some(cached) = cached {
                cached
            } else {
                let app = SafeJson::new(self.store.app_by_id(app_id).await?.ok_or(Failure)?);
                self.apps.lock().await.set(app_id, app.clone(), now_ms);
                app
            };
            let permission = clone_json(app.get("permission").ok_or(Failure)?);
            return Ok(Context {
                user,
                token: Some(json!({ "id":access_token["id"], "permission":permission })),
                access_token_reduced: true,
            });
        }
        Ok(Context {
            user,
            token: Some(access_token),
            access_token_reduced: false,
        })
    }

    async fn all_roles(&self, now_ms: i64) -> Result<Vec<SafeJson>, Failure> {
        {
            let mut cached = self.roles.lock().await;
            if let Some(entry) = &*cached {
                if now_ms.saturating_sub(entry.at) <= ROLE_TTL {
                    return Ok(entry.value.clone());
                }
                *cached = None;
            }
        }
        let roles: Vec<_> = self
            .store
            .roles()
            .await?
            .into_iter()
            .map(SafeJson::new)
            .collect();
        *self.roles.lock().await = Some(Entry {
            at: now_ms,
            value: roles.clone(),
        });
        Ok(roles)
    }

    async fn active_assignments(
        &self,
        user_id: &str,
        now_ms: i64,
    ) -> Result<Vec<SafeJson>, Failure> {
        let cached = self.assigns.lock().await.get(user_id, now_ms);
        let assigns = if let Some(cached) = cached {
            cached
        } else {
            let assigns: Vec<_> = self
                .store
                .assignments(user_id)
                .await?
                .into_iter()
                .map(SafeJson::new)
                .collect();
            self.assigns
                .lock()
                .await
                .set(user_id, assigns.clone(), now_ms);
            assigns
        };
        Ok(assigns
            .into_iter()
            .filter(|a| {
                a.get("expiresAt").is_none_or(Value::is_null)
                    || a.get("expiresAt")
                        .and_then(Value::as_f64)
                        .is_some_and(|expires| expires > now_ms as f64)
            })
            .collect())
    }

    pub async fn roles_for(&self, user_id: &str, now_ms: i64) -> Result<Vec<SafeJson>, Failure> {
        let roles = self.all_roles(now_ms).await?;
        let assigns = self.active_assignments(user_id, now_ms).await?;
        let assigned: Vec<_> = roles
            .iter()
            .filter(|r| assigns.iter().any(|a| a.get("roleId") == r.get("id")))
            .cloned()
            .collect();
        let has_conditional = roles.iter().any(|r| r["target"] == "conditional");
        let user = if has_conditional {
            let cached = self.users.lock().await.get(user_id, now_ms);
            if let Some(cached) = cached {
                cached
            } else {
                let user = Some(cached_user(
                    self.store.user_by_id(user_id).await?.ok_or(Failure)?,
                ));
                self.users.lock().await.set(user_id, user.clone(), now_ms);
                user
            }
        } else {
            None
        };
        let user = snapshot(user)?.map(SafeJson::new);
        let mut result = assigned.clone();
        result.extend(roles.into_iter().filter(|r| {
            r["target"] == "conditional"
                && user.as_ref().is_some_and(|user| {
                    evaluate_condition(
                        user,
                        &assigned,
                        &r["condFormula"],
                        &self.config.id_method,
                        now_ms,
                    )
                })
        }));
        // An assigned conditional role is intentionally present twice, as in TS.
        Ok(result)
    }

    pub async fn policies_for(
        &self,
        user_id: Option<&str>,
        now_ms: i64,
    ) -> Result<PolicySnapshot, Failure> {
        let mut base = SafeJson::new(Value::Object(Map::new()));
        merge_object(
            base.as_object_mut().ok_or(Failure)?,
            self.config.default_policies.as_object().ok_or(Failure)?,
        );
        if let Some(overrides) = self
            .meta
            .read()
            .await
            .get("policies")
            .and_then(Value::as_object)
        {
            merge_object(base.as_object_mut().ok_or(Failure)?, overrides);
        }
        let Some(user_id) = user_id else {
            return Ok(PolicySnapshot::raw(base));
        };
        let roles = self.roles_for(user_id, now_ms).await?;
        role::aggregate_policies(
            base.as_object().ok_or(Failure)?,
            &roles,
            self.config.max_file_size,
        )
    }

    /// A direct Redis subscription calls this with the published JSON payload.
    /// This is event delivery, never a TS helper RPC or an auth fallback.
    pub async fn apply_event(&self, event: &Value, now_ms: i64) -> Result<(), Failure> {
        if event.get("channel").and_then(Value::as_str) != Some("internal") {
            return Ok(());
        }
        let message = &event["message"];
        let body = &message["body"];
        match message.get("type").and_then(Value::as_str) {
            Some("metaUpdated") => {
                if let Some(after) = body.get("after").and_then(Value::as_object) {
                    merge_object(
                        self.meta.write().await.as_object_mut().ok_or(Failure)?,
                        after,
                    );
                }
            }
            Some(
                "userChangeSuspendedState"
                | "userChangeDeletedState"
                | "remoteUserUpdated"
                | "localUserUpdated",
            ) => {
                let id = string_field(body, "id")?;
                let user = self.store.user_by_id(&id).await?;
                if let Some(user) = user {
                    let local_token = if user.get("host").is_none_or(Value::is_null) {
                        Some(string_field(&user, "token")?)
                    } else {
                        None
                    };
                    let user = cached_user(user);
                    self.users.lock().await.set(&id, Some(user.clone()), now_ms);
                    if let Some(token) = local_token {
                        self.native_users
                            .lock()
                            .await
                            .set(token, Some(user.clone()), now_ms);
                        self.local_users.lock().await.set(id, Some(user), now_ms);
                    }
                } else {
                    self.users.lock().await.remove(&id);
                    self.local_users.lock().await.remove(&id);
                    // TS does not delete native-token entries in this branch.
                }
            }
            Some("userTokenRegenerated") => {
                let user = self
                    .store
                    .user_by_id(&string_field(body, "id")?)
                    .await?
                    .ok_or(Failure)?;
                self.native_users
                    .lock()
                    .await
                    .remove(&string_field(body, "oldToken")?);
                self.native_users.lock().await.set(
                    string_field(body, "newToken")?,
                    Some(cached_user(user)),
                    now_ms,
                );
            }
            Some("follow") => {
                let mut users = self.users.lock().await;
                for (id_name, count_name) in [
                    ("followerId", "followingCount"),
                    ("followeeId", "followersCount"),
                ] {
                    if let Some(id) = body.get(id_name).and_then(Value::as_str)
                        && let Some(Some(user)) = users.get(id, now_ms)
                    {
                        let mut user = user.write().map_err(|_| Failure)?;
                        let count = user[count_name].as_f64().ok_or(Failure)? + 1.0;
                        user[count_name] = json!(count);
                        // Sharing after localUserUpdated mirrors TS references;
                        // direct object mutation does not refresh any TTL.
                    }
                }
            }
            Some("roleCreated") => {
                if let Some(cached) = self.roles.lock().await.as_mut()
                    && now_ms.saturating_sub(cached.at) <= ROLE_TTL
                {
                    cached
                        .value
                        .push(SafeJson::new(normalize_role_event(clone_json(body))));
                }
            }
            Some("roleUpdated") => {
                if let Some(cached) = self.roles.lock().await.as_mut()
                    && now_ms.saturating_sub(cached.at) <= ROLE_TTL
                    && let Some(role) = cached
                        .value
                        .iter_mut()
                        .find(|role| role.get("id") == body.get("id"))
                {
                    *role = SafeJson::new(normalize_role_event(clone_json(body)));
                }
            }
            Some("roleDeleted") => {
                if let Some(cached) = self.roles.lock().await.as_mut()
                    && now_ms.saturating_sub(cached.at) <= ROLE_TTL
                {
                    cached.value.retain(|role| role.get("id") != body.get("id"));
                    // RoleService.rolesCache.set refreshes TTL on deletion.
                    cached.at = now_ms;
                }
            }
            Some("userRoleAssigned") => {
                let id = string_field(body, "userId")?;
                let mut assigns = self.assigns.lock().await;
                if let Some(mut cached) = assigns.get(&id, now_ms) {
                    let mut assignment = clone_json(body);
                    if let Some(map) = assignment.as_object_mut() {
                        map.insert("user".into(), Value::Null);
                        map.insert("role".into(), Value::Null);
                        normalize_event_date(map, "expiresAt");
                    }
                    cached.push(SafeJson::new(assignment));
                    if let Some(entry) = assigns.entries.get_mut(&id) {
                        entry.value = cached;
                    }
                }
            }
            Some("userRoleUnassigned") => {
                let id = string_field(body, "userId")?;
                let mut assigns = self.assigns.lock().await;
                if let Some(mut cached) = assigns.get(&id, now_ms) {
                    cached.retain(|a| a.get("id") != body.get("id"));
                    assigns.set(id, cached, now_ms);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Called asynchronously after the API operation begins, as before.
    pub async fn log_ip(&self, user_id: &str, ip: &str, now_ms: i64) -> Result<(), Failure> {
        if !self.meta.read().await["enableIpLogging"]
            .as_bool()
            .unwrap_or(false)
        {
            return Ok(());
        }
        {
            let mut history = self.ip_history.lock().await;
            let ips = history.users.entry(user_id.to_owned()).or_default();
            if ips.iter().any(|old| old == ip) {
                return Ok(());
            }
            ips.push(ip.to_owned());
        }
        self.store.insert_ip(now_ms, user_id, ip).await
    }
}

fn string_field(value: &Value, field: &str) -> Result<String, Failure> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(Failure)
}

fn merge_object(target: &mut Map<String, Value>, source: &Map<String, Value>) {
    for (key, value) in source {
        if let Some(old) = target.insert(key.clone(), clone_json(value)) {
            drop_json(old);
        }
    }
}

fn normalize_role_event(mut role: Value) -> Value {
    if let Some(map) = role.as_object_mut() {
        normalize_event_date(map, "updatedAt");
        normalize_event_date(map, "lastUsedAt");
    }
    role
}

fn normalize_event_date(map: &mut Map<String, Value>, name: &str) {
    if let Some(Value::String(text)) = map.get(name) {
        // ISO date serialization is produced by GlobalEventService. Its parser
        // is shared with the role expiration code rather than a JS runtime.
        let millis = role::parse_iso_date(text);
        map.insert(name.into(), millis.map_or(Value::Null, |ms| json!(ms)));
    }
}

#[cfg(test)]
mod tests;
