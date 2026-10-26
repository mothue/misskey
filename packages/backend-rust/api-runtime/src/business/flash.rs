/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
//! Complete Flash reads with source ordering, viewer and packing semantics.
pub use super::entity_layout::LayoutFailure as FlashLayoutFailure;
use super::{
    Error,
    eager_batch::{PackFailure, eager_batch},
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
use std::{
    collections::{HashMap, HashSet},
    fmt,
    future::Future,
};

const SHOW_SQL: &str = "SELECT {projection} FROM {flash_table} f WHERE f.{flash_id}=$1 LIMIT 1";
const FEATURED_SQL: &str = "SELECT {projection} FROM {flash_table} f WHERE f.{liked_count}>0 AND f.{visibility}=$1 ORDER BY f.{liked_count} DESC,f.{updated_at} DESC,f.{flash_id} DESC";
const LIKED_IDS_SQL: &str = "SELECT l.{flash_id} FROM {like_table} l WHERE l.{user_id}=$1";
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
    Featured,
    FeaturedAuthors,
    FeaturedLikedIds,
    FeaturedPack,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Show => "flash.show",
            Self::Author => "flash.show.author",
            Self::Liked => "flash.show.liked",
            Self::Pack => "flash.show.pack",
            Self::Featured => "flash.featured",
            Self::FeaturedAuthors => "flash.featured.authors",
            Self::FeaturedLikedIds => "flash.featured.liked_ids",
            Self::FeaturedPack => "flash.featured.pack",
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
        operation: Operation,
        source: std::string::FromUtf16Error,
    },
    Int4 {
        operation: Operation,
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
            Self::Utf16 { operation, .. } => tracing::error!(
                operation = operation.name(),
                error.kind = "row_utf16",
                "endpoint failed"
            ),
            Self::Int4 { operation, .. } => tracing::error!(
                operation = operation.name(),
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
            Self::Utf16 { source, .. } => Some(source),
            Self::Int4 { source, .. } => Some(source),
        }
    }
}
fn shape(operation: Operation, kind: ShapeKind) -> Error {
    Error::Flash(FlashFailure::Shape { operation, kind })
}
fn field<'a>(row: &'a Value, name: &str) -> Result<&'a Value, Error> {
    field_for(row, name, Operation::Pack)
}
fn field_for<'a>(row: &'a Value, name: &str, operation: Operation) -> Result<&'a Value, Error> {
    row.get(name)
        .ok_or_else(|| shape(operation, ShapeKind::MissingField))
}
fn string(value: &Value) -> Result<String, Error> {
    string_for(value, Operation::Pack)
}
fn string_for(value: &Value, operation: Operation) -> Result<String, Error> {
    match value {
        Value::String(units) => String::from_utf16(units)
            .map_err(|source| Error::Flash(FlashFailure::Utf16 { operation, source })),
        _ => Err(shape(operation, ShapeKind::FieldType)),
    }
}

pub(crate) struct FlashQueries {
    show: String,
    liked: String,
    featured: String,
    liked_ids: String,
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
        let featured = compile_query(
            FEATURED_SQL,
            &[
                (
                    "projection",
                    format!("json_build_object({projection})::text"),
                ),
                ("flash_table", quote_identifier(flash_table)),
                ("flash_id", column_name(flash_columns, "id")?),
                ("liked_count", column_name(flash_columns, "likedCount")?),
                ("visibility", column_name(flash_columns, "visibility")?),
                ("updated_at", column_name(flash_columns, "updatedAt")?),
            ],
        )?;
        let liked_ids = compile_query(
            LIKED_IDS_SQL,
            &[
                ("like_table", quote_identifier(like_table)),
                ("flash_id", column_name(like_columns, "flashId")?),
                ("user_id", column_name(like_columns, "userId")?),
            ],
        )?;
        Ok(Self {
            show,
            liked,
            featured,
            liked_ids,
        })
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
    fn decode(raw: &Value, id_method: &str, operation: Operation) -> Result<Self, Error> {
        let get = |name| field_for(raw, name, operation);
        let text = |name| string_for(get(name)?, operation);
        let id = text("id")?;
        // FlashEntityService.pack reaches these conversions only AFTER author
        // and authenticated like resolution. Never invoke this decoder early.
        let created_at = stored_value::created_at(id_method, &id)?;
        let updated_at =
            stored_value::updated_at(get("updatedAt")?).map_err(|failure| match failure {
                StoredDateFailure::FieldType => shape(operation, ShapeKind::FieldType),
                StoredDateFailure::Conversion(source) => source,
            })?;
        let liked_count =
            stored_value::int4(get("likedCount")?).map_err(|failure| match failure {
                StoredInt4Failure::FieldType => shape(operation, ShapeKind::FieldType),
                StoredInt4Failure::Parse(source) => {
                    Error::Flash(FlashFailure::Int4 { operation, source })
                }
            })?;
        Ok(Self {
            id,
            created_at,
            updated_at,
            user_id: text("userId")?,
            title: text("title")?,
            summary: text("summary")?,
            script: text("script")?,
            visibility: text("visibility")?,
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

fn project_loaded(
    raw: &Value,
    id_method: &str,
    user: Value,
    is_liked: Option<bool>,
    operation: Operation,
) -> Result<Value, Error> {
    let flash = FlashRow::decode(raw, id_method, operation)?;
    Ok(flash.pack(user, is_liked))
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
    project_loaded(&raw, id_method, user, is_liked, Operation::Pack)
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

#[derive(Clone, Copy)]
struct Pagination {
    limit: f64,
    offset: f64,
}
impl Pagination {
    fn from_params(params: &Value) -> Result<Self, Error> {
        let number = |name| match params.get(name) {
            Some(Value::Number(value)) if value.is_finite() && value.fract() == 0. => Ok(*value),
            _ => Err(shape(Operation::Featured, ShapeKind::InvalidParameter)),
        };
        let limit = number("limit")?;
        let offset = number("offset")?;
        if !(1. ..=100.).contains(&limit) || offset < 0. {
            return Err(shape(Operation::Featured, ShapeKind::InvalidParameter));
        }
        Ok(Self { limit, offset })
    }
    fn sql(self) -> String {
        // TypeORM inserts validated Number text. Actual TS HTTP proves that
        // overflowing offsets must reach PG and its QueryFailedError mapper.
        let mut buffer = ryu_js::Buffer::new();
        let limit = buffer.format(self.limit).to_owned();
        if self.offset == 0. {
            format!(" LIMIT {limit}")
        } else {
            let offset = buffer.format(self.offset);
            format!(" LIMIT {limit} OFFSET {offset}")
        }
    }
}
trait FeaturedStore {
    fn select(&self, page: Pagination) -> impl Future<Output = Result<Vec<Value>, Error>> + Send;
    fn authors<'a>(
        &'a self,
        ids: &'a [String],
    ) -> impl Future<Output = Result<HashMap<String, Value>, Error>> + Send + 'a;
    fn liked_ids<'a>(
        &'a self,
        viewer: &'a str,
    ) -> impl Future<Output = Result<Vec<String>, Error>> + Send + 'a;
    fn author<'a>(&'a self, id: &'a str) -> impl Future<Output = Result<Value, Error>> + Send + 'a;
}
impl FeaturedStore for RuntimeStore<'_> {
    async fn select(&self, page: Pagination) -> Result<Vec<Value>, Error> {
        let operation = Operation::Featured;
        let sql = format!("{}{}", self.runtime.flash_queries.featured, page.sql());
        let rows = self
            .runtime
            .database
            .query_typed(true, &sql, &[&"public"])
            .await
            .map_err(|source| Error::Flash(FlashFailure::Query { operation, source }))?;
        rows.iter()
            .map(|row| {
                let text: String = row
                    .try_get(0)
                    .map_err(|source| Error::Flash(FlashFailure::Row { operation, source }))?;
                Value::parse_json(&text)
                    .map_err(|source| Error::Flash(FlashFailure::Json { operation, source }))
            })
            .collect()
    }
    async fn authors(&self, ids: &[String]) -> Result<HashMap<String, Value>, Error> {
        notes::pack_many_users(self.runtime, ids, self.auth).await
    }
    async fn liked_ids(&self, viewer: &str) -> Result<Vec<String>, Error> {
        let operation = Operation::FeaturedLikedIds;
        let rows = self
            .runtime
            .database
            .query_typed(true, &self.runtime.flash_queries.liked_ids, &[&viewer])
            .await
            .map_err(|source| Error::Flash(FlashFailure::Query { operation, source }))?;
        rows.iter()
            .map(|row| {
                row.try_get(0)
                    .map_err(|source| Error::Flash(FlashFailure::Row { operation, source }))
            })
            .collect()
    }
    async fn author(&self, id: &str) -> Result<Value, Error> {
        FlashStore::author(self, id).await
    }
}
fn featured_projection(
    raw: &Value,
    method: &str,
    user: Value,
    viewer: Option<&str>,
    liked: &HashSet<String>,
) -> Result<Value, Error> {
    let operation = Operation::FeaturedPack;
    let is_liked = match viewer.filter(|id| !id.is_empty()) {
        Some(_) => Some(liked.contains(&string_for(field_for(raw, "id", operation)?, operation)?)),
        None => None,
    };
    project_loaded(raw, method, user, is_liked, operation)
}
async fn pack_featured_rows(
    store: &(impl FeaturedStore + Sync),
    rows: &[Value],
    method: &str,
    viewer: Option<&str>,
) -> Result<Value, Error> {
    let operation = Operation::FeaturedAuthors;
    let ids = rows
        .iter()
        .map(|row| string_for(field_for(row, "userId", operation)?, operation))
        .collect::<Result<Vec<_>, Error>>()?;
    let authors = store.authors(&ids).await?;
    // Source fetches ALL viewer likes even when selected rows is empty. [] is
    // a supplied truthy hint, so a nonempty viewer never needs per-row EXISTS.
    let liked: HashSet<_> = match viewer {
        Some(viewer) => store.liked_ids(viewer).await?.into_iter().collect(),
        None => HashSet::new(),
    };
    let output = eager_batch(rows.iter().map(|raw| {
        let authors = &authors;
        let liked = &liked;
        async move {
            let operation = Operation::FeaturedPack;
            let id = string_for(
                field_for(raw, "userId", operation).map_err(PackFailure::Preparation)?,
                operation,
            )
            .map_err(PackFailure::Preparation)?;
            match authors
                .get(&id)
                .filter(|user| !matches!(user, Value::Null | Value::Undefined))
            {
                Some(user) => featured_projection(raw, method, user.clone(), viewer, liked)
                    .map_err(PackFailure::Preparation),
                None => {
                    let user = store.author(&id).await.map_err(PackFailure::Async)?;
                    // A Date error after this author await is asynchronous,
                    // unlike the immediately projected hinted branch above.
                    featured_projection(raw, method, user, viewer, liked)
                        .map_err(PackFailure::Async)
                }
            }
        }
    }))
    .await?;
    Ok(Value::Array(output))
}
async fn run_featured(
    store: &(impl FeaturedStore + Sync),
    params: &Value,
    method: &str,
    viewer: Option<&str>,
) -> Result<Value, Error> {
    let rows = store.select(Pagination::from_params(params)?).await?;
    pack_featured_rows(store, &rows, method, viewer).await
}
pub(super) async fn featured(
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
                .ok_or_else(|| shape(Operation::FeaturedAuthors, ShapeKind::InvalidPrincipal))
        })
        .transpose()?;
    run_featured(
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
            let response = FlashRow::decode(&raw, "aidx", Operation::Pack)
                .err()
                .unwrap()
                .response();
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

#[cfg(test)]
mod featured_tests {
    use super::*;
    use serde_json::Value as Json;
    use std::sync::Mutex;

    const BATCH: &str = include_str!("fixtures/flash-featured-batch-source-oracle.json");
    const HTTP: &str = include_str!("fixtures/flash-featured-http-oracle.json");
    struct Store {
        rows: Vec<Value>,
        authors: HashMap<String, Value>,
        likes: Vec<String>,
        viewer_id: Option<String>,
        author_inputs: Vec<String>,
        calls: Mutex<Vec<&'static str>>,
        reject_authors: bool,
        reject_likes: bool,
        reject_fallback: bool,
        authors_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        likes_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        fallback_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }
    impl FeaturedStore for Store {
        async fn select(&self, _page: Pagination) -> Result<Vec<Value>, Error> {
            self.calls.lock().unwrap().push("select");
            Ok(self.rows.clone())
        }
        async fn authors(&self, ids: &[String]) -> Result<HashMap<String, Value>, Error> {
            assert_eq!(ids, self.author_inputs);
            self.calls.lock().unwrap().push("authors");
            let gate = self.authors_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                gate.await.unwrap();
            }
            if self.reject_authors {
                return Err(Error::Internal);
            }
            Ok(self.authors.clone())
        }
        async fn liked_ids(&self, viewer: &str) -> Result<Vec<String>, Error> {
            assert_eq!(Some(viewer), self.viewer_id.as_deref());
            self.calls.lock().unwrap().push("likes");
            let gate = self.likes_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                gate.await.unwrap();
            }
            if self.reject_likes {
                return Err(Error::Internal);
            }
            Ok(self.likes.clone())
        }
        async fn author(&self, _id: &str) -> Result<Value, Error> {
            self.calls.lock().unwrap().push("fallback");
            let gate = self.fallback_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                gate.await.unwrap();
            }
            if self.reject_fallback {
                return Err(Error::Internal);
            }
            Ok(Value::Null)
        }
    }
    fn raw_row(input: &Json) -> Value {
        let mut row = Value::parse_json(&input.to_string()).unwrap();
        let date = &input["updatedAt"];
        let timestamp = if date["runtime"] == "Date" {
            if date["invalid"] == true {
                // Invalid source Date and finite PG year294000 share the
                // captured RangeError class. This is a pure class boundary;
                // exact PostgreSQL hydration is the actual HTTP comparison.
                Value::Number(8_640_000_000_000_001.)
            } else {
                Value::Number(date["millis"].as_f64().unwrap())
            }
        } else {
            Value::string(date["value"].as_str().unwrap())
        };
        row.set("updatedAt", timestamp);
        row
    }
    fn fixture_store(case: &Json) -> Store {
        let input = &case["input"];
        Store {
            rows: input["repositoryRowsInReturnOrder"]
                .as_array()
                .unwrap()
                .iter()
                .map(raw_row)
                .collect(),
            authors: input["returnedAuthorStubs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|user| {
                    (
                        user["id"].as_str().unwrap().to_owned(),
                        Value::parse_json(&user.to_string()).unwrap(),
                    )
                })
                .collect(),
            likes: input["rawViewerLikeRows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["flashLike_flashId"].as_str().unwrap().to_owned())
                .collect(),
            viewer_id: input["viewer"]["id"].as_str().map(str::to_owned),
            author_inputs: case["trace"]
                .as_array()
                .unwrap()
                .iter()
                .find(|event| event["operation"] == "stub.UserLite.packMany.start")
                .unwrap()["ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_str().unwrap().to_owned())
                .collect(),
            calls: Mutex::new(Vec::new()),
            reject_authors: input["controls"]["rejectAuthors"].as_bool().unwrap(),
            reject_likes: input["controls"]["rejectLikes"].as_bool().unwrap(),
            reject_fallback: input["controls"]["rejectFallback"].as_bool().unwrap(),
            authors_gate: Mutex::new(None),
            likes_gate: Mutex::new(None),
            fallback_gate: Mutex::new(None),
        }
    }
    fn source_calls(trace: &Json) -> Vec<&'static str> {
        trace
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|event| match event["operation"].as_str().unwrap() {
                "fake.featured.getMany" => Some("select"),
                "stub.UserLite.packMany.start" => Some("authors"),
                "fake.allViewerLikes.start" => Some("likes"),
                "stub.UserLite.fallback.start" => Some("fallback"),
                _ => None,
            })
            .collect()
    }
    fn viewer(case: &Json) -> Option<&str> {
        case["input"]["viewer"]["id"].as_str()
    }
    fn without_users(mut value: Value) -> Value {
        if let Value::Array(rows) = &mut value {
            for row in rows {
                row.set("user", Value::Undefined);
            }
        }
        value
    }
    async fn run_case(store: &Store, case: &Json) -> Result<Value, Error> {
        if case["entrypoint"] == "packMany" {
            pack_featured_rows(store, &store.rows, "aidx", viewer(case)).await
        } else {
            let params = Value::parse_json(&case["params"].to_string()).unwrap();
            run_featured(store, &params, "aidx", viewer(case)).await
        }
    }
    #[tokio::test]
    async fn batch_projection_empty_likes_duplicates_order_fallback_and_error_priority_match_source()
     {
        let oracle: Json = serde_json::from_str(BATCH).unwrap();
        for case in oracle["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["name"] != "null-summary-script-packer-only-control")
        {
            let store = fixture_store(case);
            let result = run_case(&store, case).await;
            if let Some(expected) = case.get("expectedProjection") {
                assert_eq!(
                    without_users(result.unwrap()).stringify(),
                    Value::parse_json(&expected.to_string())
                        .unwrap()
                        .stringify(),
                    "{}",
                    case["name"]
                );
            } else if store.reject_authors || store.reject_likes {
                assert!(matches!(result.unwrap_err(), Error::Internal));
            } else {
                let Error::Legacy { code, .. } = result.unwrap_err() else {
                    panic!("captured Date rejection must win")
                };
                assert_eq!(code, case["observedError"]["name"], "{}", case["name"]);
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
    async fn author_and_all_likes_barriers_match_actual_source_and_fallback_keeps_result_order() {
        let oracle: Json = serde_json::from_str(BATCH).unwrap();
        let case = oracle["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == "deferred-authors-then-all-likes-then-eager-pack")
            .unwrap();
        let store = fixture_store(case);
        let (author_tx, author_rx) = tokio::sync::oneshot::channel();
        let (likes_tx, likes_rx) = tokio::sync::oneshot::channel();
        *store.authors_gate.lock().unwrap() = Some(author_rx);
        *store.likes_gate.lock().unwrap() = Some(likes_rx);
        let mut pending = Box::pin(run_case(&store, case));
        assert!(futures_util::poll!(&mut pending).is_pending());
        assert_eq!(
            *store.calls.lock().unwrap(),
            source_calls(&case["beforeAuthorsResolve"])
        );
        author_tx.send(()).unwrap();
        assert!(futures_util::poll!(&mut pending).is_pending());
        assert_eq!(
            *store.calls.lock().unwrap(),
            source_calls(&case["beforeLikesResolve"])
        );
        likes_tx.send(()).unwrap();
        assert_eq!(
            without_users(pending.await.unwrap()).stringify(),
            Value::parse_json(&case["expectedProjection"].to_string())
                .unwrap()
                .stringify()
        );

        let case = oracle["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| {
                case["name"] == "absent-author-hint-fallback-viewer-and-input-result-order"
            })
            .unwrap();
        let fallback_store = fixture_store(case);
        let (fallback_tx, fallback_rx) = tokio::sync::oneshot::channel();
        *fallback_store.fallback_gate.lock().unwrap() = Some(fallback_rx);
        let mut pending = Box::pin(run_case(&fallback_store, case));
        assert!(futures_util::poll!(&mut pending).is_pending());
        assert_eq!(
            *fallback_store.calls.lock().unwrap(),
            source_calls(&case["beforeFallbackResolve"])
        );
        fallback_tx.send(()).unwrap();
        assert_eq!(
            without_users(pending.await.unwrap()).stringify(),
            Value::parse_json(&case["expectedProjection"].to_string())
                .unwrap()
                .stringify()
        );
    }
    #[test]
    fn canonical_numeric_pagination_matches_actual_typeorm_text_without_integer_narrowing() {
        let http: Json = serde_json::from_str(HTTP).unwrap();
        let oracle = &http["sourcePaginationOracle"];
        for case in oracle["cases"].as_array().unwrap() {
            let mut params = match case.get("params") {
                Some(value) => Value::parse_json(&value.to_string()).unwrap(),
                None => Value::object(std::iter::empty::<(&str, Value)>()),
            };
            for name in ["offset", "limit"] {
                if params.get(name).is_none() {
                    params.set(
                        name,
                        Value::parse_json(
                            &oracle["paramDef"]["properties"][name]["default"].to_string(),
                        )
                        .unwrap(),
                    );
                }
            }
            if case["inputOffset"]["negativeZero"] == true {
                params.set("offset", Value::Number(-0.));
            }
            let expected = case["trace"]
                .as_array()
                .unwrap()
                .iter()
                .find(|event| event["operation"] == "actual.TypeORM.limitOffsetExpression")
                .unwrap()["text"]
                .as_str()
                .unwrap();
            assert_eq!(
                Pagination::from_params(&params).unwrap().sql(),
                expected,
                "{}",
                case["name"]
            );
        }
    }
    #[test]
    fn actual_http_success_rows_reuse_flash_projection_without_asserting_shared_user_payloads() {
        let oracle: Json = serde_json::from_str(HTTP).unwrap();
        for case in oracle["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["status"] == 200)
        {
            for expected in case["body"].as_array().unwrap() {
                let mut raw = Value::parse_json(&expected.to_string()).unwrap();
                let timestamp: jiff::Timestamp =
                    expected["updatedAt"].as_str().unwrap().parse().unwrap();
                // Actual captured whole-millisecond dates are below2^53.
                raw.set(
                    "updatedAt",
                    Value::Number(timestamp.as_millisecond() as f64),
                );
                let is_liked = expected
                    .get("isLiked")
                    .map(|value| value.as_bool().unwrap());
                let mut actual =
                    project_loaded(&raw, "aidx", Value::Null, is_liked, Operation::FeaturedPack)
                        .unwrap();
                actual.set("user", Value::Undefined);
                assert_eq!(
                    actual.stringify(),
                    Value::parse_json(&expected.to_string())
                        .unwrap()
                        .stringify(),
                    "{}",
                    case["id"]
                );
            }
        }
    }
}
