//! Complete opaque profile achievement read, preserving the stored JSON value.
use super::{Error, rows, text};
use crate::Runtime;
use misskey_api_validation::Value;

/// Storage JSON is intentionally opaque: TS does not validate an achievement
/// array, enum names, dates, or individual fields when reading this column.
struct ProfileAchievements(Value);

impl ProfileAchievements {
    fn decode(values: &[Value], user_id: &str) -> Result<Self, Error> {
        let row = match values {
            [] => return Err(missing_profile(user_id)),
            [row] => row,
            _ => return Err(Error::Internal),
        };
        match row.get("achievements") {
            // Missing SQL rows, absent wrapper fields and present JSON null
            // are distinct. rows() parses JSON, so nested values stay opaque.
            None | Some(Value::Undefined) => Err(Error::Internal),
            Some(value) => Ok(Self(value.clone())),
        }
    }
}

fn missing_profile(user_id: &str) -> Error {
    let Ok(encoded) = serde_json::to_string(user_id) else {
        return Error::Internal;
    };
    Error::legacy(
        format!(
            "Could not find any entity of type \"MiUserProfile\" matching: {{\n    \"userId\": {encoded}\n}}"
        ),
        "EntityNotFoundError",
    )
}

pub(super) async fn achievements(runtime: &Runtime, params: &Value) -> Result<Value, Error> {
    let user_id = text(params, "userId")?;
    let values = rows(
        &runtime.database,
        true,
        "SELECT json_build_object('achievements',p.achievements)::text FROM user_profile p WHERE p.\"userId\"=$1 LIMIT 1",
        &[&user_id],
    )
    .await?;
    // Existing response::value preserves root null's 204 and string handling.
    Ok(ProfileAchievements::decode(&values, &user_id)?.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(value: &Value) -> Value {
        ProfileAchievements::decode(
            &[Value::object([("achievements", value.clone())])],
            "viewer",
        )
        .unwrap()
        .0
    }

    #[test]
    fn achievement_payload_preserves_order_duplicates_unknown_fields_and_number_types() {
        for json in [
            "[]",
            r#"[{"name":"unknown","unlockedAt":"9007199254740993","extra":{"text":"😀"}},{"name":"notes1","unlockedAt":-0.5},{"name":"unknown","unlockedAt":null}]"#,
            r#"[{"name":"notes1","unlockedAt":9007199254740993},{"unlockedAt":0}]"#,
        ] {
            let value = Value::parse_json(json).unwrap();
            assert_eq!(decode(&value), value);
        }
        let rounded = Value::parse_json(r#"[{"unlockedAt":9007199254740993}]"#).unwrap();
        let Value::Array(values) = &decode(&rounded) else {
            panic!("expected preserved array")
        };
        assert_eq!(
            values[0].get("unlockedAt").and_then(Value::as_number),
            Some(9_007_199_254_740_992.)
        );
    }

    #[test]
    fn root_json_null_scalar_string_and_object_are_returned_without_array_narrowing() {
        for json in ["null", "false", "17.5", r#""raw""#, r#"{"name":"unknown"}"#] {
            let value = Value::parse_json(json).unwrap();
            assert_eq!(decode(&value), value);
        }
        let absent = Value::parse_json("{}").unwrap();
        assert!(matches!(
            ProfileAchievements::decode(&[absent], "viewer"),
            Err(Error::Internal)
        ));
        let undefined = Value::object([("achievements", Value::Undefined)]);
        assert!(matches!(
            ProfileAchievements::decode(&[undefined], "viewer"),
            Err(Error::Internal)
        ));
    }

    #[test]
    fn a_missing_profile_keeps_the_ordinary_entity_not_found_contract() {
        let Err(Error::Legacy { message, code, .. }) =
            ProfileAchievements::decode(&[], "UNKNOWN123")
        else {
            panic!("expected missing profile legacy error")
        };
        assert_eq!(code, "EntityNotFoundError");
        assert_eq!(
            message,
            "Could not find any entity of type \"MiUserProfile\" matching: {\n    \"userId\": \"UNKNOWN123\"\n}"
        );
    }
}
