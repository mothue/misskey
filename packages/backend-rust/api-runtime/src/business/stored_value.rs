/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
//! Shared stored scalar conversion with caller-owned row-shape errors.
use super::{Error, iso_date, parse_id_date};
use misskey_api_validation::Value;
use std::num::ParseIntError;

const TIME_CLIP_LIMIT: f64 = 8_640_000_000_000_000.;

#[derive(Debug)]
pub(super) enum StoredDateFailure {
    FieldType,
    Conversion(Error),
}

#[derive(Debug)]
pub(super) enum StoredInt4Failure {
    FieldType,
    Parse(ParseIntError),
}

fn invalid_date() -> Error {
    // Actual gallery TS HTTP: finite PG year294000 becomes an invalid JS Date.
    // Reuse the shared legacy/correlation/envelope mapper, not a local payload.
    let definition = &crate::response::INVALID_DATE;
    Error::legacy(definition.message.to_owned(), definition.code)
}
fn non_date_updated_timestamp(definition: &crate::response::LegacyErrorDef) -> Error {
    Error::legacy(definition.message.to_owned(), definition.code)
}

/// Matches the typed Date projection, including PG infinities that pg/TypeORM
/// exposes as primitive infinities rather than Date instances.
enum UpdatedDate {
    Finite(f64),
    PositiveInfinity,
    NegativeInfinity,
}
impl UpdatedDate {
    fn decode(value: &Value) -> Result<Self, StoredDateFailure> {
        match value {
            Value::Number(milliseconds) if milliseconds.is_finite() => {
                Ok(Self::Finite(*milliseconds))
            }
            Value::String(units) if units.iter().copied().eq("Infinity".encode_utf16()) => {
                Ok(Self::PositiveInfinity)
            }
            Value::String(units) if units.iter().copied().eq("-Infinity".encode_utf16()) => {
                Ok(Self::NegativeInfinity)
            }
            _ => Err(StoredDateFailure::FieldType),
        }
    }
    fn iso(&self, non_date_error: &crate::response::LegacyErrorDef) -> Result<String, Error> {
        match self {
            Self::Finite(milliseconds) if milliseconds.abs() <= TIME_CLIP_LIMIT => {
                iso_date(*milliseconds)
            }
            Self::Finite(_) => Err(invalid_date()),
            Self::PositiveInfinity | Self::NegativeInfinity => {
                Err(non_date_updated_timestamp(non_date_error))
            }
        }
    }
}
pub(super) fn created_at(method: &str, id: &str) -> Result<String, Error> {
    let milliseconds = parse_id_date(method, id).ok_or_else(invalid_date)?;
    iso_date(milliseconds)
}

pub(super) fn updated_at(value: &Value) -> Result<String, StoredDateFailure> {
    updated_at_with_error(value, &crate::response::NON_DATE_UPDATED_AT)
}

/// Date hydration/TimeClip remain shared; only the observed JS call-site
/// expression in a primitive timestamp's TypeError belongs to the caller.
pub(super) fn updated_at_with_error(
    value: &Value,
    non_date_error: &crate::response::LegacyErrorDef,
) -> Result<String, StoredDateFailure> {
    UpdatedDate::decode(value)?
        .iso(non_date_error)
        .map_err(StoredDateFailure::Conversion)
}

pub(super) fn int4(value: &Value) -> Result<i32, StoredInt4Failure> {
    match value {
        Value::Number(value) => {
            // int4 values are exact Numbers. Checked integer parsing also
            // rejects fractions and overflow without a narrowing cast.
            ryu_js::Buffer::new()
                .format(*value)
                .parse()
                .map_err(StoredInt4Failure::Parse)
        }
        _ => Err(StoredInt4Failure::FieldType),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as Json;

    const DATE_HTTP: &str = include_str!("fixtures/gallery-date-http-oracle.json");

    fn without_request_correlation(mut body: Value) -> Value {
        let mut error = body.get("error").unwrap().clone();
        let mut info = error.get("info").unwrap().clone();
        let mut detail = info.get("e").unwrap().clone();
        detail.set("id", Value::Undefined);
        info.set("e", detail);
        error.set("info", info);
        body.set("error", error);
        body
    }

    #[tokio::test]
    async fn date_failure_projection_matches_captured_production_http_fields() {
        let fixture: Json = serde_json::from_str(DATE_HTTP).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let date = match case["dateKind"].as_str().unwrap() {
                "positive-infinity" => Value::string("Infinity"),
                "negative-infinity" => Value::string("-Infinity"),
                // The expected body is captured for year294000. This samples
                // the same finite-outside-TimeClip class at its first boundary;
                // real SQL-date/native parity remains an end-to-end HTTP check.
                "finite-outside-js" => Value::Number(TIME_CLIP_LIMIT + 1.),
                _ => panic!("unknown captured date class"),
            };
            let StoredDateFailure::Conversion(error) = updated_at(&date).unwrap_err() else {
                panic!("captured dates must decode before conversion fails");
            };
            let response = error.response();
            assert_eq!(
                u64::from(response.status().as_u16()),
                case["status"].as_u64().unwrap()
            );
            let bytes = axum::body::to_bytes(response.into_body(), 16 * 1024)
                .await
                .unwrap();
            let actual = Value::parse_json(std::str::from_utf8(&bytes).unwrap()).unwrap();
            let expected =
                Value::parse_json(&case["bodyWithoutRequestCorrelationId"].to_string()).unwrap();
            assert_eq!(
                without_request_correlation(actual),
                without_request_correlation(expected),
                "{}",
                case["name"]
            );
        }
    }
}
