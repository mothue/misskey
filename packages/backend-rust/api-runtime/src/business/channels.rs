/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
//! Own favorites with source snapshots, independent viewer reads and URL-only banners.
pub use super::entity_layout::LayoutFailure as ChannelsLayoutFailure;
use super::{
    Error,
    eager_batch::{PackFailure, eager_batch},
    entity_layout::{column_name, compile_query, projected_layout, validated_layout},
    notes,
    stored_text_array::{TextArray, TextArrayFailure},
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
use tokio_postgres::types::ToSql;

// Keep favorite columns selected too: TypeORM getMany selects these along with
// the joined snapshot. Dropping them could enable a different covering-index plan.
const FAVORITES_SQL: &str = "SELECT f.{favorite_id},f.{favorite_channel},f.{favorite_user},CASE WHEN c.{channel_id} IS NULL THEN NULL ELSE json_build_object({projection}) END::text FROM {favorites} f LEFT JOIN {channels} c ON c.{channel_id}=f.{favorite_channel} WHERE f.{favorite_user}=$1";
const BANNER_SQL: &str =
    "SELECT json_build_object({projection})::text FROM {table} b WHERE b.{id}=$1 LIMIT 1";
// Actual TypeORM executeExistsQuery selects a dummy row WHERE EXISTS, LIMIT 1.
const EXISTS_SQL: &str = "SELECT 1 AS row_exists FROM (SELECT 1 AS dummy_column) dummy_table WHERE EXISTS (SELECT 1 FROM {table} s WHERE s.{viewer}=$1 AND s.{channel}=$2) LIMIT 1";
const PINS_SQL_PREFIX: &str = "SELECT {projection} FROM {table} n WHERE n.{id} IN (";
const CHANNEL_PROPERTIES: &[(&str, ColumnKind)] = &[
    ("id", ColumnKind::Json),
    ("lastNotedAt", ColumnKind::Date),
    ("userId", ColumnKind::Json),
    ("name", ColumnKind::Json),
    ("description", ColumnKind::Json),
    ("bannerId", ColumnKind::Json),
    ("pinnedNoteIds", ColumnKind::Json),
    ("color", ColumnKind::Json),
    ("isArchived", ColumnKind::Json),
    ("notesCount", ColumnKind::Json),
    ("usersCount", ColumnKind::Json),
    ("isSensitive", ColumnKind::Json),
    ("allowRenoteToExternal", ColumnKind::Json),
];
const FAVORITE_PROPERTIES: &[(&str, ColumnKind)] = &[
    ("id", ColumnKind::Json),
    ("channelId", ColumnKind::Json),
    ("userId", ColumnKind::Json),
];
const FOLLOWING_PROPERTIES: &[(&str, ColumnKind)] = &[
    ("id", ColumnKind::Json),
    ("followeeId", ColumnKind::Json),
    ("followerId", ColumnKind::Json),
];
const MUTING_PROPERTIES: &[(&str, ColumnKind)] = &[
    ("id", ColumnKind::Json),
    ("userId", ColumnKind::Json),
    ("channelId", ColumnKind::Json),
    ("expiresAt", ColumnKind::Date),
];
const BANNER_FIELDS: &[&str] = &[
    "id",
    "uri",
    "userHost",
    "isLink",
    "webpublicAccessKey",
    "webpublicUrl",
    "url",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Operation {
    Favorites,
    Banner,
    Following,
    Favorite,
    Muting,
    PinnedRead,
    Pack,
    PublicUrl,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Favorites => "channels.my_favorites.select",
            Self::Banner => "channels.my_favorites.banner",
            Self::Following => "channels.my_favorites.following",
            Self::Favorite => "channels.my_favorites.favorite",
            Self::Muting => "channels.my_favorites.muting",
            Self::PinnedRead => "channels.my_favorites.pinned_read",
            Self::Pack => "channels.my_favorites.pack",
            Self::PublicUrl => "channels.my_favorites.public_url",
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum ShapeKind {
    InvalidPrincipal,
    QueryShape,
    MissingField,
    FieldType,
    ArrayElement,
}
#[derive(Debug)]
pub(crate) enum ChannelsFailure {
    MissingBanner {
        id: String,
    },
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
    Conversion {
        operation: Operation,
        source: Box<Error>,
    },
}
impl ChannelsFailure {
    pub(super) fn report(&self) {
        match self {
            Self::MissingBanner { .. } => tracing::error!(
                operation = "channels.my_favorites.banner",
                error.kind = "missing_entity",
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
                operation = "channels.my_favorites.pack",
                error.kind = "utf16",
                "endpoint failed"
            ),
            Self::Int4 { .. } => tracing::error!(
                operation = "channels.my_favorites.pack",
                error.kind = "int4",
                "endpoint failed"
            ),
            Self::Conversion { operation, .. } => tracing::error!(
                operation = operation.name(),
                error.kind = "conversion",
                "endpoint failed"
            ),
        }
    }
}
impl fmt::Display for ChannelsFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("native channel favorites failed")
    }
}
impl std::error::Error for ChannelsFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Query { source, .. } => Some(source),
            Self::Row { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::Utf16 { source } => Some(source),
            Self::Int4 { source } => Some(source),
            // The existing business Error has no std::error::Error impl. Keep
            // the exact boxed cause for delegation at the response boundary.
            Self::Conversion { .. } | Self::Shape { .. } | Self::MissingBanner { .. } => None,
        }
    }
}
fn shape(operation: Operation, kind: ShapeKind) -> Error {
    Error::Channels(ChannelsFailure::Shape { operation, kind })
}
fn conversion(operation: Operation, source: Error) -> Error {
    Error::Channels(ChannelsFailure::Conversion {
        operation,
        source: Box::new(source),
    })
}

pub(crate) struct ChannelQueries {
    favorites: String,
    banner: String,
    following: String,
    favorite: String,
    muting: String,
    pins_prefix: String,
}
impl ChannelQueries {
    pub(crate) fn new(
        layouts: &HashMap<String, Vec<EntityColumn>>,
    ) -> Result<Self, ChannelsLayoutFailure> {
        let (channels, channel) = validated_layout(layouts, "channel", CHANNEL_PROPERTIES)?;
        let (favorites, favorite) =
            validated_layout(layouts, "channel_favorite", FAVORITE_PROPERTIES)?;
        let favorites = compile_query(
            FAVORITES_SQL,
            &[
                ("favorite_id", column_name(favorite, "id")?),
                ("favorite_channel", column_name(favorite, "channelId")?),
                ("favorite_user", column_name(favorite, "userId")?),
                ("channel_id", column_name(channel, "id")?),
                ("projection", json_projection(channel, "c")?),
                ("favorites", quote_identifier(favorites)),
                ("channels", quote_identifier(channels)),
            ],
        )?;
        let (files, file) = projected_layout(layouts, "drive_file", BANNER_FIELDS)?;
        let banner_columns: Vec<_> = file
            .iter()
            .filter(|column| BANNER_FIELDS.contains(&column.property_name.as_str()))
            .cloned()
            .collect();
        let banner = compile_query(
            BANNER_SQL,
            &[
                ("projection", json_projection(&banner_columns, "b")?),
                ("table", quote_identifier(files)),
                ("id", column_name(file, "id")?),
            ],
        )?;
        let (notes, note) = projected_layout(layouts, "note", &["id"])?;
        // Preserve the unused source read and selected-column permissions. Raw
        // Row bytes are discarded: no JSON/date/reaction/Note decoding occurs.
        let projection = note
            .iter()
            .map(|column| format!("n.{}", quote_identifier(&column.database_name)))
            .collect::<Vec<_>>()
            .join(",");
        let pins_prefix = compile_query(
            PINS_SQL_PREFIX,
            &[
                ("projection", projection),
                ("table", quote_identifier(notes)),
                ("id", column_name(note, "id")?),
            ],
        )?;
        Ok(Self {
            favorites,
            banner,
            pins_prefix,
            following: exists_query(
                layouts,
                "channel_following",
                FOLLOWING_PROPERTIES,
                "followerId",
                "followeeId",
            )?,
            favorite: exists_query(
                layouts,
                "channel_favorite",
                FAVORITE_PROPERTIES,
                "userId",
                "channelId",
            )?,
            muting: exists_query(
                layouts,
                "channel_muting",
                MUTING_PROPERTIES,
                "userId",
                "channelId",
            )?,
        })
    }
}
fn json_projection(columns: &[EntityColumn], alias: &str) -> Result<String, ChannelsLayoutFailure> {
    columns
        .iter()
        .map(|column| {
            let reference = format!("{alias}.{}", quote_identifier(&column.database_name));
            let value = match column.kind {
                ColumnKind::Json => reference,
                ColumnKind::Date => {
                    format!("floor(extract(epoch FROM {reference})*1000)::double precision")
                }
                ColumnKind::SimpleArray => return Err(ChannelsLayoutFailure::HydrationKind),
            };
            Ok(format!("{}, {value}", quote_literal(&column.property_name)))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join(", "))
}
fn exists_query(
    layouts: &HashMap<String, Vec<EntityColumn>>,
    table: &str,
    properties: &[(&str, ColumnKind)],
    viewer: &str,
    channel: &str,
) -> Result<String, ChannelsLayoutFailure> {
    let (table, columns) = validated_layout(layouts, table, properties)?;
    compile_query(
        EXISTS_SQL,
        &[
            ("table", quote_identifier(table)),
            ("viewer", column_name(columns, viewer)?),
            ("channel", column_name(columns, channel)?),
        ],
    )
}
fn pins_query(prefix: &str, count: usize) -> String {
    let placeholders = (1..=count)
        .map(|index| format!("${index}"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{prefix}{placeholders})")
}

trait ChannelStore {
    fn favorites(&self, viewer: &str) -> impl Future<Output = Result<Vec<Value>, Error>> + Send;
    fn banner(&self, id: &str) -> impl Future<Output = Result<Value, Error>> + Send;
    fn exists(
        &self,
        operation: Operation,
        viewer: &str,
        id: &str,
    ) -> impl Future<Output = Result<bool, Error>> + Send;
    fn pinned_read(&self, ids: &[Option<String>])
    -> impl Future<Output = Result<(), Error>> + Send;
    fn public_url(&self, file: &Value) -> impl Future<Output = Result<String, Error>> + Send;
}
struct RuntimeStore<'a>(&'a Runtime);
impl RuntimeStore<'_> {
    async fn query(
        &self,
        operation: Operation,
        sql: &str,
        parameters: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<tokio_postgres::Row>, Error> {
        self.0
            .database
            .query_typed(true, sql, parameters)
            .await
            .map_err(|source| Error::Channels(ChannelsFailure::Query { operation, source }))
    }
    fn json(
        &self,
        operation: Operation,
        row: &tokio_postgres::Row,
        index: usize,
    ) -> Result<Value, Error> {
        let text: Option<String> = row
            .try_get(index)
            .map_err(|source| Error::Channels(ChannelsFailure::Row { operation, source }))?;
        match text {
            Some(text) => Value::parse_json(&text)
                .map_err(|source| Error::Channels(ChannelsFailure::Json { operation, source })),
            None => Ok(Value::Null),
        }
    }
}
impl ChannelStore for RuntimeStore<'_> {
    async fn favorites(&self, viewer: &str) -> Result<Vec<Value>, Error> {
        let rows = self
            .query(
                Operation::Favorites,
                &self.0.channel_queries.favorites,
                &[&viewer],
            )
            .await?;
        rows.iter()
            .map(|row| self.json(Operation::Favorites, row, 3))
            .collect()
    }
    async fn banner(&self, id: &str) -> Result<Value, Error> {
        let rows = self
            .query(Operation::Banner, &self.0.channel_queries.banner, &[&id])
            .await?;
        self.json(Operation::Banner, banner_row(&rows, id)?, 0)
    }
    async fn exists(&self, operation: Operation, viewer: &str, id: &str) -> Result<bool, Error> {
        let query = match operation {
            Operation::Following => &self.0.channel_queries.following,
            Operation::Favorite => &self.0.channel_queries.favorite,
            Operation::Muting => &self.0.channel_queries.muting,
            _ => return Err(shape(operation, ShapeKind::QueryShape)),
        };
        let rows = self.query(operation, query, &[&viewer, &id]).await?;
        match rows.len() {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(shape(operation, ShapeKind::QueryShape)),
        }
    }
    async fn pinned_read(&self, ids: &[Option<String>]) -> Result<(), Error> {
        let query = pins_query(&self.0.channel_queries.pins_prefix, ids.len());
        let parameters: Vec<&(dyn ToSql + Sync)> =
            ids.iter().map(|id| id as &(dyn ToSql + Sync)).collect();
        self.query(Operation::PinnedRead, &query, &parameters)
            .await?;
        Ok(())
    }
    async fn public_url(&self, file: &Value) -> Result<String, Error> {
        notes::public_file_url(self.0, file)
            .await
            .map_err(|source| conversion(Operation::PublicUrl, source))
    }
}
fn banner_row<'a>(
    rows: &'a [tokio_postgres::Row],
    id: &str,
) -> Result<&'a tokio_postgres::Row, Error> {
    match rows {
        [row] => Ok(row),
        [] => Err(Error::Channels(ChannelsFailure::MissingBanner {
            id: id.to_owned(),
        })),
        _ => Err(shape(Operation::Banner, ShapeKind::QueryShape)),
    }
}
pub(super) fn missing_banner_error(id: &str) -> Error {
    // Actual TypeORM EntityNotFoundError(MiDriveFile, {id}) uses
    // JSON.stringify(criteria, null, 4). Serialize only its scalar id with the
    // existing JS serializer, then apply this fixed observed criteria shape.
    let quoted = Value::string(id)
        .stringify()
        .expect("a Value::String is always JSON serializable");
    Error::legacy(
        format!(
            "Could not find any entity of type \"{}\" matching: {{\n    \"id\": {quoted}\n}}",
            crate::response::CHANNEL_BANNER_ENTITY_TARGET
        ),
        crate::response::ENTITY_NOT_FOUND_ERROR,
    )
}
fn field<'a>(row: &'a Value, name: &str) -> Result<&'a Value, Error> {
    row.get(name)
        .ok_or_else(|| shape(Operation::Pack, ShapeKind::MissingField))
}
fn string(value: &Value) -> Result<String, Error> {
    match value {
        Value::String(units) => String::from_utf16(units)
            .map_err(|source| Error::Channels(ChannelsFailure::Utf16 { source })),
        _ => Err(shape(Operation::Pack, ShapeKind::FieldType)),
    }
}
fn string_units(value: &Value) -> Result<&[u16], Error> {
    match value {
        Value::String(units) => Ok(units),
        _ => Err(shape(Operation::Pack, ShapeKind::FieldType)),
    }
}
fn nullable_string_units(value: &Value) -> Result<Option<&[u16]>, Error> {
    match value {
        Value::Null => Ok(None),
        _ => string_units(value).map(Some),
    }
}
fn packed_nullable_string(value: &Value) -> Result<Value, Error> {
    Ok(nullable_string_units(value)?.map_or(Value::Null, |units| Value::String(units.to_vec())))
}
fn boolean(value: &Value) -> Result<bool, Error> {
    match value {
        Value::Boolean(value) => Ok(*value),
        _ => Err(shape(Operation::Pack, ShapeKind::FieldType)),
    }
}
fn array_failure(failure: TextArrayFailure) -> Error {
    match failure {
        TextArrayFailure::FieldType => shape(Operation::PinnedRead, ShapeKind::FieldType),
        TextArrayFailure::ArrayElement => shape(Operation::PinnedRead, ShapeKind::ArrayElement),
        TextArrayFailure::Utf16 { source } => Error::Channels(ChannelsFailure::Utf16 { source }),
    }
}
fn last_noted_at(value: &Value) -> Result<Value, Error> {
    if matches!(value, Value::Null) {
        return Ok(Value::Null);
    }
    // A hydrated Date(0) is truthy. Nullness, not the projected number's JS
    // truthiness, chooses this conversion. Bun wording is from frozen TS HTTP.
    stored_value::updated_at_with_error(value, &crate::response::NON_DATE_CHANNEL_TIMESTAMP)
        .map(|date| Value::string(&date))
        .map_err(|failure| match failure {
            StoredDateFailure::FieldType => shape(Operation::Pack, ShapeKind::FieldType),
            StoredDateFailure::Conversion(source) => conversion(Operation::Pack, source),
        })
}
fn count(value: &Value) -> Result<Value, Error> {
    stored_value::int4(value)
        .map(|count| Value::Number(f64::from(count)))
        .map_err(|failure| match failure {
            StoredInt4Failure::FieldType => shape(Operation::Pack, ShapeKind::FieldType),
            StoredInt4Failure::Parse(source) => Error::Channels(ChannelsFailure::Int4 { source }),
        })
}
async fn pack_channel(
    store: &(impl ChannelStore + Sync),
    channel: &Value,
    viewer: &str,
    id_method: &str,
) -> Result<Value, Error> {
    let banner_id = field(channel, "bannerId")?;
    let banner = if banner_id.truthy() {
        Some(store.banner(&string(banner_id)?).await?)
    } else {
        None
    };
    let id = string(field(channel, "id")?)?;
    let following = store.exists(Operation::Following, viewer, &id).await?;
    let favorite = store.exists(Operation::Favorite, viewer, &id).await?;
    let muting = store.exists(Operation::Muting, viewer, &id).await?;
    let pins = TextArray::new(field(channel, "pinnedNoteIds")?).map_err(array_failure)?;
    if !pins.is_empty() {
        store
            .pinned_read(&pins.query_ids().map_err(array_failure)?)
            .await?;
    }
    let created_at = stored_value::created_at(id_method, &id)
        .map_err(|source| conversion(Operation::Pack, source))?;
    let mut packed = Value::object([
        ("id", field(channel, "id")?.clone()),
        ("createdAt", Value::string(&created_at)),
        (
            "lastNotedAt",
            last_noted_at(field(channel, "lastNotedAt")?)?,
        ),
        (
            "name",
            Value::String(string_units(field(channel, "name")?)?.to_vec()),
        ),
        (
            "description",
            packed_nullable_string(field(channel, "description")?)?,
        ),
        ("userId", packed_nullable_string(field(channel, "userId")?)?),
    ]);
    packed.set(
        "bannerUrl",
        match banner {
            Some(file) => Value::string(&store.public_url(&file).await?),
            None => Value::Null,
        },
    );
    packed.set("bannerId", packed_nullable_string(banner_id)?);
    packed.set("pinnedNoteIds", pins.packed());
    packed.set(
        "color",
        Value::String(string_units(field(channel, "color")?)?.to_vec()),
    );
    packed.set(
        "isArchived",
        Value::Boolean(boolean(field(channel, "isArchived")?)?),
    );
    for key in ["usersCount", "notesCount"] {
        packed.set(key, count(field(channel, key)?)?);
    }
    for key in ["isSensitive", "allowRenoteToExternal"] {
        packed.set(key, Value::Boolean(boolean(field(channel, key)?)?));
    }
    for (key, flag) in [
        ("isFollowing", following),
        ("isFavorited", favorite),
        ("isMuting", muting),
        ("hasUnreadNote", false),
    ] {
        packed.set(key, Value::Boolean(flag));
    }
    // Detailed is omitted by the endpoint. The source read above remains, but
    // the unused pinned rows never reach Note packing or the public response.
    Ok(packed)
}
async fn run(
    store: &(impl ChannelStore + Sync),
    viewer: &str,
    id_method: &str,
) -> Result<Value, Error> {
    let channels = store.favorites(viewer).await?;
    eager_batch(channels.iter().map(|channel| async {
        pack_channel(store, channel, viewer, id_method)
            .await
            .map_err(PackFailure::Async)
    }))
    .await
    .map(Value::Array)
}
pub(super) async fn my_favorites(runtime: &Runtime, auth: &Context) -> Result<Value, Error> {
    let viewer = auth
        .user
        .as_ref()
        .and_then(|user| user.get("id"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| shape(Operation::Favorites, ShapeKind::InvalidPrincipal))?;
    run(
        &RuntimeStore(runtime),
        viewer,
        &runtime.business_config.id_method,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value as Json, json};
    use std::sync::Mutex;

    const HTTP: &str = include_str!("fixtures/channels-http.json");
    const SOURCE: &str = include_str!("fixtures/channels-source-oracle.json");
    struct Store {
        rows: Vec<Value>,
        flags: HashMap<String, [bool; 3]>,
        calls: Mutex<Vec<&'static str>>,
    }
    impl ChannelStore for Store {
        async fn favorites(&self, _: &str) -> Result<Vec<Value>, Error> {
            self.calls.lock().unwrap().push("getMany");
            Ok(self.rows.clone())
        }
        async fn banner(&self, id: &str) -> Result<Value, Error> {
            self.calls.lock().unwrap().push("banner");
            Ok(Value::object([("id", Value::string(id))]))
        }
        async fn exists(&self, operation: Operation, _: &str, id: &str) -> Result<bool, Error> {
            let (name, index) = match operation {
                Operation::Following => ("following", 0),
                Operation::Favorite => ("favorite", 1),
                Operation::Muting => ("muting", 2),
                _ => panic!("only source existence operations are expected"),
            };
            self.calls.lock().unwrap().push(name);
            // All packs must reach their first await before any pack finishes.
            if operation == Operation::Following {
                tokio::task::yield_now().await;
            }
            Ok(self
                .flags
                .get(id)
                .map_or([false, false, true], |flags| *flags)[index])
        }
        async fn pinned_read(&self, _: &[Option<String>]) -> Result<(), Error> {
            self.calls.lock().unwrap().push("unused-pinned-read");
            Ok(())
        }
        async fn public_url(&self, _: &Value) -> Result<String, Error> {
            self.calls.lock().unwrap().push("public-url");
            Ok("controlled-url-only".to_owned())
        }
    }
    fn store(rows: Vec<Value>) -> Store {
        Store {
            rows,
            flags: HashMap::new(),
            calls: Mutex::new(Vec::new()),
        }
    }
    fn http_fixture() -> Json {
        serde_json::from_str(HTTP).unwrap()
    }
    fn compact_row() -> Value {
        let fixture = http_fixture();
        Value::from_json(
            &fixture["records"]
                .as_array()
                .unwrap()
                .iter()
                .find(|case| case["id"] == "one-compact-account")
                .unwrap()["rows"][0],
        )
    }
    #[tokio::test]
    async fn selected_actual_ts_http_bodies_preserve_dates_flags_arrays_and_order() {
        let fixture = http_fixture();
        for case in fixture["records"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["status"] == 200)
        {
            let rows = case["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(Value::from_json)
                .collect();
            let mut store = store(rows);
            for channel in case["body"].as_array().unwrap() {
                store.flags.insert(
                    channel["id"].as_str().unwrap().to_owned(),
                    [
                        channel["isFollowing"].as_bool().unwrap(),
                        channel["isFavorited"].as_bool().unwrap(),
                        channel["isMuting"].as_bool().unwrap(),
                    ],
                );
            }
            let actual = run(&store, "actual-viewer", "aidx").await.unwrap();
            assert_eq!(actual, Value::from_json(&case["body"]), "{}", case["id"]);
        }
    }
    #[test]
    fn actual_ts_http_ordinary_date_errors_keep_callsite_and_cause() {
        let fixture = http_fixture();
        for case in fixture["records"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|case| case["status"] == 500)
        {
            let error = last_noted_at(&Value::from_json(&case["lastNotedAtInput"])).unwrap_err();
            let Error::Channels(ChannelsFailure::Conversion { source, .. }) = error else {
                panic!("typed conversion retains legacy cause")
            };
            let Error::Legacy { message, code, .. } = *source else {
                panic!("actual date failures are legacy errors")
            };
            assert_eq!(
                message,
                case["body"]["error"]["info"]["e"]["message"]
                    .as_str()
                    .unwrap()
            );
            assert_eq!(
                code,
                case["body"]["error"]["info"]["e"]["code"].as_str().unwrap()
            );
        }
    }
    #[tokio::test]
    async fn captured_source_phases_keep_banner_then_three_reads_then_unused_pins_then_url() {
        let source: Json = serde_json::from_str(SOURCE).unwrap();
        let case = source["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == "banner-pins-unused-malformed-note")
            .unwrap();
        let mut row = compact_row();
        row.set("bannerId", Value::string("opaque-banner"));
        row.set(
            "pinnedNoteIds",
            Value::from_json(&case["controlledOutput"][0]["pinnedNoteIds"]),
        );
        let store = store(vec![row]);
        let output = run(&store, "controlled-viewer", "aidx").await.unwrap();
        let expected: Vec<_> = case["calls"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|call| call["op"].as_str())
            .filter(|op| {
                matches!(
                    *op,
                    "getMany"
                        | "banner"
                        | "following"
                        | "favorite"
                        | "muting"
                        | "unused-pinned-read"
                        | "public-url"
                )
            })
            .collect();
        assert_eq!(*store.calls.lock().unwrap(), expected);
        let Value::Array(ref channels) = output else {
            panic!("array result")
        };
        assert_eq!(channels[0].get("isFavorited"), Some(&Value::Boolean(false)));
        assert_eq!(channels[0].get("isMuting"), Some(&Value::Boolean(true)));
        assert!(channels[0].get("pinnedNotes").is_none());
    }
    #[tokio::test]
    async fn all_packs_start_and_maintain_snapshot_order() {
        let mut first = compact_row();
        first.set("name", Value::string("first"));
        let mut second = compact_row();
        second.set("name", Value::string("second"));
        let store = store(vec![first, second]);
        let Value::Array(ref output) = run(&store, "viewer", "aidx").await.unwrap() else {
            panic!("array result")
        };
        assert_eq!(
            &store.calls.lock().unwrap()[..3],
            &["getMany", "following", "following"]
        );
        assert_eq!(output[0].get("name"), Some(&Value::string("first")));
        assert_eq!(output[1].get("name"), Some(&Value::string("second")));
    }
    #[tokio::test]
    async fn date_failure_follows_reads_and_prevents_public_url() {
        let mut row = compact_row();
        row.set("bannerId", Value::string("banner"));
        row.set("pinnedNoteIds", Value::Array(vec![Value::Null]));
        row.set("lastNotedAt", Value::string("Infinity"));
        let store = store(vec![row]);
        assert!(run(&store, "viewer", "aidx").await.is_err());
        assert_eq!(
            *store.calls.lock().unwrap(),
            vec![
                "getMany",
                "banner",
                "following",
                "favorite",
                "muting",
                "unused-pinned-read"
            ]
        );
    }
    #[tokio::test]
    async fn rejection_still_starts_later_source_packs() {
        let mut first = compact_row();
        first.set("lastNotedAt", Value::string("Infinity"));
        let store = store(vec![first, compact_row()]);
        assert!(run(&store, "viewer", "aidx").await.is_err());
        assert_eq!(
            &store.calls.lock().unwrap()[..3],
            &["getMany", "following", "following"]
        );
    }
    #[tokio::test]
    async fn empty_banner_rows_preserve_captured_actual_ts_error_response() {
        let fixture: Json =
            serde_json::from_str(include_str!("fixtures/channels-banner-missing-http.json"))
                .unwrap();
        // This is the same row-count boundary used by RuntimeStore.banner.
        let response = banner_row(&[], "invalid-date-file").unwrap_err().response();
        assert_eq!(
            u64::from(response.status().as_u16()),
            fixture["http"]["status"].as_u64().unwrap()
        );
        let bytes = axum::body::to_bytes(response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let mut actual: Json = serde_json::from_slice(&bytes).unwrap();
        let mut expected = fixture["http"]["body"].clone();
        for body in [&mut actual, &mut expected] {
            if let Some(detail) = body
                .get_mut("error")
                .and_then(|error| error.get_mut("info"))
                .and_then(|info| info.get_mut("e"))
                .and_then(Json::as_object_mut)
            {
                detail.remove("id"); // Actual request correlation is fresh.
            }
        }
        assert_eq!(actual, expected);
    }
    #[test]
    fn missing_banner_criteria_matches_actual_typeorm_constructor_escaping() {
        let fixture: Json = serde_json::from_str(include_str!(
            "fixtures/channels-banner-missing-criteria.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let id = case["id"].as_str().unwrap();
            let Error::Channels(ChannelsFailure::MissingBanner { id: retained }) =
                banner_row(&[], id).unwrap_err()
            else {
                panic!("typed missing-banner context must survive the empty-row boundary")
            };
            assert_eq!(retained, id);
            let Error::Legacy { message, code, .. } = missing_banner_error(&retained) else {
                panic!("missing banner uses the existing legacy formatter")
            };
            assert_eq!(message, case["message"].as_str().unwrap());
            assert_eq!(code, case["name"].as_str().unwrap());
        }
    }
    #[tokio::test]
    async fn consumed_scalar_types_fail_at_their_response_phase_after_required_reads() {
        let controls = [
            ("name", Value::Null, false),
            (
                "description",
                Value::object(std::iter::empty::<(&str, Value)>()),
                false,
            ),
            ("userId", Value::Number(0.), false),
            ("color", Value::Array(Vec::new()), true),
            ("isArchived", Value::string("false"), true),
            ("isSensitive", Value::Null, true),
            ("allowRenoteToExternal", Value::Number(1.), true),
            ("bannerId", Value::Number(0.), false),
        ];
        for (field, invalid, url_evaluated) in controls {
            let mut row = compact_row();
            row.set("bannerId", Value::string("banner"));
            row.set("pinnedNoteIds", Value::Array(vec![Value::Null]));
            row.set(field, invalid);
            let store = store(vec![row]);
            let failure = run(&store, "viewer", "aidx").await.unwrap_err();
            assert!(
                matches!(
                    failure,
                    Error::Channels(ChannelsFailure::Shape {
                        operation: Operation::Pack,
                        kind: ShapeKind::FieldType
                    })
                ),
                "{field}"
            );
            let mut expected = vec!["getMany"];
            if field != "bannerId" {
                expected.push("banner");
            }
            expected.extend(["following", "favorite", "muting", "unused-pinned-read"]);
            if url_evaluated {
                expected.push("public-url");
            }
            assert_eq!(*store.calls.lock().unwrap(), expected, "{field}");
        }
    }
    #[tokio::test]
    async fn checked_strings_borrow_utf16_and_preserve_null_empty_and_false() {
        let original = Value::String(vec![0xd800, 0x41, 0xd83d, 0xde00]);
        let Value::String(ref units) = original else {
            panic!("string fixture")
        };
        assert!(std::ptr::eq(
            string_units(&original).unwrap().as_ptr(),
            units.as_ptr()
        ));
        assert_eq!(nullable_string_units(&Value::Null).unwrap(), None);
        assert_eq!(
            nullable_string_units(&Value::string("")).unwrap(),
            Some(&[][..])
        );
        let mut row = compact_row();
        row.set("name", original.clone());
        row.set("description", Value::Null);
        row.set("userId", Value::Null);
        row.set("bannerId", Value::string(""));
        row.set("color", Value::string(""));
        row.set("isArchived", Value::Boolean(false));
        row.set("isSensitive", Value::Boolean(false));
        row.set("allowRenoteToExternal", Value::Boolean(false));
        let store = store(vec![row]);
        let result = run(&store, "viewer", "aidx").await.unwrap();
        let Value::Array(ref rows) = result else {
            panic!("array output")
        };
        assert_eq!(rows[0].get("name"), Some(&original));
        for key in ["description", "userId", "bannerUrl"] {
            assert_eq!(rows[0].get(key), Some(&Value::Null));
        }
        for key in ["bannerId", "color"] {
            assert_eq!(rows[0].get(key), Some(&Value::string("")));
        }
        for key in ["isArchived", "isSensitive", "allowRenoteToExternal"] {
            assert_eq!(rows[0].get(key), Some(&Value::Boolean(false)));
        }
        assert_eq!(
            *store.calls.lock().unwrap(),
            vec!["getMany", "following", "favorite", "muting"]
        );
    }
    #[tokio::test]
    async fn stored_date_failure_precedes_later_scalar_type_failures() {
        let mut row = compact_row();
        row.set("name", Value::Null);
        row.set("lastNotedAt", Value::string("Infinity"));
        let store = store(vec![row]);
        let error = run(&store, "viewer", "aidx").await.unwrap_err();
        let Error::Channels(ChannelsFailure::Conversion { source, .. }) = error else {
            panic!("date conversion precedes name response field")
        };
        assert!(matches!(
            *source,
            Error::Legacy {
                code: "TypeError",
                ..
            }
        ));
        assert_eq!(
            *store.calls.lock().unwrap(),
            vec!["getMany", "following", "favorite", "muting"]
        );
    }
    fn column(name: &str, kind: ColumnKind) -> EntityColumn {
        EntityColumn {
            property_name: name.to_owned(),
            database_name: format!("stored {name}\""),
            kind,
        }
    }
    fn layouts() -> HashMap<String, Vec<EntityColumn>> {
        let mut layouts = HashMap::new();
        for (table, properties) in [
            ("channel", CHANNEL_PROPERTIES),
            ("channel_favorite", FAVORITE_PROPERTIES),
            ("channel_following", FOLLOWING_PROPERTIES),
            ("channel_muting", MUTING_PROPERTIES),
        ] {
            layouts.insert(
                table.to_owned(),
                properties
                    .iter()
                    .map(|(name, kind)| column(name, *kind))
                    .collect(),
            );
        }
        layouts.insert(
            "drive_file".to_owned(),
            BANNER_FIELDS
                .iter()
                .map(|name| column(name, ColumnKind::Json))
                .chain([column("unrelated", ColumnKind::Date)])
                .collect(),
        );
        layouts.insert(
            "note".to_owned(),
            vec![
                column("id", ColumnKind::Json),
                column("reactions", ColumnKind::Json),
                column("updatedAt", ColumnKind::Date),
            ],
        );
        layouts
    }
    #[test]
    fn metadata_queries_keep_owner_filter_no_sort_expiry_and_opaque_note_projection() {
        let queries = ChannelQueries::new(&layouts()).unwrap();
        assert!(
            queries
                .favorites
                .contains("WHERE f.\"stored userId\"\"\"=$1")
        );
        assert!(queries.favorites.contains("LEFT JOIN"));
        assert!(!queries.favorites.contains("ORDER BY"));
        assert!(!queries.favorites.contains("LIMIT"));
        assert!(!queries.muting.contains("expiresAt"));
        assert!(queries.pins_prefix.contains("n.\"stored reactions\"\"\""));
        assert!(!queries.pins_prefix.contains("json"));
        assert_eq!(pins_query("prefix(", 3), "prefix($1,$2,$3)");
        for table in [
            "channel",
            "channel_favorite",
            "channel_following",
            "channel_muting",
            "note",
            "drive_file",
        ] {
            let mut layouts = layouts();
            layouts.remove(table);
            assert!(matches!(
                ChannelQueries::new(&layouts),
                Err(ChannelsLayoutFailure::MissingLayout)
            ));
        }
        let mut layouts = layouts();
        layouts
            .get_mut("channel")
            .unwrap()
            .push(column("unexpected", ColumnKind::Json));
        assert!(matches!(
            ChannelQueries::new(&layouts),
            Err(ChannelsLayoutFailure::UnknownProperty)
        ));
        let captured: Json = serde_json::from_str(SOURCE).unwrap();
        assert_eq!(captured["metadata"]["meta"]["kind"], json!("read:channels"));
    }
}
