/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
//! Complete announcement detail reads with privacy before read-state and dates.
pub use super::entity_layout::LayoutFailure as AnnouncementLayoutFailure;
use super::{
    Error,
    entity_layout::{column_name, compile_query, validated_layout},
    stored_value::{self, StoredDateFailure},
};
use crate::Runtime;
use misskey_api_validation::{ParseError, Value};
use misskey_backend_api_auth::{
    ColumnKind, Context, EntityColumn,
    postgres::{quote_identifier, quote_literal},
};
use misskey_backend_postgres::QueryError;
use std::{collections::HashMap, fmt, future::Future};

const SHOW_SQL: &str = "SELECT {projection} FROM {table} a WHERE a.{id}=$1 LIMIT 1";
const READ_SQL: &str =
    "SELECT EXISTS(SELECT 1 FROM {table} r WHERE r.{announcement_id}=$1 AND r.{user_id}=$2)";
// Validate the complete exported metadata, without hydrating unused flags or
// treating their values as additional visibility conditions.
const ANNOUNCEMENT_PROPERTIES: &[(&str, ColumnKind)] = &[
    ("id", ColumnKind::Json),
    ("updatedAt", ColumnKind::Date),
    ("text", ColumnKind::Json),
    ("title", ColumnKind::Json),
    ("imageUrl", ColumnKind::Json),
    ("icon", ColumnKind::Json),
    ("display", ColumnKind::Json),
    ("needConfirmationToRead", ColumnKind::Json),
    ("isActive", ColumnKind::Json),
    ("forExistingUsers", ColumnKind::Json),
    ("silence", ColumnKind::Json),
    ("userId", ColumnKind::Json),
];
const READ_PROPERTIES: &[(&str, ColumnKind)] = &[
    ("id", ColumnKind::Json),
    ("userId", ColumnKind::Json),
    ("announcementId", ColumnKind::Json),
];

#[derive(Clone, Copy, Debug)]
pub(crate) enum Stage {
    Lookup,
    ReadState,
    Pack,
}
impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::Lookup => "lookup",
            Self::ReadState => "read_state",
            Self::Pack => "pack",
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
pub(crate) enum AnnouncementFailure {
    Query {
        stage: Stage,
        source: QueryError,
    },
    Row {
        stage: Stage,
        source: tokio_postgres::Error,
    },
    Json {
        source: ParseError,
    },
    Shape {
        stage: Stage,
        kind: ShapeKind,
    },
}
impl AnnouncementFailure {
    pub(super) fn report(&self) {
        match self {
            Self::Query { stage, .. } => tracing::error!(
                operation = "announcements.show",
                stage = stage.name(),
                error.kind = "query",
                "endpoint failed"
            ),
            Self::Row { stage, .. } => tracing::error!(
                operation = "announcements.show",
                stage = stage.name(),
                error.kind = "row_decode",
                "endpoint failed"
            ),
            Self::Json { .. } => tracing::error!(
                operation = "announcements.show",
                stage = "lookup",
                error.kind = "row_json",
                "endpoint failed"
            ),
            Self::Shape { stage, kind } => tracing::error!(
                operation = "announcements.show", stage = stage.name(),
                error.kind = ?kind, "endpoint failed"
            ),
        }
    }
}
impl fmt::Display for AnnouncementFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("native announcement detail read failed")
    }
}
impl std::error::Error for AnnouncementFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Query { source, .. } => Some(source),
            Self::Row { source, .. } => Some(source),
            Self::Json { source } => Some(source),
            Self::Shape { .. } => None,
        }
    }
}
fn shape(stage: Stage, kind: ShapeKind) -> Error {
    Error::Announcement(AnnouncementFailure::Shape { stage, kind })
}

pub(crate) struct AnnouncementQueries {
    show: String,
    read: String,
}
impl AnnouncementQueries {
    pub(crate) fn new(
        layouts: &HashMap<String, Vec<EntityColumn>>,
    ) -> Result<Self, AnnouncementLayoutFailure> {
        let (table, columns) = validated_layout(layouts, "announcement", ANNOUNCEMENT_PROPERTIES)?;
        let (read_table, read_columns) =
            validated_layout(layouts, "announcement_read", READ_PROPERTIES)?;
        let projection = columns
            .iter()
            .filter(|column| {
                !matches!(
                    column.property_name.as_str(),
                    "isActive" | "forExistingUsers"
                )
            })
            .map(|column| {
                let reference = format!("a.{}", quote_identifier(&column.database_name));
                let value = match column.kind {
                    // PG infinity becomes a JSON string tag, rather than an
                    // eager Date error that could precede the privacy check.
                    ColumnKind::Date => {
                        format!("floor(extract(epoch FROM {reference})*1000)::double precision")
                    }
                    ColumnKind::Json => reference,
                    ColumnKind::SimpleArray => {
                        return Err(AnnouncementLayoutFailure::HydrationKind);
                    }
                };
                Ok(format!("{}, {value}", quote_literal(&column.property_name)))
            })
            .collect::<Result<Vec<_>, AnnouncementLayoutFailure>>()?
            .join(", ");
        Ok(Self {
            show: compile_query(
                SHOW_SQL,
                &[
                    (
                        "projection",
                        format!("json_build_object({projection})::text"),
                    ),
                    ("table", quote_identifier(table)),
                    ("id", column_name(columns, "id")?),
                ],
            )?,
            read: compile_query(
                READ_SQL,
                &[
                    ("table", quote_identifier(read_table)),
                    (
                        "announcement_id",
                        column_name(read_columns, "announcementId")?,
                    ),
                    ("user_id", column_name(read_columns, "userId")?),
                ],
            )?,
        })
    }
}

fn field<'a>(row: &'a Value, name: &str, stage: Stage) -> Result<&'a Value, Error> {
    row.get(name)
        .ok_or_else(|| shape(stage, ShapeKind::MissingField))
}
fn text(value: &Value, stage: Stage) -> Result<&[u16], Error> {
    value
        .as_utf16()
        .ok_or_else(|| shape(stage, ShapeKind::FieldType))
}
fn nullable_text(value: &Value, stage: Stage) -> Result<Option<&[u16]>, Error> {
    match value {
        Value::Null => Ok(None),
        Value::String(units) => Ok(Some(units)),
        _ => Err(shape(stage, ShapeKind::FieldType)),
    }
}
fn boolean(value: &Value) -> Result<bool, Error> {
    match value {
        Value::Boolean(value) => Ok(*value),
        _ => Err(shape(Stage::Pack, ShapeKind::FieldType)),
    }
}

struct AnnouncementRow<'a> {
    id: &'a [u16],
    created_at: String,
    updated_at: Option<String>,
    title: &'a [u16],
    text: &'a [u16],
    image_url: Option<&'a [u16]>,
    icon: &'a [u16],
    display: &'a [u16],
    need_confirmation_to_read: bool,
    silence: bool,
}
impl<'a> AnnouncementRow<'a> {
    fn decode(raw: &'a Value, id_method: &str) -> Result<Self, Error> {
        let get = |name| field(raw, name, Stage::Pack);
        let id = text(get("id")?, Stage::Pack)?;
        // AnnouncementEntityService.pack formats createdAt before updatedAt,
        // after getAnnouncement has checked privacy and loaded read state.
        let created_at = stored_value::created_at(id_method, &String::from_utf16_lossy(id))?;
        let updated_at = match get("updatedAt")? {
            Value::Null => None,
            value => Some(
                stored_value::updated_at_with_error(
                    value,
                    &crate::response::NON_DATE_ANNOUNCEMENT_TIMESTAMP,
                )
                .map_err(|failure| match failure {
                    StoredDateFailure::FieldType => shape(Stage::Pack, ShapeKind::FieldType),
                    StoredDateFailure::Conversion(source) => source,
                })?,
            ),
        };
        Ok(Self {
            id,
            created_at,
            updated_at,
            title: text(get("title")?, Stage::Pack)?,
            text: text(get("text")?, Stage::Pack)?,
            image_url: nullable_text(get("imageUrl")?, Stage::Pack)?,
            // These database strings are opaque, even outside the TS unions.
            icon: text(get("icon")?, Stage::Pack)?,
            display: text(get("display")?, Stage::Pack)?,
            need_confirmation_to_read: boolean(get("needConfirmationToRead")?)?,
            silence: boolean(get("silence")?)?,
        })
    }
    fn pack(self, for_you: bool, is_read: Option<bool>) -> Value {
        Value::object([
            ("id", Value::String(self.id.to_vec())),
            ("createdAt", Value::string(&self.created_at)),
            (
                "updatedAt",
                self.updated_at
                    .as_deref()
                    .map(Value::string)
                    .unwrap_or(Value::Null),
            ),
            ("title", Value::String(self.title.to_vec())),
            ("text", Value::String(self.text.to_vec())),
            (
                "imageUrl",
                self.image_url
                    .map(|units| Value::String(units.to_vec()))
                    .unwrap_or(Value::Null),
            ),
            ("icon", Value::String(self.icon.to_vec())),
            ("display", Value::String(self.display.to_vec())),
            ("forYou", Value::Boolean(for_you)),
            (
                "needConfirmationToRead",
                Value::Boolean(self.need_confirmation_to_read),
            ),
            ("silence", Value::Boolean(self.silence)),
            (
                "isRead",
                is_read.map(Value::Boolean).unwrap_or(Value::Undefined),
            ),
        ])
    }
}

trait AnnouncementStore {
    fn load<'a>(
        &'a self,
        id: &'a str,
    ) -> impl Future<Output = Result<Option<Value>, Error>> + Send + 'a;
    fn read<'a>(
        &'a self,
        announcement_id: &'a str,
        viewer_id: &'a str,
    ) -> impl Future<Output = Result<bool, Error>> + Send + 'a;
}
struct RuntimeStore<'a>(&'a Runtime);
impl AnnouncementStore for RuntimeStore<'_> {
    async fn load(&self, id: &str) -> Result<Option<Value>, Error> {
        let stage = Stage::Lookup;
        let rows = self
            .0
            .database
            .query_typed(true, &self.0.announcement_queries.show, &[&id])
            .await
            .map_err(|source| Error::Announcement(AnnouncementFailure::Query { stage, source }))?;
        let row = match rows.as_slice() {
            [] => return Ok(None),
            [row] => row,
            _ => return Err(shape(stage, ShapeKind::QueryShape)),
        };
        let json: String = row
            .try_get(0)
            .map_err(|source| Error::Announcement(AnnouncementFailure::Row { stage, source }))?;
        Value::parse_json(&json)
            .map(Some)
            .map_err(|source| Error::Announcement(AnnouncementFailure::Json { source }))
    }
    async fn read(&self, announcement_id: &str, viewer_id: &str) -> Result<bool, Error> {
        let stage = Stage::ReadState;
        // The source makes two ordinary repository calls: each read selects
        // and releases its own target/session, without a shared lease or TX.
        let rows = self
            .0
            .database
            .query_typed(
                true,
                &self.0.announcement_queries.read,
                &[&announcement_id, &viewer_id],
            )
            .await
            .map_err(|source| Error::Announcement(AnnouncementFailure::Query { stage, source }))?;
        let [row] = rows.as_slice() else {
            return Err(shape(stage, ShapeKind::QueryShape));
        };
        row.try_get(0)
            .map_err(|source| Error::Announcement(AnnouncementFailure::Row { stage, source }))
    }
}
async fn run(
    store: &(impl AnnouncementStore + Sync),
    params: &Value,
    id_method: &str,
    viewer: Option<&str>,
) -> Result<Value, Error> {
    let requested_id = params
        .get("announcementId")
        .and_then(Value::as_utf16)
        .ok_or_else(|| shape(Stage::Lookup, ShapeKind::InvalidParameter))?;
    // This is the pg bind boundary; the public JS string itself stays UTF-16.
    let requested_id = String::from_utf16_lossy(requested_id);
    let raw = store.load(&requested_id).await?.ok_or(Error::Fixed(
        crate::response::ApiFailure::NoSuchAnnouncement,
    ))?;
    let target = nullable_text(field(&raw, "userId", Stage::Lookup)?, Stage::Lookup)?;
    let viewer_units = viewer.map(|id| id.encode_utf16().collect::<Vec<_>>());
    // AnnouncementService.getAnnouncement deliberately hides a truthy target
    // from every nonowner, including root, before read-state/date processing.
    if target.is_some_and(|id| !id.is_empty() && Some(id) != viewer_units.as_deref()) {
        return Err(Error::Fixed(
            crate::response::ApiFailure::NoSuchAnnouncement,
        ));
    }
    let is_read = match viewer {
        Some(viewer) => {
            let id = String::from_utf16_lossy(text(
                field(&raw, "id", Stage::ReadState)?,
                Stage::ReadState,
            )?);
            Some(store.read(&id, viewer).await?)
        }
        None => None,
    };
    let for_you = target
        .zip(viewer_units.as_deref())
        .is_some_and(|(target, viewer)| target == viewer);
    Ok(AnnouncementRow::decode(&raw, id_method)?.pack(for_you, is_read))
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
                .ok_or_else(|| shape(Stage::Lookup, ShapeKind::InvalidPrincipal))
        })
        .transpose()?;
    run(
        &RuntimeStore(runtime),
        params,
        &runtime.business_config.id_method,
        viewer,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn fixture() -> Value {
        Value::parse_json(include_str!(
            "fixtures/announcements-show-source-oracle.json"
        ))
        .unwrap()
    }
    fn array(value: &Value) -> &[Value] {
        let Value::Array(values) = value else {
            panic!("captured array")
        };
        values
    }
    fn http_input(case: &Value) -> Value {
        let mut row = case.get("storedRow").unwrap().clone();
        row.set("updatedAt", row.get("updatedAtMs").unwrap().clone());
        row
    }
    fn source_case<'a>(fixture: &'a Value, name: &str) -> &'a Value {
        array(fixture.get("source").unwrap().get("cases").unwrap())
            .iter()
            .find(|case| case.get("name") == Some(&Value::string(name)))
            .unwrap()
    }
    struct Store {
        row: Option<Value>,
        read: bool,
        calls: Mutex<Vec<Value>>,
    }
    impl Store {
        fn new(row: Option<Value>, read: bool) -> Self {
            Self {
                row,
                read,
                calls: Mutex::new(Vec::new()),
            }
        }
    }
    impl AnnouncementStore for Store {
        async fn load(&self, id: &str) -> Result<Option<Value>, Error> {
            self.calls.lock().unwrap().push(Value::string("lookup"));
            if let Some(row) = &self.row {
                assert_eq!(row.get("id"), Some(&Value::string(id)));
            }
            Ok(self.row.clone())
        }
        async fn read(&self, announcement_id: &str, _: &str) -> Result<bool, Error> {
            self.calls.lock().unwrap().push(Value::string("read_state"));
            assert_eq!(
                self.row.as_ref().unwrap().get("id"),
                Some(&Value::string(announcement_id))
            );
            Ok(self.read)
        }
    }
    fn without_correlation(mut body: Value) -> Value {
        let Some(info) = body.get("error").and_then(|error| error.get("info")) else {
            return body;
        };
        let Some(detail) = info.get("e") else {
            return body;
        };
        let mut detail = detail.clone();
        detail.set("id", Value::Undefined);
        let mut info = info.clone();
        info.set("e", detail);
        let mut error = body.get("error").unwrap().clone();
        error.set("info", info);
        body.set("error", error);
        body
    }

    #[tokio::test]
    async fn actual_http_rows_preserve_raw_success_dates_privacy_and_query_order() {
        let fixture = fixture();
        for case in array(fixture.get("httpCases").unwrap()) {
            let raw = http_input(case);
            let params = Value::object([("announcementId", raw.get("id").unwrap().clone())]);
            let viewer = case.get("viewerId").unwrap().to_utf8();
            let read = matches!(case.get("readBoolean"), Some(Value::Boolean(true)));
            let store = Store::new(Some(raw), read);
            let result = run(
                &store,
                &params,
                &case.get("idMethod").unwrap().to_utf8().unwrap(),
                viewer.as_deref(),
            )
            .await;
            let status = case.get("status").unwrap().as_number().unwrap();
            if status == 200. {
                assert_eq!(
                    result.unwrap().stringify().unwrap(),
                    case.get("rawBody").unwrap().to_utf8().unwrap()
                );
            } else {
                let response = result.unwrap_err().response();
                assert_eq!(f64::from(response.status().as_u16()), status);
                let bytes = axum::body::to_bytes(response.into_body(), 16 * 1024)
                    .await
                    .unwrap();
                let actual = Value::parse_json(std::str::from_utf8(&bytes).unwrap()).unwrap();
                assert_eq!(
                    without_correlation(actual),
                    without_correlation(case.get("body").unwrap().clone())
                );
            }
            assert_eq!(
                &*store.calls.lock().unwrap(),
                array(case.get("businessQueryOrder").unwrap())
            );
        }
    }

    #[tokio::test]
    async fn opaque_utf16_and_stored_enum_strings_match_actual_source_fields() {
        let fixture = fixture();
        let mut raw = http_input(&array(fixture.get("httpCases").unwrap())[0]);
        // These inputs are the actual source-oracle experiment, not values
        // reconstructed from its expected DTO. ID/date remain the real HTTP row.
        for (name, value) in [
            ("title", Value::string("")),
            ("text", Value::String(vec![0x96ea, 0xd800, 0x78, 0xdfff])),
            (
                "imageUrl",
                Value::string("https://example.invalid/opaque-image"),
            ),
            ("icon", Value::string("stored-other-icon")),
            ("display", Value::string("stored-other-display")),
            ("needConfirmationToRead", Value::Boolean(true)),
            ("isActive", Value::Boolean(false)),
            ("forExistingUsers", Value::Boolean(true)),
            ("silence", Value::Boolean(true)),
        ] {
            raw.set(name, value);
        }
        let params = Value::object([("announcementId", raw.get("id").unwrap().clone())]);
        let actual = run(&Store::new(Some(raw), false), &params, "aidx", None)
            .await
            .unwrap();
        let source = source_case(
            &fixture,
            "global-inactive-forExistingUsers-silent-opaque-text-enum-values",
        );
        let expected = source.get("output").unwrap();
        for name in [
            "title",
            "text",
            "imageUrl",
            "icon",
            "display",
            "forYou",
            "needConfirmationToRead",
            "silence",
        ] {
            assert_eq!(actual.get(name), expected.get(name));
        }
        let actual_keys = match &actual {
            Value::Object(fields) => fields
                .iter()
                .map(|(key, _)| Value::String(key.clone()))
                .collect::<Vec<_>>(),
            _ => panic!("packed object"),
        };
        assert_eq!(
            actual_keys,
            array(
                fixture
                    .get("source")
                    .unwrap()
                    .get("expectedSourceObjectKeys")
                    .unwrap()
            )
        );
        assert_eq!(actual.get("isRead"), Some(&Value::Undefined));
        assert!(actual.get("isActive").is_none());
        assert!(actual.get("forExistingUsers").is_none());
    }

    #[tokio::test]
    async fn missing_row_stops_before_read_state_like_actual_source() {
        let fixture = fixture();
        let store = Store::new(None, false);
        let params = Value::object([("announcementId", Value::string("missing-announcement"))]);
        let failure = run(&store, &params, "aidx", Some("owner"))
            .await
            .unwrap_err();
        assert!(matches!(
            failure,
            Error::Fixed(crate::response::ApiFailure::NoSuchAnnouncement)
        ));
        let source = source_case(&fixture, "missing-maps-actual-EntityNotFoundError");
        assert_eq!(
            array(source.get("calls").unwrap()).len(),
            store.calls.lock().unwrap().len()
        );
    }
}
