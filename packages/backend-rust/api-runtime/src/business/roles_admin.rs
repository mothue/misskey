//! Native read operations for roles and administrator catalog/statistics APIs.
use super::{Error, iso_date, number, rows, text};
use crate::Runtime;
use misskey_api_validation::Value;
use misskey_backend_api_auth::Context;

pub(super) async fn execute(
    runtime: &Runtime,
    name: &str,
    params: &Value,
    _auth: &Context,
) -> Option<Result<Value, Error>> {
    Some(match name {
        "roles/list" => roles(runtime, false, None).await,
        "roles/show" => match text(params, "roleId") {
            Ok(id) => roles(runtime, false, Some(&id)).await,
            Err(error) => Err(error),
        },
        "admin/roles/list" => roles(runtime, true, None).await,
        "admin/roles/show" => match text(params, "roleId") {
            Ok(id) => roles(runtime, true, Some(&id)).await,
            Err(error) => Err(error),
        },
        "admin/relays/list" => rows(
            &runtime.database,
            true,
            "SELECT row_to_json(t)::text FROM relay t",
            &[],
        )
        .await
        .map(Value::Array),
        "admin/get-index-stats" => rows(
            &runtime.database,
            false,
            "SELECT row_to_json(t)::text FROM pg_indexes t",
            &[],
        )
        .await
        .map(Value::Array),
        "admin/get-table-stats" => table_stats(runtime).await,
        _ => return None,
    })
}

async fn roles(runtime: &Runtime, admin: bool, role_id: Option<&str>) -> Result<Value, Error> {
    let (tail, params): (&str, Vec<&(dyn tokio_postgres::types::ToSql + Sync)>) =
        if let Some(ref id) = role_id {
            (
                if admin {
                    "WHERE t.id = $1 LIMIT 1"
                } else {
                    "WHERE t.id = $1 AND t.\"isPublic\" = true LIMIT 1"
                },
                vec![id],
            )
        } else if admin {
            ("ORDER BY t.\"lastUsedAt\" DESC", vec![])
        } else {
            (
                "WHERE t.\"isPublic\" = true AND t.\"isExplorable\" = true",
                vec![],
            )
        };
    let query = format!(
        "SELECT json_build_object('id', t.id, 'updatedAt', floor(extract(epoch FROM t.\"updatedAt\") * 1000)::double precision, \
         'name', t.name, 'description', t.description, 'color', t.color, 'iconUrl', t.\"iconUrl\", \
         'target', t.target, 'condFormula', t.\"condFormula\", 'isPublic', t.\"isPublic\", \
         'isAdministrator', t.\"isAdministrator\", 'isModerator', t.\"isModerator\", \
         'isExplorable', t.\"isExplorable\", 'asBadge', t.\"asBadge\", \
         'preserveAssignmentOnMoveAccount', t.\"preserveAssignmentOnMoveAccount\", \
         'canEditMembersByModerator', t.\"canEditMembersByModerator\", 'displayOrder', t.\"displayOrder\", \
         'policies', t.policies)::text FROM role t {tail}"
    );
    let records = rows(&runtime.database, true, &query, &params).await?;
    if role_id.is_some() && records.is_empty() {
        return Err(Error::Api {
            message: "No such role.",
            code: "NO_SUCH_ROLE",
            id: if admin {
                "07dc7d34-c0d8-49b7-96c6-db3ce64ee0b3"
            } else {
                "de5502bf-009a-4639-86c1-fec349e46dcb"
            },
            kind: "client",
            status: None,
            info: None,
        });
    }
    // packMany starts every independent pack before waiting for its results.
    let packed = futures_util::future::try_join_all(
        records.into_iter().map(|role| pack_role(runtime, role)),
    )
    .await?;
    if role_id.is_some() {
        packed.into_iter().next().ok_or(Error::Internal)
    } else {
        Ok(Value::Array(packed))
    }
}

async fn pack_role(runtime: &Runtime, role: Value) -> Result<Value, Error> {
    let id = text(&role, "id")?;
    let now = crate::now_ms() as f64;
    let count = rows(
        &runtime.database,
        true,
        "SELECT json_build_object('count', count(*))::text FROM role_assignment WHERE \"roleId\" = $1 AND (\"expiresAt\" IS NULL OR \"expiresAt\" > to_timestamp($2::double precision / 1000))",
        &[&id, &now],
    )
    .await?;
    let mut policies = spread(role.get("policies").ok_or(Error::Internal)?);
    let Value::Object(ref defaults) = runtime.business_config.default_policies else {
        return Err(Error::Internal);
    };
    for (key, value) in defaults {
        if policies
            .get_units(key)
            .is_none_or(|value| matches!(value, Value::Null | Value::Undefined))
        {
            policies.set_units(
                key.clone(),
                Value::object([
                    ("useDefault", Value::Boolean(true)),
                    ("priority", Value::Number(0.0)),
                    ("value", value.clone()),
                ]),
            );
        }
    }
    let mut packed = Value::object([
        ("id", Value::string(&id)),
        (
            "createdAt",
            Value::string(&id_iso(&runtime.business_config.id_method, &id)?),
        ),
        (
            "updatedAt",
            Value::string(&iso_date(number(&role, "updatedAt")?)?),
        ),
    ]);
    for key in [
        "name",
        "description",
        "color",
        "iconUrl",
        "target",
        "condFormula",
        "isPublic",
        "isAdministrator",
        "isModerator",
        "isExplorable",
        "asBadge",
        "preserveAssignmentOnMoveAccount",
        "canEditMembersByModerator",
        "displayOrder",
    ] {
        packed.set(key, role.get(key).ok_or(Error::Internal)?.clone());
    }
    packed.set("policies", policies);
    packed.set(
        "usersCount",
        Value::Number(number(count.first().ok_or(Error::Internal)?, "count")?),
    );
    Ok(packed)
}

/// Object spread copies enumerable own properties, including array indices and
/// the UTF-16 code units of a string; null and other primitives contribute none.
fn spread(value: &Value) -> Value {
    match value {
        Value::Object(_) => value.clone(),
        Value::Array(values) => Value::object(
            values
                .iter()
                .enumerate()
                .map(|(i, value)| (i.to_string(), value.clone())),
        ),
        Value::String(units) => Value::object(
            units
                .iter()
                .enumerate()
                .map(|(i, unit)| (i.to_string(), Value::String(vec![*unit]))),
        ),
        _ => Value::object(Vec::<(&str, Value)>::new()),
    }
}

async fn table_stats(runtime: &Runtime) -> Result<Value, Error> {
    let records = rows(
        &runtime.database,
        false,
        "SELECT json_build_object('table', relname, 'count', reltuples::text, 'size', pg_total_relation_size(C.oid)::text)::text \
         FROM pg_class C LEFT JOIN pg_namespace N ON (N.oid = C.relnamespace) \
         WHERE nspname NOT IN ('pg_catalog', 'information_schema') \
         AND C.relkind <> 'i' AND nspname !~ '^pg_toast'",
        &[],
    )
    .await?;
    let mut result = Value::object(Vec::<(&str, Value)>::new());
    for record in records {
        let table = text(&record, "table")?;
        // Assignment to Object.prototype.__proto__ changes the prototype; it
        // never creates the enumerable own property returned by JSON.stringify.
        if table == "__proto__" {
            continue;
        }
        result.set(
            &table,
            Value::object([
                (
                    "count",
                    Value::Number(parse_int_number(pg_number(&record, "count")?)),
                ),
                (
                    "size",
                    Value::Number(parse_int_number(pg_number(&record, "size")?)),
                ),
            ]),
        );
    }
    Ok(result)
}

fn pg_number(record: &Value, key: &str) -> Result<f64, Error> {
    // Both pg's float4 parser and the configured int8 parser first consume the
    // original PostgreSQL text as a binary64 Number. JSON's special float
    // spelling would otherwise turn NaN/Infinity into strings unexpectedly.
    let text = text(record, key)?;
    match text.as_str() {
        "NaN" => Ok(f64::NAN),
        "Infinity" => Ok(f64::INFINITY),
        "-Infinity" => Ok(f64::NEG_INFINITY),
        _ => text.parse().map_err(|_| Error::Internal),
    }
}

/// node-postgres first decodes float4/int8 as Number. parseInt then consumes
/// Number.toString(), whose exponent spelling matters at 1e21 and below 1e-6.
fn parse_int_number(number: f64) -> f64 {
    if !number.is_finite() {
        return f64::NAN;
    }
    let mut buffer = ryu_js::Buffer::new();
    let text = buffer.format(number);
    let end = text
        .char_indices()
        .find(|(i, c)| !c.is_ascii_digit() && !(*i == 0 && *c == '-'))
        .map_or(text.len(), |(i, _)| i);
    text[..end].parse().unwrap_or(f64::NAN)
}

fn id_iso(method: &str, id: &str) -> Result<String, Error> {
    iso_date(super::id::parse_id_date(method, id).ok_or(Error::Internal)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_stats_uses_number_to_string_before_parse_int() {
        assert_eq!(parse_int_number(1e21), 1.0);
        assert_eq!(parse_int_number(1e20), 1e20);
        assert_eq!(parse_int_number(1e-7), 1.0);
        assert_eq!(parse_int_number(-1e-7), -1.0);
        assert_eq!(parse_int_number(123.75), 123.0);
        assert_eq!(parse_int_number(-0.5).to_bits(), (-0.0f64).to_bits());
        assert!(parse_int_number(f64::NAN).is_nan());
        assert_eq!(
            parse_int_number(i64::MAX as f64),
            9_223_372_036_854_776_000.0
        );
        let maximum = Value::object([("size", Value::string("9223372036854775807"))]);
        assert_eq!(pg_number(&maximum, "size").unwrap(), i64::MAX as f64);
        let nonfinite = Value::object([("count", Value::string("Infinity"))]);
        assert!(parse_int_number(pg_number(&nonfinite, "count").unwrap()).is_nan());
    }

    #[test]
    fn policy_spread_keeps_js_enumeration_and_utf16_units() {
        let value = Value::String(vec![0xd83d, 0xde00]);
        assert_eq!(spread(&value).get("0"), Some(&Value::String(vec![0xd83d])));
        assert_eq!(spread(&value).get("1"), Some(&Value::String(vec![0xde00])));
        assert_eq!(
            spread(&Value::Null),
            Value::object(Vec::<(&str, Value)>::new())
        );
    }
}
