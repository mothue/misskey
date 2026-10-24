//! Complete owner-scoped draft count, without interpreting draft payload.
use super::{Error, Value, rows};
use crate::Runtime;
use misskey_backend_api_auth::Context;

fn count_value(values: &[Value]) -> Result<Value, Error> {
    match values {
        [Value::Number(count)] if count.is_finite() && *count >= 0. && count.fract() == 0. => {
            // PostgreSQL COUNT is int8, but TypeORM exposes parseInt's Number.
            // Keep binary64 rounding; do not impose a safe-integer restriction.
            Ok(Value::Number(*count))
        }
        _ => Err(Error::Internal),
    }
}

pub(in crate::business) async fn count(runtime: &Runtime, auth: &Context) -> Result<Value, Error> {
    let owner = auth
        .user
        .as_ref()
        .and_then(|user| user.get("id"))
        .and_then(serde_json::Value::as_str)
        .ok_or(Error::Internal)?;
    // Shared authentication, scope and prohibitMoved gates precede this body.
    // Every owned draft counts, including scheduled and unscheduled drafts;
    // there is no draft-content decoding or caller-supplied filter here.
    count_value(
        &rows(
            &runtime.database,
            true,
            "SELECT to_json(COUNT(*))::text FROM note_draft WHERE \"userId\"=$1",
            &[&owner],
        )
        .await?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_and_multiple_drafts_are_json_numbers() {
        for count in [0., 1., 17.] {
            let packed = count_value(&[Value::Number(count)]).unwrap();
            assert_eq!(packed.as_number(), Some(count));
        }
        assert_eq!(
            count_value(&[Value::Number(0.)])
                .unwrap()
                .stringify()
                .as_deref(),
            Some("0")
        );
    }

    #[test]
    fn postgres_int8_counts_keep_typescript_number_rounding() {
        for (json, expected) in [
            ("9007199254740993", 9_007_199_254_740_992.),
            ("9223372036854775807", 9_223_372_036_854_775_808.),
        ] {
            let parsed = Value::parse_json(json).unwrap();
            assert_eq!(count_value(&[parsed]).unwrap().as_number(), Some(expected));
        }
    }

    #[test]
    fn missing_or_invalid_count_scalars_do_not_become_zero() {
        assert!(count_value(&[]).is_err());
        for invalid in [
            Value::Null,
            Value::string("1"),
            Value::Number(-1.),
            Value::Number(1.5),
        ] {
            assert!(count_value(&[invalid]).is_err());
        }
    }
}
