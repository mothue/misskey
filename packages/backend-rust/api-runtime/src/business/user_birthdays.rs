//! Followed-user birthday selection, reusing the native UserLite packer.
use super::{Error, js_value, local_date::CalendarError, notes};
use crate::Runtime;
use bytes::BytesMut;
use misskey_api_validation::Value;
use misskey_backend_api_auth::Context;
use serde_json::Value as Json;
use std::{collections::HashMap, fmt};
use tokio_postgres::types::{Format, IsNull, ToSql, Type, to_sql_checked};

#[derive(Debug)]
enum BirthdayFailure {
    BirthdayShape,
    ValidatedInteger,
    ArithmeticResult,
    Pagination,
    Viewer,
    UserIdColumn,
    BirthdayColumn,
    Calendar(CalendarError),
}

impl BirthdayFailure {
    fn into_error(self) -> Error {
        // Only typed failure kinds are logged, never values or driver diagnostics.
        match self {
            Self::Calendar(kind) => tracing::error!(
                operation = "users.get-following-users-by-birthday.calendar",
                error.kind = ?kind,
                "birthday conversion failed"
            ),
            kind => tracing::error!(
                operation = "users.get-following-users-by-birthday",
                error.kind = ?kind,
                "birthday operation failed"
            ),
        }
        Error::Internal
    }
}

/// JS arithmetic may produce a Number or a UTF-16 string even though the valid
/// schema branch contains month/day integers: its extra begin/end fields are open.
#[derive(PartialEq)]
enum Bound {
    Number(f64),
    String(Vec<u16>),
}

impl fmt::Debug for Bound {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BirthdayBound")
    }
}

impl Bound {
    fn from_value(value: &Value) -> Result<Self, Error> {
        match value {
            Value::Number(number) => Ok(Self::Number(*number)),
            Value::String(units) => Ok(Self::String(units.clone())),
            _ => Err(BirthdayFailure::ArithmeticResult.into_error()),
        }
    }

    fn to_number(&self) -> Result<f64, Error> {
        match self {
            Self::Number(number) => Ok(*number),
            Self::String(units) => js_value::number(&Value::String(units.clone())),
        }
    }

    fn less_equal(&self, other: &Self) -> Result<bool, Error> {
        match (self, other) {
            (Self::String(left), Self::String(right)) => Ok(left <= right),
            _ => Ok(self.to_number()? <= other.to_number()?),
        }
    }

    fn to_parameter_text(&self) -> String {
        match self {
            Self::Number(number) if *number == 0. => "0".to_owned(),
            Self::Number(number) => ryu_js::Buffer::new().format(*number).to_owned(),
            // node-postgres encodes JS strings as UTF-8, replacing lone surrogates.
            Self::String(units) => String::from_utf16_lossy(units),
        }
    }
}

impl ToSql for Bound {
    fn to_sql(
        &self,
        _: &Type,
        output: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        // pg's prepareValue sends JS Number/String values as parameter text. Let
        // PostgreSQL's original inferred type reject NaN/fractions/out-of-range
        // values and produce the same QueryFailedError, rather than narrowing here.
        output.extend_from_slice(self.to_parameter_text().as_bytes());
        Ok(IsNull::No)
    }

    fn accepts(kind: &Type) -> bool {
        matches!(*kind, Type::INT2 | Type::INT4)
    }

    fn encode_format(&self, _: &Type) -> Format {
        Format::Text
    }

    to_sql_checked!();
}

#[derive(Clone, Copy)]
enum RangeSide {
    Begin,
    End,
}

fn range_bound(value: &Value, side: RangeSide) -> Result<Bound, Error> {
    if matches!(value, Value::Null | Value::Undefined) {
        // Captured from the pinned production Bun bundle (baseline 21b810d).
        // The source-only oracle says `range.begin.month`; the bundle says `t`.
        let expression = match side {
            RangeSide::Begin => "t.begin.month",
            RangeSide::End => "t.end.month",
        };
        let receiver = match value {
            Value::Null => "null",
            _ => "undefined",
        };
        return Err(Error::legacy(
            format!("{receiver} is not an object (evaluating '{expression}')"),
            "TypeError",
        ));
    }
    // Original body: range.begin.month * 100 + range.begin.day. Primitive
    // receivers have neither field. Multiplication coerces; addition may concat.
    let undefined = Value::Undefined;
    let month = js_value::number(value.get("month").unwrap_or(&undefined))?;
    let day = value.get("day").unwrap_or(&undefined);
    Bound::from_value(&js_value::add(&Value::Number(month * 100.), day)?)
}

#[derive(Debug, PartialEq)]
enum Selection {
    Day(Bound),
    Range {
        begin: Bound,
        end: Bound,
        wraps: bool,
    },
}

impl Selection {
    fn wraps(&self) -> bool {
        matches!(self, Self::Range { wraps: true, .. })
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
        _ => Err(BirthdayFailure::ValidatedInteger.into_error()),
    }
}

fn mmdd(value: &Value) -> Result<i16, Error> {
    if !matches!(value, Value::Object(_)) {
        return Err(BirthdayFailure::BirthdayShape.into_error());
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
            Value::Number(number) if number.is_finite() && number.fract() == 0. => Ok(Self(
                value
                    .stringify()
                    .ok_or_else(|| BirthdayFailure::Pagination.into_error())?,
            )),
            _ => Err(BirthdayFailure::Pagination.into_error()),
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
        let birthday = params
            .get("birthday")
            .ok_or_else(|| BirthdayFailure::BirthdayShape.into_error())?;
        if !matches!(birthday, Value::Object(_)) {
            return Err(BirthdayFailure::BirthdayShape.into_error());
        }
        // Value::get sees own fields only, matching both Object.hasOwn tests.
        let selection = match (birthday.get("begin"), birthday.get("end")) {
            (Some(begin), Some(end)) => {
                let begin = range_bound(begin, RangeSide::Begin)?;
                let end = range_bound(end, RangeSide::End)?;
                let wraps = !begin.less_equal(&end)?;
                Selection::Range { begin, end, wraps }
            }
            _ => Selection::Day(Bound::Number(f64::from(mmdd(birthday)?))),
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

const SINGLE: &str = r#"SELECT "following"."followeeId" AS "user_id", get_birthday_date("followeeProfile"."birthday") AS "birthday_date" FROM "following" "following" INNER JOIN "user_profile" "followeeProfile" ON "followeeProfile"."userId" = "following"."followeeId" WHERE "following"."followerId" = $1 AND get_birthday_date("followeeProfile"."birthday") BETWEEN $2 AND $2 ORDER BY birthday_date ASC"#;
const RANGE: &str = r#"SELECT "following"."followeeId" AS "user_id", get_birthday_date("followeeProfile"."birthday") AS "birthday_date" FROM "following" "following" INNER JOIN "user_profile" "followeeProfile" ON "followeeProfile"."userId" = "following"."followeeId" WHERE "following"."followerId" = $1 AND get_birthday_date("followeeProfile"."birthday") BETWEEN $2 AND $3 ORDER BY birthday_date ASC"#;
const WRAPPED: &str = r#"SELECT "following"."followeeId" AS "user_id", get_birthday_date("followeeProfile"."birthday") AS "birthday_date" FROM "following" "following" INNER JOIN "user_profile" "followeeProfile" ON "followeeProfile"."userId" = "following"."followeeId" WHERE "following"."followerId" = $1 AND (get_birthday_date("followeeProfile"."birthday") BETWEEN $2 AND 1231 OR get_birthday_date("followeeProfile"."birthday") BETWEEN 101 AND $3) ORDER BY birthday_date ASC"#;

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
        Selection::Range { begin, end, .. } => {
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
        .map_err(|kind| {
            // QueryError's Debug deliberately exposes only its typed variant.
            tracing::error!(
                operation = "users.get-following-users-by-birthday.select",
                error.kind = ?kind,
                "birthday query failed"
            );
            Error::query(kind)
        })?
        .into_iter()
        .map(|row| {
            Ok(BirthdayRow {
                user_id: row
                    .try_get("user_id")
                    .map_err(|_| BirthdayFailure::UserIdColumn.into_error())?,
                birthday_date: row
                    .try_get("birthday_date")
                    .map_err(|_| BirthdayFailure::BirthdayColumn.into_error())?,
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
        .and_then(Json::as_str)
        .ok_or_else(|| BirthdayFailure::Viewer.into_error())?;
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
            .map_err(|kind| BirthdayFailure::Calendar(kind).into_error())
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
        assert_eq!(single.selection, Selection::Day(Bound::Number(231.)));
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
    fn accepted_extra_fields_match_original_ts_arithmetic_and_range_branch() {
        // Captured by executing this endpoint's actual TS selection body with
        // Bun 1.4.4 + its AJV schema; these extras all pass the single-day branch.
        let cases = [
            (
                "extra-string-month",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":"1","day":1},"end":{"month":"12","day":31}}}"##,
                "101",
                "1231",
                false,
            ),
            (
                "extra-zero-day",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":1,"day":0},"end":{"month":12,"day":31}}}"##,
                "100",
                "1231",
                false,
            ),
            (
                "review-string-day",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":1,"day":"1"},"end":{"month":12,"day":"31"}}}"##,
                "1001",
                "120031",
                false,
            ),
            (
                "review-fractional",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":1.5,"day":1},"end":{"month":12,"day":31}}}"##,
                "151",
                "1231",
                false,
            ),
            (
                "review-huge-month",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":1000,"day":1},"end":{"month":12,"day":31}}}"##,
                "100001",
                "1231",
                true,
            ),
            (
                "review-negative-month",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":-1,"day":1},"end":{"month":12,"day":31}}}"##,
                "-99",
                "1231",
                false,
            ),
            (
                "review-boolean-month",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":true,"day":1},"end":{"month":12,"day":31}}}"##,
                "101",
                "1231",
                false,
            ),
            (
                "review-null-month",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":null,"day":1},"end":{"month":12,"day":31}}}"##,
                "1",
                "1231",
                false,
            ),
            (
                "review-array-month",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":["1"],"day":1},"end":{"month":12,"day":31}}}"##,
                "101",
                "1231",
                false,
            ),
            (
                "review-array-day",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":1,"day":["1"]},"end":{"month":12,"day":31}}}"##,
                "1001",
                "1231",
                false,
            ),
            (
                "review-object-month",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":{},"day":1},"end":{"month":12,"day":31}}}"##,
                "NaN",
                "1231",
                true,
            ),
            (
                "review-object-day",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":1,"day":{}},"end":{"month":12,"day":31}}}"##,
                "100[object Object]",
                "1231",
                true,
            ),
            (
                "review-missing-month",
                r##"{"birthday":{"month":10,"day":24,"begin":{"day":1},"end":{"month":12,"day":31}}}"##,
                "NaN",
                "1231",
                true,
            ),
            (
                "review-missing-day",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":1},"end":{"month":12,"day":31}}}"##,
                "NaN",
                "1231",
                true,
            ),
            (
                "review-begin-number",
                r##"{"birthday":{"month":10,"day":24,"begin":3,"end":{"month":12,"day":31}}}"##,
                "NaN",
                "1231",
                true,
            ),
            (
                "review-begin-string",
                r##"{"birthday":{"month":10,"day":24,"begin":"abc","end":{"month":12,"day":31}}}"##,
                "NaN",
                "1231",
                true,
            ),
            (
                "review-begin-array",
                r##"{"birthday":{"month":10,"day":24,"begin":[],"end":{"month":12,"day":31}}}"##,
                "NaN",
                "1231",
                true,
            ),
            (
                "review-infinite-month",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":"Infinity","day":1},"end":{"month":12,"day":31}}}"##,
                "Infinity",
                "1231",
                true,
            ),
            (
                "review-lexical-range",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":1,"day":"1"},"end":{"month":2,"day":"2"}}}"##,
                "1001",
                "2002",
                false,
            ),
            (
                "review-null-day",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":1,"day":null},"end":{"month":12,"day":31}}}"##,
                "100",
                "1231",
                false,
            ),
            (
                "review-boolean-day",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":1,"day":false},"end":{"month":12,"day":31}}}"##,
                "100",
                "1231",
                false,
            ),
            (
                "review-hex-month",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":"0x1","day":1},"end":{"month":12,"day":31}}}"##,
                "101",
                "1231",
                false,
            ),
            (
                "month-shadowed-valueOf",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":{"valueOf":1},"day":1},"end":{"month":12,"day":31}}}"##,
                "NaN",
                "1231",
                true,
            ),
            (
                "day-shadowed-valueOf",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":1,"day":{"valueOf":1}},"end":{"month":12,"day":31}}}"##,
                "100[object Object]",
                "1231",
                true,
            ),
            (
                "string-lexical-wrap",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":2,"day":"2"},"end":{"month":1,"day":"1"}}}"##,
                "2002",
                "1001",
                true,
            ),
            (
                "string-lexical-not-numeric",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":10,"day":"1"},"end":{"month":2,"day":"1"}}}"##,
                "10001",
                "2001",
                false,
            ),
            (
                "fractional-result",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":1.005,"day":1},"end":{"month":12,"day":31}}}"##,
                "101.49999999999999",
                "1231",
                false,
            ),
            (
                "day-unpaired-surrogate",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":1,"day":"\ud800"},"end":{"month":12,"day":31}}}"##,
                "100�",
                "1231",
                true,
            ),
            (
                "month-negative-infinity",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":"-Infinity","day":1},"end":{"month":12,"day":31}}}"##,
                "-Infinity",
                "1231",
                false,
            ),
        ];
        for (label, json, expected_begin, expected_end, expected_wraps) in cases {
            let decoded = input(json);
            assert_eq!(decoded.selection.wraps(), expected_wraps, "{label}");
            let Selection::Range { begin, end, .. } = decoded.selection else {
                panic!("expected range: {label}");
            };
            assert_eq!(begin.to_parameter_text(), expected_begin, "{label}");
            assert_eq!(end.to_parameter_text(), expected_end, "{label}");
        }
    }

    #[test]
    fn accepted_extra_field_errors_match_pinned_production_http() {
        // null property messages come from the production bundle's HTTP output;
        // the same source without bundling has a different local variable name.
        let cases = [
            (
                "extra-null",
                r##"{"birthday":{"month":10,"day":24,"begin":null,"end":{}}}"##,
                "TypeError",
                "null is not an object (evaluating 't.begin.month')",
            ),
            (
                "review-end-null",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":1,"day":1},"end":null}}"##,
                "TypeError",
                "null is not an object (evaluating 't.end.month')",
            ),
            (
                "review-shadowed-toString",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":{"toString":null},"day":1},"end":{"month":12,"day":31}}}"##,
                "TypeError",
                "No default value",
            ),
            (
                "day-shadowed-toString",
                r##"{"birthday":{"month":10,"day":24,"begin":{"month":1,"day":{"toString":null}},"end":{"month":12,"day":31}}}"##,
                "TypeError",
                "No default value",
            ),
        ];
        for (label, json, expected_code, expected_message) in cases {
            let decoded = Input::decode(&Value::parse_json(json).unwrap());
            let Err(Error::Legacy { message, code, .. }) = decoded else {
                panic!("expected legacy error: {label}");
            };
            assert_eq!(code, expected_code, "{label}");
            assert_eq!(message, expected_message, "{label}");
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
