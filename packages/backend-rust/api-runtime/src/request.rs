use crate::{RequestContext, response};
use axum::{
    body::{Body, to_bytes},
    http::{HeaderValue, Response},
};
use futures_util::FutureExt;
use misskey_api_validation::Value;
use serde::Serialize;
use std::{net::IpAddr, path::PathBuf};
use tokio::io::AsyncWriteExt;

pub struct Input {
    pub params: Value,
    pub upload: Option<Upload>,
    _temporary: Option<tempfile::TempPath>,
}
#[derive(Serialize)]
pub struct Upload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub path: String,
}

pub fn preflight(context: &RequestContext) -> Response<Body> {
    let mut reply = if context.header("origin").is_none()
        || context.header("access-control-request-method").is_none()
    {
        response::plain(400, Some("text/plain"), "Invalid Preflight Request")
    } else {
        let mut reply = response::plain(204, None, Body::empty());
        reply.headers_mut().insert(
            "access-control-allow-methods",
            HeaderValue::from_static("GET,HEAD,POST"),
        );
        reply.headers_mut().insert(
            "vary",
            HeaderValue::from_static("Access-Control-Request-Headers"),
        );
        if let Some(headers) = context.header("access-control-request-headers")
            && let Ok(value) = HeaderValue::from_str(headers)
        {
            reply
                .headers_mut()
                .insert("access-control-allow-headers", value);
        }
        reply
    };
    reply
        .headers_mut()
        .insert("access-control-allow-origin", HeaderValue::from_static("*"));
    reply
}
pub async fn parse(
    context: &RequestContext,
    body: Body,
    file: bool,
    max_file_size: u64,
    directory: &str,
) -> Result<Input, Box<Response<Body>>> {
    let mut params = Value::Undefined;
    let content_type = context.header("content-type").unwrap_or("");
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let method = context.method.as_str();
    if method == "GET" {
        params = query(context.path.split_once('?').map(|(_, q)| q).unwrap_or(""));
    }
    let body_empty = context.header("transfer-encoding").is_none()
        && context
            .header("content-length")
            .is_none_or(|length| length == "0");
    if method == "QUERY" {
        if context.header("content-type").is_none() {
            return Err(response::framework(
                400,
                "FST_ERR_ROUTE_MISSING_CONTENT_TYPE",
                "Method 'QUERY' must provide a 'Content-Type' header.",
            )
            .into());
        }
        if body_empty {
            return Err(response::framework(
                400,
                "FST_ERR_ROUTE_MISSING_CONTENT",
                "Method 'QUERY' must provide a request body.",
            )
            .into());
        }
    }
    if file && media_type == "multipart/form-data" {
        // @fastify/multipart uses `fileSize || bodyLimit`; an explicit zero
        // therefore retains the default 1 MiB parser limit.
        return multipart(
            body,
            content_type,
            if max_file_size == 0 {
                1024 * 1024
            } else {
                max_file_size
            },
            directory,
        )
        .await;
    }
    // Fastify treats TRACE like GET/HEAD, and parses body-bearing methods
    // whenever Content-Type is present, including DELETE without CL or TE.
    if matches!(
        method,
        "DELETE" | "OPTIONS" | "PATCH" | "PUT" | "POST" | "QUERY"
    ) && (context.header("content-type").is_some() || !body_empty)
    {
        if media_type == "multipart/form-data" {
            return Ok(Input {
                params,
                upload: None,
                _temporary: None,
            });
        }
        let bytes = to_bytes(body, 1024 * 1024).await.map_err(|_| {
            response::framework(
                413,
                "FST_ERR_CTP_BODY_TOO_LARGE",
                "Request body is too large",
            )
        })?;
        let text = String::from_utf8_lossy(&bytes);
        if media_type == "application/json" {
            if bytes.is_empty() {
                return Err(response::framework(
                    400,
                    "FST_ERR_CTP_EMPTY_JSON_BODY",
                    "Body cannot be empty when content-type is set to 'application/json'",
                )
                .into());
            }
            params = Value::parse_json(text.strip_prefix('\u{feff}').unwrap_or(&text))
                .map_err(|_| invalid_json())?;
            if poisoned(&params) {
                return Err(invalid_json().into());
            }
        } else if media_type == "text/plain" {
            params = Value::string(&text);
        } else if !bytes.is_empty() || !content_type.is_empty() {
            return Err(response::framework(
                415,
                "FST_ERR_CTP_INVALID_MEDIA_TYPE",
                "Unsupported Media Type",
            )
            .into());
        }
    }
    Ok(Input {
        params,
        upload: None,
        _temporary: None,
    })
}
fn invalid_json() -> Response<Body> {
    response::framework(
        400,
        "FST_ERR_CTP_INVALID_JSON_BODY",
        "Body is not valid JSON but content-type is set to 'application/json'",
    )
}
fn poisoned(value: &Value) -> bool {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(entries) => {
                if value.get("__proto__").is_some()
                    || value
                        .get("constructor")
                        .is_some_and(|v| v.get("prototype").is_some())
                {
                    return true;
                }
                pending.extend(entries.iter().map(|(_, v)| v));
            }
            Value::Array(values) => pending.extend(values),
            _ => {}
        }
    }
    false
}
async fn multipart(
    body: Body,
    content_type: &str,
    max_file_size: u64,
    directory: &str,
) -> Result<Input, Box<Response<Body>>> {
    let boundary = multer::parse_boundary(content_type)
        .map_err(|_| response::plain(400, None, Body::empty()))?;
    let stream = body.into_data_stream();
    let mut multipart = multer::Multipart::new(stream, boundary);
    let mut input = Input {
        params: Value::Object(Vec::new()),
        upload: None,
        _temporary: None,
    };
    let mut count = 0;
    let mut first_file_size = 0;
    loop {
        let next = if input.upload.is_some() {
            // request.file() awaits the first file only. Later parts that have
            // not arrived yet must not delay authentication or the callback.
            match multipart.next_field().now_or_never() {
                Some(next) => next,
                None => return Ok(input),
            }
        } else {
            multipart.next_field().await
        };
        let mut field = match next {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(_) if input.upload.is_some() && first_file_size == 0 => return Ok(input),
            Err(_) => return Err(response::plain(400, None, Body::empty()).into()),
        };
        count += 1;
        if count > 1000 {
            if input.upload.is_some() && first_file_size == 0 {
                return Ok(input);
            }
            return Err(response::plain(400, None, Body::empty()).into());
        }
        let name = field.name().unwrap_or("").to_owned();
        let is_file = field.file_name().is_some()
            || field
                .content_type()
                .is_some_and(|kind| kind.essence_str() == "application/octet-stream");
        if is_file {
            if input.upload.is_some() {
                // A first empty file has already signalled end when Busboy's
                // subsequent file-limit cleanup destroys its current stream.
                // With unread nonempty data, finished() instead reports a
                // client stream error. Both baseline branches are covered by
                // real HTTP evidence and retained here.
                if first_file_size == 0 {
                    return Ok(input);
                }
                return Err(response::plain(400, None, Body::empty()).into());
            }
            if prototype_name(&name) {
                return Err(response::plain(400, None, Body::empty()).into());
            }
            let filename = field.file_name().map(str::to_owned);
            let filename = filename
                .map(|filename| filename.rsplit(['/', '\\']).next().unwrap_or("").to_owned());
            let temporary = tempfile::Builder::new()
                .prefix("upload-")
                .tempfile_in(PathBuf::from(directory))
                .map_err(|_| response::internal())?;
            let (file, path) = temporary.into_parts();
            let mut file = tokio::fs::File::from_std(file);
            let mut size = 0_u64;
            while let Some(bytes) = field
                .chunk()
                .await
                .map_err(|_| response::plain(400, None, Body::empty()))?
            {
                size = size.saturating_add(bytes.len() as u64);
                if size > max_file_size {
                    return Err(response::plain(413, None, Body::empty()).into());
                }
                file.write_all(&bytes)
                    .await
                    .map_err(|_| response::internal())?;
            }
            file.flush().await.map_err(|_| response::internal())?;
            drop(file);
            let path_string = path.to_str().ok_or_else(response::internal)?.to_owned();
            first_file_size = size;
            input._temporary = Some(path);
            input.upload = Some(Upload {
                name: filename,
                path: path_string,
            });
            input.params.set(&name, Value::Undefined);
        } else {
            // Busboy's default fieldSize is 1 MiB. Values are truncated before
            // AJV, and repeated multipart fields are represented as undefined.
            let is_json = field
                .content_type()
                .is_some_and(|v| v.essence_str().starts_with("application/json"));
            let mut bytes = Vec::new();
            let mut truncated = false;
            loop {
                let next = if input.upload.is_some() {
                    match field.chunk().now_or_never() {
                        Some(next) => next,
                        None => return Ok(input),
                    }
                } else {
                    field.chunk().await
                };
                let Some(chunk) = next.map_err(|_| response::plain(400, None, Body::empty()))?
                else {
                    break;
                };
                let remain = (1024 * 1024_usize).saturating_sub(bytes.len());
                truncated |= chunk.len() > remain;
                bytes.extend_from_slice(&chunk[..chunk.len().min(remain)]);
            }
            // request.file() consumes the first file, not the parts iterator.
            // The plugin records bad fields as an iterator error but omits the
            // field; a later valid file can still be processed successfully.
            if prototype_name(&name) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            let value = if is_json {
                if truncated {
                    continue;
                }
                match Value::parse_json(&text) {
                    Ok(value) if !poisoned(&value) => value,
                    _ => continue,
                }
            } else {
                Value::string(&text)
            };
            if input.params.get(&name).is_some() {
                input.params.set(&name, Value::Undefined)
            } else {
                input.params.set(&name, value)
            }
        }
    }
    if input.upload.is_none() {
        return Err(response::plain(400, None, Body::empty()).into());
    }
    Ok(input)
}
fn prototype_name(name: &str) -> bool {
    matches!(
        name,
        "constructor"
            | "__defineGetter__"
            | "__defineSetter__"
            | "hasOwnProperty"
            | "__lookupGetter__"
            | "__lookupSetter__"
            | "isPrototypeOf"
            | "propertyIsEnumerable"
            | "toString"
            | "valueOf"
            | "__proto__"
            | "toLocaleString"
    )
}
fn percent_decode(input: &str, plus: bool) -> String {
    let bytes = input.as_bytes();
    let mut result = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            )
        {
            result.push((hi * 16 + lo) as u8);
            i += 3;
            continue;
        }
        result.push(if plus && bytes[i] == b'+' {
            b' '
        } else {
            bytes[i]
        });
        i += 1;
    }
    String::from_utf8_lossy(&result).into_owned()
}
pub fn decode_path(path: &str) -> String {
    percent_decode(path, false)
}
// fast-querystring replaces '+' first and falls back to the whole component
// if decoding encounters any malformed escape or invalid UTF-8 sequence.
fn decode_query_component(input: &str) -> String {
    let replaced = input.replace('+', " ");
    let bytes = replaced.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(pair) = bytes.get(index + 1..index + 3) else {
                return replaced;
            };
            let (Some(hi), Some(lo)) = (
                (pair[0] as char).to_digit(16),
                (pair[1] as char).to_digit(16),
            ) else {
                return replaced;
            };
            decoded.push((hi * 16 + lo) as u8);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).unwrap_or(replaced)
}
fn query(query: &str) -> Value {
    let mut result = Value::Object(Vec::new());
    for part in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        let key = decode_query_component(key);
        let value = Value::string(&decode_query_component(value));
        if let Some(current) = result.get_mut(&key) {
            match current {
                Value::Array(values) => values.push(value),
                _ => {
                    let old = std::mem::replace(current, Value::Undefined);
                    *current = Value::Array(vec![old, value]);
                }
            }
        } else {
            result.set(&key, value)
        }
    }
    result
}
pub fn miauth_session(name: &str) -> Option<String> {
    name.strip_prefix("miauth/")
        .and_then(|s| s.strip_suffix("/check"))
        .filter(|s| !s.is_empty() && !s.contains('/'))
        .map(str::to_owned)
}
pub fn miauth_session_encoded(name: &str) -> Option<String> {
    let parts: Vec<_> = name.split('/').collect();
    (parts.len() == 3
        && decode_path(parts[0]) == "miauth"
        && decode_path(parts[2]) == "check"
        && !parts[1].is_empty())
    .then(|| decode_path(parts[1]))
}
pub fn special_method(name: &str, method: &str) -> bool {
    if name == "v1/instance/peers" {
        matches!(method, "GET" | "HEAD")
    } else {
        method == "POST"
    }
}
pub fn ip_hash(input: &str) -> Option<String> {
    fn parse(input: &str) -> Option<IpAddr> {
        input.parse().ok()
    }
    let address = parse(input).or_else(|| {
        input.rsplit_once(':').and_then(|(host, port)| {
            port.bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| parse(host))
                .flatten()
        })
    })?;
    let number = match address {
        IpAddr::V4(v) => u128::from(u32::from(v)),
        IpAddr::V6(v) => match v.to_ipv4() {
            Some(v) => u128::from(u32::from(v)),
            None => u128::from(v) >> 64,
        },
    };
    let mut number = number;
    let mut digits = Vec::new();
    loop {
        digits.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(number % 36) as usize]);
        number /= 36;
        if number == 0 {
            break;
        }
    }
    digits.reverse();
    Some(format!("ip-{}", String::from_utf8(digits).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_values_preserve_duplicates_and_prototypes() {
        let parsed = query("x=1&x=2&__proto__=own&a=%F0%9F%90%88");
        assert!(matches!(parsed.get("x"),Some(Value::Array(v)) if v.len()==2));
        assert_eq!(
            parsed.get("__proto__").and_then(Value::as_str).as_deref(),
            Some("own")
        );
    }
    #[test]
    fn query_matches_installed_fast_querystring_reference() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/query-reference.json")).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let input = case["input"].as_str().unwrap();
            assert_eq!(
                query(input).stringify().unwrap(),
                case["json"].as_str().unwrap(),
                "{input:?}"
            );
        }
        // Route decoding intentionally keeps its independent behavior. Public
        // malformed paths are rejected by the outer HTTP router before Rust.
        assert_eq!(decode_path("%41%ZZ"), "A%ZZ");
        assert_eq!(decode_path("plus+%2B"), "plus++");
    }
    #[test]
    fn ip_prefix_matches_mapped_ipv4() {
        assert_eq!(ip_hash("::ffff:192.0.2.1"), ip_hash("192.0.2.1"));
        assert_eq!(
            ip_hash("2001:db8:1234:5678::1"),
            ip_hash("2001:db8:1234:5678:ffff::")
        );
    }
    #[test]
    fn nested_poisoning_rejected() {
        assert!(poisoned(
            &Value::parse_json("{\"x\":[{\"__proto__\":{}}]}").unwrap()
        ));
        assert!(!poisoned(
            &Value::parse_json("{\"constructor\":\"own\"}").unwrap()
        ));
    }
}
