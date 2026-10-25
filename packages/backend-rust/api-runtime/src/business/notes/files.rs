//! Public attachment packing used by notes. No folder or owner-detail lookup.
use super::super::{Error, iso_date, parse_id_date, rows};
use super::{
    Config,
    entities::{boolean, field, nullable_string, string},
};
use crate::Runtime;
use misskey_api_validation::Value;
use std::collections::HashMap;

fn encode_uri_component(text: &str) -> String {
    let mut result = String::new();
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            )
        {
            result.push(char::from(byte));
        } else {
            result.push('%');
            result.push(char::from(HEX[usize::from(byte >> 4)]));
            result.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    result
}

fn append_query(url: &str, query: &str) -> String {
    let separator = if url.contains('?') {
        if url.ends_with('?') { "" } else { "&" }
    } else {
        "?"
    };
    format!("{url}{separator}{query}")
}

fn proxied_url(config: &Config, url: &str, mode: Option<&str>) -> String {
    let mut query = format!("url={}", encode_uri_component(url));
    if let Some(mode) = mode {
        query.push_str(&format!("&{mode}=1"));
    }
    append_query(
        &format!("{}/{}.webp", config.media_proxy, mode.unwrap_or("image")),
        &query,
    )
}

fn public_url(config: &Config, file: &Value, proxy_remote_files: bool) -> Result<String, Error> {
    let uri = nullable_string(file, "uri")?;
    let remote = nullable_string(file, "userHost")?.is_some();
    if let Some(ref uri) = uri {
        if remote && config.external_media_proxy_enabled {
            return Ok(proxied_url(config, uri, None));
        }
        if boolean(file, "isLink")?
            && proxy_remote_files
            && let Some(key) = nullable_string(file, "webpublicAccessKey")?
            && !key.is_empty()
            && !key.contains('/')
        {
            return Ok(format!("{}/files/{key}", config.url));
        }
    }
    Ok(nullable_string(file, "webpublicUrl")?.unwrap_or(string(file, "url")?))
}

fn thumbnail_url(config: &Config, file: &Value, proxy_remote_files: bool) -> Result<Value, Error> {
    let mime = string(file, "type")?;
    let thumbnail = nullable_string(file, "thumbnailUrl")?;
    if mime.starts_with("video") {
        if let Some(ref thumbnail) = thumbnail
            && !thumbnail.is_empty()
        {
            return Ok(Value::string(thumbnail));
        }
        let Some(generator) = &config.video_thumbnail_generator else {
            return Ok(Value::Null);
        };
        let url = nullable_string(file, "webpublicUrl")?.unwrap_or(string(file, "url")?);
        return Ok(Value::string(&append_query(
            &format!("{generator}/thumbnail.webp"),
            &format!("thumbnail=1&url={}", encode_uri_component(&url)),
        )));
    }
    if let Some(uri) = nullable_string(file, "uri")?
        && (nullable_string(file, "userHost")?.is_some() && config.external_media_proxy_enabled
            || boolean(file, "isLink")? && proxy_remote_files)
    {
        return Ok(Value::string(&proxied_url(config, &uri, Some("static"))));
    }
    if let Some(thumbnail) = thumbnail {
        return Ok(Value::string(&thumbnail));
    }
    if matches!(
        mime.as_str(),
        "image/jpeg"
            | "image/tiff"
            | "image/png"
            | "image/gif"
            | "image/apng"
            | "image/vnd.mozilla.apng"
            | "image/webp"
            | "image/avif"
            | "image/svg+xml"
    ) {
        return Ok(Value::string(
            &nullable_string(file, "webpublicUrl")?.unwrap_or(string(file, "url")?),
        ));
    }
    Ok(Value::Null)
}

fn public_properties(properties: &Value) -> Result<Value, Error> {
    if !matches!(properties, Value::Object(_)) {
        return Err(Error::Internal);
    }
    let Some(orientation) = properties
        .get("orientation")
        .filter(|value| !matches!(value, Value::Null | Value::Undefined))
    else {
        return Ok(properties.clone());
    };
    let mut packed = properties.clone();
    // The stored typed orientation is numeric; retain JS relational coercion
    // for older JSON records whose orientation was stored as a numeric string.
    let orientation_number = orientation.as_number().or_else(|| {
        orientation
            .to_utf8()
            .and_then(|text| text.trim().parse::<f64>().ok())
    });
    if orientation_number.is_some_and(|orientation| orientation >= 5.0) {
        packed.set(
            "width",
            properties
                .get("height")
                .cloned()
                .unwrap_or(Value::Undefined),
        );
        packed.set(
            "height",
            properties.get("width").cloned().unwrap_or(Value::Undefined),
        );
    }
    packed.set("orientation", Value::Undefined);
    Ok(packed)
}

fn pack_file(
    config: &Config,
    id_method: &str,
    file: &Value,
    proxy_remote_files: bool,
) -> Result<Value, Error> {
    let id = string(file, "id")?;
    let date = parse_id_date(id_method, &id).ok_or(Error::Internal)?;
    let mut packed = Value::object([
        ("id", Value::string(&id)),
        ("createdAt", Value::string(&iso_date(date)?)),
    ]);
    for key in ["name", "type", "md5", "size", "isSensitive", "blurhash"] {
        packed.set(key, field(file, key)?.clone());
    }
    packed.set("properties", public_properties(field(file, "properties")?)?);
    packed.set(
        "url",
        Value::string(&public_url(config, file, proxy_remote_files)?),
    );
    packed.set(
        "thumbnailUrl",
        thumbnail_url(config, file, proxy_remote_files)?,
    );
    for key in ["comment", "folderId"] {
        packed.set(key, field(file, key)?.clone());
    }
    packed.set("folder", Value::Null);
    // packManyByIds -> packNullable retains the true owner id even when
    // withUser is omitted; direct pack() has a different default.
    packed.set("userId", field(file, "userId")?.clone());
    packed.set("user", Value::Null);
    Ok(packed)
}

pub(super) async fn pack_files(runtime: &Runtime, ids: &[String]) -> Result<Value, Error> {
    let query_ids: Vec<_> = ids.iter().map(|id| Some(id.as_str())).collect();
    let packed = pack_files_map(runtime, &query_ids).await?;
    Ok(Value::Array(
        ids.iter()
            .filter_map(|id| packed.get(id).cloned())
            .collect(),
    ))
}

pub(super) async fn pack_files_map(
    runtime: &Runtime,
    ids: &[Option<&str>],
) -> Result<HashMap<String, Value>, Error> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let files = rows(
        &runtime.database,
        true,
        "SELECT row_to_json(f)::text FROM drive_file f WHERE f.id = ANY($1::varchar[])",
        &[&ids],
    )
    .await?;
    let meta = runtime.auth.meta().await;
    let proxy_remote_files = meta
        .get("proxyRemoteFiles")
        .and_then(serde_json::Value::as_bool)
        .ok_or(Error::Internal)?;
    let mut packed = HashMap::new();
    for file in files {
        let id = string(&file, "id")?;
        packed.insert(
            id,
            pack_file(
                &runtime.business_config.notes,
                &runtime.business_config.id_method,
                &file,
                proxy_remote_files,
            )?,
        );
    }
    Ok(packed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_encoding_matches_encode_uri_component_without_form_plus_conversion() {
        assert_eq!(
            encode_uri_component("https://a.example/a b?x=雪&y=!'()*~"),
            "https%3A%2F%2Fa.example%2Fa%20b%3Fx%3D%E9%9B%AA%26y%3D!'()*~"
        );
        assert_eq!(
            append_query("https://a.example?", "url=x"),
            "https://a.example?url=x"
        );
        assert_eq!(
            append_query("https://a.example?x=1", "url=x"),
            "https://a.example?x=1&url=x"
        );
    }

    #[test]
    fn image_orientation_is_removed_and_missing_dimensions_remain_omitted() {
        let raw =
            Value::from_json(&serde_json::json!({"width":30,"orientation":6,"avgColor":"red"}));
        let packed = public_properties(&raw).unwrap();
        assert!(matches!(packed.get("width"), Some(Value::Undefined)));
        assert_eq!(packed.get("height"), Some(&Value::Number(30.0)));
        assert!(matches!(packed.get("orientation"), Some(Value::Undefined)));
        assert_eq!(raw.get("orientation"), Some(&Value::Number(6.0)));
        let null_orientation =
            Value::from_json(&serde_json::json!({"orientation":null,"width":30}));
        assert_eq!(
            public_properties(&null_orientation).unwrap(),
            null_orientation
        );
    }
    fn fixture() -> Value {
        Value::from_json(&serde_json::json!({
            "id":"00000000abcdef", "userId":"owner", "userHost":null,
            "name":"a.png", "type":"image/png", "md5":"hash", "size":12,
            "isSensitive":false, "blurhash":null,"properties":{"width":100,"height":50},
            "url":"https://local.example/original.png", "webpublicUrl":null,"thumbnailUrl":null,
            "comment":null,"folderId":"folder", "uri":null,"isLink":false,"webpublicAccessKey":null,
            "requestHeaders":{"secret":"not public"}
        }))
    }

    #[test]
    fn attachment_uses_nullable_pack_owner_and_excludes_private_columns() {
        let config = Config::default();
        let packed = pack_file(&config, "aidx", &fixture(), false).unwrap();
        assert_eq!(packed.get("userId"), Some(&Value::string("owner")));
        assert_eq!(packed.get("user"), Some(&Value::Null));
        assert_eq!(packed.get("folderId"), Some(&Value::string("folder")));
        assert_eq!(packed.get("folder"), Some(&Value::Null));
        assert_eq!(
            packed.get("thumbnailUrl"),
            Some(&Value::string("https://local.example/original.png"))
        );
        assert!(packed.get("requestHeaders").is_none());
        assert!(packed.get("webpublicAccessKey").is_none());
    }

    #[test]
    fn remote_file_and_video_url_modes_follow_existing_precedence() {
        let mut config = Config {
            url: "https://local.example".into(),
            media_proxy: "https://proxy.example".into(),
            external_media_proxy_enabled: true,
            video_thumbnail_generator: Some("https://video.example".into()),
            ..Default::default()
        };
        let mut file = fixture();
        file.set("uri", Value::string("https://remote.example/a b.png"));
        file.set("userHost", Value::string("remote.example"));
        assert_eq!(
            public_url(&config, &file, false).unwrap(),
            "https://proxy.example/image.webp?url=https%3A%2F%2Fremote.example%2Fa%20b.png"
        );
        assert_eq!(
            thumbnail_url(&config, &file, false).unwrap(),
            Value::string(
                "https://proxy.example/static.webp?url=https%3A%2F%2Fremote.example%2Fa%20b.png&static=1"
            )
        );
        file.set("type", Value::string("video/mp4"));
        assert_eq!(
            thumbnail_url(&config, &file, false).unwrap(),
            Value::string(
                "https://video.example/thumbnail.webp?thumbnail=1&url=https%3A%2F%2Flocal.example%2Foriginal.png"
            )
        );
        config.external_media_proxy_enabled = false;
        file.set("isLink", Value::Boolean(true));
        file.set("webpublicAccessKey", Value::string("key"));
        assert_eq!(
            public_url(&config, &file, true).unwrap(),
            "https://local.example/files/key"
        );
        file.set("webpublicAccessKey", Value::string("old/key"));
        assert_eq!(
            public_url(&config, &file, true).unwrap(),
            "https://local.example/original.png"
        );
    }
}
