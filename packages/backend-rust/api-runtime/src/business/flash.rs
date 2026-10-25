/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
//! Single Flash reads with the source author, like, then Date evaluation order.
pub use super::entity_layout::LayoutFailure as FlashLayoutFailure;
use super::{
    Error,
    entity_layout::{column_name, compile_query, validated_layout},
    notes,
    stored_value::{self, StoredDateFailure, StoredInt4Failure},
};
use crate::Runtime;
use misskey_api_validation::{ParseError, Value};
use misskey_backend_api_auth::{
    ColumnKind, Context, EntityColumn,
    postgres::{quote_identifier, quote_literal},
};
use misskey_backend_postgres::QueryError;
use std::{collections::HashMap, fmt, future::Future};

const SHOW_SQL: &str = "SELECT {projection} FROM {flash_table} f WHERE f.{flash_id}=$1 LIMIT 1";
const LIKED_SQL: &str =
    "SELECT EXISTS(SELECT 1 FROM {like_table} l WHERE l.{flash_id}=$1 AND l.{user_id}=$2)";
// Property names are the consumed TypeORM contract, never copied SQL identifiers.
const FLASH_PROPERTIES: &[(&str, ColumnKind)] = &[
    ("id", ColumnKind::Json),
    ("updatedAt", ColumnKind::Date),
    ("title", ColumnKind::Json),
    ("summary", ColumnKind::Json),
    ("userId", ColumnKind::Json),
    ("script", ColumnKind::Json),
    ("permissions", ColumnKind::Json),
    ("likedCount", ColumnKind::Json),
    ("visibility", ColumnKind::Json),
];
const LIKE_PROPERTIES: &[(&str, ColumnKind)] = &[
    ("id", ColumnKind::Json),
    ("userId", ColumnKind::Json),
    ("flashId", ColumnKind::Json),
];

#[derive(Clone, Copy, Debug)]
pub(crate) enum Operation {
    Show,
    Author,
    Liked,
    Pack,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Show => "flash.show",
            Self::Author => "flash.show.author",
            Self::Liked => "flash.show.liked",
            Self::Pack => "flash.show.pack",
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum ShapeKind {
    QueryShape,
    InvalidParameter,
    InvalidPrincipal,
    MissingField,
    FieldType,
}
#[derive(Debug)]
pub(crate) enum FlashFailure {
    Shape {
        operation: Operation,
        kind: ShapeKind,
    },
    Query {
        operation: Operation,
        source: QueryError,
    },
    Row {
        operation: Operation,
        source: tokio_postgres::Error,
    },
    Json {
        operation: Operation,
        source: ParseError,
    },
    Utf16 {
        source: std::string::FromUtf16Error,
    },
    Int4 {
        source: std::num::ParseIntError,
    },
}
impl FlashFailure {
    pub(super) fn report(&self) {
        match self {
            Self::Shape { operation, kind } => {
                tracing::error!(operation = operation.name(), error.kind = ?kind, "endpoint failed")
            }
            Self::Query { operation, .. } => tracing::error!(
                operation = operation.name(),
                error.kind = "query",
                "endpoint failed"
            ),
            Self::Row { operation, .. } => tracing::error!(
                operation = operation.name(),
                error.kind = "row_decode",
                "endpoint failed"
            ),
            Self::Json { operation, .. } => tracing::error!(
                operation = operation.name(),
                error.kind = "row_json",
                "endpoint failed"
            ),
            Self::Utf16 { .. } => tracing::error!(
                operation = "flash.show.pack",
                error.kind = "row_utf16",
                "endpoint failed"
            ),
            Self::Int4 { .. } => tracing::error!(
                operation = "flash.show.pack",
                error.kind = "row_int4",
                "endpoint failed"
            ),
        }
    }
}
impl fmt::Display for FlashFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("native Flash read failed")
    }
}
impl std::error::Error for FlashFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Shape { .. } => None,
            Self::Query { source, .. } => Some(source),
            Self::Row { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::Utf16 { source } => Some(source),
            Self::Int4 { source } => Some(source),
        }
    }
}
fn shape(operation: Operation, kind: ShapeKind) -> Error {
    Error::Flash(FlashFailure::Shape { operation, kind })
}
fn field<'a>(row: &'a Value, name: &str) -> Result<&'a Value, Error> {
    row.get(name)
        .ok_or_else(|| shape(Operation::Pack, ShapeKind::MissingField))
}
fn string(value: &Value) -> Result<String, Error> {
    match value {
        Value::String(units) => {
            String::from_utf16(units).map_err(|source| Error::Flash(FlashFailure::Utf16 { source }))
        }
        _ => Err(shape(Operation::Pack, ShapeKind::FieldType)),
    }
}

pub(crate) struct FlashQueries {
    show: String,
    liked: String,
}
impl FlashQueries {
    pub(crate) fn new(
        layouts: &HashMap<String, Vec<EntityColumn>>,
    ) -> Result<Self, FlashLayoutFailure> {
        let (flash_table, flash_columns) = validated_layout(layouts, "flash", FLASH_PROPERTIES)?;
        let (like_table, like_columns) = validated_layout(layouts, "flash_like", LIKE_PROPERTIES)?;
        let projection = flash_columns
            .iter()
            // FlashEntityService.pack does not read permissions. In particular,
            // legal nullable/multidimensional array elements cannot affect show.
            .filter(|column| column.property_name != "permissions")
            .map(|column| {
                let reference = format!("f.{}", quote_identifier(&column.database_name));
                let value = match column.kind {
                    ColumnKind::Date => {
                        format!("floor(extract(epoch FROM {reference})*1000)::double precision")
                    }
                    ColumnKind::Json => reference,
                    ColumnKind::SimpleArray => return Err(FlashLayoutFailure::HydrationKind),
                };
                Ok(format!("{}, {value}", quote_literal(&column.property_name)))
            })
            .collect::<Result<Vec<_>, FlashLayoutFailure>>()?
            .join(", ");
        let show = compile_query(
            SHOW_SQL,
            &[
                (
                    "projection",
                    format!("json_build_object({projection})::text"),
                ),
                ("flash_table", quote_identifier(flash_table)),
                ("flash_id", column_name(flash_columns, "id")?),
            ],
        )?;
        let liked = compile_query(
            LIKED_SQL,
            &[
                ("like_table", quote_identifier(like_table)),
                ("flash_id", column_name(like_columns, "flashId")?),
                ("user_id", column_name(like_columns, "userId")?),
            ],
        )?;
        Ok(Self { show, liked })
    }
}

struct FlashRow {
    id: String,
    created_at: String,
    updated_at: String,
    user_id: String,
    title: String,
    summary: String,
    script: String,
    visibility: String,
    liked_count: i32,
}
impl FlashRow {
    fn decode(raw: &Value, id_method: &str) -> Result<Self, Error> {
        let id = string(field(raw, "id")?)?;
        // FlashEntityService.pack reaches these conversions only AFTER author
        // and authenticated like resolution. Never invoke this decoder early.
        let created_at = stored_value::created_at(id_method, &id)?;
        let updated_at =
            stored_value::updated_at(field(raw, "updatedAt")?).map_err(
                |failure| match failure {
                    StoredDateFailure::FieldType => shape(Operation::Pack, ShapeKind::FieldType),
                    StoredDateFailure::Conversion(source) => source,
                },
            )?;
        let liked_count =
            stored_value::int4(field(raw, "likedCount")?).map_err(|failure| match failure {
                StoredInt4Failure::FieldType => shape(Operation::Pack, ShapeKind::FieldType),
                StoredInt4Failure::Parse(source) => Error::Flash(FlashFailure::Int4 { source }),
            })?;
        Ok(Self {
            id,
            created_at,
            updated_at,
            user_id: string(field(raw, "userId")?)?,
            title: string(field(raw, "title")?)?,
            summary: string(field(raw, "summary")?)?,
            script: string(field(raw, "script")?)?,
            visibility: string(field(raw, "visibility")?)?,
            liked_count,
        })
    }
    fn pack(&self, user: Value, is_liked: Option<bool>) -> Value {
        Value::object([
            ("id", Value::string(&self.id)),
            ("createdAt", Value::string(&self.created_at)),
            ("updatedAt", Value::string(&self.updated_at)),
            ("userId", Value::string(&self.user_id)),
            ("user", user),
            ("title", Value::string(&self.title)),
            ("summary", Value::string(&self.summary)),
            ("script", Value::string(&self.script)),
            ("visibility", Value::string(&self.visibility)),
            ("likedCount", Value::Number(f64::from(self.liked_count))),
            (
                "isLiked",
                is_liked.map(Value::Boolean).unwrap_or(Value::Undefined),
            ),
        ])
    }
}

trait FlashStore {
    fn load<'a>(
        &'a self,
        id: &'a str,
    ) -> impl Future<Output = Result<Option<Value>, Error>> + Send + 'a;
    fn author<'a>(
        &'a self,
        user_id: &'a str,
    ) -> impl Future<Output = Result<Value, Error>> + Send + 'a;
    fn liked<'a>(
        &'a self,
        flash_id: &'a str,
        viewer: &'a str,
    ) -> impl Future<Output = Result<bool, Error>> + Send + 'a;
}
struct RuntimeStore<'a> {
    runtime: &'a Runtime,
    auth: &'a Context,
}
impl FlashStore for RuntimeStore<'_> {
    async fn load(&self, id: &str) -> Result<Option<Value>, Error> {
        let operation = Operation::Show;
        let rows = self
            .runtime
            .database
            .query_typed(true, &self.runtime.flash_queries.show, &[&id])
            .await
            .map_err(|source| Error::Flash(FlashFailure::Query { operation, source }))?;
        let row = match rows.as_slice() {
            [] => return Ok(None),
            [row] => row,
            _ => return Err(shape(operation, ShapeKind::QueryShape)),
        };
        let text: String = row
            .try_get(0)
            .map_err(|source| Error::Flash(FlashFailure::Row { operation, source }))?;
        Value::parse_json(&text)
            .map(Some)
            .map_err(|source| Error::Flash(FlashFailure::Json { operation, source }))
    }
    async fn author(&self, user_id: &str) -> Result<Value, Error> {
        notes::pack_user_lite_by_id(self.runtime, user_id, self.auth).await
    }
    async fn liked(&self, flash_id: &str, viewer: &str) -> Result<bool, Error> {
        let operation = Operation::Liked;
        let rows = self
            .runtime
            .database
            .query_typed(
                true,
                &self.runtime.flash_queries.liked,
                &[&flash_id, &viewer],
            )
            .await
            .map_err(|source| Error::Flash(FlashFailure::Query { operation, source }))?;
        let [row] = rows.as_slice() else {
            return Err(shape(operation, ShapeKind::QueryShape));
        };
        row.try_get(0)
            .map_err(|source| Error::Flash(FlashFailure::Row { operation, source }))
    }
}
async fn run(
    store: &(impl FlashStore + Sync),
    params: &Value,
    id_method: &str,
    viewer: Option<&str>,
) -> Result<Value, Error> {
    let flash_id = params
        .get("flashId")
        .and_then(Value::to_utf8)
        .ok_or_else(|| shape(Operation::Show, ShapeKind::InvalidParameter))?;
    let raw = store
        .load(&flash_id)
        .await?
        .ok_or(Error::Fixed(crate::response::ApiFailure::NoSuchFlash))?;
    let author_id = string(field(&raw, "userId")?)?;
    let user = store.author(&author_id).await?;
    let is_liked = match viewer.filter(|id| !id.is_empty()) {
        Some(viewer) => Some(store.liked(&string(field(&raw, "id")?)?, viewer).await?),
        None => None,
    };
    let flash = FlashRow::decode(&raw, id_method)?;
    Ok(flash.pack(user, is_liked))
}
pub(super) async fn show(
    runtime: &Runtime,
    params: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    let viewer = auth
        .user
        .as_ref()
        .map(|user| {
            user.get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| shape(Operation::Author, ShapeKind::InvalidPrincipal))
        })
        .transpose()?;
    run(
        &RuntimeStore { runtime, auth },
        params,
        &runtime.business_config.id_method,
        viewer,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as Json;
    use std::sync::Mutex;

    const SOURCE: &str = include_str!("fixtures/flash-show-source-oracle.json");
    const HTTP: &str = include_str!("fixtures/flash-show-http-oracle.json");
    struct FixtureStore {
        row: Option<Value>,
        liked: bool,
        reject_author: bool,
        reject_like: bool,
        calls: Mutex<Vec<&'static str>>,
        author_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        like_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }
    impl FixtureStore {
        fn new(row: Option<Value>, liked: bool) -> Self {
            Self {
                row,
                liked,
                reject_author: false,
                reject_like: false,
                calls: Mutex::new(Vec::new()),
                author_gate: Mutex::new(None),
                like_gate: Mutex::new(None),
            }
        }
    }
    impl FlashStore for FixtureStore {
        async fn load(&self, _id: &str) -> Result<Option<Value>, Error> {
            self.calls.lock().unwrap().push("load");
            Ok(self.row.clone())
        }
        async fn author(&self, _id: &str) -> Result<Value, Error> {
            self.calls.lock().unwrap().push("author");
            let gate = self.author_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                gate.await.unwrap();
            }
            if self.reject_author {
                return Err(Error::Internal);
            }
            // Shared UserLite content is outside this domain fixture. Only
            // actual source projection and call ordering are compared below.
            Ok(Value::Null)
        }
        async fn liked(&self, _id: &str, _viewer: &str) -> Result<bool, Error> {
            self.calls.lock().unwrap().push("liked");
            let gate = self.like_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                gate.await.unwrap();
            }
            if self.reject_like {
                return Err(Error::Internal);
            }
            Ok(self.liked)
        }
    }
    fn source_row(case: &Json) -> Option<Value> {
        if case["input"]["missing"] == true {
            return None;
        }
        let input = &case["input"]["flash"];
        let mut row = Value::parse_json(&input.to_string()).unwrap();
        let date = &input["updatedAt"];
        let updated = if date["runtime"] == "Date" {
            Value::Number(date["millis"].as_f64().unwrap())
        } else {
            Value::string(date["value"].as_str().unwrap())
        };
        row.set("updatedAt", updated);
        Some(row)
    }
    fn source_calls(trace: &Json) -> Vec<&'static str> {
        trace
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|event| match event["operation"].as_str().unwrap() {
                "fake.flash.findOneBy" => Some("load"),
                "stub.UserLite.start" => Some("author"),
                "fake.flashLike.exists.start" => Some("liked"),
                _ => None,
            })
            .collect()
    }
    fn viewer(case: &Json) -> Option<&str> {
        case["input"]["viewer"]["id"].as_str()
    }

    #[tokio::test]
    async fn projection_viewer_like_omission_raw_visibility_and_int4_match_actual_source() {
        let oracle: Json = serde_json::from_str(SOURCE).unwrap();
        for case in oracle["cases"].as_array().unwrap().iter().filter(|case| {
            case.get("expectedProjection").is_some()
                && case["input"]["flash"]["summary"].is_string()
                && case["input"]["flash"]["script"].is_string()
        }) {
            let store =
                FixtureStore::new(source_row(case), case["input"]["liked"].as_bool().unwrap());
            let params = Value::parse_json(&case["params"].to_string()).unwrap();
            let mut actual = run(&store, &params, "aidx", viewer(case)).await.unwrap();
            actual.set("user", Value::Undefined);
            let expected = Value::parse_json(&case["expectedProjection"].to_string()).unwrap();
            assert_eq!(actual.stringify(), expected.stringify(), "{}", case["name"]);
            assert_eq!(*store.calls.lock().unwrap(), source_calls(&case["trace"]));
        }
    }

    #[tokio::test]
    async fn actual_native_path_keeps_source_author_then_like_before_date_rejections() {
        let oracle: Json = serde_json::from_str(SOURCE).unwrap();
        for case in oracle["cases"].as_array().unwrap().iter().filter(|case| {
            case.get("observedError").is_some()
                && case["name"] != "invalid-Date-after-author-like-createdAt"
        }) {
            let mut store =
                FixtureStore::new(source_row(case), case["input"]["liked"].as_bool().unwrap());
            store.reject_author = case["input"]["controls"]["rejectAuthor"].as_bool().unwrap();
            store.reject_like = case["input"]["controls"]["rejectLike"].as_bool().unwrap();
            let params = Value::parse_json(&case["params"].to_string()).unwrap();
            let error = run(&store, &params, "aidx", viewer(case))
                .await
                .unwrap_err();
            if store.reject_author || store.reject_like {
                assert!(matches!(error, Error::Internal));
            } else if case["input"]["missing"] == true {
                assert!(matches!(
                    error,
                    Error::Fixed(crate::response::ApiFailure::NoSuchFlash)
                ));
            } else {
                let Error::Legacy { code, .. } = error else {
                    panic!("source Date rejection expected")
                };
                // Source TypeError expression is unminified; public wording is
                // compared separately against actual production HTTP below.
                assert_eq!(code, case["observedError"]["name"]);
            }
            assert_eq!(
                *store.calls.lock().unwrap(),
                source_calls(&case["trace"]),
                "{}",
                case["name"]
            );
        }
    }

    #[tokio::test]
    async fn serial_native_author_and_like_barriers_match_actual_source_control() {
        let oracle: Json = serde_json::from_str(SOURCE).unwrap();
        let case = oracle["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == "deferred-author-then-like-then-dates")
            .unwrap();
        let store = FixtureStore::new(source_row(case), case["input"]["liked"].as_bool().unwrap());
        let (author_sender, author_receiver) = tokio::sync::oneshot::channel();
        let (like_sender, like_receiver) = tokio::sync::oneshot::channel();
        *store.author_gate.lock().unwrap() = Some(author_receiver);
        *store.like_gate.lock().unwrap() = Some(like_receiver);
        let params = Value::parse_json(&case["params"].to_string()).unwrap();
        let mut pending = Box::pin(run(&store, &params, "aidx", viewer(case)));
        assert!(futures_util::poll!(&mut pending).is_pending());
        assert_eq!(
            *store.calls.lock().unwrap(),
            source_calls(&case["beforeAuthorResolve"])
        );
        author_sender.send(()).unwrap();
        assert!(futures_util::poll!(&mut pending).is_pending());
        assert_eq!(
            *store.calls.lock().unwrap(),
            source_calls(&case["beforeLikeResolve"])
        );
        like_sender.send(()).unwrap();
        let mut actual = pending.await.unwrap();
        actual.set("user", Value::Undefined);
        assert_eq!(
            actual.stringify(),
            Value::parse_json(&case["expectedProjection"].to_string())
                .unwrap()
                .stringify()
        );
    }

    #[tokio::test]
    async fn pure_projection_matches_actual_http_successes_without_asserting_user_stubs() {
        let oracle: Json = serde_json::from_str(HTTP).unwrap();
        for case in oracle["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["status"] == 200)
        {
            let mut raw = Value::parse_json(&case["body"].to_string()).unwrap();
            let timestamp: jiff::Timestamp =
                case["body"]["updatedAt"].as_str().unwrap().parse().unwrap();
            // These actual capture timestamps are integral and below2^53.
            raw.set(
                "updatedAt",
                Value::Number(timestamp.as_millisecond() as f64),
            );
            let is_liked = case["body"]
                .get("isLiked")
                .map(|value| value.as_bool().unwrap());
            let store = FixtureStore::new(Some(raw), is_liked.unwrap_or(false));
            let params = Value::object([(
                "flashId",
                Value::string(case["body"]["id"].as_str().unwrap()),
            )]);
            let mut actual = run(&store, &params, "aidx", is_liked.map(|_| "viewer"))
                .await
                .unwrap();
            actual.set("user", Value::Undefined);
            assert_eq!(
                actual.stringify(),
                Value::parse_json(&case["body"].to_string())
                    .unwrap()
                    .stringify(),
                "{}",
                case["id"]
            );
        }
    }

    #[tokio::test]
    async fn missing_flash_response_matches_actual_http_without_starting_author() {
        let oracle: Json = serde_json::from_str(HTTP).unwrap();
        let case = oracle["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == "missing-flash:anon")
            .unwrap();
        let source: Json = serde_json::from_str(SOURCE).unwrap();
        let source_case = &source["cases"][0];
        let params = Value::parse_json(&source_case["params"].to_string()).unwrap();
        let store = FixtureStore::new(None, false);
        let response = run(&store, &params, "aidx", None)
            .await
            .unwrap_err()
            .response();
        assert_eq!(
            u64::from(response.status().as_u16()),
            case["status"].as_u64().unwrap()
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(serde_json::from_slice::<Json>(&body).unwrap(), case["body"]);
        assert_eq!(
            *store.calls.lock().unwrap(),
            source_calls(&source_case["trace"])
        );
    }
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
    async fn stored_timestamp_error_envelopes_match_actual_production_http() {
        let oracle: Json = serde_json::from_str(HTTP).unwrap();
        let source: Json = serde_json::from_str(SOURCE).unwrap();
        for case in oracle["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["status"] == 500)
        {
            let mut raw = source_row(&source["cases"][1]).unwrap();
            let timestamp = match case["dateInput"].as_str().unwrap() {
                "infinity" => Value::string("Infinity"),
                "-infinity" => Value::string("-Infinity"),
                // Actual HTTP captured finite PG year294000. This pure test
                // samples the same finite-outside-TimeClip class at its first
                // boundary; exact SQL hydration remains the HTTP differential.
                "294000-01-01 00:00:00+00" => Value::Number(8_640_000_000_000_001.),
                _ => panic!("unknown captured timestamp class"),
            };
            raw.set("updatedAt", timestamp);
            let response = FlashRow::decode(&raw, "aidx").err().unwrap().response();
            assert_eq!(
                u64::from(response.status().as_u16()),
                case["status"].as_u64().unwrap()
            );
            let bytes = axum::body::to_bytes(response.into_body(), 16 * 1024)
                .await
                .unwrap();
            let actual = Value::parse_json(std::str::from_utf8(&bytes).unwrap()).unwrap();
            let expected = Value::parse_json(&case["body"].to_string()).unwrap();
            assert_eq!(
                without_request_correlation(actual),
                without_request_correlation(expected),
                "{}",
                case["id"]
            );
        }
    }
}
