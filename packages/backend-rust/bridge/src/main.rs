//! Temporary authenticated loopback adapter. Stdout is reserved for readiness.
use axum::{
    Router,
    body::{Bytes, to_bytes},
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use misskey_backend_core::{Catalog, Endpoint, Property, echo, ping_at};
use serde::{Deserialize, Serialize};
use std::{
    future::IntoFuture,
    io::{self, BufRead, Read, Write},
    sync::Arc,
    time::{Duration, SystemTime},
};

const LIMIT: usize = 16 * 1024 * 1024;
#[derive(Debug, thiserror::Error)]
#[error("Rust backend unavailable")]
struct Failure;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bootstrap {
    v: u8,
    token: String,
    endpoints: Vec<EndpointDefinition>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct EndpointDefinition {
    name_utf16: Vec<u16>,
    properties: Vec<PropertyDefinition>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PropertyDefinition {
    name_utf16: Vec<u16>,
    #[serde(rename = "type", deserialize_with = "required_nullable")]
    kind: Option<String>,
}
fn required_nullable<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}
#[derive(Deserialize)]
#[serde(tag = "op", deny_unknown_fields)]
enum Rpc {
    #[serde(rename = "ping")]
    Ping { v: u8 },
    #[serde(rename = "test", rename_all = "camelCase")]
    Test { v: u8, params_json: String },
    #[serde(rename = "endpoint", rename_all = "camelCase")]
    Endpoint { v: u8, endpoint_utf16: Vec<u16> },
}
#[derive(Serialize)]
#[serde(tag = "op")]
enum Success<'a> {
    #[serde(rename = "ping")]
    Ping { v: u8, pong: i64 },
    #[serde(rename = "test", rename_all = "camelCase")]
    Test { v: u8, result_json: &'a str },
    #[serde(rename = "endpoint")]
    Endpoint { v: u8, result: Option<Description> },
}
#[derive(Serialize)]
struct Description {
    params: Vec<Parameter>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Parameter {
    name_utf16: Vec<u16>,
    #[serde(rename = "type")]
    kind: String,
}
struct App {
    bearer: String,
    catalog: Catalog,
}
fn unavailable(status: StatusCode) -> Response {
    (status, "Rust backend unavailable").into_response()
}
struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > LIMIT.saturating_sub(self.0.len()) {
            return Err(io::Error::other("response too large"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn success(value: Success<'_>) -> Response {
    let mut writer = Bounded(Vec::new());
    if serde_json::to_writer(&mut writer, &value).is_err() {
        return unavailable(StatusCode::INTERNAL_SERVER_ERROR);
    }
    ([(header::CONTENT_TYPE, "application/json")], writer.0).into_response()
}
async fn rpc(State(app): State<Arc<App>>, request: Request) -> Response {
    if request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        != Some(app.bearer.as_str())
    {
        return unavailable(StatusCode::UNAUTHORIZED);
    }
    let bytes: Bytes = match to_bytes(request.into_body(), LIMIT).await {
        Ok(bytes) => bytes,
        Err(_) => return unavailable(StatusCode::PAYLOAD_TOO_LARGE),
    };
    let operation: Rpc = match serde_json::from_slice(&bytes) {
        Ok(operation) => operation,
        Err(_) => return unavailable(StatusCode::BAD_REQUEST),
    };
    match operation {
        Rpc::Ping { v: 1 } => match ping_at(SystemTime::now()) {
            Ok(pong) => success(Success::Ping { v: 1, pong }),
            Err(_) => unavailable(StatusCode::INTERNAL_SERVER_ERROR),
        },
        Rpc::Test { v: 1, params_json } => success(Success::Test {
            v: 1,
            result_json: echo(&params_json),
        }),
        Rpc::Endpoint {
            v: 1,
            endpoint_utf16,
        } => {
            let result = app
                .catalog
                .lookup(&endpoint_utf16)
                .map(|params| Description {
                    params: params
                        .into_iter()
                        .map(|(name_utf16, kind)| Parameter { name_utf16, kind })
                        .collect(),
                });
            success(Success::Endpoint { v: 1, result })
        }
        _ => unavailable(StatusCode::BAD_REQUEST),
    }
}
fn bootstrap() -> Result<App, Failure> {
    let mut frame = Vec::new();
    io::stdin()
        .lock()
        .take((LIMIT + 1) as u64)
        .read_until(b'\n', &mut frame)
        .map_err(|_| Failure)?;
    if frame.len() > LIMIT || frame.last() != Some(&b'\n') {
        return Err(Failure);
    }
    let bootstrap: Bootstrap = serde_json::from_slice(&frame).map_err(|_| Failure)?;
    if bootstrap.v != 1
        || bootstrap.token.len() != 64
        || !bootstrap.token.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(Failure);
    }
    if bootstrap
        .endpoints
        .iter()
        .flat_map(|endpoint| &endpoint.properties)
        .any(|property| {
            property.kind.as_ref().is_some_and(|kind| {
                !matches!(
                    kind.as_str(),
                    "null"
                        | "boolean"
                        | "integer"
                        | "number"
                        | "string"
                        | "array"
                        | "object"
                        | "any"
                )
            })
        })
    {
        return Err(Failure);
    }
    let endpoints = bootstrap
        .endpoints
        .into_iter()
        .map(|endpoint| Endpoint {
            name: endpoint.name_utf16,
            properties: endpoint
                .properties
                .into_iter()
                .map(|property| Property {
                    name: property.name_utf16,
                    kind: property.kind,
                })
                .collect(),
        })
        .collect();
    Ok(App {
        bearer: format!("Bearer {}", bootstrap.token),
        catalog: Catalog::new(endpoints),
    })
}
async fn signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("signal handler");
        tokio::select! { _=tokio::signal::ctrl_c()=>{}, _=terminate.recv()=>{} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
async fn serve(app: App, eof: tokio::sync::oneshot::Receiver<()>) -> Result<(), Failure> {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|_| Failure)?;
    let port = listener.local_addr().map_err(|_| Failure)?.port();
    let router = Router::new()
        .route("/rpc", post(rpc))
        .fallback(|| async { unavailable(StatusCode::NOT_FOUND) })
        .with_state(Arc::new(app));
    writeln!(io::stdout().lock(), "{{\"v\":1,\"port\":{port}}}").map_err(|_| Failure)?;
    io::stdout().flush().map_err(|_| Failure)?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server = axum::serve(listener, router).with_graceful_shutdown(async {
        let _ = shutdown_rx.await;
    });
    let server = std::pin::pin!(server.into_future());
    let stop = async {
        tokio::select! { _ = signal() => {}, _ = eof => {} }
    };
    tokio::pin!(stop);
    let mut server = server;
    tokio::select! {
        result = &mut server => return result.map_err(|_| Failure),
        _ = &mut stop => {},
    }
    let _ = shutdown_tx.send(());
    match tokio::time::timeout(Duration::from_secs(2), &mut server).await {
        Ok(result) => result.map_err(|_| Failure),
        Err(_) => Ok(()),
    }
}
fn main() {
    let outcome = (|| {
        let app = bootstrap()?;
        // A detached OS thread cannot keep runtime shutdown or process exit blocked.
        let (eof_tx, eof_rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 1024];
            loop {
                match io::stdin().read(&mut buffer) {
                    Ok(0) | Err(_) => {
                        let _ = eof_tx.send(());
                        return;
                    }
                    Ok(_) => {}
                }
            }
        });
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|_| Failure)?;
        runtime.block_on(serve(app, eof_rx))
    })();
    if outcome.is_err() {
        eprintln!("Rust backend unavailable");
        std::process::exit(1);
    }
}
