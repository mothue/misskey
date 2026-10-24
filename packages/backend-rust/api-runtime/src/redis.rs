//! Direct Redis ownership for auth invalidations and the existing limiter.
//! Responses retain the legacy MULTI command errors: fractional rank/TTL
//! arguments fail per command while the other commands still execute.
use crate::meta::RateLimit;
use futures_util::{FutureExt, StreamExt};
use misskey_backend_api_auth::{AuthService, SafeJson, Store, postgres::PostgresAuthStore};
use redis::{
    Client, ConnectionAddr, ConnectionInfo, ProtocolVersion, RedisConnectionInfo,
    Value as RedisValue,
    aio::{ConnectionLike, MultiplexedConnection, PubSub},
};
use serde::Deserialize;
use serde_json::{Value, json, value::RawValue};
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
        host: String,
        auth: Arc<AuthService<PostgresAuthStore>>,
    ) -> Result<Self, Failure> {
        Self::new_with_auth(config, pubsub_config, host, auth)
            .await
            .inspect_err(report)
    }

    async fn new_with_auth<S: Store>(
        config: RedisConfig,
        pubsub_config: RedisConfig,
        host: String,
        auth: Arc<AuthService<S>>,
    ) -> Result<Self, Failure> {
        let client = client(&config)?;
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
        };
        let maintenance_auth = auth.clone();
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
                    if auth.apply_event(&event, now).await.is_err() {
                        // A missed refresh could retain revoked privileges.
                        // Re-enter subscription recovery with admission shut.
                        report(&Failure::new(FailureStage::EventApply));
                        break;
                    }
                }
                drop(stream);
                ready.store(false, Ordering::Release);
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
                                    if auth.apply_event(&event, crate::now_ms()).await.is_err() {
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
                    _ = ticker.tick() => maintenance_auth.collect_garbage(crate::now_ms()).await,
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
