//! Direct Redis ownership for auth invalidations and the existing limiter.
//! Responses retain the legacy MULTI command errors: fractional rank/TTL
//! arguments fail per command while the other commands still execute.
use crate::meta::RateLimit;
use futures_util::{FutureExt, StreamExt};
use misskey_api_validation::Value as JsValue;
use misskey_backend_api_auth::{AuthService, SafeJson, Store, postgres::PostgresAuthStore};
use misskey_backend_postgres::Database;
use redis::{
    Client, ConnectionAddr, ConnectionInfo, ProtocolVersion, RedisConnectionInfo,
    Value as RedisValue,
    aio::{ConnectionLike, MultiplexedConnection, PubSub},
};
use serde::Deserialize;
use serde_json::{Value, json, value::RawValue};
use std::collections::{HashMap, HashSet};
use std::{
    fmt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    sync::{Mutex, watch},
    task::JoinHandle,
};

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RedisConfig {
    pub host: String,
    pub port: u16,
    pub path: Option<String>,
    pub db: i64,
    pub username: Option<String>,
    pub password: Option<String>,
    pub key_prefix: String,
    pub connect_timeout_ms: u64,
    pub tls: Option<RedisTls>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RedisTls {
    pub ca: Vec<String>,
    pub cert: Option<String>,
    pub key: Option<String>,
    pub insecure: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailureStage {
    Configuration,
    Client,
    SubscriptionConnect,
    SubscriptionSubscribe,
    SubscriptionTimeout,
    SubscriptionClosed,
    AuthRefresh,
    EventDecode,
    EventEnvelope,
    EventApply,
    Unavailable,
    ConnectionTimeout,
    Connection,
    Command,
    CommandResponse,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Failure {
    stage: FailureStage,
}
impl Failure {
    const fn new(stage: FailureStage) -> Self {
        Self { stage }
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Rust Redis unavailable at {:?}", self.stage)
    }
}
impl std::error::Error for Failure {}

fn report(failure: &Failure) {
    eprintln!("Native API Redis failure: {failure}");
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LimitInfo {
    remaining: f64,
    reset: f64,
    reset_ms: f64,
    total: f64,
}
impl LimitInfo {
    pub(crate) fn to_json(self) -> Value {
        // Keep the Node limiter's JSON projection: nonfinite values become
        // null, including resetMs (which must then omit Retry-After).
        json!({
            "remaining": self.remaining,
            "reset": self.reset,
            "resetMs": self.reset_ms,
            "total": self.total,
        })
    }
}

pub struct RedisState {
    client: Client,
    config: RedisConfig,
    connection: Mutex<Option<MultiplexedConnection>>,
    ready: Arc<AtomicBool>,
    stopped: AtomicBool,
    stop: watch::Sender<bool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    clock: Clock,
    reads: Arc<RedisReads>,
    reactions: Arc<RedisReads>,
}

/// A decoded reaction buffer retains Redis's entry order until JS-compatible
/// object construction orders canonical integer property names.
pub(crate) struct ReactionBuffer {
    pub deltas: JsValue,
    pub pairs: Vec<(String, String)>,
}

struct CacheEntry {
    stored: Instant,
    lifetime: Duration,
    value: JsValue,
}

#[derive(Default)]
struct ReadCache {
    generation: u64,
    entries: HashMap<String, CacheEntry>,
}
impl ReadCache {
    fn invalidate(&mut self, key: Option<&str>) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(key) = key {
            self.entries.remove(key);
        } else {
            self.entries.clear();
        }
    }
    fn remember(&mut self, key: &str, value: &JsValue, lifetime: Duration) {
        self.entries
            .retain(|_, entry| entry.stored.elapsed() <= entry.lifetime);
        self.entries.insert(
            key.to_owned(),
            CacheEntry {
                stored: Instant::now(),
                lifetime,
                value: value.clone(),
            },
        );
    }
}

/// Lazy native data connection, separate from subscription and limiter state.
/// The reactions lane uses redisForReactions, including its own DB and prefix.
struct RedisReads {
    client: Client,
    config: RedisConfig,
    connection: Mutex<Option<MultiplexedConnection>>,
    stopped: AtomicBool,
    stop: watch::Sender<bool>,
    cache: Mutex<ReadCache>,
}

#[derive(Clone, Copy)]
enum ReplyErrors {
    Extract,
    PerCommand,
}

/// The legacy callers keep their existing stage projection. New operations
/// retain concrete causes internally without exposing Redis details in logs.
#[derive(Debug)]
pub(crate) enum ReadExecutionFailure {
    Shutdown,
    ConnectionTimeout(tokio::time::error::Elapsed),
    Connection(redis::RedisError),
    MissingConnection,
    Command(redis::RedisError),
    UnexpectedTransaction,
}

impl ReadExecutionFailure {
    fn legacy_failure(&self) -> Failure {
        Failure::new(match self {
            Self::Shutdown => FailureStage::Shutdown,
            Self::ConnectionTimeout(_) => FailureStage::ConnectionTimeout,
            Self::Connection(_) | Self::MissingConnection => FailureStage::Connection,
            Self::Command(_) | Self::UnexpectedTransaction => FailureStage::Command,
        })
    }

    fn safe_kind(&self) -> &'static str {
        match self {
            Self::Shutdown => "redis_shutdown",
            Self::ConnectionTimeout(_) => "redis_connection_timeout",
            Self::Connection(_) => "redis_connection",
            Self::MissingConnection => "redis_connection_invariant",
            Self::Command(_) => "redis_command",
            Self::UnexpectedTransaction => "redis_unexpected_transaction",
        }
    }
}

impl fmt::Display for ReadExecutionFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.safe_kind())
    }
}

impl std::error::Error for ReadExecutionFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ConnectionTimeout(source) => Some(source),
            Self::Connection(source) | Self::Command(source) => Some(source),
            Self::Shutdown | Self::MissingConnection | Self::UnexpectedTransaction => None,
        }
    }
}

impl RedisReads {
    fn new(config: RedisConfig) -> Result<Self, Failure> {
        let (stop, _) = watch::channel(false);
        Ok(Self {
            client: client(&config)?,
            config,
            connection: Mutex::new(None),
            stopped: AtomicBool::new(false),
            stop,
            cache: Mutex::new(ReadCache::default()),
        })
    }

    async fn execute(&self, pipeline: &redis::Pipeline) -> Result<Vec<RedisValue>, Failure> {
        self.execute_with(pipeline, ReplyErrors::Extract)
            .await
            .map_err(|source| source.legacy_failure())
    }

    async fn execute_per_command(
        &self,
        pipeline: &redis::Pipeline,
    ) -> Result<Vec<RedisValue>, ReadExecutionFailure> {
        if pipeline.is_transaction() {
            return Err(ReadExecutionFailure::UnexpectedTransaction);
        }
        self.execute_with(pipeline, ReplyErrors::PerCommand).await
    }

    async fn execute_with(
        &self,
        pipeline: &redis::Pipeline,
        reply_errors: ReplyErrors,
    ) -> Result<Vec<RedisValue>, ReadExecutionFailure> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(ReadExecutionFailure::Shutdown);
        }
        // No command means no reply to wake the raw pipeline collector.
        if pipeline.is_empty() && matches!(reply_errors, ReplyErrors::PerCommand) {
            return Ok(Vec::new());
        }
        let mut stop = self.stop.subscribe();
        let operation = async {
            let mut connection = self.connection.lock().await;
            if connection.is_none() {
                *connection = Some(
                    tokio::time::timeout(
                        connect_deadline(&self.config),
                        self.client.get_multiplexed_async_connection(),
                    )
                    .await
                    .map_err(ReadExecutionFailure::ConnectionTimeout)?
                    .map_err(ReadExecutionFailure::Connection)?,
                );
            }
            let live = connection
                .as_mut()
                .ok_or(ReadExecutionFailure::MissingConnection)?;
            let result = match reply_errors {
                ReplyErrors::Extract => pipeline.query_async(live).await,
                // ioredis pipeline.exec keeps [error] alongside successful
                // siblings. query_async would extract ANY ServerError and
                // reject the whole pipeline before the business decoder.
                ReplyErrors::PerCommand => {
                    live.req_packed_commands(pipeline, 0, pipeline.len()).await
                }
            }
            .map_err(ReadExecutionFailure::Command);
            if result.is_err() {
                *connection = None;
            }
            result
        };
        tokio::select! {
            biased;
            _ = stop.changed() => Err(ReadExecutionFailure::Shutdown),
            result = operation => result,
        }
    }

    fn key(&self, name: &str) -> String {
        format!("{}{name}", self.config.key_prefix)
    }

    async fn get_cache(
        &self,
        key: &str,
        lifetime: Duration,
    ) -> Result<(u64, Option<JsValue>), Failure> {
        loop {
            let generation = {
                let mut cache = self.cache.lock().await;
                if let Some(entry) = cache.entries.get(key) {
                    if entry.stored.elapsed() <= lifetime {
                        return Ok((cache.generation, Some(entry.value.clone())));
                    }
                    cache.entries.remove(key);
                }
                cache.generation
            };
            let mut pipeline = redis::pipe();
            pipeline.cmd("GET").arg(self.key(key));
            let response = self.execute(&pipeline).await?;
            let text: Option<String> = redis::from_redis_value(
                response
                    .first()
                    .ok_or_else(|| Failure::new(FailureStage::CommandResponse))?,
            )
            .map_err(|_| Failure::new(FailureStage::CommandResponse))?;
            let value = text
                .map(|text| {
                    JsValue::parse_json(&text)
                        .map_err(|_| Failure::new(FailureStage::CommandResponse))
                })
                .transpose()?;
            let mut cache = self.cache.lock().await;
            // A follow/unfollow may have arrived while GET was in flight.
            // Neither publish nor return the stale value after that event.
            if cache.generation != generation {
                continue;
            }
            if let Some(value) = &value {
                cache.remember(key, value, lifetime);
            }
            return Ok((generation, value));
        }
    }

    async fn set_cache(
        &self,
        key: &str,
        value: &JsValue,
        generation: u64,
    ) -> Result<bool, Failure> {
        let text = value
            .stringify()
            .ok_or_else(|| Failure::new(FailureStage::CommandResponse))?;
        let mut cache = self.cache.lock().await;
        if cache.generation != generation {
            return Ok(false);
        }
        // Serialize publication with the event's DEL. An old DB fill must not
        // overwrite the invalidation in either memory or the shared Redis key.
        cache.remember(
            key,
            value,
            Duration::from_secs(if key.starts_with("kvcache:userFollowings:") {
                60
            } else {
                180
            }),
        );
        let mut pipeline = redis::pipe();
        pipeline
            .cmd("SET")
            .arg(self.key(key))
            .arg(text)
            .arg("EX")
            .arg(1800);
        self.execute(&pipeline).await?;
        Ok(true)
    }

    async fn apply_event(&self, event: &Value) -> Result<(), Failure> {
        let Some(message) = event.get("message") else {
            return Ok(());
        };
        match message.get("type").and_then(Value::as_str) {
            Some(
                "avatarDecorationCreated" | "avatarDecorationUpdated" | "avatarDecorationDeleted",
            ) => {
                self.cache
                    .lock()
                    .await
                    .invalidate(Some("native:avatarDecorations"));
            }
            Some("follow" | "unfollow") => {
                if let Some(follower) = message
                    .get("body")
                    .and_then(|body| body.get("followerId"))
                    .and_then(Value::as_str)
                {
                    let key = format!("kvcache:userFollowings:{follower}");
                    let mut cache = self.cache.lock().await;
                    cache.invalidate(Some(&key));
                    let mut pipeline = redis::pipe();
                    pipeline.cmd("DEL").arg(self.key(&key));
                    self.execute(&pipeline).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn shutdown(&self) {
        self.stopped.store(true, Ordering::Release);
        self.stop.send_replace(true);
        *self.connection.lock().await = None;
        self.cache.lock().await.invalidate(None);
    }
}

struct Clock {
    origin_us: f64,
    instant: Instant,
}
impl Clock {
    fn new() -> Self {
        // ratelimiter/microtime.js initializes with Date.now()*1000 and uses
        // rounded monotonic microseconds thereafter (not repeated wall time).
        let origin_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        Self {
            origin_us: origin_ms as f64 * 1000.0,
            instant: Instant::now(),
        }
    }
    fn now(&self) -> f64 {
        self.origin_us + ((self.instant.elapsed().as_nanos() + 500) / 1000) as f64
    }
}

fn client(config: &RedisConfig) -> Result<Client, Failure> {
    if config.db < 0
        || config.connect_timeout_ms > u64::MAX / 2
        || config.path.as_ref().is_some_and(|path| path.is_empty())
    {
        return Err(Failure::new(FailureStage::Configuration));
    }
    let address = if let Some(path) = &config.path {
        if config.tls.is_some() {
            return Err(Failure::new(FailureStage::Configuration));
        }
        ConnectionAddr::Unix(PathBuf::from(path))
    } else {
        if config.host.is_empty() || config.port == 0 {
            return Err(Failure::new(FailureStage::Configuration));
        }
        if let Some(tls) = &config.tls {
            ConnectionAddr::TcpTls {
                host: config.host.clone(),
                port: config.port,
                insecure: tls.insecure,
                tls_params: None,
            }
        } else {
            ConnectionAddr::Tcp(config.host.clone(), config.port)
        }
    };
    let info = ConnectionInfo {
        addr: address,
        redis: RedisConnectionInfo {
            db: config.db,
            username: config.username.clone(),
            password: config.password.clone(),
            protocol: ProtocolVersion::RESP2,
        },
    };
    if let Some(tls) = &config.tls {
        if tls.cert.is_some() != tls.key.is_some() {
            return Err(Failure::new(FailureStage::Configuration));
        }
        let client_tls = match (&tls.cert, &tls.key) {
            (Some(cert), Some(key)) => Some(redis::ClientTlsConfig {
                client_cert: cert.as_bytes().to_vec(),
                client_key: key.as_bytes().to_vec(),
            }),
            _ => None,
        };
        // The bootstrap supplies Node's actual default roots if no explicit ca
        // was configured; explicit ca replaces the trust store, as in ioredis.
        Client::build_with_tls(
            info,
            redis::TlsCertificates {
                client_tls,
                root_cert: Some(tls.ca.join("\n").into_bytes()),
            },
        )
        .map_err(|_| Failure::new(FailureStage::Client))
    } else {
        Client::open(info).map_err(|_| Failure::new(FailureStage::Client))
    }
}

fn connect_deadline(config: &RedisConfig) -> Duration {
    Duration::from_millis(if config.connect_timeout_ms == 0 {
        10_000
    } else {
        config.connect_timeout_ms
    })
}

async fn subscribe(
    client: &Client,
    config: &RedisConfig,
    channel: &str,
) -> Result<PubSub, Failure> {
    tokio::time::timeout(connect_deadline(config), async {
        let mut subscription = client
            .get_async_pubsub()
            .await
            .map_err(|_| Failure::new(FailureStage::SubscriptionConnect))?;
        subscription
            .subscribe(channel)
            .await
            .map_err(|_| Failure::new(FailureStage::SubscriptionSubscribe))?;
        Ok(subscription)
    })
    .await
    .map_err(|_| Failure::new(FailureStage::SubscriptionTimeout))?
}

impl RedisState {
    pub async fn new(
        config: RedisConfig,
        pubsub_config: RedisConfig,
        reactions_config: Option<RedisConfig>,
        host: String,
        auth: Arc<AuthService<PostgresAuthStore>>,
    ) -> Result<Self, Failure> {
        let reactions = reactions_config.map(RedisReads::new).transpose()?;
        let mut state = Self::new_with_auth(config, pubsub_config, host, auth)
            .await
            .inspect_err(report)?;
        if let Some(reactions) = reactions {
            state.reactions = Arc::new(reactions);
        }
        Ok(state)
    }

    async fn new_with_auth<S: Store>(
        config: RedisConfig,
        pubsub_config: RedisConfig,
        host: String,
        auth: Arc<AuthService<S>>,
    ) -> Result<Self, Failure> {
        let client = client(&config)?;
        let reads = Arc::new(RedisReads::new(config.clone())?);
        let reactions = Arc::new(RedisReads::new(config.clone())?);
        let subscription_client = self::client(&pubsub_config)?;
        let subscription = subscribe(&subscription_client, &pubsub_config, &host).await?;
        auth.invalidate_all()
            .await
            .map_err(|_| Failure::new(FailureStage::AuthRefresh))?;
        let (stop, _) = watch::channel(false);
        let ready = Arc::new(AtomicBool::new(true));
        let state = Self {
            client,
            config,
            connection: Mutex::new(None),
            ready: ready.clone(),
            stopped: AtomicBool::new(false),
            stop: stop.clone(),
            tasks: Mutex::new(Vec::new()),
            clock: Clock::new(),
            reads: reads.clone(),
            reactions,
        };
        let maintenance_auth = auth.clone();
        let maintenance_reads = reads.clone();
        let mut signal = stop.subscribe();
        let invalidation = tokio::spawn(async move {
            let mut subscription = subscription;
            loop {
                let mut stream = subscription.on_message();
                loop {
                    let message = tokio::select! {
                        biased;
                        _ = signal.changed() => { ready.store(false,Ordering::Release); return; },
                        message = stream.next() => message,
                    };
                    let Some(message) = message else {
                        report(&Failure::new(FailureStage::SubscriptionClosed));
                        break;
                    };
                    let event = match decode_event(message.get_payload_bytes()) {
                        Ok(Some(event)) => event,
                        Ok(None) => continue,
                        Err(failure) => {
                            report(&failure);
                            break;
                        }
                    };
                    let now = crate::now_ms();
                    if reads.apply_event(&event).await.is_err() {
                        report(&Failure::new(FailureStage::EventApply));
                        break;
                    }
                    if auth.apply_event(&event, now).await.is_err() {
                        // A missed refresh could retain revoked privileges.
                        // Re-enter subscription recovery with admission shut.
                        report(&Failure::new(FailureStage::EventApply));
                        break;
                    }
                }
                drop(stream);
                ready.store(false, Ordering::Release);
                reads.cache.lock().await.invalidate(None);
                // ioredis reconnects its existing subscription; do the same for
                // future requests, without replaying any application operation.
                loop {
                    tokio::select! {
                        biased;
                        _ = signal.changed() => return,
                        _ = tokio::time::sleep(Duration::from_millis(1000)) => {},
                    }
                    let replacement = tokio::select! {
                        biased;
                        _ = signal.changed() => return,
                        result = subscribe(&subscription_client,&pubsub_config,&host) => result,
                    };
                    if let Ok(mut replacement) = replacement {
                        let refreshed = tokio::select! {
                            biased;
                            _ = signal.changed() => return,
                            result = auth.invalidate_all() => result,
                        };
                        if refreshed.is_err() {
                            report(&Failure::new(FailureStage::AuthRefresh));
                            continue;
                        }
                        // Apply already-buffered events before reopening auth.
                        let mut buffered = replacement.on_message();
                        let mut failed = false;
                        loop {
                            let message = match buffered.next().now_or_never() {
                                Some(Some(message)) => message,
                                Some(None) => {
                                    report(&Failure::new(FailureStage::SubscriptionClosed));
                                    failed = true;
                                    break;
                                }
                                None => break,
                            };
                            let event = decode_event(message.get_payload_bytes());
                            let failed_event = match event {
                                Ok(Some(event)) => {
                                    if reads.apply_event(&event).await.is_err()
                                        || auth.apply_event(&event, crate::now_ms()).await.is_err()
                                    {
                                        report(&Failure::new(FailureStage::EventApply));
                                        true
                                    } else {
                                        false
                                    }
                                }
                                Ok(None) => false,
                                Err(failure) => {
                                    report(&failure);
                                    true
                                }
                            };
                            if failed_event {
                                failed = true;
                                break;
                            }
                        }
                        drop(buffered);
                        if failed {
                            continue;
                        }
                        subscription = replacement;
                        ready.store(true, Ordering::Release);
                        break;
                    } else if let Err(failure) = replacement {
                        report(&failure);
                    }
                }
            }
        });
        let mut signal = stop.subscribe();
        let maintenance = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(3 * 60));
            let mut ip_ticker = tokio::time::interval(Duration::from_secs(60 * 60));
            ticker.tick().await; // No immediate GC, matching setInterval.
            ip_ticker.tick().await;
            loop {
                tokio::select! {
                    biased;
                    _ = signal.changed() => return,
                    _ = ticker.tick() => {
                        maintenance_auth.collect_garbage(crate::now_ms()).await;
                        maintenance_reads.cache.lock().await.entries.retain(|_, entry| entry.stored.elapsed() <= entry.lifetime);
                    },
                    _ = ip_ticker.tick() => maintenance_auth.clear_ip_history().await,
                }
            }
        });
        state.tasks.lock().await.extend([invalidation, maintenance]);
        Ok(state)
    }

    pub fn ready(&self) -> bool {
        !self.stopped.load(Ordering::Acquire) && self.ready.load(Ordering::Acquire)
    }

    /// Read buffered reaction deltas/pairs in one pipeline on the configured
    /// reactions Redis lane, not the main limiter/cache database.
    pub(crate) async fn reaction_buffers(
        &self,
        note_ids: &[String],
    ) -> Result<HashMap<String, ReactionBuffer>, Failure> {
        if !self.ready() {
            return Err(Failure::new(FailureStage::Unavailable));
        }
        if note_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut pipeline = redis::pipe();
        for id in note_ids {
            pipeline
                .cmd("HGETALL")
                .arg(self.reactions.key(&format!("reactionsBufferDeltas:{id}")));
            pipeline
                .cmd("ZRANGE")
                .arg(self.reactions.key(&format!("reactionsBufferPairs:{id}")))
                .arg(0)
                .arg(-1);
        }
        let results = self.reactions.execute(&pipeline).await?;
        if results.len() != note_ids.len() * 2 {
            return Err(Failure::new(FailureStage::CommandResponse));
        }
        let mut buffers = HashMap::new();
        for (id, values) in note_ids.iter().zip(results.as_chunks::<2>().0) {
            let raw_deltas: Vec<(String, String)> = redis::from_redis_value(&values[0])
                .map_err(|_| Failure::new(FailureStage::CommandResponse))?;
            let raw_pairs: Vec<String> = redis::from_redis_value(&values[1])
                .map_err(|_| Failure::new(FailureStage::CommandResponse))?;
            let deltas = JsValue::object(raw_deltas.into_iter().map(|(name, count)| {
                // HINCRBY stores canonical signed decimal integers. Decode to
                // JS Number, including its rounding outside the safe range.
                let value = count.parse::<f64>().unwrap_or(f64::NAN);
                (name, JsValue::Number(value))
            }));
            let pairs = raw_pairs
                .into_iter()
                .map(|pair| {
                    let mut parts = pair.split('/');
                    (
                        parts.next().unwrap_or_default().to_owned(),
                        parts.next().unwrap_or_default().to_owned(),
                    )
                })
                .collect();
            buffers.insert(id.clone(), ReactionBuffer { deltas, pairs });
        }
        Ok(buffers)
    }

    pub(crate) async fn avatar_decorations(&self, database: &Database) -> Result<JsValue, Failure> {
        let key = "native:avatarDecorations";
        loop {
            if !self.ready() {
                return Err(Failure::new(FailureStage::Unavailable));
            }
            let generation = {
                let cache = self.reads.cache.lock().await;
                if let Some(entry) = cache.entries.get(key)
                    && entry.stored.elapsed() <= Duration::from_secs(1800)
                {
                    return Ok(entry.value.clone());
                }
                cache.generation
            };
            let rows = database
                .query_typed(
                    true,
                    "SELECT row_to_json(t)::text FROM avatar_decoration t",
                    &[],
                )
                .await
                .map_err(|_| Failure::new(FailureStage::Command))?;
            let values = rows
                .into_iter()
                .map(|row| {
                    let text: String = row
                        .try_get(0)
                        .map_err(|_| Failure::new(FailureStage::CommandResponse))?;
                    JsValue::parse_json(&text)
                        .map_err(|_| Failure::new(FailureStage::CommandResponse))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let value = JsValue::Array(values);
            let mut cache = self.reads.cache.lock().await;
            if cache.generation != generation {
                continue;
            }
            cache.remember(key, &value, Duration::from_secs(1800));
            return Ok(value);
        }
    }

    pub(crate) async fn following_ids(
        &self,
        viewer: &str,
        database: &Database,
    ) -> Result<HashSet<String>, Failure> {
        let key = format!("kvcache:userFollowings:{viewer}");
        loop {
            if !self.ready() {
                return Err(Failure::new(FailureStage::Unavailable));
            }
            let (generation, cached) = self.reads.get_cache(&key, Duration::from_secs(60)).await?;
            let value = if let Some(value) = cached {
                value
            } else {
                let rows = database.query_typed(true,
                    "SELECT json_build_object('followeeId', \"followeeId\", 'withReplies', \"withReplies\")::text FROM following WHERE \"followerId\" = $1",
                    &[&viewer],
                ).await.map_err(|_| Failure::new(FailureStage::Command))?;
                let mut entries = Vec::with_capacity(rows.len());
                for row in rows {
                    let text: String = row
                        .try_get(0)
                        .map_err(|_| Failure::new(FailureStage::CommandResponse))?;
                    let row = JsValue::parse_json(&text)
                        .map_err(|_| Failure::new(FailureStage::CommandResponse))?;
                    let id = row
                        .get("followeeId")
                        .and_then(JsValue::to_utf8)
                        .ok_or_else(|| Failure::new(FailureStage::CommandResponse))?;
                    let with_replies = row
                        .get("withReplies")
                        .cloned()
                        .ok_or_else(|| Failure::new(FailureStage::CommandResponse))?;
                    entries.push((id, JsValue::object([("withReplies", with_replies)])));
                }
                let value = JsValue::object(entries);
                if !self.reads.set_cache(&key, &value, generation).await? {
                    continue;
                }
                value
            };
            let JsValue::Object(entries) = &value else {
                return Err(Failure::new(FailureStage::CommandResponse));
            };
            return entries
                .iter()
                .filter(|(_, value)| value.truthy())
                .map(|(key, _)| {
                    String::from_utf16(key).map_err(|_| Failure::new(FailureStage::CommandResponse))
                })
                .collect();
        }
    }

    pub(crate) async fn federated_instance(
        &self,
        host: &str,
        database: &Database,
    ) -> Result<Option<JsValue>, Failure> {
        let key = format!("kvcache:federatedInstance:{host}");
        loop {
            if !self.ready() {
                return Err(Failure::new(FailureStage::Unavailable));
            }
            let (generation, cached) = self.reads.get_cache(&key, Duration::from_secs(180)).await?;
            let value = if let Some(value) = cached {
                value
            } else {
                let rows = database
                    .query_typed(
                        true,
                        "SELECT row_to_json(t)::text FROM instance t WHERE host = $1 LIMIT 1",
                        &[&host],
                    )
                    .await
                    .map_err(|_| Failure::new(FailureStage::Command))?;
                let value = if let Some(row) = rows.first() {
                    let text: String = row
                        .try_get(0)
                        .map_err(|_| Failure::new(FailureStage::CommandResponse))?;
                    JsValue::parse_json(&text)
                        .map_err(|_| Failure::new(FailureStage::CommandResponse))?
                } else {
                    JsValue::Null
                };
                if !self.reads.set_cache(&key, &value, generation).await? {
                    continue;
                }
                value
            };
            return match value {
                JsValue::Null => Ok(None),
                JsValue::Object(_) => Ok(Some(value)),
                _ => Err(Failure::new(FailureStage::CommandResponse)),
            };
        }
    }

    pub(crate) async fn limit(
        &self,
        limitation: &RateLimit,
        endpoint: &str,
        actor: &str,
        factor: f64,
    ) -> Result<Option<LimitInfo>, Failure> {
        if !self.ready() {
            return Err(Failure::new(FailureStage::Unavailable));
        }
        let key = limitation.key.as_deref().unwrap_or(endpoint);
        if let Some(interval) = limitation.min_interval {
            let duration = interval * factor;
            let info = self
                .check(&format!("{actor}:{key}:min"), duration, 1.0)
                .await?;
            if info.remaining == 0.0 {
                return Ok(Some(info));
            }
        }
        if let (Some(duration), Some(max)) = (limitation.duration, limitation.max) {
            let info = self
                .check(&format!("{actor}:{key}"), duration, max / factor)
                .await?;
            if info.remaining == 0.0 {
                return Ok(Some(info));
            }
        }
        Ok(None)
    }

    async fn check(&self, id: &str, duration: f64, max: f64) -> Result<LimitInfo, Failure> {
        let mut stop = self.stop.subscribe();
        if self.stopped.load(Ordering::Acquire) {
            return Err(Failure::new(FailureStage::Shutdown));
        }
        let operation = async {
            let mut connection = self.connection.lock().await;
            if connection.is_none() {
                *connection = Some(
                    tokio::time::timeout(
                        connect_deadline(&self.config),
                        self.client.get_multiplexed_async_connection(),
                    )
                    .await
                    .map_err(|_| Failure::new(FailureStage::ConnectionTimeout))?
                    .map_err(|_| Failure::new(FailureStage::Connection))?,
                );
            }
            let result = check_at(
                connection
                    .as_mut()
                    .ok_or_else(|| Failure::new(FailureStage::Connection))?,
                &format!("{}limit:{id}", self.config.key_prefix),
                duration,
                max,
                self.clock.now(),
            )
            .await;
            if result.is_err() {
                *connection = None;
            }
            result
        };
        let result = tokio::select! {
            biased;
            _ = stop.changed() => Err(Failure::new(FailureStage::Shutdown)),
            result = operation => result,
        };
        result.inspect_err(report)
    }

    pub async fn shutdown(&self) {
        self.stopped.store(true, Ordering::Release);
        self.ready.store(false, Ordering::Release);
        self.stop.send_replace(true);
        // Abort is necessary if an event refresh is waiting for a stalled DB;
        // the database owner separately cancels its in-flight SQL on shutdown.
        for task in self.tasks.lock().await.drain(..) {
            task.abort();
            let _ = task.await;
        }
        *self.connection.lock().await = None;
        self.reads.shutdown().await;
        self.reactions.shutdown().await;
    }
}

struct EventEnvelope<'a> {
    channel: Option<&'a RawValue>,
    message: Option<&'a RawValue>,
}

impl<'de> Deserialize<'de> for EventEnvelope<'de> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EnvelopeVisitor;
        impl<'de> serde::de::Visitor<'de> for EnvelopeVisitor {
            type Value = EventEnvelope<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a host event object")
            }

            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut envelope = EventEnvelope {
                    channel: None,
                    message: None,
                };
                // Raw values validate all JSON syntax without decoding an
                // unrelated business payload into Rust's UTF-8 value tree.
                // Retaining the last occurrence matches JSON.parse duplicates.
                while let Some(key) = map.next_key::<&'de RawValue>()? {
                    let value = map.next_value::<&'de RawValue>()?;
                    match serde_json::from_str::<String>(key.get()).as_deref() {
                        Ok("channel") => envelope.channel = Some(value),
                        Ok("message") => envelope.message = Some(value),
                        // A valid JS key containing a lone surrogate cannot
                        // equal either ASCII envelope key and is irrelevant.
                        _ => {}
                    }
                }
                Ok(envelope)
            }
        }
        deserializer.deserialize_map(EnvelopeVisitor)
    }
}

fn decode_event(bytes: &[u8]) -> Result<Option<SafeJson>, Failure> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    decoder.disable_recursion_limit();
    let envelope = EventEnvelope::deserialize(&mut decoder)
        .map_err(|_| Failure::new(FailureStage::EventDecode))?;
    decoder
        .end()
        .map_err(|_| Failure::new(FailureStage::EventDecode))?;
    let channel = envelope
        .channel
        .ok_or_else(|| Failure::new(FailureStage::EventEnvelope))?;
    if !channel.get().starts_with('"') {
        return Err(Failure::new(FailureStage::EventEnvelope));
    }
    // This host subscription also carries raw notesStream and other business
    // payloads. A JS channel with a lone surrogate is still a string and cannot
    // equal "internal". Its business message need never enter auth decoding.
    if !serde_json::from_str::<String>(channel.get()).is_ok_and(|channel| channel == "internal") {
        return Ok(None);
    }
    let message = envelope
        .message
        .ok_or_else(|| Failure::new(FailureStage::EventEnvelope))?;
    let mut decoder = serde_json::Deserializer::from_str(message.get());
    decoder.disable_recursion_limit();
    let mut fields = serde_json::Map::new();
    fields.insert("channel".into(), Value::String("internal".into()));
    fields.insert(
        "message".into(),
        Value::deserialize(serde_stacker::Deserializer::new(&mut decoder))
            .map_err(|_| Failure::new(FailureStage::EventDecode))?,
    );
    let event = SafeJson::new(Value::Object(fields));
    decoder
        .end()
        .map_err(|_| Failure::new(FailureStage::EventDecode))?;
    if !event["message"].is_object() || !event["message"].get("type").is_some_and(Value::is_string)
    {
        return Err(Failure::new(FailureStage::EventEnvelope));
    }
    Ok(Some(event))
}

async fn check_at(
    connection: &mut MultiplexedConnection,
    key: &str,
    duration: f64,
    max: f64,
    now_us: f64,
) -> Result<LimitInfo, Failure> {
    // Limiter(opts) applies JavaScript's `||` defaults, including zero/NaN.
    let duration = if duration == 0.0 || duration.is_nan() {
        3_600_000.0
    } else {
        duration
    };
    let max = if max == 0.0 || max.is_nan() {
        2500.0
    } else {
        max
    };
    let start = now_us - duration * 1000.0;
    let mut pipeline = redis::pipe();
    pipeline
        .atomic()
        .cmd("ZREMRANGEBYSCORE")
        .arg(key)
        .arg(0)
        .arg(js_number(start))
        .cmd("ZCARD")
        .arg(key)
        .cmd("ZADD")
        .arg(key)
        .arg(js_number(now_us))
        .arg(js_number(now_us))
        .cmd("ZRANGE")
        .arg(key)
        .arg(0)
        .arg(0)
        .cmd("ZRANGE")
        .arg(key)
        .arg(js_number(-max))
        .arg(js_number(-max))
        .cmd("ZREMRANGEBYRANK")
        .arg(key)
        .arg(0)
        .arg(js_number(-(max + 1.0)))
        .cmd("PEXPIRE")
        .arg(key)
        .arg(js_number(duration));
    // query_async recursively rejects errors inside EXEC. ioredis exposes
    // [error,value] per command and ratelimiter ignores those errors. Request
    // the raw EXEC result so rank/TTL errors keep their actual legacy behavior.
    let mut response = connection
        .req_packed_commands(&pipeline, 8, 1)
        .await
        .map_err(|_| Failure::new(FailureStage::Command))?;
    let RedisValue::Array(values) = response
        .pop()
        .ok_or_else(|| Failure::new(FailureStage::CommandResponse))?
    else {
        return Err(Failure::new(FailureStage::CommandResponse));
    };
    if values.len() != 7 {
        return Err(Failure::new(FailureStage::CommandResponse));
    }
    let count = parse_int_reply(&values[1]);
    let oldest = parse_int_reply(&values[3]);
    let in_range = parse_int_reply(&values[4]);
    let reset_micro = if in_range.is_nan() { oldest } else { in_range } + duration * 1000.0;
    Ok(LimitInfo {
        remaining: if count < max { max - count } else { 0.0 },
        reset: (reset_micro / 1_000_000.0).floor(),
        reset_ms: (reset_micro / 1000.0).floor(),
        total: max,
    })
}

fn js_number(value: f64) -> String {
    if value.is_nan() {
        "NaN".into()
    } else if value == f64::INFINITY {
        "Infinity".into()
    } else if value == f64::NEG_INFINITY {
        "-Infinity".into()
    } else {
        ryu_js::Buffer::new().format(value).to_owned()
    }
}

fn parse_int_reply(value: &RedisValue) -> f64 {
    // Array.toString joins values with comma. ZRANGE in this contract has at
    // most one member, and parseInt stops at its first nondecimal character.
    match value {
        RedisValue::Int(n) => *n as f64,
        RedisValue::Array(values) => values.first().map_or(f64::NAN, parse_int_reply),
        RedisValue::BulkString(bytes) => {
            let Ok(text) = std::str::from_utf8(bytes) else {
                return f64::NAN;
            };
            let text = text.trim_start();
            let len = text
                .char_indices()
                .take_while(|(index, c)| {
                    c.is_ascii_digit() || (*index == 0 && matches!(c, '+' | '-'))
                })
                .last()
                .map_or(0, |(index, c)| index + c.len_utf8());
            text[..len].parse().unwrap_or(f64::NAN)
        }
        _ => f64::NAN,
    }
}

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug)]
pub(crate) enum HashtagCount {
    Number(f64),
    Undefined,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum HashtagOperation {
    Ranking,
    Chart,
}
impl HashtagOperation {
    fn name(self) -> &'static str {
        match self {
            Self::Ranking => "hashtags.trend.ranking",
            Self::Chart => "hashtags.trend.chart",
        }
    }
}
#[derive(Debug)]
pub(crate) struct HashtagCommandError {
    pub operation: HashtagOperation,
    pub command_index: usize,
    pub source: redis::RedisError,
}
#[derive(Debug)]
pub(crate) struct HashtagBatch<T> {
    pub values: T,
    pub command_errors: Vec<HashtagCommandError>,
}
impl<T> HashtagBatch<T> {
    pub(crate) fn report(&self) {
        for error in &self.command_errors {
            // Only a typed Redis error category is logged. ServerError details,
            // key/tag/member text, reply data and credentials stay private.
            tracing::warn!(operation = error.operation.name(),
                command_index = error.command_index, error.kind = ?error.source.kind(),
                "native hashtag command returned an error");
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum HashtagResponseKind {
    RankingOuter,
    RankingWindow,
    RankingPairs,
    RankingMember,
    CountsOuter,
    CountElement,
}
#[derive(Debug)]
pub(crate) enum HashtagReadFailure {
    Read(ReadExecutionFailure),
    Unavailable,
    Response(HashtagResponseKind),
}
impl HashtagReadFailure {
    pub(crate) fn safe_kind(&self) -> &'static str {
        match self {
            Self::Read(source) => source.safe_kind(),
            Self::Unavailable => "redis_unavailable",
            Self::Response(HashtagResponseKind::RankingOuter) => "ranking_outer_shape",
            Self::Response(HashtagResponseKind::RankingWindow) => "ranking_window_shape",
            Self::Response(HashtagResponseKind::RankingPairs) => "ranking_pair_shape",
            Self::Response(HashtagResponseKind::RankingMember) => "ranking_member_shape",
            Self::Response(HashtagResponseKind::CountsOuter) => "chart_outer_shape",
            Self::Response(HashtagResponseKind::CountElement) => "chart_count_shape",
        }
    }
}
impl fmt::Display for HashtagReadFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.safe_kind())
    }
}
impl std::error::Error for HashtagReadFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(source) => Some(source),
            Self::Unavailable | Self::Response(_) => None,
        }
    }
}

fn hashtag_ranking_pipeline(
    keys: &[String; 2],
    mut prefix: impl FnMut(&str) -> String,
) -> redis::Pipeline {
    let mut pipeline = redis::pipe(); // Non-transactional: never call atomic().
    for key in keys {
        pipeline
            .cmd("ZRANGE")
            .arg(prefix(key))
            .arg(0)
            .arg(10)
            .arg("REV")
            .arg("WITHSCORES");
    }
    pipeline
}
fn hashtag_count_pipeline(
    keys: &[String],
    mut prefix: impl FnMut(&str) -> String,
) -> redis::Pipeline {
    let mut pipeline = redis::pipe();
    for key in keys {
        pipeline.cmd("PFCOUNT").arg(prefix(key));
    }
    pipeline
}

fn decode_hashtag_ranking(
    values: &[RedisValue],
) -> Result<HashtagBatch<[Vec<String>; 2]>, HashtagReadFailure> {
    let [current, previous] = values else {
        return Err(HashtagReadFailure::Response(
            HashtagResponseKind::RankingOuter,
        ));
    };
    let mut result = HashtagBatch {
        values: [Vec::new(), Vec::new()],
        command_errors: Vec::new(),
    };
    for (index, reply) in [current, previous].into_iter().enumerate() {
        if let RedisValue::ServerError(source) = reply {
            // Actual ioredis Pipeline.fillResult([error], position); actual
            // FeaturedService maps tuple[1] ?? [] to an empty window only.
            result.command_errors.push(HashtagCommandError {
                operation: HashtagOperation::Ranking,
                command_index: index,
                source: source.clone().into(),
            });
            continue;
        }
        let RedisValue::Array(pairs) = reply else {
            return Err(HashtagReadFailure::Response(
                HashtagResponseKind::RankingWindow,
            ));
        };
        if pairs.len() % 2 != 0 {
            return Err(HashtagReadFailure::Response(
                HashtagResponseKind::RankingPairs,
            ));
        }
        for pair in pairs.as_chunks::<2>().0 {
            let member = match &pair[0] {
                RedisValue::BulkString(bytes) => String::from_utf8_lossy(bytes).into_owned(),
                RedisValue::SimpleString(text) => text.clone(),
                _ => {
                    return Err(HashtagReadFailure::Response(
                        HashtagResponseKind::RankingMember,
                    ));
                }
            };
            // Scores are deliberately NOT parsed or narrowed. Every legal RESP2
            // ZRANGE WITHSCORES score, including +/-inf, has the same key-order
            // behavior: TS parseInt gives a Number (possibly NaN), never throws
            // for Redis-decoded strings, and only changes unused Map values.
            result.values[index].push(member);
        }
    }
    Ok(result)
}

fn decode_hashtag_counts(
    expected: usize,
    values: &[RedisValue],
) -> Result<HashtagBatch<Vec<HashtagCount>>, HashtagReadFailure> {
    if values.len() != expected {
        return Err(HashtagReadFailure::Response(
            HashtagResponseKind::CountsOuter,
        ));
    }
    let mut result = HashtagBatch {
        values: Vec::with_capacity(expected),
        command_errors: Vec::new(),
    };
    for (index, reply) in values.iter().enumerate() {
        match reply {
            RedisValue::Int(value) => result
                .values
                .push(HashtagCount::Number(hashtag_integer(*value))),
            RedisValue::ServerError(source) => {
                // HashtagService pushes tuple[1], which is undefined for [error].
                // Preserve this until Math.max and JSON projection (null in arrays).
                result.values.push(HashtagCount::Undefined);
                result.command_errors.push(HashtagCommandError {
                    operation: HashtagOperation::Chart,
                    command_index: index,
                    source: source.clone().into(),
                });
            }
            _ => {
                return Err(HashtagReadFailure::Response(
                    HashtagResponseKind::CountElement,
                ));
            }
        }
    }
    Ok(result)
}

impl RedisState {
    pub(crate) async fn hashtag_rankings(
        &self,
        keys: &[String; 2],
    ) -> Result<HashtagBatch<[Vec<String>; 2]>, HashtagReadFailure> {
        if !self.ready() {
            return Err(HashtagReadFailure::Unavailable);
        }
        let pipeline = hashtag_ranking_pipeline(keys, |key| self.reads.key(key));
        let values = self
            .reads
            .execute_per_command(&pipeline)
            .await
            .map_err(HashtagReadFailure::Read)?;
        decode_hashtag_ranking(&values)
    }
    pub(crate) async fn hashtag_counts(
        &self,
        keys: &[String],
    ) -> Result<HashtagBatch<Vec<HashtagCount>>, HashtagReadFailure> {
        if !self.ready() {
            return Err(HashtagReadFailure::Unavailable);
        }
        let pipeline = hashtag_count_pipeline(keys, |key| self.reads.key(key));
        let values = self
            .reads
            .execute_per_command(&pipeline)
            .await
            .map_err(HashtagReadFailure::Read)?;
        decode_hashtag_counts(keys.len(), &values)
    }
}

// redis-parser@3.0.0 parseSimpleNumbers accumulates decimal digits as JS
// Number. Integer-to-f64 casting rounds only once and differs above 2^53.
fn hashtag_integer(value: i64) -> f64 {
    let text = value.to_string();
    let mut number = 0_f64;
    for byte in text.bytes().filter(|byte| *byte != b'-') {
        number = number * 10. + f64::from(byte - b'0');
    }
    if value.is_negative() { -number } else { number }
}

#[cfg(test)]
mod hashtag_trend_source_tests {
    use super::*;
    use serde_json::Value as Json;
    const SOURCE: &str = include_str!("business/fixtures/hashtag-trend-source-oracle.json");
    const ERRORS: &str = include_str!("business/fixtures/hashtag-trend-errors-oracle.json");

    fn error() -> RedisValue {
        redis::parse_redis_value(b"-ERR source fixture\r\n").unwrap()
    }
    fn reply(fixture: &Json, name: &str) -> RedisValue {
        if name == "current" && fixture["input"]["rankingErrorCurrent"] == true {
            return error();
        }
        RedisValue::Array(
            fixture["input"][name]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|pair| {
                    [
                        RedisValue::BulkString(pair[0].as_str().unwrap().as_bytes().to_vec()),
                        RedisValue::BulkString(pair[1].as_str().unwrap().as_bytes().to_vec()),
                    ]
                })
                .collect(),
        )
    }
    #[test]
    fn ranking_decode_keeps_member_order_and_command_errors_from_actual_source_cases() {
        for text in [SOURCE, ERRORS] {
            let oracle: Json = serde_json::from_str(text).unwrap();
            for fixture in oracle["cases"].as_array().unwrap() {
                if fixture["input"]["rankingNull"] == true {
                    continue;
                } // Source-only null-exec control.
                let decoded = decode_hashtag_ranking(&[
                    reply(fixture, "current"),
                    reply(fixture, "previous"),
                ])
                .unwrap();
                let mut seen = HashSet::new();
                let tags: Vec<&str> = decoded
                    .values
                    .iter()
                    .flatten()
                    .filter_map(|tag| {
                        if seen.insert(tag.as_str()) {
                            Some(tag.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();
                let expected: Vec<&str> = fixture["observed"]["ranking"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|tag| tag.as_str().unwrap())
                    .collect();
                assert_eq!(tags, expected, "{}", fixture["name"]);
                assert_eq!(
                    decoded.command_errors.len(),
                    usize::from(fixture["input"]["rankingErrorCurrent"] == true)
                );
            }
        }
    }
    #[test]
    fn chart_command_error_remains_undefined_and_retains_the_concrete_server_error() {
        let oracle: Json = serde_json::from_str(ERRORS).unwrap();
        let fixture = oracle["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["input"]["chartErrorAtIndex"].is_number())
            .unwrap();
        let raw: Vec<RedisValue> = fixture["chartResponse"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| {
                if x.is_null() {
                    error()
                } else {
                    RedisValue::Int(x.as_i64().unwrap())
                }
            })
            .collect();
        let result = decode_hashtag_counts(raw.len(), &raw).unwrap();
        assert_eq!(result.command_errors.len(), 1);
        for (value, source) in result.values.iter().zip(
            fixture["observed"]["chartValueKinds"][0]["values"]
                .as_array()
                .unwrap(),
        ) {
            match value {
                HashtagCount::Undefined => assert_eq!(source["type"], "undefined"),
                HashtagCount::Number(number) => assert_eq!(Some(*number), source["value"].as_f64()),
            }
        }
    }

    #[test]
    fn cardinalities_match_actual_ioredis_parser_number_rounding() {
        let oracle: Json = serde_json::from_str(include_str!(
            "business/fixtures/hashtag-trend-integer-oracle.json"
        ))
        .unwrap();
        for fixture in oracle["cases"].as_array().unwrap() {
            let decimal = fixture["decimal"].as_str().unwrap();
            let input: i64 = decimal.parse().unwrap();
            let actual = hashtag_integer(input);
            let expected =
                u64::from_str_radix(fixture["sourceBits"].as_str().unwrap(), 16).unwrap();
            assert_eq!(actual.to_bits(), expected, "{decimal}");
            assert_eq!(
                JsValue::Number(actual).stringify().as_deref(),
                fixture["sourceJson"].as_str(),
                "{decimal}"
            );
        }
    }
}
