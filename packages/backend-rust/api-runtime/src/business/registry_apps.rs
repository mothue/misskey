use super::{Error, iso_date, number, parse_id_date, rows, text};
use futures_util::future::try_join_all;
use misskey_api_validation::Value;
use misskey_backend_api_auth::Context;
use tokio_postgres::types::ToSql;

pub(super) async fn execute(
    runtime: &crate::Runtime,
    name: &str,
    params: &Value,
    auth: &Context,
) -> Option<Result<Value, Error>> {
    Some(match name {
        "i/registry/get"
        | "i/registry/get-detail"
        | "i/registry/get-all"
        | "i/registry/keys"
        | "i/registry/keys-with-type" => registry(runtime, name, params, auth).await,
        "i/registry/scopes-with-domain" => scopes(runtime, auth).await,
        "app/show" => show_app(runtime, params, auth).await,
        "my/apps" => my_apps(runtime, params, auth).await,
        "i/apps" => token_apps(runtime, params, auth).await,
        "i/authorized-apps" => authorized_apps(runtime, params, auth).await,
        _ => return None,
    })
}

fn user_id(auth: &Context) -> Result<&str, Error> {
    auth.user
        .as_ref()
        .and_then(|user| user.get("id"))
        .and_then(serde_json::Value::as_str)
        .ok_or(Error::Internal)
}

fn domain(params: &Value, auth: &Context) -> Result<Option<String>, Error> {
    if let Some(token) = &auth.token {
        return token
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(|value| Some(value.to_owned()))
            .ok_or(Error::Internal);
    }
    match params.get("domain") {
        None | Some(Value::Undefined | Value::Null) => Ok(None),
        Some(Value::String(units)) => Ok(Some(String::from_utf16_lossy(units))),
        _ => Err(Error::Internal),
    }
}

async fn registry(
    runtime: &crate::Runtime,
    name: &str,
    params: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    let user = user_id(auth)?;
    let domain = domain(params, auth)?;
    let Some(Value::Array(scope)) = params.get("scope") else {
        return Err(Error::Internal);
    };
    let scope = scope
        .iter()
        .map(|value| match value {
            Value::String(units) => Ok(String::from_utf16_lossy(units)),
            _ => Err(Error::Internal),
        })
        .collect::<Result<Vec<_>, _>>()?;
    // getOne deliberately has no LIMIT: TypeORM hydrates every matching row
    // before returning the first one. PostgreSQL retains the query's row order.
    let single = matches!(name, "i/registry/get" | "i/registry/get-detail");
    let projection = if single {
        "json_build_object('key', i.key, 'value', i.value, 'updatedAt', floor(extract(epoch FROM i.\"updatedAt\") * 1000)::double precision)::text"
    } else if name == "i/registry/keys" {
        "json_build_object('key', i.key)::text"
    } else {
        "json_build_object('key', i.key, 'value', i.value)::text"
    };
    let mut bindings: Vec<&(dyn ToSql + Sync)> = vec![&user, &scope];
    let domain_clause = if let Some(domain) = &domain {
        bindings.push(domain);
        "i.domain = $3"
    } else {
        "i.domain IS NULL"
    };
    let mut sql = format!(
        "SELECT {projection} FROM registry_item i WHERE {domain_clause} AND i.\"userId\" = $1 AND i.scope = $2::varchar[]"
    );
    let key;
    if single {
        key = text(params, "key")?;
        bindings.push(&key);
        sql.push_str(&format!(" AND i.key = ${}", bindings.len()));
    }
    let items = rows(&runtime.database, true, &sql, &bindings).await?;
    registry_result(name, &items)
}

fn registry_result(name: &str, items: &[Value]) -> Result<Value, Error> {
    if matches!(name, "i/registry/get" | "i/registry/get-detail") {
        let item = items.first().ok_or(Error::Api {
            message: "No such key.",
            code: "NO_SUCH_KEY",
            id: if name == "i/registry/get" {
                "ac3ed68a-62f0-422b-a7bc-d5e09e8f6a6a"
            } else {
                "97a1e8e7-c0f7-47d2-957a-92e61256e01a"
            },
            kind: "client",
            status: None,
            info: None,
        })?;
        let value = item.get("value").ok_or(Error::Internal)?.clone();
        return if name == "i/registry/get-detail" {
            Ok(Value::object([
                (
                    "updatedAt",
                    Value::string(&iso_date(number(item, "updatedAt")?)?),
                ),
                ("value", value),
            ]))
        } else {
            Ok(value)
        };
    }
    if name == "i/registry/keys" {
        return Ok(Value::Array(
            items
                .iter()
                .map(|item| item.get("key").cloned().ok_or(Error::Internal))
                .collect::<Result<_, _>>()?,
        ));
    }
    let mut result = Value::Object(Vec::new());
    // These callbacks assign into {}. The inherited __proto__ setter is
    // observable, including a later duplicate after the first value replaced
    // the prototype with null or an object with its own __proto__ property.
    let mut prototype_setter = true;
    for item in items {
        let Some(Value::String(key)) = item.get("key") else {
            return Err(Error::Internal);
        };
        let value = item.get("value").ok_or(Error::Internal)?;
        let value = if name == "i/registry/keys-with-type" {
            Value::string(match value {
                Value::Null => "null",
                Value::Array(_) => "array",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Boolean(_) => "boolean",
                Value::Object(_) => "object",
                Value::Undefined => return Err(Error::Internal),
            })
        } else {
            value.clone()
        };
        if prototype_setter && key.iter().copied().eq("__proto__".encode_utf16()) {
            match &value {
                Value::Null => prototype_setter = false,
                Value::Object(_) if value.get("__proto__").is_some() => {
                    prototype_setter = false;
                }
                _ => {}
            }
            continue;
        }
        result.set_units(key.clone(), value);
    }
    Ok(result)
}

async fn scopes(runtime: &crate::Runtime, auth: &Context) -> Result<Value, Error> {
    let user = user_id(auth)?;
    let items = rows(
        &runtime.database,
        true,
        "SELECT json_build_object('domain', i.domain, 'scope', i.scope)::text FROM registry_item i WHERE i.\"userId\" = $1",
        &[&user],
    )
    .await?;
    scope_result(&items)
}

fn scope_result(items: &[Value]) -> Result<Value, Error> {
    let mut result: Vec<Value> = Vec::new();
    for item in items {
        let domain = item.get("domain").ok_or(Error::Internal)?;
        let scope = item.get("scope").ok_or(Error::Internal)?;
        let scope_key = joined_scope(scope)?;
        if let Some(target) = result
            .iter_mut()
            .find(|target| target.get("domain") == Some(domain))
        {
            let Some(Value::Array(scopes)) = target.get_mut("scopes") else {
                return Err(Error::Internal);
            };
            if !scopes
                .iter()
                .map(joined_scope)
                .collect::<Result<Vec<_>, _>>()?
                .contains(&scope_key)
            {
                scopes.push(scope.clone());
            }
        } else {
            result.push(Value::object([
                ("domain", domain.clone()),
                ("scopes", Value::Array(vec![scope.clone()])),
            ]));
        }
    }
    Ok(Value::Array(result))
}

fn joined_scope(scope: &Value) -> Result<Vec<u16>, Error> {
    join_scope_array(scope, u16::from(b'.'))
}

fn join_scope_array(scope: &Value, separator: u16) -> Result<Vec<u16>, Error> {
    let Value::Array(parts) = scope else {
        return Err(Error::Internal);
    };
    let mut result = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if index != 0 {
            result.push(separator);
        }
        match part {
            Value::String(units) => result.extend(units),
            // PG arrays may contain NULL; Array.join treats it as empty.
            Value::Null => {}
            // PostgreSQL permits multidimensional varchar arrays, hydrated by
            // pg as nested JS arrays. Their implicit toString joins with ','.
            Value::Array(_) => result.extend(join_scope_array(part, u16::from(b','))?),
            _ => return Err(Error::Internal),
        }
    }
    Ok(result)
}

async fn app(runtime: &crate::Runtime, id: &str) -> Result<Option<Value>, Error> {
    Ok(rows(
        &runtime.database,
        true,
        "SELECT row_to_json(a)::text FROM app a WHERE a.id = $1 LIMIT 1",
        &[&id],
    )
    .await?
    .into_iter()
    .next())
}

async fn pack_app(
    runtime: &crate::Runtime,
    app: &Value,
    auth: &Context,
    include_secret: bool,
) -> Result<Value, Error> {
    let mut result = Value::object(
        ["id", "name", "callbackUrl", "permission"]
            .into_iter()
            .map(|key| (key, app.get(key).cloned().unwrap_or(Value::Undefined))),
    );
    if include_secret {
        result.set(
            "secret",
            app.get("secret").cloned().unwrap_or(Value::Undefined),
        );
    }
    if auth.user.is_some() {
        let user = user_id(auth)?;
        let id = text(app, "id")?;
        let counts = rows(&runtime.database, true, "SELECT to_json(COUNT(*))::text FROM access_token t WHERE t.\"appId\" = $1 AND t.\"userId\" = $2", &[&id, &user]).await?;
        let count = counts
            .first()
            .and_then(Value::as_number)
            .ok_or(Error::Internal)?;
        result.set("isAuthorized", Value::Boolean(count > 0.0));
    }
    Ok(result)
}

async fn show_app(
    runtime: &crate::Runtime,
    params: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    let id = text(params, "appId")?;
    let app = app(runtime, &id).await?.ok_or(Error::Api {
        message: "No such app.",
        code: "NO_SUCH_APP",
        id: "dce83913-2dc6-4093-8a7b-71dbb11718a3",
        kind: "client",
        status: None,
        info: None,
    })?;
    let include_secret = auth.token.is_none()
        && auth.user.is_some()
        && app.get("userId").and_then(Value::as_str).as_deref() == Some(user_id(auth)?);
    pack_app(runtime, &app, auth, include_secret).await
}

fn page(params: &Value) -> Result<String, Error> {
    let limit = number(params, "limit")?;
    let offset = number(params, "offset")?;
    // TypeORM inserts validated JS numbers as SQL numeric literals. In
    // particular, negative and overflowing offsets must still fail in PG.
    let mut buffer = ryu_js::Buffer::new();
    let limit = buffer.format(limit).to_owned();
    let offset = buffer.format(if offset == 0.0 { 0.0 } else { offset });
    Ok(format!(" LIMIT {limit} OFFSET {offset}"))
}

async fn my_apps(runtime: &crate::Runtime, params: &Value, auth: &Context) -> Result<Value, Error> {
    let user = user_id(auth)?;
    let sql = format!(
        "SELECT row_to_json(a)::text FROM app a WHERE a.\"userId\" = $1{}",
        page(params)?
    );
    let apps = rows(&runtime.database, true, &sql, &[&user]).await?;
    Ok(Value::Array(
        try_join_all(apps.iter().map(|app| pack_app(runtime, app, auth, false))).await?,
    ))
}

async fn authorized_apps(
    runtime: &crate::Runtime,
    params: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    let user = user_id(auth)?;
    let direction = if params.get("sort").and_then(Value::as_str).as_deref() == Some("asc") {
        "ASC"
    } else {
        "DESC"
    };
    let sql = format!(
        "SELECT json_build_object('appId', t.\"appId\")::text FROM access_token t WHERE t.\"userId\" = $1 AND t.\"appId\" IS NOT NULL ORDER BY t.id {direction}{}",
        page(params)?
    );
    let tokens = rows(&runtime.database, true, &sql, &[&user]).await?;
    let apps = try_join_all(tokens.iter().map(|token| async {
        let id = text(token, "appId")?;
        let app = app(runtime, &id)
            .await?
            .ok_or_else(|| missing_authorized_app(&id))?;
        pack_app(runtime, &app, auth, false).await
    }))
    .await?;
    Ok(Value::Array(apps))
}

fn missing_authorized_app(id: &str) -> Error {
    let Some(id) = Value::string(id).stringify() else {
        return Error::Internal;
    };
    Error::legacy(
        format!("Could not find any entity of type \"MiApp\" matching: {{\n    \"id\": {id}\n}}"),
        "EntityNotFoundError",
    )
}

async fn token_apps(
    runtime: &crate::Runtime,
    params: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    let user = user_id(auth)?;
    let order = match params.get("sort").and_then(Value::as_str).as_deref() {
        Some("+createdAt") => "t.id DESC",
        Some("+lastUsedAt") => "t.\"lastUsedAt\" DESC",
        Some("-lastUsedAt") => "t.\"lastUsedAt\" ASC",
        _ => "t.id ASC",
    };
    let sql = format!(
        "SELECT json_build_object('id', t.id, 'name', t.name, 'lastUsedAt', floor(extract(epoch FROM t.\"lastUsedAt\") * 1000)::double precision, 'permission', t.permission, 'iconUrl', t.\"iconUrl\", 'description', t.description, 'app', row_to_json(a))::text FROM access_token t LEFT JOIN app a ON a.id = t.\"appId\" WHERE t.\"userId\" = $1 ORDER BY {order}"
    );
    let tokens = rows(&runtime.database, true, &sql, &[&user]).await?;
    Ok(Value::Array(
        tokens
            .iter()
            .map(|token| token_result(token, &runtime.business_config.id_method))
            .collect::<Result<_, _>>()?,
    ))
}

fn token_result(token: &Value, method: &str) -> Result<Value, Error> {
    let id = text(token, "id")?;
    let app = token
        .get("app")
        .filter(|value| matches!(value, Value::Object(_)));
    let nullable = |key| {
        token
            .get(key)
            .filter(|value| !matches!(value, Value::Null | Value::Undefined))
    };
    let last_used = match token.get("lastUsedAt") {
        None | Some(Value::Null | Value::Undefined) => Value::Undefined,
        Some(Value::Number(time)) => Value::string(&iso_date(*time)?),
        _ => return Err(Error::Internal),
    };
    Ok(Value::object([
        ("id", Value::string(&id)),
        (
            "name",
            nullable("name")
                .or_else(|| app.and_then(|app| app.get("name")))
                .cloned()
                .unwrap_or(Value::Undefined),
        ),
        (
            "createdAt",
            Value::string(&iso_date(
                parse_id_date(method, &id).ok_or(Error::Internal)?,
            )?),
        ),
        ("lastUsedAt", last_used),
        (
            "permission",
            app.and_then(|app| app.get("permission"))
                .or_else(|| token.get("permission"))
                .cloned()
                .ok_or(Error::Internal)?,
        ),
        (
            "iconUrl",
            token.get("iconUrl").cloned().unwrap_or(Value::Undefined),
        ),
        (
            "description",
            nullable("description")
                .or_else(|| app.and_then(|app| app.get("description")))
                .cloned()
                .unwrap_or(Value::Null),
        ),
    ]))
}

#[cfg(test)]
mod tests;
