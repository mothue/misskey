//! UserLite projection for complete native note reads.
use super::super::{
    Error,
    eager_batch::{PackFailure, eager_batch},
    entity_layout::{LayoutFailure, column_name},
    rows,
};
use super::entities::{boolean, field, nullable_string, populate_emojis, string, string_array};
use crate::Runtime;
use misskey_api_validation::Value;
use misskey_backend_api_auth::{
    ColumnKind, Context, EntityColumn,
    postgres::{quote_identifier, quote_literal},
};
use std::collections::HashMap;
use tokio_postgres::types::ToSql;

// The existing loaded UserLite shape, shared by ID loads and joined entities.
const USER_LITE_PROPERTIES: &[&str] = &[
    "id",
    "name",
    "username",
    "host",
    "avatarId",
    "avatarUrl",
    "avatarBlurhash",
    "avatarDecorations",
    "isBot",
    "isCat",
    "requireSigninToViewContents",
    "makeNotesFollowersOnlyBefore",
    "makeNotesHiddenBefore",
    "emojis",
    "hideOnlineStatus",
    "lastActiveDate",
];

pub(crate) fn user_lite_projection(
    user_columns: &[EntityColumn],
    alias: &str,
) -> Result<String, LayoutFailure> {
    if alias.is_empty() || alias.contains('\0') {
        return Err(LayoutFailure::InvalidIdentifier);
    }
    let alias = quote_identifier(alias);
    let members = USER_LITE_PROPERTIES
        .iter()
        .map(|property| {
            let mut matching = user_columns
                .iter()
                .filter(|column| column.property_name == *property);
            let column = matching.next().ok_or(LayoutFailure::MissingProperty)?;
            if matching.next().is_some() {
                return Err(LayoutFailure::DuplicateProperty);
            }
            if column.database_name.is_empty() || column.database_name.contains('\0') {
                return Err(LayoutFailure::InvalidIdentifier);
            }
            let expected = if *property == "lastActiveDate" {
                ColumnKind::Date
            } else {
                ColumnKind::Json
            };
            if column.kind != expected {
                return Err(LayoutFailure::HydrationKind);
            }
            let reference = format!("{alias}.{}", column_name(user_columns, property)?);
            let value = if *property == "lastActiveDate" {
                // pg's Date hydration truncates stored submilliseconds.
                format!("floor(extract(epoch FROM {reference})*1000)::double precision")
            } else {
                reference
            };
            Ok(format!("{},{value}", quote_literal(property)))
        })
        .collect::<Result<Vec<_>, LayoutFailure>>()?;
    Ok(format!("json_build_object({})", members.join(",")))
}

async fn load_user_rows(runtime: &Runtime, ids: &[String]) -> Result<Vec<Value>, Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let slots = (1..=ids.len())
        .map(|index| format!("${index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT {}::text FROM \"user\" u WHERE u.id IN ({slots})",
        runtime.user_lite_projection,
    );
    let params: Vec<&(dyn ToSql + Sync)> =
        ids.iter().map(|id| -> &(dyn ToSql + Sync) { id }).collect();
    rows(&runtime.database, true, &sql, &params).await
}

/// UserEntityService.packMany's object path retains snapshots and duplicates.
pub(in crate::business) async fn pack_loaded_user_lites(
    runtime: &Runtime,
    users: &[Value],
    auth: &Context,
) -> Result<Vec<Value>, Error> {
    // packMany creates every pack promise before Promise.all can reject. All
    // pack_user errors are promise rejections, without a preparation phase.
    eager_batch(users.iter().map(|user| async move {
        pack_user(runtime, user, auth)
            .await
            .map_err(PackFailure::Async)
    }))
    .await
}

/// UserEntityService.packMany's ID path packs the completed database row order,
/// omitting missing IDs, rather than rebuilding the input ID sequence.
pub(in crate::business) async fn pack_id_user_lites(
    runtime: &Runtime,
    ids: &[String],
    auth: &Context,
) -> Result<Vec<Value>, Error> {
    let users = load_user_rows(runtime, ids).await?;
    pack_loaded_user_lites(runtime, &users, auth).await
}

/// Batch the same selected UserLite fields, then reuse the complete formatter.
/// IN naturally deduplicates IDs; callers retain their own result sequence.
pub(in crate::business) async fn pack_many_users(
    runtime: &Runtime,
    ids: &[String],
    auth: &Context,
) -> Result<HashMap<String, Value>, Error> {
    let users = load_user_rows(runtime, ids).await?;
    Ok(
        futures_util::future::try_join_all(users.iter().map(|user| async move {
            Ok::<_, Error>((string(user, "id")?, pack_user(runtime, user, auth).await?))
        }))
        .await?
        .into_iter()
        .collect(),
    )
}

/// Clip.pack retries an author missing from its batch hint without a viewer.
pub(super) async fn pack_user_by_id(
    runtime: &Runtime,
    id: &str,
    auth: &Context,
) -> Result<Value, Error> {
    let users = load_user_rows(runtime, &[id.to_owned()]).await?;
    let Some(user) = users.first() else {
        let encoded = serde_json::to_string(id).map_err(|_| Error::Internal)?;
        return Err(Error::legacy(
            format!(
                "Could not find any entity of type \"MiUser\" matching: {{\n    \"id\": {encoded}\n}}"
            ),
            "EntityNotFoundError",
        ));
    };
    pack_user(runtime, user, auth).await
}

fn online_status(user: &Value, now: i64) -> Result<&'static str, Error> {
    if boolean(user, "hideOnlineStatus")? {
        return Ok("unknown");
    }
    let last_active = match field(user, "lastActiveDate")? {
        Value::Null => return Ok("unknown"),
        Value::Number(date) => *date,
        _ => return Err(Error::Internal),
    };
    let elapsed = now as f64 - last_active;
    Ok(if elapsed < 600_000.0 {
        "online"
    } else if elapsed < 259_200_000.0 {
        "active"
    } else {
        "offline"
    })
}

fn identicon_url(
    config: &super::Config,
    user: &Value,
    meta: &serde_json::Value,
) -> Result<Value, Error> {
    let username = string(user, "username")?;
    let host = nullable_string(user, "host")?;
    if (host.is_none() || host.as_deref() == Some(config.host.as_str()))
        && username.contains('.')
        && let Some(icon) = meta
            .get("iconUrl")
            .and_then(serde_json::Value::as_str)
            .filter(|icon| !icon.is_empty())
    {
        return Ok(Value::string(icon));
    }
    Ok(Value::string(&format!(
        "{}/identicon/{}@{}",
        config.url,
        username.to_lowercase(),
        host.as_deref().unwrap_or(&config.host)
    )))
}

async fn moderator(runtime: &Runtime, auth: &Context, now: i64) -> Result<bool, Error> {
    let Some(user) = &auth.user else {
        return Ok(false);
    };
    let id = user
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or(Error::Internal)?;
    if runtime.auth.root_user_id().await.as_deref() == Some(id) {
        return Ok(true);
    }
    let roles = runtime
        .auth
        .roles_for(id, now)
        .await
        .map_err(|_| Error::Internal)?;
    Ok(roles.iter().any(|role| {
        role.get("isModerator").and_then(serde_json::Value::as_bool) == Some(true)
            || role
                .get("isAdministrator")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
    }))
}

async fn badge_roles(
    runtime: &Runtime,
    user_id: &str,
    viewer_moderator: bool,
    now: i64,
) -> Result<Value, Error> {
    // Filtering the shared role result yields the same assigned+conditional
    // badge list, including deliberately repeated assigned conditional roles.
    let roles = runtime
        .auth
        .roles_for(user_id, now)
        .await
        .map_err(|_| Error::Internal)?;
    let mut badges = Vec::new();
    for role in roles {
        if role.get("asBadge").and_then(serde_json::Value::as_bool) != Some(true)
            || !(viewer_moderator
                || role.get("isPublic").and_then(serde_json::Value::as_bool) == Some(true))
        {
            continue;
        }
        let order = role
            .get("displayOrder")
            .and_then(serde_json::Value::as_f64)
            .ok_or(Error::Internal)?;
        let name = role
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or(Error::Internal)?;
        let icon = match role.get("iconUrl") {
            Some(serde_json::Value::Null) => Value::Null,
            Some(serde_json::Value::String(url)) => Value::string(url),
            _ => return Err(Error::Internal),
        };
        badges.push((
            order,
            Value::object([
                ("name", Value::string(name)),
                ("iconUrl", icon),
                ("displayOrder", Value::Number(order)),
            ]),
        ));
    }
    // sort_by is stable, like Array.sort; ties preserve role-cache order.
    badges.sort_by(|a, b| b.0.total_cmp(&a.0));
    Ok(Value::Array(
        badges.into_iter().map(|(_, value)| value).collect(),
    ))
}

async fn decorations(runtime: &Runtime, user: &Value) -> Result<Value, Error> {
    let Value::Array(user_decorations) = field(user, "avatarDecorations")? else {
        return Err(Error::Internal);
    };
    if user_decorations.is_empty() {
        return Ok(Value::Array(Vec::new()));
    }
    let available = runtime
        .redis
        .avatar_decorations(&runtime.database)
        .await
        .map_err(|_| Error::Internal)?;
    let Value::Array(ref available) = available else {
        return Err(Error::Internal);
    };
    pack_decorations(user_decorations, available)
}

fn pack_decorations(user_decorations: &[Value], available: &[Value]) -> Result<Value, Error> {
    // Array.some never reads a member's id when the inventory is empty.
    if available.is_empty() {
        return Ok(Value::Array(Vec::new()));
    }
    let mut result = Vec::new();
    for decoration in user_decorations {
        let definition = match decoration {
            Value::Null => Some(&crate::response::NULL_DECORATION_ID),
            Value::Undefined => Some(&crate::response::UNDEFINED_DECORATION_ID),
            _ => None,
        };
        if let Some(definition) = definition {
            return Err(Error::legacy(
                definition.message.to_owned(),
                definition.code,
            ));
        }
        let existing = available
            .iter()
            .find(|available| available.get("id") == decoration.get("id"));
        let Some(existing) = existing else {
            continue;
        };
        // Strictly decode only selected metadata-backed entries, after the
        // legacy strict-equality filter has excluded unknown/malformed IDs.
        let id = string(decoration, "id")?;
        let mut packed = Value::object([("id", Value::string(&id))]);
        for key in ["angle", "flipH", "offsetX", "offsetY"] {
            packed.set(
                key,
                decoration
                    .get(key)
                    .filter(|value| value.truthy())
                    .cloned()
                    .unwrap_or(Value::Undefined),
            );
        }
        let url = field(existing, "url")?;
        if !matches!(url, Value::String(_)) {
            return Err(Error::Internal);
        }
        packed.set("url", url.clone());
        result.push(packed);
    }
    Ok(Value::Array(result))
}

async fn instance(runtime: &Runtime, host: Option<&str>) -> Result<Value, Error> {
    let Some(host) = host.filter(|host| !host.is_empty()) else {
        return Ok(Value::Undefined);
    };
    let record = runtime
        .redis
        .federated_instance(host, &runtime.database)
        .await
        .map_err(|_| Error::Internal)?;
    let Some(record) = record else {
        return Ok(Value::Undefined);
    };
    let mut packed = Value::object(std::iter::empty::<(&str, Value)>());
    for key in [
        "name",
        "softwareName",
        "softwareVersion",
        "iconUrl",
        "faviconUrl",
        "themeColor",
    ] {
        let value = field(&record, key)?;
        if !matches!(value, Value::String(_) | Value::Null) {
            return Err(Error::Internal);
        }
        packed.set(key, value.clone());
    }
    Ok(packed)
}

pub(super) async fn pack_user(
    runtime: &Runtime,
    user: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    let id = string(user, "id")?;
    let host = nullable_string(user, "host")?;
    let names = string_array(field(user, "emojis")?)?;
    let now = crate::now_ms();
    let meta = runtime.auth.meta().await;
    let viewer_moderator = moderator(runtime, auth, now).await?;
    let show_remote_badges = meta
        .get("showRoleBadgesOfRemoteUsers")
        .and_then(serde_json::Value::as_bool)
        .ok_or(Error::Internal)?;
    let badges = async {
        if show_remote_badges || host.is_none() {
            badge_roles(runtime, &id, viewer_moderator, now).await
        } else {
            Ok(Value::Undefined)
        }
    };
    let (decorations, instance, emojis, badges) = tokio::try_join!(
        decorations(runtime, user),
        instance(runtime, host.as_deref()),
        populate_emojis(runtime, &names, host.as_deref()),
        badges,
    )?;
    let mut packed = Value::object([
        ("id", Value::string(&id)),
        ("name", field(user, "name")?.clone()),
        ("username", field(user, "username")?.clone()),
        ("host", field(user, "host")?.clone()),
    ]);
    let avatar_id = nullable_string(user, "avatarId")?;
    let avatar = if avatar_id.is_none() || matches!(field(user, "avatarUrl")?, Value::Null) {
        identicon_url(&runtime.business_config.notes, user, &meta)?
    } else {
        field(user, "avatarUrl")?.clone()
    };
    packed.set("avatarUrl", avatar);
    packed.set(
        "avatarBlurhash",
        if avatar_id.is_none() {
            Value::Null
        } else {
            field(user, "avatarBlurhash")?.clone()
        },
    );
    packed.set("avatarDecorations", decorations);
    packed.set("isBot", Value::Boolean(boolean(user, "isBot")?));
    packed.set("isCat", Value::Boolean(boolean(user, "isCat")?));
    packed.set(
        "requireSigninToViewContents",
        if matches!(
            field(user, "requireSigninToViewContents")?,
            Value::Boolean(false)
        ) {
            Value::Undefined
        } else {
            Value::Boolean(true)
        },
    );
    for key in ["makeNotesFollowersOnlyBefore", "makeNotesHiddenBefore"] {
        let value = field(user, key)?;
        if !matches!(value, Value::Null | Value::Number(_)) {
            return Err(Error::Internal);
        }
        packed.set(
            key,
            if matches!(value, Value::Null) {
                Value::Undefined
            } else {
                value.clone()
            },
        );
    }
    packed.set("instance", instance);
    packed.set("emojis", emojis);
    packed.set("onlineStatus", Value::string(online_status(user, now)?));
    packed.set("badgeRoles", badges);
    Ok(packed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_status_has_exact_ten_minute_and_three_day_boundaries() {
        let now = 300_000_000;
        let user = |last: Value, hidden: bool| {
            Value::object([
                ("hideOnlineStatus", Value::Boolean(hidden)),
                ("lastActiveDate", last),
            ])
        };
        assert_eq!(
            online_status(&user(Value::Null, false), now).unwrap(),
            "unknown"
        );
        assert_eq!(
            online_status(&user(Value::Number(now as f64 + 1.0), false), now).unwrap(),
            "online"
        );
        assert_eq!(
            online_status(&user(Value::Number(now as f64 - 599_999.0), false), now).unwrap(),
            "online"
        );
        assert_eq!(
            online_status(&user(Value::Number(now as f64 - 600_000.0), false), now).unwrap(),
            "active"
        );
        assert_eq!(
            online_status(&user(Value::Number(now as f64 - 259_200_000.0), false), now).unwrap(),
            "offline"
        );
        assert_eq!(
            online_status(&user(Value::Number(now as f64), true), now).unwrap(),
            "unknown"
        );
        assert!(online_status(&user(Value::string("yesterday"), false), now).is_err());
    }
    #[test]
    fn identicon_selects_system_icon_only_for_local_dot_accounts() {
        let config = super::super::Config {
            url: "https://local.example".into(),
            host: "local.example".into(),
            ..Default::default()
        };
        let meta = serde_json::json!({"iconUrl":"https://local.example/icon.png"});
        let user = |name: &str, host: Value| {
            Value::object([("username", Value::string(name)), ("host", host)])
        };
        assert_eq!(
            identicon_url(&config, &user("sys.account", Value::Null), &meta).unwrap(),
            Value::string("https://local.example/icon.png")
        );
        assert_eq!(
            identicon_url(
                &config,
                &user("sys.account", Value::string("remote.example")),
                &meta
            )
            .unwrap(),
            Value::string("https://local.example/identicon/sys.account@remote.example")
        );
        assert_eq!(
            identicon_url(&config, &user("ΟΣ", Value::Null), &meta).unwrap(),
            Value::string("https://local.example/identicon/ος@local.example")
        );
    }

    #[test]
    fn decorations_follow_observed_filter_before_decode() {
        let oracle: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/decoration-packing-oracle.json")).unwrap();
        for case in oracle["cases"].as_array().unwrap() {
            let Value::Array(ref input) = Value::from_json(&case["input"]) else {
                panic!("array input required");
            };
            let Value::Array(ref available) = Value::from_json(&case["available"]) else {
                panic!("array inventory required");
            };
            let result = pack_decorations(input, available);
            if let Some(expected) = case.get("output") {
                let actual: serde_json::Value =
                    serde_json::from_str(&result.unwrap().stringify().unwrap()).unwrap();
                assert_eq!(&actual, expected, "{}", case["name"]);
            } else {
                let Error::Legacy { message, code, .. } = result.unwrap_err() else {
                    panic!("observed legacy error required: {}", case["name"]);
                };
                assert_eq!(message, case["error"]["message"].as_str().unwrap());
                assert_eq!(code, case["error"]["code"].as_str().unwrap());
            }
        }
    }

    #[test]
    fn decorations_omit_false_flags_and_keep_order_and_repetition() {
        let decoration = Value::from_json(
            &serde_json::json!({"id":"a", "angle":0,"flipH":false,"offsetX":-1,"offsetY":2}),
        );
        let missing = Value::from_json(&serde_json::json!({"id":"missing"}));
        let available = vec![Value::from_json(
            &serde_json::json!({"id":"a","url":"https://local.example/a.webp"}),
        )];
        let Value::Array(ref packed) =
            pack_decorations(&[decoration.clone(), missing, decoration], &available).unwrap()
        else {
            panic!("array required");
        };
        assert_eq!(packed.len(), 2);
        assert_eq!(packed[0], packed[1]);
        assert_eq!(packed[0].get("angle"), Some(&Value::Undefined));
        assert_eq!(packed[0].get("flipH"), Some(&Value::Undefined));
        assert_eq!(packed[0].get("offsetX"), Some(&Value::Number(-1.0)));
    }
}
