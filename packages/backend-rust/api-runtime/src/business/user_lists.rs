//! Complete user-list reads, sharing the base UserList projection.
use super::{Error, iso_date, parse_id_date, rows, text};
use crate::Runtime;
use misskey_api_validation::Value;
use misskey_backend_api_auth::Context;
use tokio_postgres::types::ToSql;

enum Selection {
    Own(String),
    Public(String),
}

impl Selection {
    fn owner(&self) -> &str {
        match self {
            Self::Own(owner) | Self::Public(owner) => owner,
        }
    }
}

fn client_error(message: &'static str, code: &'static str, id: &'static str) -> Error {
    Error::Api {
        message,
        code,
        id,
        kind: "client",
        status: None,
        info: None,
    }
}

fn selection(params: &Value, auth: &Context) -> Result<Selection, Error> {
    match params.get("userId") {
        None | Some(Value::Undefined) => {
            let Some(user) = &auth.user else {
                return Err(client_error(
                    "Invalid param.",
                    "INVALID_PARAM",
                    "ab36de0e-29e9-48cb-9732-d82f1281620d",
                ));
            };
            let owner = user
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or(Error::Internal)?;
            // The omitted-ID path has no extra user/host lookup in TS.
            Ok(Selection::Own(owner.to_owned()))
        }
        Some(Value::String(_)) => Ok(Selection::Public(text(params, "userId")?)),
        // Shared input validation rejects null/non-string IDs before this body.
        _ => Err(Error::Internal),
    }
}

fn require_local_user(users: &[Value]) -> Result<(), Error> {
    let Some(user) = users.first() else {
        return Err(client_error(
            "No such user.",
            "NO_SUCH_USER",
            "a8af4a82-0980-4cc4-a6af-8b0ffd54465e",
        ));
    };
    match user.get("host") {
        Some(Value::Null) => Ok(()),
        // TS checks host !== null; an empty host is remote in this branch too.
        Some(Value::String(_)) => Err(client_error(
            "Not allowed to load the remote user's list",
            "REMOTE_USER_NOT_ALLOWED",
            "53858f1b-3315-4a01-81b7-db9b48d4b79a",
        )),
        _ => Err(Error::Internal),
    }
}

fn string_value(row: &Value, key: &str) -> Result<Value, Error> {
    match row.get(key) {
        Some(value @ Value::String(_)) => Ok(value.clone()),
        _ => Err(Error::Internal),
    }
}

fn packed_fields(row: &Value, members: &[Value], id_method: &str) -> Result<Value, Error> {
    let id = text(row, "id")?;
    let created_at = iso_date(parse_id_date(id_method, &id).ok_or(Error::Internal)?)?;
    let users = members
        .iter()
        .map(|user| string_value(user, "userId"))
        .collect::<Result<Vec<_>, _>>()?;
    let is_public = match row.get("isPublic") {
        Some(Value::Boolean(value)) => *value,
        _ => return Err(Error::Internal),
    };
    Ok(Value::object([
        ("id", string_value(row, "id")?),
        ("createdAt", Value::string(&created_at)),
        ("name", string_value(row, "name")?),
        // Keep every member and the query's returned order, including duplicates.
        ("userIds", Value::Array(users)),
        ("isPublic", Value::Boolean(is_public)),
    ]))
}

// This bounds one statement's parameters/plan size, not the number of lists.
const MEMBERSHIP_BATCH: usize = 64;

fn membership_query(count: usize) -> String {
    let scans = (0..count)
        .map(|ordinal| {
            let slot = ordinal + 1;
            format!(
                "SELECT {ordinal} AS ordinal, ${slot}::varchar AS id, COALESCE((SELECT json_agg(membership.value) FROM (SELECT json_build_object('id',m.id,'userId',m.\"userId\",'userListId',m.\"userListId\",'withReplies',m.\"withReplies\",'userListUserId',m.\"userListUserId\") AS value FROM user_list_membership m WHERE m.\"userListId\"=${slot} OFFSET 0) membership), '[]'::json) AS members"
            )
        })
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    // Each aggregate consumes one original full-column per-ID scan without
    // sorting/deduplication. OFFSET 0 retains that scan's projection boundary.
    // Only completed per-list arrays are ordered, matching Promise.all output.
    format!(
        "SELECT json_build_object('ordinal',batch.ordinal,'id',batch.id,'members',batch.members)::text FROM ({scans}) batch ORDER BY batch.ordinal"
    )
}

fn membership_arrays(ids: &[String], values: Vec<Value>) -> Result<Vec<Vec<Value>>, Error> {
    if values.len() != ids.len() {
        return Err(Error::Internal);
    }
    let mut result = Vec::with_capacity(ids.len());
    for (ordinal, (id, mut row)) in ids.iter().zip(values).enumerate() {
        let ordinal = f64::from(u32::try_from(ordinal).map_err(|_| Error::Internal)?);
        if row.get("ordinal").and_then(Value::as_number) != Some(ordinal)
            || text(&row, "id")? != *id
        {
            return Err(Error::Internal);
        }
        let Some(Value::Array(members)) = row.get_mut("members") else {
            return Err(Error::Internal);
        };
        result.push(std::mem::take(members));
    }
    Ok(result)
}

/// Shared base projection remains reusable by a later lists/show operation.
async fn pack_lists(runtime: &Runtime, lists: &[Value]) -> Result<Value, Error> {
    let mut packed = Vec::with_capacity(lists.len());
    for chunk in lists.chunks(MEMBERSHIP_BATCH) {
        let ids = chunk
            .iter()
            .map(|row| text(row, "id"))
            .collect::<Result<Vec<_>, _>>()?;
        let params: Vec<&(dyn ToSql + Sync)> =
            ids.iter().map(|id| -> &(dyn ToSql + Sync) { id }).collect();
        let values = rows(
            &runtime.database,
            true,
            &membership_query(ids.len()),
            &params,
        )
        .await?;
        let members = membership_arrays(&ids, values)?;
        for (row, members) in chunk.iter().zip(members) {
            // As in UserListEntityService, all membership reads precede the
            // selected list's date/payload projection. No private IDs enter here.
            packed.push(packed_fields(
                row,
                &members,
                &runtime.business_config.id_method,
            )?);
        }
    }
    Ok(Value::Array(packed))
}

pub(super) async fn list(
    runtime: &Runtime,
    params: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    let selection = selection(params, auth)?;
    if let Selection::Public(owner) = &selection {
        let users = rows(
            &runtime.database,
            true,
            "SELECT json_build_object('host',host)::text FROM \"user\" WHERE id=$1 LIMIT 1",
            &[owner],
        )
        .await?;
        require_local_user(&users)?;
    }
    let sql = match &selection {
        Selection::Own(_) => {
            "SELECT json_build_object('id',l.id,'userId',l.\"userId\",'isPublic',l.\"isPublic\",'name',l.name)::text FROM user_list l WHERE l.\"userId\"=$1"
        }
        Selection::Public(_) => {
            "SELECT json_build_object('id',l.id,'userId',l.\"userId\",'isPublic',l.\"isPublic\",'name',l.name)::text FROM user_list l WHERE l.\"userId\"=$1 AND l.\"isPublic\"=true"
        }
    };
    // Explicit IDs always take the SQL public-only path, even for the caller's
    // own ID. Eligibility is resolved before list payload/membership decoding.
    let owner = selection.owner();
    let lists = rows(&runtime.database, true, sql, &[&owner]).await?;
    pack_lists(runtime, &lists).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(id: Option<&str>) -> Context {
        Context {
            user: id.map(|id| serde_json::json!({"id":id})),
            token: None,
            access_token_reduced: false,
        }
    }

    fn record() -> Value {
        Value::object([
            ("id", Value::string("0000000000000001")),
            ("name", Value::string("list 😀 e\u{301}")),
            ("isPublic", Value::Boolean(false)),
        ])
    }

    #[test]
    fn explicit_self_id_still_selects_public_lists_only() {
        let me = auth(Some("owner"));
        let omitted = selection(&Value::parse_json("{}").unwrap(), &me).unwrap();
        assert!(matches!(omitted, Selection::Own(ref id) if id == "owner"));
        let undefined = selection(&Value::object([("userId", Value::Undefined)]), &me).unwrap();
        assert!(matches!(undefined, Selection::Own(_)));
        let explicit =
            selection(&Value::object([("userId", Value::string("owner"))]), &me).unwrap();
        assert!(matches!(explicit, Selection::Public(ref id) if id == "owner"));
    }

    #[test]
    fn anonymous_requires_an_explicit_user_id() {
        let anonymous = auth(None);
        assert!(matches!(
            selection(&Value::parse_json("{}").unwrap(), &anonymous),
            Err(Error::Api {
                code: "INVALID_PARAM",
                id: "ab36de0e-29e9-48cb-9732-d82f1281620d",
                ..
            })
        ));
        assert!(matches!(
            selection(
                &Value::object([("userId", Value::string("owner"))]),
                &anonymous
            )
            .unwrap(),
            Selection::Public(_)
        ));
    }

    #[test]
    fn missing_and_remote_users_retain_the_endpoint_errors() {
        assert!(matches!(
            require_local_user(&[]),
            Err(Error::Api {
                code: "NO_SUCH_USER",
                id: "a8af4a82-0980-4cc4-a6af-8b0ffd54465e",
                ..
            })
        ));
        assert!(require_local_user(&[Value::object([("host", Value::Null)])]).is_ok());
        for host in ["", "remote.example"] {
            assert!(matches!(
                require_local_user(&[Value::object([("host", Value::string(host))])]),
                Err(Error::Api {
                    code: "REMOTE_USER_NOT_ALLOWED",
                    id: "53858f1b-3315-4a01-81b7-db9b48d4b79a",
                    ..
                })
            ));
        }
    }

    #[test]
    fn packing_keeps_member_order_duplicates_and_utf16_name() {
        let row = record();
        let ids = [Value::string("z"), Value::string("a"), Value::string("z")];
        let members = ids
            .iter()
            .map(|id| Value::object([("userId", id.clone())]))
            .collect::<Vec<_>>();
        let packed = packed_fields(&row, &members, "aidx").unwrap();
        assert_eq!(packed.get("name"), row.get("name"));
        assert_eq!(packed.get("userIds"), Some(&Value::Array(ids.to_vec())));
        assert_eq!(
            packed.get("createdAt"),
            Some(&Value::string("2000-01-01T00:00:00.000Z"))
        );
        assert_eq!(packed.get("isPublic"), Some(&Value::Boolean(false)));
    }

    #[test]
    fn an_empty_list_has_an_array_and_no_show_only_fields() {
        let mut row = record();
        row.set("likedCount", Value::Null);
        row.set("isLiked", Value::Null);
        let packed = packed_fields(&row, &[], "aidx").unwrap();
        assert_eq!(packed.get("userIds"), Some(&Value::Array(Vec::new())));
        assert_eq!(packed.get("likedCount"), None);
        assert_eq!(packed.get("isLiked"), None);
        assert!(!packed.stringify().unwrap().contains(":null"));
    }

    #[test]
    fn public_projection_requires_nonnullable_string_and_boolean_fields() {
        let mut row = record();
        row.set("name", Value::Null);
        assert!(packed_fields(&row, &[], "aidx").is_err());
        let mut row = record();
        row.set("isPublic", Value::Null);
        assert!(packed_fields(&row, &[], "aidx").is_err());
    }

    #[test]
    fn batched_members_keep_list_association_empty_arrays_and_member_order() {
        let ids = vec!["first".to_owned(), "empty".to_owned()];
        let members = vec![
            Value::object([("userId", Value::string("z"))]),
            Value::object([("userId", Value::string("a"))]),
            Value::object([("userId", Value::string("z"))]),
        ];
        let rows = vec![
            Value::object([
                ("ordinal", Value::Number(0.)),
                ("id", Value::string("first")),
                ("members", Value::Array(members.clone())),
            ]),
            Value::object([
                ("ordinal", Value::Number(1.)),
                ("id", Value::string("empty")),
                ("members", Value::Array(Vec::new())),
            ]),
        ];
        let decoded = membership_arrays(&ids, rows.clone()).unwrap();
        assert_eq!(decoded, vec![members, Vec::new()]);
        let wrong_ids = vec!["other".to_owned(), "empty".to_owned()];
        assert!(membership_arrays(&wrong_ids, rows).is_err());
    }
}
