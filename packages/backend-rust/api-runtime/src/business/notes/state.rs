//! Complete authenticated favorite and muted-thread state lookup.
use super::{
    Error, Value,
    row::{nullable_string, string},
    rows,
};
use crate::Runtime;
use misskey_backend_api_auth::Context;
use tokio_postgres::types::ToSql;

struct StateNote {
    id: String,
    thread_id: Option<String>,
}

impl StateNote {
    fn decode(row: &Value) -> Result<Self, Error> {
        // The TS operation loads the note but does not interpret its content.
        // Do not decode unrelated reactions, visibility, files or poll payloads.
        Ok(Self {
            id: string(row, "id")?,
            thread_id: nullable_string(row, "threadId")?,
        })
    }

    fn thread_id(&self) -> &str {
        // Mirrors note.threadId ?? note.id; an empty string is not null.
        self.thread_id.as_deref().unwrap_or(&self.id)
    }
}

fn missing_note(note_id: &str) -> Error {
    let Ok(encoded) = serde_json::to_string(note_id) else {
        return Error::Internal;
    };
    Error::legacy(
        format!(
            "Could not find any entity of type \"MiNote\" matching: {{\n    \"id\": {encoded}\n}}"
        ),
        "EntityNotFoundError",
    )
}

fn counted(values: &[Value]) -> Result<bool, Error> {
    match values {
        [Value::Boolean(value)] => Ok(*value),
        _ => Err(Error::Internal),
    }
}

pub(in crate::business) async fn state(
    runtime: &Runtime,
    params: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    let note_id = string(params, "noteId")?;
    let viewer = auth
        .user
        .as_ref()
        .and_then(|user| user.get("id"))
        .and_then(serde_json::Value::as_str)
        .ok_or(Error::Internal)?;
    let notes = rows(
        &runtime.database,
        true,
        "SELECT json_build_object('id',id,'threadId',\"threadId\")::text FROM note WHERE id=$1 LIMIT 1",
        &[&note_id],
    )
    .await?;
    let Some(note) = notes.first() else {
        return Err(missing_note(&note_id));
    };
    let note = StateNote::decode(note)?;
    let thread_id = note.thread_id();
    let favorite_params: [&(dyn ToSql + Sync); 2] = [&viewer, &note.id];
    let thread_params: [&(dyn ToSql + Sync); 2] = [&viewer, &thread_id];
    // Preserve the two independent read queries and their concurrent execution.
    // COUNT is nonnegative, so the SQL boolean is exactly TypeORM count !== 0.
    let (favorite, thread_muting) = tokio::try_join!(
        rows(
            &runtime.database,
            true,
            "SELECT to_json(COUNT(*) <> 0)::text FROM note_favorite WHERE \"userId\"=$1 AND \"noteId\"=$2",
            &favorite_params,
        ),
        rows(
            &runtime.database,
            true,
            "SELECT to_json(COUNT(*) <> 0)::text FROM note_thread_muting WHERE \"userId\"=$1 AND \"threadId\"=$2",
            &thread_params,
        ),
    )?;
    Ok(Value::object([
        ("isFavorited", Value::Boolean(counted(&favorite)?)),
        ("isMutedThread", Value::Boolean(counted(&thread_muting)?)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_lookup_falls_back_only_for_null() {
        for (json, expected) in [
            (r#"{"id":"note","threadId":null}"#, "note"),
            (r#"{"id":"note","threadId":"thread"}"#, "thread"),
            (r#"{"id":"note","threadId":""}"#, ""),
        ] {
            let row = Value::parse_json(json).unwrap();
            let note = StateNote::decode(&row).unwrap();
            assert_eq!(note.thread_id(), expected);
        }
    }

    #[test]
    fn state_does_not_interpret_unrelated_note_payload() {
        let row = Value::parse_json(
            r#"{"id":"note","threadId":null,"reactions":{"👍":{"toString":null}},"visibleUserIds":[null]}"#,
        )
        .unwrap();
        let note = StateNote::decode(&row).unwrap();
        assert_eq!(note.id, "note");
        assert_eq!(note.thread_id(), "note");
    }

    #[test]
    fn missing_note_retains_typeorm_criterion_and_error_class() {
        let Error::Legacy { message, code, .. } = missing_note("quoted\"\n😀") else {
            panic!("Expected the TypeORM-compatible missing-note error");
        };
        assert_eq!(code, "EntityNotFoundError");
        assert_eq!(
            message,
            "Could not find any entity of type \"MiNote\" matching: {\n    \"id\": \"quoted\\\"\\n😀\"\n}"
        );
    }

    #[test]
    fn count_projection_requires_one_boolean_result() {
        assert!(counted(&[Value::Boolean(true)]).unwrap());
        assert!(!counted(&[Value::Boolean(false)]).unwrap());
        assert!(counted(&[]).is_err());
        assert!(counted(&[Value::Number(1.)]).is_err());
    }
}
