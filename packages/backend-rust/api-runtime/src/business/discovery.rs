//! Native database reads for emoji, hashtag and account discovery endpoints.

use super::{Error, number, rows};
use crate::Runtime;
use misskey_api_validation::Value;
use misskey_backend_api_auth::Context;

mod strings;

pub(super) async fn execute(
    runtime: &Runtime,
    name: &str,
    params: &Value,
    _auth: &Context,
) -> Option<Result<Value, Error>> {
    Some(match name {
        "emoji" => emoji(runtime, params).await,
        "emojis" => emojis(runtime).await,
        "hashtags/list" => hashtag_list(runtime, params).await,
        "hashtags/search" => hashtag_search(runtime, params).await,
        "hashtags/show" => hashtag_show(runtime, params).await,
        "username/available" => username_available(runtime, params).await,
        "retention" => retention(runtime).await,
        _ => return None,
    })
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, Error> {
    value.get(key).ok_or(Error::Internal)
}

/// pg encodes a JS string as UTF-8, replacing each unpaired surrogate.
fn pg_string(value: &Value, key: &str) -> Result<String, Error> {
    match field(value, key)? {
        Value::String(units) => Ok(String::from_utf16_lossy(units)),
        _ => Err(Error::Internal),
    }
}

fn copy(value: &Value, key: &str) -> Result<Value, Error> {
    Ok(field(value, key)?.clone())
}

fn pack_emoji(emoji: &Value, detailed: bool) -> Result<Value, Error> {
    let url = if field(emoji, "publicUrl")?.truthy() {
        copy(emoji, "publicUrl")?
    } else {
        copy(emoji, "originalUrl")?
    };
    let mut fields = Vec::new();
    if detailed {
        fields.push(("id", copy(emoji, "id")?));
    }
    for key in ["aliases", "name", "category"] {
        fields.push((key, copy(emoji, key)?));
    }
    if detailed {
        fields.push(("host", copy(emoji, "host")?));
    }
    fields.push(("url", url));
    if detailed {
        fields.push(("license", copy(emoji, "license")?));
        fields.push(("isSensitive", copy(emoji, "isSensitive")?));
        fields.push(("localOnly", copy(emoji, "localOnly")?));
        fields.push((
            "roleIdsThatCanBeUsedThisEmojiAsReaction",
            copy(emoji, "roleIdsThatCanBeUsedThisEmojiAsReaction")?,
        ));
    } else {
        for key in ["localOnly", "isSensitive"] {
            fields.push((
                key,
                if field(emoji, key)?.truthy() {
                    Value::Boolean(true)
                } else {
                    Value::Undefined
                },
            ));
        }
        let roles = field(emoji, "roleIdsThatCanBeUsedThisEmojiAsReaction")?;
        let Value::Array(values) = roles else {
            return Err(Error::Internal);
        };
        fields.push((
            "roleIdsThatCanBeUsedThisEmojiAsReaction",
            if values.is_empty() {
                Value::Undefined
            } else {
                roles.clone()
            },
        ));
    }
    Ok(Value::object(fields))
}

async fn emoji(runtime: &Runtime, params: &Value) -> Result<Value, Error> {
    let name = pg_string(params, "name")?;
    let records = rows(
        &runtime.database,
        true,
        "SELECT row_to_json(e)::text FROM emoji e WHERE e.name = $1 AND e.host IS NULL LIMIT 1",
        &[&name],
    )
    .await?;
    match records.first() {
        Some(record) => pack_emoji(record, true),
        None => Err(Error::legacy(
            emoji_not_found_message(field(params, "name")?)?,
            "EntityNotFoundError",
        )),
    }
}

/// EntityNotFoundError includes JSON.stringify(criteria, null, 4). The name is
/// the original JS string, before pg's replacement of unpaired surrogates.
fn emoji_not_found_message(name: &Value) -> Result<String, Error> {
    if !matches!(name, Value::String(_)) {
        return Err(Error::Internal);
    }
    let name = name.stringify().ok_or(Error::Internal)?;
    let before = "Could not find any entity of type \"MiEmoji\" matching: {\n    \"where\": {\n        \"name\": ";
    let after = ",\n        \"host\": {\n            \"_type\": \"isNull\",\n            \"_useParameter\": false,\n            \"_multipleParameters\": false\n        }\n    }\n}";
    Ok(format!("{before}{name}{after}"))
}

async fn emojis(runtime: &Runtime) -> Result<Value, Error> {
    let records = rows(
        &runtime.database,
        true,
        "SELECT row_to_json(e)::text FROM emoji e WHERE e.host IS NULL ORDER BY e.category ASC, e.name ASC",
        &[],
    )
    .await?;
    Ok(Value::object([(
        "emojis",
        Value::Array(
            records
                .iter()
                .map(|record| pack_emoji(record, false))
                .collect::<Result<_, _>>()?,
        ),
    )]))
}

fn pack_hashtag(record: &Value) -> Result<Value, Error> {
    let mut fields = vec![("tag", copy(record, "name")?)];
    for key in [
        "mentionedUsersCount",
        "mentionedLocalUsersCount",
        "mentionedRemoteUsersCount",
        "attachedUsersCount",
        "attachedLocalUsersCount",
        "attachedRemoteUsersCount",
    ] {
        fields.push((key, copy(record, key)?));
    }
    Ok(Value::object(fields))
}

async fn hashtag_list(runtime: &Runtime, params: &Value) -> Result<Value, Error> {
    let sort = pg_string(params, "sort")?;
    let (direction, field) = sort.split_at_checked(1).ok_or(Error::Internal)?;
    let column = match field {
        "mentionedUsers" => "mentionedUsersCount",
        "mentionedLocalUsers" => "mentionedLocalUsersCount",
        "mentionedRemoteUsers" => "mentionedRemoteUsersCount",
        "attachedUsers" => "attachedUsersCount",
        "attachedLocalUsers" => "attachedLocalUsersCount",
        "attachedRemoteUsers" => "attachedRemoteUsersCount",
        _ => return Err(Error::Internal),
    };
    let order = match direction {
        "+" => "DESC",
        "-" => "ASC",
        _ => return Err(Error::Internal),
    };
    let mut conditions = vec!["TRUE"];
    for (param, condition) in [
        ("attachedToUserOnly", "h.\"attachedUsersCount\" != 0"),
        (
            "attachedToLocalUserOnly",
            "h.\"attachedLocalUsersCount\" != 0",
        ),
        (
            "attachedToRemoteUserOnly",
            "h.\"attachedRemoteUsersCount\" != 0",
        ),
    ] {
        if params.get(param).is_some_and(Value::truthy) {
            conditions.push(condition);
        }
    }
    let limit = number(params, "limit")? as i64;
    let sql = format!(
        "SELECT json_build_object('name', h.name, 'mentionedUsersCount', h.\"mentionedUsersCount\", 'mentionedLocalUsersCount', h.\"mentionedLocalUsersCount\", 'mentionedRemoteUsersCount', h.\"mentionedRemoteUsersCount\", 'attachedUsersCount', h.\"attachedUsersCount\", 'attachedLocalUsersCount', h.\"attachedLocalUsersCount\", 'attachedRemoteUsersCount', h.\"attachedRemoteUsersCount\")::text FROM hashtag h WHERE {} ORDER BY h.\"{column}\" {order} LIMIT $1",
        conditions.join(" AND "),
    );
    let records = rows(&runtime.database, true, &sql, &[&limit]).await?;
    Ok(Value::Array(
        records.iter().map(pack_hashtag).collect::<Result<_, _>>()?,
    ))
}

async fn hashtag_search(runtime: &Runtime, params: &Value) -> Result<Value, Error> {
    let Value::String(query) = field(params, "query")? else {
        return Err(Error::Internal);
    };
    let query = String::from_utf16_lossy(&strings::lowercase(query));
    let query = format!("{}%", strings::like_escape(&query));
    let limit = number(params, "limit")? as i64;
    let offset = number(params, "offset")?;
    let records = rows(
        &runtime.database,
        true,
        "SELECT row_to_json(h)::text FROM hashtag h WHERE h.name LIKE $1 GROUP BY h.id ORDER BY h.\"mentionedLocalUsersCount\" DESC LIMIT $2 OFFSET $3::float8::bigint",
        &[&query, &limit, &offset],
    )
    .await?;
    Ok(Value::Array(
        records
            .iter()
            .map(|record| copy(record, "name"))
            .collect::<Result<_, _>>()?,
    ))
}

async fn hashtag_show(runtime: &Runtime, params: &Value) -> Result<Value, Error> {
    let Value::String(tag) = field(params, "tag")? else {
        return Err(Error::Internal);
    };
    let tag = String::from_utf16_lossy(&strings::normalize_for_search(tag));
    let records = rows(
        &runtime.database,
        true,
        "SELECT row_to_json(h)::text FROM hashtag h WHERE h.name = $1 LIMIT 1",
        &[&tag],
    )
    .await?;
    pack_hashtag(records.first().ok_or(Error::Api {
        message: "No such hashtag.",
        code: "NO_SUCH_HASHTAG",
        id: "110ee688-193e-4a3a-9ecf-c167b2e6981e",
        kind: "client",
        status: None,
        info: None,
    })?)
}

async fn username_available(runtime: &Runtime, params: &Value) -> Result<Value, Error> {
    // localUsernameSchema accepts ASCII word characters only.
    let username = pg_string(params, "username")?.to_ascii_lowercase();
    let exist = rows(
        &runtime.database,
        true,
        "SELECT json_build_object('count', count(*))::text FROM \"user\" u WHERE u.host IS NULL AND u.\"usernameLower\" = $1",
        &[&username],
    )
    .await?;
    let exist2 = rows(
        &runtime.database,
        true,
        "SELECT json_build_object('count', count(*))::text FROM used_username u WHERE u.username = $1",
        &[&username],
    )
    .await?;
    let meta = runtime.auth.meta().await;
    let preserved = meta
        .get("preservedUsernames")
        .and_then(serde_json::Value::as_array)
        .ok_or(Error::Internal)?;
    // TS maps the complete list before includes; do not short circuit malformed
    // entries merely because an earlier preserved username already matched.
    let preserved = preserved
        .iter()
        .map(|value| value.as_str().map(str::to_lowercase).ok_or(Error::Internal))
        .collect::<Result<Vec<_>, _>>()?;
    let available = number(exist.first().ok_or(Error::Internal)?, "count")? == 0.0
        && number(exist2.first().ok_or(Error::Internal)?, "count")? == 0.0
        && !preserved.contains(&username);
    Ok(Value::object([("available", Value::Boolean(available))]))
}

async fn retention(runtime: &Runtime) -> Result<Value, Error> {
    let records = rows(
        &runtime.database,
        true,
        "SELECT json_build_object('createdAt', floor(extract(epoch FROM r.\"createdAt\") * 1000)::float8, 'users', r.\"usersCount\", 'data', r.data)::text FROM retention_aggregation r ORDER BY r.id DESC LIMIT 30",
        &[],
    )
    .await?;
    Ok(Value::Array(
        records
            .iter()
            .map(|record| {
                Ok(Value::object([
                    (
                        "createdAt",
                        Value::string(&super::iso_date(number(record, "createdAt")?)?),
                    ),
                    ("users", copy(record, "users")?),
                    ("data", copy(record, "data")?),
                ]))
            })
            .collect::<Result<_, Error>>()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        Value::parse_json(
            r#"{"id":"e1","name":"emoji","aliases":["astral😀"],"category":null,"host":null,"originalUrl":"original","publicUrl":"","license":null,"isSensitive":false,"localOnly":false,"roleIdsThatCanBeUsedThisEmojiAsReaction":[]}"#,
        )
        .unwrap()
    }

    #[test]
    fn simple_emoji_preserves_undefined_and_packed_frontend_contract() {
        let packed = pack_emoji(&fixture(), false).ok().unwrap();
        assert!(matches!(packed.get("localOnly"), Some(Value::Undefined)));
        assert!(matches!(packed.get("isSensitive"), Some(Value::Undefined)));
        assert!(matches!(
            packed.get("roleIdsThatCanBeUsedThisEmojiAsReaction"),
            Some(Value::Undefined)
        ));
        assert!(matches!(packed.get("category"), Some(Value::Null)));
        assert_eq!(
            packed.stringify().as_deref(),
            Some(r#"{"aliases":["astral😀"],"name":"emoji","category":null,"url":"original"}"#),
        );
    }

    #[test]
    fn detailed_emoji_preserves_false_null_and_property_order() {
        let mut fixture = fixture();
        fixture.set("publicUrl", Value::string("public"));
        let packed = pack_emoji(&fixture, true).ok().unwrap();
        assert_eq!(
            packed.stringify().as_deref(),
            Some(
                r#"{"id":"e1","aliases":["astral😀"],"name":"emoji","category":null,"host":null,"url":"public","license":null,"isSensitive":false,"localOnly":false,"roleIdsThatCanBeUsedThisEmojiAsReaction":[]}"#
            ),
        );
    }

    #[test]
    fn pg_encoding_replaces_unpaired_surrogates_without_changing_scalar_pairs() {
        let value = Value::object([("text", Value::String(vec![0xd800, 0x41, 0xd83d, 0xde00]))]);
        assert_eq!(pg_string(&value, "text").ok().as_deref(), Some("�A😀"));
    }

    #[test]
    fn missing_emoji_message_matches_typeorm_pretty_criteria() {
        for name in ["missing", "Alpha"] {
            assert_eq!(
                emoji_not_found_message(&Value::string(name)).unwrap(),
                format!(
                    "Could not find any entity of type \"MiEmoji\" matching: {{\n    \"where\": {{\n        \"name\": \"{name}\",\n        \"host\": {{\n            \"_type\": \"isNull\",\n            \"_useParameter\": false,\n            \"_multipleParameters\": false\n        }}\n    }}\n}}"
                ),
            );
        }
    }

    #[test]
    fn missing_emoji_message_retains_lone_surrogates_and_json_string_escaping() {
        for (name, expected) in [
            (Value::String(vec![0xd800]), r#""\ud800""#),
            (Value::String(vec![0xdc00]), r#""\udc00""#),
            (
                Value::string("line\n\"quote\\end\t\r\0"),
                r#""line\n\"quote\\end\t\r\u0000""#,
            ),
            (Value::string("astral😀"), "\"astral😀\""),
        ] {
            let message = emoji_not_found_message(&name).unwrap();
            assert_eq!(
                message.lines().nth(2).unwrap(),
                format!("        \"name\": {expected},"),
            );
        }
    }
}
