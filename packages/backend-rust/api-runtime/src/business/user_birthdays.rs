//! Dormant followed-user birthday candidate, reusing the native UserLite packer.
//! Held: accepted single-day inputs can carry unvalidated range extras. Preserve
//! their JS coercion/error semantics and wire effective local time before activation.
use super::{Error, notes};
use crate::Runtime;
use misskey_api_validation::Value;
use misskey_backend_api_auth::Context;
use std::collections::HashMap;
use tokio_postgres::types::ToSql;

#[derive(Debug, PartialEq)]
enum Selection {
    Day(i16),
    Range { begin: i16, end: i16 },
}

impl Selection {
    fn wraps(&self) -> bool {
        matches!(self, Self::Range { begin, end } if begin > end)
    }
}

struct Input {
    selection: Selection,
    limit: SqlInteger,
    offset: SqlInteger,
}

fn bounded_integer(value: Option<&Value>, minimum: i16, maximum: i16) -> Result<i16, Error> {
    match value {
        Some(Value::Number(value))
            if value.is_finite()
                && value.fract() == 0.
                && *value >= f64::from(minimum)
                && *value <= f64::from(maximum) =>
        {
            // Exact within the explicitly checked small integer interval.
            Ok(*value as i16)
        }
        _ => Err(Error::Internal),
    }
}

fn mmdd(value: &Value) -> Result<i16, Error> {
    if !matches!(value, Value::Object(_)) {
        return Err(Error::Internal);
    }
    let month = bounded_integer(value.get("month"), 1, 12)?;
    let day = bounded_integer(value.get("day"), 1, 31)?;
    // Input validation intentionally accepts Feb31; no calendar check here.
    Ok(month * 100 + day)
}

/// Only finite integer Numbers can create a SQL numeric literal. No input
/// string is accepted or interpolated; retain JS shortest/exponent formatting.
struct SqlInteger(String);

impl SqlInteger {
    fn decode(value: &Value) -> Result<Self, Error> {
        match value {
            Value::Number(number) if number.is_finite() && number.fract() == 0. => {
                Ok(Self(value.stringify().ok_or(Error::Internal)?))
            }
            _ => Err(Error::Internal),
        }
    }

    fn small(value: i16) -> Result<Self, Error> {
        Self::decode(&Value::Number(f64::from(value)))
    }

    fn as_sql(&self) -> &str {
        &self.0
    }
}

fn offset(value: Option<&Value>) -> Result<SqlInteger, Error> {
    match value {
        None | Some(Value::Undefined) => SqlInteger::small(0),
        Some(value) => SqlInteger::decode(value),
    }
}

impl Input {
    fn decode(params: &Value) -> Result<Self, Error> {
        let birthday = params.get("birthday").ok_or(Error::Internal)?;
        if !matches!(birthday, Value::Object(_)) {
            return Err(Error::Internal);
        }
        // Value::get sees own fields only, matching both Object.hasOwn tests.
        let selection = match (birthday.get("begin"), birthday.get("end")) {
            (Some(begin), Some(end)) => Selection::Range {
                begin: mmdd(begin)?,
                end: mmdd(end)?,
            },
            _ => Selection::Day(mmdd(birthday)?),
        };
        let limit = match params.get("limit") {
            None | Some(Value::Undefined) => 10,
            value => bounded_integer(value, 1, 100)?,
        };
        Ok(Self {
            selection,
            limit: SqlInteger::small(limit)?,
            offset: offset(params.get("offset"))?,
        })
    }
}

struct BirthdayRow {
    user_id: String,
    birthday_date: i16,
}

const SINGLE: &str = "SELECT \"following\".\"followeeId\" AS \"user_id\", get_birthday_date(\"followeeProfile\".\"birthday\") AS \"birthday_date\" FROM \"following\" \"following\" INNER JOIN \"user_profile\" \"followeeProfile\" ON \"followeeProfile\".\"userId\" = \"following\".\"followeeId\" WHERE \"following\".\"followerId\" = $1 AND get_birthday_date(\"followeeProfile\".\"birthday\") BETWEEN $2::smallint AND $2::smallint ORDER BY birthday_date ASC";
const RANGE: &str = "SELECT \"following\".\"followeeId\" AS \"user_id\", get_birthday_date(\"followeeProfile\".\"birthday\") AS \"birthday_date\" FROM \"following\" \"following\" INNER JOIN \"user_profile\" \"followeeProfile\" ON \"followeeProfile\".\"userId\" = \"following\".\"followeeId\" WHERE \"following\".\"followerId\" = $1 AND get_birthday_date(\"followeeProfile\".\"birthday\") BETWEEN $2::smallint AND $3::smallint ORDER BY birthday_date ASC";
const WRAPPED: &str = "SELECT \"following\".\"followeeId\" AS \"user_id\", get_birthday_date(\"followeeProfile\".\"birthday\") AS \"birthday_date\" FROM \"following\" \"following\" INNER JOIN \"user_profile\" \"followeeProfile\" ON \"followeeProfile\".\"userId\" = \"following\".\"followeeId\" WHERE \"following\".\"followerId\" = $1 AND (get_birthday_date(\"followeeProfile\".\"birthday\") BETWEEN $2::smallint AND 1231 OR get_birthday_date(\"followeeProfile\".\"birthday\") BETWEEN 101 AND $3::smallint) ORDER BY birthday_date ASC";

async fn selected_rows(
    runtime: &Runtime,
    input: &Input,
    viewer: &str,
) -> Result<Vec<BirthdayRow>, Error> {
    let mut params: Vec<&(dyn ToSql + Sync)> = vec![&viewer];
    let sql = match &input.selection {
        Selection::Day(day) => {
            params.push(day);
            SINGLE
        }
        Selection::Range { begin, end } => {
            params.extend([begin as &(dyn ToSql + Sync), end]);
            if input.selection.wraps() {
                WRAPPED
            } else {
                RANGE
            }
        }
    };
    // Match TypeORM's validated numeric LIMIT/OFFSET literals. The installed
    // query builder retains explicit OFFSET0; the offline oracle confirms it.
    // Only the private finite-integer Number wrapper enters the SQL text.
    let sql = format!(
        "{sql} LIMIT {} OFFSET {}",
        input.limit.as_sql(),
        input.offset.as_sql()
    );
    // Keep the original two-column projection and its birthday-only ordering.
    // Wrapping it in JSON could change a planner's unspecified equal-date ties.
    runtime
        .database
        .query_typed(true, &sql, &params)
        .await
        .map_err(Error::query)?
        .into_iter()
        .map(|row| {
            Ok(BirthdayRow {
                user_id: row.try_get("user_id").map_err(|_| Error::Internal)?,
                birthday_date: row.try_get("birthday_date").map_err(|_| Error::Internal)?,
            })
        })
        .collect()
}

fn output(
    rows: &[BirthdayRow],
    users: &HashMap<String, Value>,
    mut birthday: impl FnMut(i16) -> Result<String, Error>,
) -> Result<Value, Error> {
    let mut values = Vec::with_capacity(rows.len());
    for row in rows {
        // TS computes the date even if the packed-user map later misses this ID.
        let date = birthday(row.birthday_date)?;
        if let Some(user) = users.get(&row.user_id) {
            values.push(Value::object([
                ("id", Value::string(&row.user_id)),
                ("birthday", Value::string(&date)),
                ("user", user.clone()),
            ]));
        }
    }
    Ok(Value::Array(values))
}

pub(super) async fn birthdays(
    runtime: &Runtime,
    params: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    let input = Input::decode(params)?;
    let viewer = auth
        .user
        .as_ref()
        .and_then(|user| user.get("id"))
        .and_then(serde_json::Value::as_str)
        .ok_or(Error::Internal)?;
    let rows = selected_rows(runtime, &input, viewer).await?;
    let ids = rows
        .iter()
        .map(|row| row.user_id.clone())
        .collect::<Vec<_>>();
    // TS UserLite packMany([]) returns immediately, before role or user reads.
    let users = notes::pack_many_users(runtime, &ids, auth).await?;
    output(&rows, &users, |mmdd| {
        runtime
            .business_config
            .local_calendar
            .birthday(mmdd, crate::now_ms)
            .map(|value| value.text)
            .map_err(|_| Error::Internal)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(json: &str) -> Input {
        Input::decode(&Value::parse_json(json).unwrap()).unwrap()
    }

    #[test]
    fn typed_selection_keeps_single_range_wrap_and_calendar_overflow() {
        let single = input(r#"{"birthday":{"month":2,"day":31}}"#);
        assert_eq!(single.selection, Selection::Day(231));
        assert_eq!(single.limit.as_sql(), "10");
        assert_eq!(single.offset.as_sql(), "0");
        let wrapped = input(
            r#"{"birthday":{"begin":{"month":12,"day":31},"end":{"month":1,"day":1}},"limit":100}"#,
        );
        assert!(wrapped.selection.wraps());
        assert_eq!(wrapped.limit.as_sql(), "100");
        let normal =
            input(r#"{"birthday":{"begin":{"month":1,"day":1},"end":{"month":12,"day":31}}}"#);
        assert!(!normal.selection.wraps());
        for json in [
            r#"{"birthday":{"month":0,"day":1}}"#,
            r#"{"birthday":{"month":1,"day":0}}"#,
            r#"{"birthday":{"month":1.5,"day":1}}"#,
            r#"{"birthday":{"month":"1","day":1}}"#,
            r#"{"birthday":{"month":1,"day":1},"limit":101}"#,
            r#"{"birthday":{"month":1,"day":1},"limit":0}"#,
            r#"{"birthday":{"month":1,"day":1},"limit":"10"}"#,
            r#"{"birthday":null}"#,
        ] {
            assert!(Input::decode(&Value::parse_json(json).unwrap()).is_err());
        }
    }

    #[test]
    fn numeric_offset_keeps_negative_large_integer_exponent_and_negative_zero_spelling() {
        for (json, expected) in [
            ("-1", "-1"),
            ("-0", "0"),
            ("9007199254740993", "9007199254740992"),
            ("1e100", "1e+100"),
        ] {
            assert_eq!(
                offset(Some(&Value::parse_json(json).unwrap()))
                    .unwrap()
                    .as_sql(),
                expected
            );
        }
        for invalid in [
            Value::Null,
            Value::string("2"),
            Value::string("1; SELECT 1"),
            Value::Number(1.5),
            Value::Number(f64::INFINITY),
        ] {
            assert!(offset(Some(&invalid)).is_err());
            assert!(SqlInteger::decode(&invalid).is_err());
        }
    }

    #[test]
    fn output_keeps_query_order_duplicates_and_formats_before_missing_user_filter() {
        let rows = vec![
            BirthdayRow {
                user_id: "jan".to_owned(),
                birthday_date: 101,
            },
            BirthdayRow {
                user_id: "missing".to_owned(),
                birthday_date: 200,
            },
            BirthdayRow {
                user_id: "dec".to_owned(),
                birthday_date: 1231,
            },
            BirthdayRow {
                user_id: "jan".to_owned(),
                birthday_date: 101,
            },
        ];
        let users = HashMap::from([
            (
                "jan".to_owned(),
                Value::object([("id", Value::string("jan"))]),
            ),
            (
                "dec".to_owned(),
                Value::object([("id", Value::string("dec"))]),
            ),
        ]);
        let mut seen = Vec::new();
        let result = output(&rows, &users, |mmdd| {
            seen.push(mmdd);
            Ok(format!("date{mmdd}"))
        })
        .unwrap();
        assert_eq!(seen, vec![101, 200, 1231, 101]);
        let Value::Array(values) = &result else {
            panic!("expected array")
        };
        let actual = values
            .iter()
            .map(|value| value.get("id").unwrap().clone())
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            vec![
                Value::string("jan"),
                Value::string("dec"),
                Value::string("jan")
            ]
        );
        assert_eq!(values[0].get("birthday"), Some(&Value::string("date101")));
        assert_eq!(
            output(&[], &users, |_| Err(Error::Internal)).unwrap(),
            Value::Array(Vec::new())
        );
    }
}
