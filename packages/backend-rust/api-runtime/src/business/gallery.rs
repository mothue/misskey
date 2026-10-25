/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
//! Complete gallery reads, preserving public attachment, ranking and viewer semantics.
use super::{Error, iso_date, notes, parse_id_date};
use crate::{
    Runtime,
    redis::{CommandBatch, CommandReadFailure, RedisState},
};
use misskey_api_validation::{ParseError, Value};
use misskey_backend_api_auth::{
    ColumnKind, Context, EntityColumn,
    postgres::{quote_identifier, quote_literal},
};
use misskey_backend_postgres::QueryError;
use std::{
    collections::{HashMap, HashSet},
    fmt,
    future::{Future, poll_fn},
    pin::Pin,
    sync::Mutex,
    task::Poll,
};

const SHOW_SQL: &str = "SELECT {projection} FROM {post_table} p WHERE p.{post_id}=$1 LIMIT 1";
const FEATURED_SQL_PREFIX: &str = "SELECT {projection} FROM {post_table} p WHERE p.{post_id} IN (";
const FEATURED_EPOCH_MS: i64 = 1_672_531_200_000;
const FEATURED_WINDOW_MS: i64 = 259_200_000;
const FEATURED_CACHE_TTL_MS: i64 = 1_800_000;
const FEATURED_MAX_LIMIT: usize = 100;
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
    Featured,
    Liked,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Show => "gallery.posts.show",
            Self::Featured => "gallery.featured",
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
    RankingClockOverflow,
    Ranking {
        source: CommandReadFailure,
    },
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
    Limit {
        source: std::num::ParseIntError,
    },
}
impl GalleryFailure {
    pub(super) fn report(&self) {
        match self {
            Self::RankingClockOverflow => tracing::error!(
                operation = "gallery.featured",
                error.kind = "ranking_clock_overflow",
                "endpoint failed"
            ),
            Self::Ranking { source } => tracing::error!(
                operation = "gallery.featured",
                error.kind = source.safe_kind(),
                "endpoint failed"
            ),
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
            Self::Limit { .. } => tracing::error!(
                operation = "gallery.featured",
                error.kind = "parameter_limit_parse",
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
            Self::Ranking { source } => Some(source),
            Self::Query { source, .. } => Some(source),
            Self::Row { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::Utf16 { source } => Some(source),
            Self::Int4 { source } | Self::Limit { source } => Some(source),
            Self::RankingClockOverflow | Self::Shape { .. } => None,
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
    featured_prefix: String,
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
        let featured_prefix = compile_query(
            FEATURED_SQL_PREFIX,
            &[
                (
                    "projection",
                    format!("json_build_object({projection})::text"),
                ),
                ("post_table", quote_identifier(post_table)),
                ("post_id", column_name(post_columns, "id")?),
            ],
        )?;
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
        Ok(Self {
            show,
            featured_prefix,
            liked,
        })
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
    pack_post(runtime, &raw, auth, None).await
}

async fn pack_post(
    runtime: &Runtime,
    raw: &Value,
    auth: &Context,
    packed_user: Option<&Value>,
) -> Result<Value, Error> {
    let post = GalleryRow::decode(raw, &runtime.business_config.id_method)?;
    pack_decoded_post(runtime, &post, auth, packed_user).await
}

async fn pack_decoded_post(
    runtime: &Runtime,
    post: &GalleryRow,
    auth: &Context,
    packed_user: Option<&Value>,
) -> Result<Value, Error> {
    let viewer = viewer_id(auth)?;
    let query_ids = post.file_ids.query_ids()?;
    let borrowed_ids: Vec<_> = query_ids.iter().map(Option::as_deref).collect();
    // GalleryPostEntityService.pack uses a non-nullish batch author hint,
    // otherwise it passes the actual viewer to the shared UserLite formatter.
    let user = async {
        match packed_user {
            Some(user) if !matches!(user, Value::Null | Value::Undefined) => Ok(user.clone()),
            _ => notes::pack_user_lite_by_id(runtime, &post.user_id, auth).await,
        }
    };
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

trait GalleryRankingStore {
    fn rankings<'a>(
        &'a self,
        keys: &'a [String; 2],
    ) -> impl Future<Output = Result<CommandBatch<[Vec<String>; 2]>, CommandReadFailure>> + Send + 'a;
}
impl GalleryRankingStore for RedisState {
    fn rankings<'a>(
        &'a self,
        keys: &'a [String; 2],
    ) -> impl Future<Output = Result<CommandBatch<[Vec<String>; 2]>, CommandReadFailure>> + Send + 'a
    {
        self.gallery_rankings(keys)
    }
}

#[derive(Default)]
struct FeaturedRanking {
    ids: Vec<String>,
    last_fetched_at: i64,
}
#[derive(Default)]
pub(crate) struct GalleryFeaturedCache {
    ranking: Mutex<FeaturedRanking>,
}
impl GalleryFeaturedCache {
    async fn ids(
        &self,
        store: &(impl GalleryRankingStore + Sync),
        mut now: impl FnMut() -> i64,
    ) -> Result<Vec<String>, GalleryFailure> {
        {
            let ranking = self
                .ranking
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // gallery/featured uses wall time, a strict TTL and zero sentinel.
            // Signed subtraction preserves cache hits when the clock rolls back.
            if ranking.last_fetched_at != 0
                && i128::from(now()) - i128::from(ranking.last_fetched_at)
                    < i128::from(FEATURED_CACHE_TTL_MS)
            {
                return Ok(ranking.ids.clone());
            }
        }
        // Source refreshes are independent. Never hold the cache lock over I/O
        // or coalesce a second miss behind a pending refresh.
        let keys = featured_ranking_keys(now())?;
        let reply = store
            .rankings(&keys)
            .await
            .map_err(|source| GalleryFailure::Ranking { source })?;
        reply.report();
        let mut ids = first_seen_post_ids(&reply.values);
        let last_fetched_at = now(); // Fresh source Date.now AFTER ranking await.
        ids.sort_by(|a, b| b.encode_utf16().cmp(a.encode_utf16()));
        {
            let mut ranking = self
                .ranking
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ranking.ids = ids.clone();
            ranking.last_fetched_at = last_fetched_at;
        }
        Ok(ids)
    }
}
fn featured_ranking_keys(now: i64) -> Result<[String; 2], GalleryFailure> {
    let elapsed = now
        .checked_sub(FEATURED_EPOCH_MS)
        .ok_or(GalleryFailure::RankingClockOverflow)?;
    let window = elapsed.div_euclid(FEATURED_WINDOW_MS);
    let previous = window
        .checked_sub(1)
        .ok_or(GalleryFailure::RankingClockOverflow)?;
    Ok([
        format!("featuredGalleryPostsRanking:{window}"),
        format!("featuredGalleryPostsRanking:{previous}"),
    ])
}
fn first_seen_post_ids(windows: &[Vec<String>; 2]) -> Vec<String> {
    // FeaturedService.getRankingOf returns Map keys in first-insertion order.
    // Its score arithmetic is pure and unused; legal +/-inf scores stay valid.
    let mut seen = HashSet::new();
    windows
        .iter()
        .flatten()
        .filter(|id| seen.insert(id.as_str()))
        .cloned()
        .collect()
}
fn selected_post_ids(ids: Vec<String>, params: &Value) -> Result<Vec<String>, Error> {
    let limit = match params.get("limit") {
        Some(Value::Number(limit))
            if limit.fract() == 0. && *limit >= 1. && *limit <= FEATURED_MAX_LIMIT as f64 =>
        {
            ryu_js::Buffer::new()
                .format(*limit)
                .parse::<usize>()
                .map_err(|source| Error::Gallery(GalleryFailure::Limit { source }))?
        }
        _ => return Err(shape(FailureKind::InvalidParameter)),
    };
    let until = match params.get("untilId") {
        None => None,
        Some(Value::String(units)) => Some(units.as_slice()).filter(|units| !units.is_empty()),
        _ => return Err(shape(FailureKind::InvalidParameter)),
    };
    Ok(ids
        .into_iter()
        .filter(|id| until.is_none_or(|until| id.encode_utf16().cmp(until.iter().copied()).is_lt()))
        .take(limit)
        .collect())
}
fn featured_query(prefix: &str, count: usize) -> String {
    let parameters = (1..=count)
        .map(|index| format!("${index}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{prefix}{parameters})")
}
async fn load_featured_posts(runtime: &Runtime, ids: &[String]) -> Result<Vec<Value>, Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let operation = Operation::Featured;
    let query = featured_query(&runtime.gallery_queries.featured_prefix, ids.len());
    let parameters: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = ids
        .iter()
        .map(|id| id as &(dyn tokio_postgres::types::ToSql + Sync))
        .collect();
    let rows = runtime
        .database
        .query_typed(true, &query, &parameters)
        .await
        .map_err(|source| Error::Gallery(GalleryFailure::Query { operation, source }))?;
    rows.iter()
        .map(|row| {
            let text: String = row
                .try_get(0)
                .map_err(|source| Error::Gallery(GalleryFailure::Row { operation, source }))?;
            Value::parse_json(&text)
                .map_err(|source| Error::Gallery(GalleryFailure::Json { operation, source }))
        })
        .collect()
}

// JavaScript Promise.all receives an eagerly-created promise for EVERY post,
// even when an earlier post synchronously fails its Date conversion. Poll all
// newly-created futures once before exposing a ready error; retain input order.
enum PackFailure<E> {
    Preparation(E),
    Async(E),
}
async fn eager_batch<F, T, E>(futures: impl IntoIterator<Item = F>) -> Result<Vec<T>, E>
where
    F: Future<Output = Result<T, PackFailure<E>>>,
{
    let mut pending: Vec<_> = futures
        .into_iter()
        .map(|future| Some(Box::pin(future)))
        .collect();
    let mut output: Vec<Option<T>> = (0..pending.len()).map(|_| None).collect();
    poll_fn(move |context| {
        let mut preparation_error = None;
        let mut async_error = None;
        for (index, slot) in pending.iter_mut().enumerate() {
            let Some(future) = slot else { continue };
            match Future::poll(Pin::as_mut(future), context) {
                Poll::Pending => {}
                Poll::Ready(result) => {
                    *slot = None;
                    match result {
                        Ok(value) => output[index] = Some(value),
                        Err(PackFailure::Preparation(error)) if preparation_error.is_none() => {
                            preparation_error = Some(error)
                        }
                        Err(PackFailure::Async(error)) if async_error.is_none() => {
                            async_error = Some(error)
                        }
                        Err(_) => {}
                    }
                }
            }
        }
        if let Some(error) = preparation_error.or(async_error) {
            return Poll::Ready(Err(error));
        }
        if pending.iter().all(Option::is_none) {
            Poll::Ready(Ok(output.iter_mut().filter_map(Option::take).collect()))
        } else {
            Poll::Pending
        }
    })
    .await
}
async fn pack_many(runtime: &Runtime, rows: &[Value], auth: &Context) -> Result<Value, Error> {
    let author_ids = rows
        .iter()
        .map(|row| string(field(row, "userId")?))
        .collect::<Result<Vec<_>, Error>>()?;
    // GalleryPostEntityService.packMany resolves UserLite.packMany BEFORE it
    // invokes any per-post Date, public-file or authenticated-like operation.
    let authors = notes::pack_many_users(runtime, &author_ids, auth).await?;
    // Date/raw-field failures happen before the source's first await. Keep
    // them separate from I/O rejection while still starting every valid post.
    let prepared: Vec<_> = rows
        .iter()
        .map(|raw| GalleryRow::decode(raw, &runtime.business_config.id_method))
        .collect();
    let packed = eager_batch(prepared.into_iter().map(|prepared| {
        let authors = &authors;
        async move {
            let post = prepared.map_err(PackFailure::Preparation)?;
            pack_decoded_post(runtime, &post, auth, authors.get(&post.user_id))
                .await
                .map_err(PackFailure::Async)
        }
    }))
    .await?;
    Ok(Value::Array(packed))
}
pub(super) async fn featured(
    runtime: &Runtime,
    params: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    let ids = runtime
        .gallery_featured_cache
        .ids(&runtime.redis, crate::now_ms)
        .await
        .map_err(Error::Gallery)?;
    let ids = selected_post_ids(ids, params)?;
    if ids.is_empty() {
        return Ok(Value::Array(Vec::new()));
    }
    let posts = load_featured_posts(runtime, &ids).await?;
    pack_many(runtime, &posts, auth).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as Json;
    use std::collections::VecDeque;
    const SOURCE: &str = include_str!("fixtures/gallery-packing-oracle.json");
    const DATE_HTTP: &str = include_str!("fixtures/gallery-date-http-oracle.json");
    const ARRAY_SOURCE: &str = include_str!("fixtures/gallery-array-source-oracle.json");
    const FEATURED_CACHE: &str = include_str!("fixtures/gallery-featured-cache-source-oracle.json");
    const FEATURED_ORDER: &str =
        include_str!("fixtures/gallery-featured-ranking-order-source-oracle.json");
    const FEATURED_BULK: &str =
        include_str!("fixtures/gallery-featured-bulk-packing-source-oracle.json");
    const FEATURED_ERRORS: &str =
        include_str!("fixtures/gallery-featured-ordinary-errors-source-oracle.json");

    fn source_windows(reply: &Json) -> [Vec<String>; 2] {
        ["current", "previous"].map(|name| {
            reply[name]
                .as_array()
                .unwrap()
                .iter()
                .map(|pair| pair[0].as_str().unwrap().to_owned())
                .collect()
        })
    }
    fn source_strings(value: &Json) -> Vec<String> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap().to_owned())
            .collect()
    }
    struct FixtureRankings {
        replies: Mutex<VecDeque<[Vec<String>; 2]>>,
        keys: Mutex<Vec<[String; 2]>>,
    }
    impl GalleryRankingStore for FixtureRankings {
        async fn rankings(
            &self,
            keys: &[String; 2],
        ) -> Result<CommandBatch<[Vec<String>; 2]>, CommandReadFailure> {
            self.keys.lock().unwrap().push(keys.clone());
            Ok(CommandBatch {
                values: self.replies.lock().unwrap().pop_front().unwrap(),
                command_errors: Vec::new(),
            })
        }
    }
    fn fixture_store(case: &Json) -> FixtureRankings {
        FixtureRankings {
            replies: Mutex::new(
                case["inputs"]["rankingReplies"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(source_windows)
                    .collect(),
            ),
            keys: Mutex::new(Vec::new()),
        }
    }
    fn source_clocks(case: &Json) -> VecDeque<i64> {
        case["trace"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| {
                matches!(
                    event["operation"].as_str(),
                    Some("clock.DateNow" | "clock.newDate")
                )
            })
            .map(|event| event["millis"].as_i64().unwrap())
            .collect()
    }
    fn source_pipeline_keys(case: &Json) -> Vec<[String; 2]> {
        case["trace"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["operation"] == "fake.redis.pipeline")
            .map(|event| {
                [
                    event["commands"][0][1].as_str().unwrap().to_owned(),
                    event["commands"][1][1].as_str().unwrap().to_owned(),
                ]
            })
            .collect()
    }

    #[tokio::test]
    async fn featured_cache_matches_source_cold_hit_ttl_rollback_zero_empty_and_post_await_stamp() {
        let oracle: Json = serde_json::from_str(FEATURED_CACHE).unwrap();
        for case in oracle["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["name"] != "concurrent-independent-refresh-completion-order")
        {
            let cache = GalleryFeaturedCache::default();
            let store = fixture_store(case);
            let mut clocks = source_clocks(case);
            for request in case["requests"].as_array().unwrap() {
                let actual = cache
                    .ids(&store, || clocks.pop_front().unwrap())
                    .await
                    .unwrap();
                assert_eq!(
                    actual,
                    source_strings(&request["cacheIds"]),
                    "{}",
                    request["label"]
                );
                assert_eq!(
                    cache.ranking.lock().unwrap().last_fetched_at,
                    request["lastFetchedAt"].as_i64().unwrap()
                );
            }
            assert!(clocks.is_empty(), "{}", case["name"]);
            assert_eq!(*store.keys.lock().unwrap(), source_pipeline_keys(case));
            assert!(store.replies.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn featured_selection_utf16_until_slice_union_and_window_keys_match_actual_source() {
        let oracle: Json = serde_json::from_str(FEATURED_ORDER).unwrap();
        for case in oracle["cases"].as_array().unwrap() {
            let store = fixture_store(case);
            let cache = GalleryFeaturedCache::default();
            let mut clocks = source_clocks(case);
            let ids = cache
                .ids(&store, || clocks.pop_front().unwrap())
                .await
                .unwrap();
            let request = &case["requests"][0];
            let params = Value::parse_json(&request["params"].to_string()).unwrap();
            let selected = selected_post_ids(ids, &params).unwrap();
            let where_event = case["trace"]
                .as_array()
                .unwrap()
                .iter()
                .find(|event| event["operation"] == "fake.galleryPosts.where");
            let expected = where_event
                .map(|event| source_strings(&event["parameters"]["postIds"]))
                .unwrap_or_default();
            assert_eq!(selected, expected, "{}", case["name"]);
            assert_eq!(*store.keys.lock().unwrap(), source_pipeline_keys(case));
            let ranking = case["trace"]
                .as_array()
                .unwrap()
                .iter()
                .find(|event| event["operation"] == "actual.featured.rankingResult")
                .unwrap();
            assert_eq!(
                first_seen_post_ids(&source_windows(&case["inputs"]["rankingReplies"][0])),
                source_strings(&ranking["ids"])
            );
            assert!(clocks.is_empty());
        }
    }

    struct DeferredRankings {
        replies: Mutex<VecDeque<tokio::sync::oneshot::Receiver<[Vec<String>; 2]>>>,
    }
    impl GalleryRankingStore for DeferredRankings {
        async fn rankings(
            &self,
            _keys: &[String; 2],
        ) -> Result<CommandBatch<[Vec<String>; 2]>, CommandReadFailure> {
            let reply = self.replies.lock().unwrap().pop_front().unwrap();
            Ok(CommandBatch {
                values: reply.await.unwrap(),
                command_errors: Vec::new(),
            })
        }
    }
    fn assert_cache_snapshot(cache: &GalleryFeaturedCache, ids: &[String], request: &Json) {
        assert_eq!(ids, source_strings(&request["cacheIds"]));
        let ranking = cache.ranking.lock().unwrap();
        assert_eq!(ranking.ids, ids);
        assert_eq!(
            ranking.last_fetched_at,
            request["lastFetchedAt"].as_i64().unwrap()
        );
    }
    #[tokio::test]
    async fn featured_refreshes_are_independent_and_each_completion_replaces_cache() {
        use std::sync::atomic::{AtomicI64, Ordering};
        let oracle: Json = serde_json::from_str(FEATURED_CACHE).unwrap();
        let case = oracle["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == "concurrent-independent-refresh-completion-order")
            .unwrap();
        let phases = case["trace"].as_array().unwrap();
        let clock_for = |phase: &str| {
            phases
                .iter()
                .find(|event| event["phase"] == phase && event.get("millis").is_some())
                .unwrap()["millis"]
                .as_i64()
                .unwrap()
        };
        let (send_a, receive_a) = tokio::sync::oneshot::channel();
        let (send_b, receive_b) = tokio::sync::oneshot::channel();
        let store = DeferredRankings {
            replies: Mutex::new(VecDeque::from([receive_a, receive_b])),
        };
        let cache = GalleryFeaturedCache::default();
        let clock = AtomicI64::new(clock_for("refresh-a-start"));
        let mut a = Box::pin(cache.ids(&store, || clock.load(Ordering::SeqCst)));
        assert!(futures_util::poll!(&mut a).is_pending());
        clock.store(clock_for("refresh-b-start"), Ordering::SeqCst);
        let mut b = Box::pin(cache.ids(&store, || clock.load(Ordering::SeqCst)));
        assert!(futures_util::poll!(&mut b).is_pending());
        assert!(store.replies.lock().unwrap().is_empty());
        clock.store(clock_for("refresh-b-complete"), Ordering::SeqCst);
        send_b
            .send(source_windows(&case["inputs"]["rankingReplies"][1]))
            .unwrap();
        assert_cache_snapshot(&cache, &b.await.unwrap(), &case["requests"][0]);
        clock.store(clock_for("hit-while-a-pending"), Ordering::SeqCst);
        let hit = cache
            .ids(&store, || clock.load(Ordering::SeqCst))
            .await
            .unwrap();
        assert_cache_snapshot(&cache, &hit, &case["requests"][1]);
        clock.store(clock_for("refresh-a-complete"), Ordering::SeqCst);
        send_a
            .send(source_windows(&case["inputs"]["rankingReplies"][0]))
            .unwrap();
        assert_cache_snapshot(&cache, &a.await.unwrap(), &case["requests"][2]);
        clock.store(clock_for("hit-after-both"), Ordering::SeqCst);
        let hit = cache
            .ids(&store, || clock.load(Ordering::SeqCst))
            .await
            .unwrap();
        assert_cache_snapshot(&cache, &hit, &case["requests"][3]);
    }

    fn fixture_gallery_row(input: &Json) -> Value {
        let mut row = Value::parse_json(&input.to_string()).unwrap();
        let updated = match input["updatedAtRuntimeType"].as_str() {
            Some("number") => match input["updatedAtRuntimeNumber"].as_str().unwrap() {
                "Infinity" => Value::string("Infinity"),
                "-Infinity" => Value::string("-Infinity"),
                value => Value::Number(value.parse().unwrap()),
            },
            _ => {
                let timestamp: jiff::Timestamp =
                    input["updatedAt"].as_str().unwrap().parse().unwrap();
                // Source Date fixture integral milliseconds are below 2^53.
                Value::Number(timestamp.as_millisecond() as f64)
            }
        };
        row.set("updatedAt", updated);
        row
    }
    #[tokio::test]
    async fn featured_batch_preparation_rejection_precedes_async_io_and_starts_all_posts() {
        let oracle: Json = serde_json::from_str(FEATURED_ERRORS).unwrap();
        for case in oracle["cases"].as_array().unwrap() {
            let prepared: Vec<_> = case["inputs"]["databaseRowsInReturnOrder"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| GalleryRow::decode(&fixture_gallery_row(row), "aidx"))
                .collect();
            let started = Mutex::new(Vec::new());
            let result: Result<Vec<()>, Error> =
                eager_batch(prepared.into_iter().enumerate().map(|(index, post)| {
                    let started = &started;
                    async move {
                        started.lock().unwrap().push(index);
                        post.map_err(PackFailure::Preparation)?;
                        Err(PackFailure::Async(Error::Internal))
                    }
                }))
                .await;
            let Error::Legacy { message, code, .. } = result.unwrap_err() else {
                panic!("source Date rejection must win")
            };
            assert_eq!(message, case["observed"]["message"]);
            assert_eq!(code, case["observed"]["name"]);
            let actual_calls = case["trace"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|event| event["operation"] == "actual.GalleryPost.pack.start")
                .count();
            assert_eq!(started.lock().unwrap().len(), actual_calls);
        }
    }

    #[tokio::test]
    async fn featured_batch_keeps_source_db_order_and_reuses_gallery_projection() {
        let oracle: Json = serde_json::from_str(FEATURED_BULK).unwrap();
        let case = &oracle["cases"][0];
        let expected = &case["requests"][0]["result"];
        let rows = case["inputs"]["databaseRowsInReturnOrder"]
            .as_array()
            .unwrap();
        let packed = eager_batch(rows.iter().enumerate().map(|(index, input)| {
            let expected = &expected[index];
            async move {
                let post = GalleryRow::decode(&fixture_gallery_row(input), "aidx")
                    .map_err(PackFailure::Preparation)?;
                let is_liked = expected
                    .get("isLiked")
                    .map(|value| value.as_bool().unwrap());
                let mut packed = post.pack(Value::Null, Value::Null, is_liked);
                packed.set("user", Value::Undefined);
                packed.set("files", Value::Undefined);
                Ok::<_, PackFailure<Error>>(packed)
            }
        }))
        .await
        .unwrap();
        for (actual, expected) in packed.iter().zip(expected.as_array().unwrap()) {
            let mut expected = Value::parse_json(&expected.to_string()).unwrap();
            expected.set("user", Value::Undefined);
            expected.set("files", Value::Undefined);
            assert_eq!(actual.stringify(), expected.stringify());
        }
        assert_eq!(packed.len(), rows.len());
    }

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
