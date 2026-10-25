/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
//! Complete gallery post detail reads, preserving public attachment and viewer semantics.
use super::{Error, iso_date, notes, parse_id_date};
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
};

const SHOW_SQL: &str = "SELECT {projection} FROM {post_table} p WHERE p.{post_id}=$1 LIMIT 1";
const LIKED_SQL: &str =
    "SELECT EXISTS(SELECT 1 FROM {like_table} l WHERE l.{post_id}=$1 AND l.{user_id}=$2)";
const TIME_CLIP_LIMIT: f64 = 8_640_000_000_000_000.;
// Consumed entity PROPERTY names/kinds. Database identifiers come ONLY from
// actual exported TypeORM metadata, following the existing ClipQueries contract.
const POST_PROPERTIES: &[(&str, ColumnKind)] = &[
    ("id", ColumnKind::Json),
    ("updatedAt", ColumnKind::Date),
    ("title", ColumnKind::Json),
    ("description", ColumnKind::Json),
    ("userId", ColumnKind::Json),
    ("fileIds", ColumnKind::Json),
    ("isSensitive", ColumnKind::Json),
    ("likedCount", ColumnKind::Json),
    ("tags", ColumnKind::Json),
];
const LIKE_PROPERTIES: &[(&str, ColumnKind)] = &[
    ("id", ColumnKind::Json),
    ("userId", ColumnKind::Json),
    ("postId", ColumnKind::Json),
];

#[derive(Clone, Copy, Debug)]
pub(crate) enum Operation {
    Show,
    Liked,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Show => "gallery.posts.show",
            Self::Liked => "gallery.posts.show.liked",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GalleryLayoutFailure {
    MissingLayout,
    InvalidIdentifier,
    UnknownProperty,
    DuplicateProperty,
    MissingProperty,
    HydrationKind,
    QueryTemplate,
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum FailureKind {
    QueryShape,
    InvalidParameter,
    InvalidPrincipal,
    MissingField,
    FieldType,
    ArrayElement,
}
#[derive(Debug)]
pub(crate) enum GalleryFailure {
    Shape {
        operation: Operation,
        kind: FailureKind,
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
impl GalleryFailure {
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
                operation = "gallery.posts.show",
                error.kind = "row_utf16",
                "endpoint failed"
            ),
            Self::Int4 { .. } => tracing::error!(
                operation = "gallery.posts.show",
                error.kind = "row_int4",
                "endpoint failed"
            ),
        }
    }
}
impl fmt::Display for GalleryFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("native gallery read failed")
    }
}
impl std::error::Error for GalleryFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Query { source, .. } => Some(source),
            Self::Row { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::Utf16 { source } => Some(source),
            Self::Int4 { source } => Some(source),
            Self::Shape { .. } => None,
        }
    }
}
fn shape(kind: FailureKind) -> Error {
    Error::Gallery(GalleryFailure::Shape {
        operation: Operation::Show,
        kind,
    })
}

pub(crate) struct GalleryQueries {
    show: String,
    liked: String,
}
fn validated_layout<'a>(
    layouts: &'a HashMap<String, Vec<EntityColumn>>,
    table_name: &str,
    properties: &[(&str, ColumnKind)],
) -> Result<(&'a str, &'a [EntityColumn]), GalleryLayoutFailure> {
    let (table, columns) = layouts
        .get_key_value(table_name)
        .ok_or(GalleryLayoutFailure::MissingLayout)?;
    if table.is_empty() || table.contains('\0') {
        return Err(GalleryLayoutFailure::InvalidIdentifier);
    }
    let mut seen = HashSet::new();
    for column in columns {
        if column.database_name.is_empty() || column.database_name.contains('\0') {
            return Err(GalleryLayoutFailure::InvalidIdentifier);
        }
        let (_, kind) = properties
            .iter()
            .find(|(name, _)| *name == column.property_name)
            .ok_or(GalleryLayoutFailure::UnknownProperty)?;
        if !seen.insert(column.property_name.as_str()) {
            return Err(GalleryLayoutFailure::DuplicateProperty);
        }
        if column.kind != *kind {
            return Err(GalleryLayoutFailure::HydrationKind);
        }
    }
    if seen.len() != properties.len() {
        return Err(GalleryLayoutFailure::MissingProperty);
    }
    Ok((table.as_str(), columns.as_slice()))
}
fn column_name(columns: &[EntityColumn], property: &str) -> Result<String, GalleryLayoutFailure> {
    columns
        .iter()
        .find(|column| column.property_name == property)
        .map(|column| quote_identifier(&column.database_name))
        .ok_or(GalleryLayoutFailure::MissingProperty)
}
fn compile_query(
    template: &str,
    replacements: &[(&str, String)],
) -> Result<String, GalleryLayoutFailure> {
    // Substitute only spans of the trusted source template. Braces inside
    // quoted metadata identifiers are appended literally, never reinterpreted.
    let mut remaining = template;
    let mut compiled = String::new();
    while let Some(start) = remaining.find('{') {
        compiled.push_str(&remaining[..start]);
        let tail = &remaining[start + 1..];
        let end = tail.find('}').ok_or(GalleryLayoutFailure::QueryTemplate)?;
        let (_, replacement) = replacements
            .iter()
            .find(|(name, _)| *name == &tail[..end])
            .ok_or(GalleryLayoutFailure::QueryTemplate)?;
        compiled.push_str(replacement);
        remaining = &tail[end + 1..];
    }
    compiled.push_str(remaining);
    Ok(compiled)
}
impl GalleryQueries {
    pub(crate) fn new(
        layouts: &HashMap<String, Vec<EntityColumn>>,
    ) -> Result<Self, GalleryLayoutFailure> {
        let (post_table, post_columns) =
            validated_layout(layouts, "gallery_post", POST_PROPERTIES)?;
        let (like_table, like_columns) =
            validated_layout(layouts, "gallery_like", LIKE_PROPERTIES)?;
        let projection = post_columns
            .iter()
            .map(|column| {
                let reference = format!("p.{}", quote_identifier(&column.database_name));
                let value = match column.kind {
                    ColumnKind::Date => {
                        format!("floor(extract(epoch FROM {reference})*1000)::double precision")
                    }
                    ColumnKind::Json => reference,
                    ColumnKind::SimpleArray => return Err(GalleryLayoutFailure::HydrationKind),
                };
                Ok(format!("{}, {value}", quote_literal(&column.property_name)))
            })
            .collect::<Result<Vec<_>, GalleryLayoutFailure>>()?
            .join(", ");
        let show = compile_query(
            SHOW_SQL,
            &[
                (
                    "projection",
                    format!("json_build_object({projection})::text"),
                ),
                ("post_table", quote_identifier(post_table)),
                ("post_id", column_name(post_columns, "id")?),
            ],
        )?;
        let liked = compile_query(
            LIKED_SQL,
            &[
                ("like_table", quote_identifier(like_table)),
                ("post_id", column_name(like_columns, "postId")?),
                ("user_id", column_name(like_columns, "userId")?),
            ],
        )?;
        Ok(Self { show, liked })
    }
}

fn field<'a>(row: &'a Value, name: &str) -> Result<&'a Value, Error> {
    row.get(name)
        .ok_or_else(|| shape(FailureKind::MissingField))
}
fn string(value: &Value) -> Result<String, Error> {
    match value {
        Value::String(units) => String::from_utf16(units)
            .map_err(|source| Error::Gallery(GalleryFailure::Utf16 { source })),
        _ => Err(shape(FailureKind::FieldType)),
    }
}
/// Validated PostgreSQL text-array data, including multiple dimensions.
/// Each Value clone/drop uses the existing iterative implementation; this type
/// adds no recursive Clone/Drop or unrestricted opaque JSON payload.
struct TextArray {
    elements: Vec<Value>,
}
impl TextArray {
    fn new(value: &Value) -> Result<Self, Error> {
        let Value::Array(elements) = value else {
            return Err(shape(FailureKind::FieldType));
        };
        let mut pending: Vec<_> = elements.iter().collect();
        while let Some(element) = pending.pop() {
            match element {
                Value::String(_) | Value::Null => {}
                Value::Array(children) => pending.extend(children),
                _ => return Err(shape(FailureKind::ArrayElement)),
            }
        }
        Ok(Self {
            elements: elements.clone(),
        })
    }

    fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    fn packed(&self) -> Value {
        Value::Array(self.elements.clone())
    }

    fn pg_array_literal(elements: &[Value]) -> Result<String, Error> {
        // Actual pg/lib/utils.js prepareValue -> arrayString + escapeElement.
        // Generate the same text using an explicit stack instead of recursively
        // traversing a potentially deep public Value. Quotes/backslashes are the
        // only escaped text characters; SQL NULL is unquoted inside the literal.
        enum Emit<'a> {
            Array(&'a [Value]),
            Element(&'a Value),
            Ascii(u8),
        }
        let mut pending = vec![Emit::Array(elements)];
        let mut output = Vec::<u16>::new();
        while let Some(task) = pending.pop() {
            match task {
                Emit::Array(elements) => {
                    output.push(u16::from(b'{'));
                    pending.push(Emit::Ascii(b'}'));
                    for (index, element) in elements.iter().enumerate().rev() {
                        pending.push(Emit::Element(element));
                        if index > 0 {
                            pending.push(Emit::Ascii(b','));
                        }
                    }
                }
                Emit::Ascii(byte) => output.push(u16::from(byte)),
                Emit::Element(Value::Null) => output.extend("NULL".encode_utf16()),
                Emit::Element(Value::String(units)) => {
                    output.push(u16::from(b'"'));
                    for unit in units {
                        if *unit == u16::from(b'"') || *unit == u16::from(b'\\') {
                            output.push(u16::from(b'\\'));
                        }
                        output.push(*unit);
                    }
                    output.push(u16::from(b'"'));
                }
                Emit::Element(Value::Array(children)) => pending.push(Emit::Array(children)),
                Emit::Element(_) => return Err(shape(FailureKind::ArrayElement)),
            }
        }
        String::from_utf16(&output)
            .map_err(|source| Error::Gallery(GalleryFailure::Utf16 { source }))
    }

    fn query_ids(&self) -> Result<Vec<Option<String>>, Error> {
        // TypeORM passes each OUTER item as one IN parameter. Scalar text stays
        // text, NULL stays NULL, and an array becomes pg's array-literal string.
        // Keeping [None] distinct from [] preserves the source file query even
        // when the original nonempty array contains only NULL elements.
        self.elements
            .iter()
            .map(|element| match element {
                Value::Null => Ok(None),
                Value::String(_) => string(element).map(Some),
                Value::Array(children) => Self::pg_array_literal(children).map(Some),
                _ => Err(shape(FailureKind::ArrayElement)),
            })
            .collect()
    }

    fn reconstruct_files(&self, packed: &HashMap<String, Value>) -> Result<Value, Error> {
        let mut result = Vec::new();
        for element in &self.elements {
            match element {
                Value::String(_) => {
                    if let Some(file) = packed.get(&string(element)?) {
                        result.push(file.clone());
                    }
                }
                // Source Map keys are strings from rows; an original Array key
                // does not match its serialized query text. Pack ALL found rows
                // first, then omit these unmatched array/NULL keys from output.
                Value::Array(_) | Value::Null => {}
                _ => return Err(shape(FailureKind::ArrayElement)),
            }
        }
        Ok(Value::Array(result))
    }
}

fn invalid_date() -> Error {
    // Actual gallery TS HTTP: finite PG year294000 becomes an invalid JS Date.
    // Reuse the shared legacy/correlation/envelope mapper, not a local payload.
    let definition = &crate::response::INVALID_DATE;
    Error::legacy(definition.message.to_owned(), definition.code)
}
fn non_date_updated_timestamp() -> Error {
    // Actual pinned production gallery HTTP for BOTH +/-infinity. The central
    // definition contains the observed minified `i.updatedAt` expression.
    let definition = &crate::response::NON_DATE_GALLERY_TIMESTAMP;
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
    fn decode(value: &Value) -> Result<Self, Error> {
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
            _ => Err(shape(FailureKind::FieldType)),
        }
    }
    fn iso(&self) -> Result<String, Error> {
        match self {
            Self::Finite(milliseconds) if milliseconds.abs() <= TIME_CLIP_LIMIT => {
                iso_date(*milliseconds)
            }
            Self::Finite(_) => Err(invalid_date()),
            Self::PositiveInfinity | Self::NegativeInfinity => Err(non_date_updated_timestamp()),
        }
    }
}
struct GalleryRow {
    id: String,
    created_at: String,
    updated_at: String,
    user_id: String,
    title: String,
    description: Option<String>,
    file_ids: TextArray,
    tags: TextArray,
    is_sensitive: bool,
    liked_count: i32,
}
impl GalleryRow {
    fn decode(row: &Value, id_method: &str) -> Result<Self, Error> {
        // GalleryPostEntityService converts both dates before it starts the
        // author/files promises or the inline awaited authenticated like read.
        let id = string(field(row, "id")?)?;
        let created_ms = parse_id_date(id_method, &id).ok_or_else(invalid_date)?;
        let created_at = iso_date(created_ms)?;
        let updated_at = UpdatedDate::decode(field(row, "updatedAt")?)?.iso()?;
        let description = match field(row, "description")? {
            Value::Null => None,
            value => Some(string(value)?),
        };
        let is_sensitive = match field(row, "isSensitive")? {
            Value::Boolean(value) => *value,
            _ => return Err(shape(FailureKind::FieldType)),
        };
        let liked_count = match field(row, "likedCount")? {
            Value::Number(value) => {
                // int4 values are exact Numbers. Checked integer parsing also
                // rejects fractions and overflow without a narrowing cast.
                ryu_js::Buffer::new()
                    .format(*value)
                    .parse()
                    .map_err(|source| Error::Gallery(GalleryFailure::Int4 { source }))?
            }
            _ => return Err(shape(FailureKind::FieldType)),
        };
        Ok(Self {
            id,
            created_at,
            updated_at,
            user_id: string(field(row, "userId")?)?,
            title: string(field(row, "title")?)?,
            description,
            file_ids: TextArray::new(field(row, "fileIds")?)?,
            tags: TextArray::new(field(row, "tags")?)?,
            is_sensitive,
            liked_count,
        })
    }
    fn pack(&self, user: Value, files: Value, is_liked: Option<bool>) -> Value {
        Value::object([
            ("id", Value::string(&self.id)),
            ("createdAt", Value::string(&self.created_at)),
            ("updatedAt", Value::string(&self.updated_at)),
            ("userId", Value::string(&self.user_id)),
            ("user", user),
            ("title", Value::string(&self.title)),
            (
                "description",
                self.description
                    .as_deref()
                    .map(Value::string)
                    .unwrap_or(Value::Null),
            ),
            ("fileIds", self.file_ids.packed()),
            ("files", files),
            (
                "tags",
                if self.tags.is_empty() {
                    Value::Undefined
                } else {
                    self.tags.packed()
                },
            ),
            ("isSensitive", Value::Boolean(self.is_sensitive)),
            ("likedCount", Value::Number(f64::from(self.liked_count))),
            (
                "isLiked",
                is_liked.map(Value::Boolean).unwrap_or(Value::Undefined),
            ),
        ])
    }
}

async fn load_post(runtime: &Runtime, post_id: &str) -> Result<Option<Value>, Error> {
    let operation = Operation::Show;
    let rows = runtime
        .database
        .query_typed(true, &runtime.gallery_queries.show, &[&post_id])
        .await
        .map_err(|source| Error::Gallery(GalleryFailure::Query { operation, source }))?;
    let row = match rows.as_slice() {
        [] => return Ok(None),
        [row] => row,
        _ => return Err(shape(FailureKind::QueryShape)),
    };
    let text: String = row
        .try_get(0)
        .map_err(|source| Error::Gallery(GalleryFailure::Row { operation, source }))?;
    Value::parse_json(&text)
        .map(Some)
        .map_err(|source| Error::Gallery(GalleryFailure::Json { operation, source }))
}
async fn liked(
    runtime: &Runtime,
    post_id: &str,
    viewer: Option<&str>,
) -> Result<Option<bool>, Error> {
    let Some(viewer) = viewer.filter(|id| !id.is_empty()) else {
        return Ok(None);
    };
    let operation = Operation::Liked;
    let rows = runtime
        .database
        .query_typed(true, &runtime.gallery_queries.liked, &[&post_id, &viewer])
        .await
        .map_err(|source| Error::Gallery(GalleryFailure::Query { operation, source }))?;
    let [row] = rows.as_slice() else {
        return Err(Error::Gallery(GalleryFailure::Shape {
            operation,
            kind: FailureKind::QueryShape,
        }));
    };
    row.try_get(0)
        .map(Some)
        .map_err(|source| Error::Gallery(GalleryFailure::Row { operation, source }))
}
fn viewer_id(auth: &Context) -> Result<Option<&str>, Error> {
    auth.user
        .as_ref()
        .map(|user| {
            user.get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| shape(FailureKind::InvalidPrincipal))
        })
        .transpose()
}
pub(super) async fn show(
    runtime: &Runtime,
    params: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    let post_id = params
        .get("postId")
        .and_then(Value::to_utf8)
        .ok_or_else(|| shape(FailureKind::InvalidParameter))?;
    let raw = load_post(runtime, &post_id)
        .await?
        .ok_or(Error::Fixed(crate::response::ApiFailure::NoSuchPost))?;
    let post = GalleryRow::decode(&raw, &runtime.business_config.id_method)?;
    let viewer = viewer_id(auth)?;
    let query_ids = post.file_ids.query_ids()?;
    let borrowed_ids: Vec<_> = query_ids.iter().map(Option::as_deref).collect();
    // Unlike Clip.pack, Gallery's SINGLE pack passes the actual viewer to
    // UserLite. No ownership/block/sensitive/UGC gate is introduced here.
    let user = notes::pack_user_lite_by_id(runtime, &post.user_id, auth);
    let files = notes::pack_public_files_map(runtime, &borrowed_ids);
    let like = liked(runtime, &post.id, viewer);
    // Own all futures; source starts author/files before awaiting like, then
    // awaitAll. Like failure takes precedence over the collected pack results.
    let (user, files, like) = tokio::join!(user, files, like);
    let like = like?;
    let user = user?;
    let packed_files = files?;
    let files = post.file_ids.reconstruct_files(&packed_files)?;
    Ok(post.pack(user, files, like))
}

#[cfg(test)]
mod tests {
    use super::*;
    const SOURCE: &str = include_str!("fixtures/gallery-packing-oracle.json");
    const DATE_HTTP: &str = include_str!("fixtures/gallery-date-http-oracle.json");
    const ARRAY_SOURCE: &str = include_str!("fixtures/gallery-array-source-oracle.json");

    #[test]
    fn dates_nullable_arrays_scalars_and_omission_match_actual_source_projection() {
        for source in [SOURCE, ARRAY_SOURCE] {
            let oracle: serde_json::Value = serde_json::from_str(source).unwrap();
            let mut compared = 0;
            for fixture in oracle["cases"].as_array().unwrap() {
                let mut row = Value::parse_json(&fixture["input"]["post"].to_string()).unwrap();
                let timestamp: jiff::Timestamp = fixture["input"]["post"]["updatedAt"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap();
                // Valid JS Date integral milliseconds lie below 2^53 and are exact f64.
                row.set(
                    "updatedAt",
                    Value::Number(timestamp.as_millisecond() as f64),
                );
                let row = GalleryRow::decode(&row, fixture["method"].as_str().unwrap()).unwrap();
                let expected =
                    Value::parse_json(&fixture["expectedProjection"].to_string()).unwrap();
                let is_liked = match expected.get("isLiked") {
                    Some(Value::Boolean(value)) => Some(*value),
                    None => None,
                    _ => panic!("unexpected source isLiked"),
                };
                // The source oracle explicitly stubs UserLite; shared attachment
                // contents are also outside this pure Gallery projection test.
                let mut actual = row.pack(Value::Null, Value::Null, is_liked);
                actual.set("user", Value::Undefined);
                actual.set("files", Value::Undefined);
                assert_eq!(
                    actual.stringify(),
                    expected.stringify(),
                    "{}",
                    fixture["name"]
                );
                compared += 1;
            }
            assert_eq!(compared, oracle["summary"]["success"].as_u64().unwrap());
        }
    }

    #[test]
    fn nested_array_query_values_match_actual_pg_prepare_value() {
        let oracle: serde_json::Value = serde_json::from_str(ARRAY_SOURCE).unwrap();
        for fixture in oracle["pgPreparation"].as_array().unwrap() {
            let value = Value::parse_json(&fixture["input"].to_string()).unwrap();
            let array = TextArray::new(&value).unwrap();
            assert_eq!(
                TextArray::pg_array_literal(&array.elements).unwrap(),
                fixture["prepared"].as_str().unwrap()
            );
        }
        for fixture in oracle["fileQueries"].as_array().unwrap() {
            let value = Value::parse_json(&fixture["fileIds"].to_string()).unwrap();
            let array = TextArray::new(&value).unwrap();
            assert_eq!(
                serde_json::to_value(array.query_ids().unwrap()).unwrap(),
                fixture["expectedQueryIds"],
                "{}",
                fixture["name"]
            );
        }
    }

    #[test]
    fn file_map_uses_original_top_level_string_keys_after_all_rows_are_packed() {
        let oracle: serde_json::Value = serde_json::from_str(ARRAY_SOURCE).unwrap();
        for fixture in oracle["fileQueries"].as_array().unwrap() {
            let Some(expected_ids) = fixture.get("expectedPackedFileIds") else {
                continue;
            };
            let value = Value::parse_json(&fixture["fileIds"].to_string()).unwrap();
            let array = TextArray::new(&value).unwrap();
            // Opaque stand-ins expose only the real source-selected row keys;
            // the shared public file formatter is not copied into this test.
            let packed: HashMap<_, _> = fixture["selectedRowIds"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| {
                    let id = id.as_str().unwrap();
                    (id.to_owned(), Value::string(id))
                })
                .collect();
            let actual = array.reconstruct_files(&packed).unwrap();
            let expected = Value::parse_json(&expected_ids.to_string()).unwrap();
            assert_eq!(actual, expected, "{}", fixture["name"]);
        }
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
    async fn date_failure_projection_matches_captured_production_http_fields() {
        let fixture: serde_json::Value = serde_json::from_str(DATE_HTTP).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let date = match case["dateKind"].as_str().unwrap() {
                "positive-infinity" => UpdatedDate::PositiveInfinity,
                "negative-infinity" => UpdatedDate::NegativeInfinity,
                // The expected body is captured for year294000. This samples
                // the same finite-outside-TimeClip class at its first boundary;
                // real SQL-date/native parity remains an end-to-end HTTP check.
                "finite-outside-js" => UpdatedDate::Finite(TIME_CLIP_LIMIT + 1.),
                _ => panic!("unknown captured date class"),
            };
            let response = date.iso().unwrap_err().response();
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
