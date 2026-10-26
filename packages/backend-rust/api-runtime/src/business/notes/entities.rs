//! Shared value boundaries and ordered remote custom-emoji projection.
use super::super::{Error, rows};
use crate::Runtime;
use misskey_api_validation::Value;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(crate) struct EmojiCache {
    entries: HashMap<(String, String), (Instant, Option<Value>)>,
    last_cleanup: Option<Instant>,
}

impl EmojiCache {
    fn clean_expired(&mut self, now: Instant) {
        if self
            .last_cleanup
            .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(60))
        {
            self.entries.retain(|_, (stored, _)| {
                now.duration_since(*stored) < Duration::from_secs(12 * 60 * 60)
            });
            self.last_cleanup = Some(now);
        }
    }
}

pub(super) fn field<'a>(record: &'a Value, key: &str) -> Result<&'a Value, Error> {
    record.get(key).ok_or(Error::Internal)
}

pub(super) fn string(record: &Value, key: &str) -> Result<String, Error> {
    field(record, key)?.to_utf8().ok_or(Error::Internal)
}

pub(super) fn nullable_string(record: &Value, key: &str) -> Result<Option<String>, Error> {
    match field(record, key)? {
        Value::Null => Ok(None),
        Value::String(_) => Ok(Some(string(record, key)?)),
        _ => Err(Error::Internal),
    }
}

pub(super) fn boolean(record: &Value, key: &str) -> Result<bool, Error> {
    match field(record, key)? {
        Value::Boolean(value) => Ok(*value),
        _ => Err(Error::Internal),
    }
}

pub(super) fn string_array(value: &Value) -> Result<Vec<String>, Error> {
    match value {
        Value::Array(values) => values
            .iter()
            .map(|value| value.to_utf8().ok_or(Error::Internal))
            .collect(),
        // TypeORM simple-array hydration: an empty column is [], otherwise a
        // literal comma split, including empty elements between separators.
        Value::String(_) => {
            let text = value.to_utf8().ok_or(Error::Internal)?;
            Ok(if text.is_empty() {
                Vec::new()
            } else {
                text.split(',').map(str::to_owned).collect()
            })
        }
        _ => Err(Error::Internal),
    }
}

fn ascii_domain(host: &str) -> String {
    // Host strings originate in normalized DB records or the ASCII-only emoji
    // regex. WHATWG URL's host parser supplies the same UTS46 ASCII mapping.
    let Ok(url) = reqwest::Url::parse(&format!("http://{host}")) else {
        return String::new();
    };
    if url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return String::new();
    }
    url.host_str().unwrap_or("").to_owned()
}

fn parse_emoji(name: &str, author_host: Option<&str>, own_host: &str) -> Option<(String, String)> {
    let (name, host) = match name.split_once('@') {
        Some((name, host)) => (name, Some(host)),
        None => (name, None),
    };
    let word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    if name.is_empty() || !name.bytes().all(|byte| word(byte) || byte == b'-') {
        return None;
    }
    if host.is_some_and(|host| {
        host.is_empty()
            || !host
                .bytes()
                .all(|byte| word(byte) || matches!(byte, b'.' | b'-'))
    }) {
        return None;
    }
    let host = match host {
        Some(".") => return None,
        Some(host) if ascii_domain(host) == ascii_domain(own_host) => return None,
        Some(host) => host,
        None => author_host?,
    };
    Some((name.to_owned(), ascii_domain(host)))
}

pub(super) async fn populate_emojis(
    runtime: &Runtime,
    names: &[String],
    host: Option<&str>,
) -> Result<Value, Error> {
    let parsed: Vec<_> = names
        .iter()
        .map(|name| parse_emoji(name, host, &runtime.business_config.notes.host))
        .collect();
    let mut query_names = Vec::new();
    let mut query_hosts = Vec::new();
    let mut urls = HashMap::<(String, String), Option<Value>>::new();
    {
        let mut cache = runtime.business_config.notes.emoji_cache.lock().await;
        let now = Instant::now();
        cache.clean_expired(now);
        for key in parsed.iter().flatten() {
            if urls.contains_key(key) {
                continue;
            }
            if let Some((stored, value)) = cache.entries.get(key)
                && now.duration_since(*stored) < Duration::from_secs(12 * 60 * 60)
            {
                urls.insert(key.clone(), value.clone());
            } else {
                // None is also a cacheable answer; placeholder only while the
                // missing keys are resolved together below.
                urls.insert(key.clone(), None);
                query_names.push(key.0.clone());
                query_hosts.push(key.1.clone());
            }
        }
    }
    if !query_names.is_empty() {
        let records = rows(
            &runtime.database,
            true,
            "SELECT json_build_object('name', e.name, 'host', e.host, 'publicUrl', e.\"publicUrl\", 'originalUrl', e.\"originalUrl\")::text FROM emoji e WHERE (e.name, e.host) IN (SELECT * FROM unnest($1::varchar[], $2::varchar[]))",
            &[&query_names, &query_hosts],
        )
        .await?;
        for record in records {
            let url = if field(&record, "publicUrl")?.truthy() {
                field(&record, "publicUrl")?
            } else {
                field(&record, "originalUrl")?
            };
            if !matches!(url, Value::String(_)) {
                return Err(Error::Internal);
            }
            urls.insert(
                (string(&record, "name")?, string(&record, "host")?),
                Some(url.clone()),
            );
        }
        let mut cache = runtime.business_config.notes.emoji_cache.lock().await;
        let now = Instant::now();
        for (name, host) in query_names.into_iter().zip(query_hosts) {
            let key = (name, host);
            cache
                .entries
                .insert(key.clone(), (now, urls.get(&key).cloned().flatten()));
        }
    }
    let mut result = Value::object(std::iter::empty::<(&str, Value)>());
    for (original, parsed) in names.iter().zip(parsed.iter()) {
        // TS assigns to a normal {}, whose legacy __proto__ setter ignores a
        // string URL. It does not create an own response property for this key.
        if original != "__proto__"
            && let Some(url) = parsed
                .as_ref()
                .and_then(|key| urls.get(key))
                .and_then(Option::as_ref)
        {
            result.set(original, url.clone());
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_emoji_names_keep_case_and_resolve_host_markers() {
        assert_eq!(
            parse_emoji("Smile", Some("REMOTE.EXAMPLE"), "local.example"),
            Some(("Smile".into(), "remote.example".into()))
        );
        assert_eq!(
            parse_emoji("Smile@other.example", None, "local.example"),
            Some(("Smile".into(), "other.example".into()))
        );
        for name in [
            "Smile",
            "Smile@.",
            "Smile@LOCAL.EXAMPLE",
            ":Smile:",
            "Smile@",
            "Smile@host:80",
            "笑",
            "Smile@host/path",
        ] {
            assert!(parse_emoji(name, None, "local.example").is_none(), "{name}");
        }
        assert_eq!(
            parse_emoji("a@localhost", None, "localhost:3000"),
            Some(("a".into(), "localhost".into()))
        );
    }

    #[test]
    fn simple_array_hydration_preserves_empty_members() {
        assert_eq!(
            string_array(&Value::string("")).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            string_array(&Value::string("a,,b,")).unwrap(),
            vec!["a", "", "b", ""]
        );
        assert!(string_array(&Value::Array(vec![Value::Null])).is_err());
    }
}
