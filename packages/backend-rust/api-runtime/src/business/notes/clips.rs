//! Clip reads, including viewer-specific fields.
use super::{
    Error, Value, api_error, iso_date, parse_id_date,
    row::{boolean, field, nullable_string, string},
    rows, users,
};
use crate::Runtime;
use misskey_backend_api_auth::{
    ColumnKind, Context, EntityColumn,
    postgres::{quote_identifier, quote_literal},
};
use std::collections::{HashMap, HashSet};
use tokio_postgres::types::ToSql;

const SHOW_SQL: &str = r#"SELECT {projection} FROM {table} c WHERE c.{id}=$1 LIMIT 1"#;

// Consumed TypeORM properties and hydration kinds, not database identifiers.
const SHOW_PROPERTIES: &[(&str, ColumnKind)] = &[
    ("id", ColumnKind::Json),
    ("userId", ColumnKind::Json),
    ("lastClippedAt", ColumnKind::Date),
    ("name", ColumnKind::Json),
    ("description", ColumnKind::Json),
    ("isPublic", ColumnKind::Json),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureKind {
    MissingLayout,
    InvalidIdentifier,
    UnknownProperty,
    DuplicateProperty,
    MissingProperty,
    HydrationKind,
    QueryTemplate,
    Query,
    QueryRowType,
    QueryRowJson,
    InvalidParameter,
    InvalidPrincipal,
    AccessMissingField,
    AccessFieldType,
    PackedTimestampType,
}

pub(crate) struct Queries {
    show: String,
}

fn validate_show_columns(columns: &[EntityColumn]) -> Result<(), FailureKind> {
    let mut seen = HashSet::new();
    for column in columns {
        if column.database_name.is_empty() || column.database_name.contains('\0') {
            return Err(FailureKind::InvalidIdentifier);
        }
        let (_, expected_kind) = SHOW_PROPERTIES
            .iter()
            .find(|(property, _)| *property == column.property_name)
            .ok_or(FailureKind::UnknownProperty)?;
        if !seen.insert(column.property_name.as_str()) {
            return Err(FailureKind::DuplicateProperty);
        }
        if column.kind != *expected_kind {
            return Err(FailureKind::HydrationKind);
        }
    }
    if seen.len() != SHOW_PROPERTIES.len() {
        return Err(FailureKind::MissingProperty);
    }
    Ok(())
}

impl Queries {
    pub(crate) fn new(layouts: &HashMap<String, Vec<EntityColumn>>) -> Result<Self, FailureKind> {
        let (table, columns) = layouts
            .get_key_value("clip")
            .ok_or(FailureKind::MissingLayout)?;
        if table.is_empty() || table.contains('\0') {
            return Err(FailureKind::InvalidIdentifier);
        }
        validate_show_columns(columns)?;
        let id = columns
            .iter()
            .find(|column| column.property_name == "id")
            .ok_or(FailureKind::MissingProperty)?;
        let projection = columns
            .iter()
            .map(|column| {
                let reference = format!("c.{}", quote_identifier(&column.database_name));
                let value = match column.kind {
                    ColumnKind::Date => {
                        format!("floor(extract(epoch FROM {reference})*1000)::double precision")
                    }
                    ColumnKind::Json => reference,
                    ColumnKind::SimpleArray => return Err(FailureKind::HydrationKind),
                };
                Ok(format!("{}, {value}", quote_literal(&column.property_name)))
            })
            .collect::<Result<Vec<_>, FailureKind>>()?
            .join(", ");
        let projection = format!("json_build_object({projection})::text");
        let table = quote_identifier(table);
        let id = quote_identifier(&id.database_name);
        // Replace only source spans: braces inside metadata identifiers must
        // not be interpreted as another placeholder after insertion.
        let mut show = String::new();
        let mut remaining = SHOW_SQL;
        while let Some(start) = remaining.find('{') {
            show.push_str(&remaining[..start]);
            let tail = &remaining[start + 1..];
            let end = tail.find('}').ok_or(FailureKind::QueryTemplate)?;
            show.push_str(match &tail[..end] {
                "projection" => &projection,
                "table" => &table,
                "id" => &id,
                _ => return Err(FailureKind::QueryTemplate),
            });
            remaining = &tail[end + 1..];
        }
        show.push_str(remaining);
        Ok(Self { show })
    }
}

fn invalid_date() -> Error {
    // Reuse the observed AntennaEntityService/IdService Date.toISOString error.
    // Clip-specific runtime differential evidence is still required.
    let definition = &crate::response::INVALID_DATE;
    Error::legacy(definition.message.to_owned(), definition.code)
}

struct Clip {
    id: String,
    user_id: String,
    created_at: String,
    last_clipped_at: Option<String>,
    name: String,
    description: Option<String>,
    is_public: bool,
}

impl Clip {
    fn decode(row: &Value, id_method: &str) -> Result<Self, Error> {
        let id = string(row, "id")?;
        let milliseconds = parse_id_date(id_method, &id).ok_or_else(invalid_date)?;
        let created_at = iso_date(milliseconds).map_err(|_date_error| invalid_date())?;
        let last_clipped_at = match field(row, "lastClippedAt")? {
            Value::Null => None,
            // iso_date can only reject an unrepresentable JavaScript Date.
            Value::Number(milliseconds) => {
                Some(iso_date(*milliseconds).map_err(|_date_error| invalid_date())?)
            }
            Value::String(value)
                if value.iter().copied().eq("Infinity".encode_utf16())
                    || value.iter().copied().eq("-Infinity".encode_utf16()) =>
            {
                let definition = &crate::response::NON_DATE_CLIP_TIMESTAMP;
                return Err(Error::legacy(
                    definition.message.to_owned(),
                    definition.code,
                ));
            }
            _ => return Err(Error::ClipShow(FailureKind::PackedTimestampType)),
        };
        Ok(Self {
            id,
            user_id: string(row, "userId")?,
            created_at,
            last_clipped_at,
            name: string(row, "name")?,
            description: nullable_string(row, "description")?,
            is_public: boolean(row, "isPublic")?,
        })
    }

    fn pack(
        &self,
        user: Value,
        favorited_count: f64,
        is_favorited: Option<bool>,
        notes_count: Option<f64>,
    ) -> Value {
        Value::object([
            ("id", Value::string(&self.id)),
            ("createdAt", Value::string(&self.created_at)),
            (
                "lastClippedAt",
                self.last_clipped_at
                    .as_deref()
                    .map(Value::string)
                    .unwrap_or(Value::Null),
            ),
            ("userId", Value::string(&self.user_id)),
            ("user", user),
            ("name", Value::string(&self.name)),
            (
                "description",
                self.description
                    .as_deref()
                    .map(Value::string)
                    .unwrap_or(Value::Null),
            ),
            ("isPublic", Value::Boolean(self.is_public)),
            ("favoritedCount", Value::Number(favorited_count)),
            (
                "isFavorited",
                is_favorited.map(Value::Boolean).unwrap_or(Value::Undefined),
            ),
            (
                "notesCount",
                notes_count.map(Value::Number).unwrap_or(Value::Undefined),
            ),
        ])
    }
}

fn viewer_id(auth: &Context) -> Result<Option<&str>, Error> {
    auth.user
        .as_ref()
        .map(|user| {
            user.get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or(Error::ClipShow(FailureKind::InvalidPrincipal))
        })
        .transpose()
}

fn scalar_count(values: &[Value]) -> Result<f64, Error> {
    match values {
        [Value::Number(count)] if count.is_finite() && *count >= 0. && count.fract() == 0. => {
            Ok(*count)
        }
        _ => Err(Error::Internal),
    }
}

fn scalar_exists(values: &[Value]) -> Result<bool, Error> {
    match values {
        [Value::Boolean(exists)] => Ok(*exists),
        _ => Err(Error::Internal),
    }
}

async fn pack_clip(
    runtime: &Runtime,
    row: &Value,
    packed_users: &HashMap<String, Value>,
    viewer: Option<&str>,
) -> Result<Value, Error> {
    // packMany supplies already-packed authors; direct pack supplies no hint.
    // Decode at this stage in either case, just like ClipEntityService.pack.
    let clip = Clip::decode(row, &runtime.business_config.id_method)?;
    let user = async {
        match packed_users.get(&clip.user_id) {
            Some(user) => Ok(user.clone()),
            None => {
                let anonymous = Context {
                    user: None,
                    token: None,
                    access_token_reduced: false,
                };
                users::pack_user_by_id(runtime, &clip.user_id, &anonymous).await
            }
        }
    };
    // The awaits in the TS object literal are sequential per clip. The outer
    // packMany still runs clips concurrently and retains the input order.
    let counts = async {
        let favorited_count = scalar_count(
            &rows(
                &runtime.database,
                true,
                "SELECT to_json(COUNT(*))::text FROM clip_favorite WHERE \"clipId\"=$1",
                &[&clip.id],
            )
            .await?,
        )?;
        let is_favorited = match viewer.filter(|id| !id.is_empty()) {
            Some(viewer) => Some(scalar_exists(
                &rows(
                    &runtime.database,
                    true,
                    "SELECT to_json(EXISTS(SELECT 1 FROM clip_favorite WHERE \"clipId\"=$1 AND \"userId\"=$2))::text",
                    &[&clip.id, &viewer],
                )
                .await?,
            )?),
            None => None,
        };
        let notes_count = if viewer == Some(clip.user_id.as_str()) {
            Some(scalar_count(
                &rows(
                    &runtime.database,
                    true,
                    "SELECT to_json(COUNT(*))::text FROM clip_note WHERE \"clipId\"=$1",
                    &[&clip.id],
                )
                .await?,
            )?)
        } else {
            None
        };
        Ok::<_, Error>((favorited_count, is_favorited, notes_count))
    };
    // TS starts the missing-author promise before the scalar awaits, then
    // resolves it with awaitAll. Keep both futures owned by this request and
    // both paths in flight; scalar errors take precedence.
    let (user, counts) = tokio::join!(user, counts);
    let (favorited_count, is_favorited, notes_count) = counts?;
    Ok(clip.pack(user?, favorited_count, is_favorited, notes_count))
}

fn no_such_clip() -> Error {
    Error::Fixed(crate::response::ApiFailure::NoSuchClip)
}

fn visible_clip<'a>(row: Option<&'a Value>, viewer: Option<&str>) -> Result<&'a Value, Error> {
    let row = row.ok_or_else(no_such_clip)?;
    // clips/show hides private existence before Clip.pack reads dates, author
    // content or counts. Anonymous denial also short-circuits the owner read.
    let is_public = match row.get("isPublic") {
        Some(Value::Boolean(value)) => *value,
        Some(_) => return Err(Error::ClipShow(FailureKind::AccessFieldType)),
        None => return Err(Error::ClipShow(FailureKind::AccessMissingField)),
    };
    if is_public {
        return Ok(row);
    }
    let Some(viewer) = viewer else {
        return Err(no_such_clip());
    };
    let owner = match row.get("userId") {
        Some(Value::String(units)) => String::from_utf16(units)
            .map_err(|_unicode_error| Error::ClipShow(FailureKind::AccessFieldType))?,
        Some(_) => return Err(Error::ClipShow(FailureKind::AccessFieldType)),
        None => return Err(Error::ClipShow(FailureKind::AccessMissingField)),
    };
    if owner == viewer {
        Ok(row)
    } else {
        Err(no_such_clip())
    }
}

async fn load_clip(runtime: &Runtime, clip_id: &str) -> Result<Option<Value>, Error> {
    // New query failures retain QueryError for the shared legacy projection;
    // row decoding failures carry bounded kinds rather than opaque Internal.
    let rows = runtime
        .database
        .query_typed(true, &runtime.clip_queries.show, &[&clip_id])
        .await
        .map_err(|source| Error::ClipShowQuery {
            kind: FailureKind::Query,
            source,
        })?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let text: String = row
        .try_get(0)
        .map_err(|_row_error| Error::ClipShow(FailureKind::QueryRowType))?;
    Value::parse_json(&text)
        .map(Some)
        .map_err(|_json_error| Error::ClipShow(FailureKind::QueryRowJson))
}

pub(in crate::business) async fn show(
    runtime: &Runtime,
    params: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    // Shared request validation guarantees an ASCII misskey:id string.
    let clip_id = match params.get("clipId").and_then(Value::to_utf8) {
        Some(clip_id) => clip_id,
        None => return Err(Error::ClipShow(FailureKind::InvalidParameter)),
    };
    let row = load_clip(runtime, &clip_id).await?;
    let viewer = viewer_id(auth)?;
    let clip = visible_clip(row.as_ref(), viewer)?;
    // ClipEntityService.pack(clip, me) calls UserEntityService.pack without me.
    // An empty hint preserves anonymous author visibility even for root/owner,
    // while pack_clip still derives favorite and note counts from the viewer.
    pack_clip(runtime, clip, &HashMap::new(), viewer).await
}

pub(in crate::business) async fn clips(
    runtime: &Runtime,
    params: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    let note_id = string(params, "noteId")?;
    // GetterService checks existence only for this endpoint. Do not decode
    // note content or introduce notes/show/notes/reactions visibility gates.
    let notes = rows(
        &runtime.database,
        true,
        "SELECT to_json(id)::text FROM note WHERE id=$1 LIMIT 1",
        &[&note_id],
    )
    .await?;
    let Some(note) = notes.first() else {
        return Err(api_error(
            "No such note.",
            "NO_SUCH_NOTE",
            "47db1a1c-b0af-458d-8fb4-986e4efafe1e",
        ));
    };
    let note_id = note.to_utf8().ok_or(Error::Internal)?;
    let mappings = rows(
        &runtime.database,
        true,
        "SELECT json_build_object('clipId',\"clipId\")::text FROM clip_note WHERE \"noteId\"=$1",
        &[&note_id],
    )
    .await?;
    let ids = mappings
        .iter()
        .map(|row| string(row, "clipId"))
        .collect::<Result<Vec<_>, _>>()?;
    if ids.is_empty() {
        return Ok(Value::Array(Vec::new()));
    }
    let slots = (1..=ids.len())
        .map(|index| format!("${index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT json_build_object('id',c.id,'userId',c.\"userId\",'lastClippedAt',floor(extract(epoch FROM c.\"lastClippedAt\")*1000)::double precision,'name',c.name,'description',c.description,'isPublic',c.\"isPublic\")::text FROM clip c WHERE c.id IN ({slots}) AND c.\"isPublic\"=true"
    );
    let params: Vec<&(dyn ToSql + Sync)> =
        ids.iter().map(|id| -> &(dyn ToSql + Sync) { id }).collect();
    // Public selection precedes any clip or author payload interpretation.
    // Keep IN deduplication and the DB's returned sequence, without ORDER BY.
    let clips = rows(&runtime.database, true, &sql, &params).await?;
    let user_ids = clips
        .iter()
        .map(|clip| string(clip, "userId"))
        .collect::<Result<Vec<_>, _>>()?;
    let packed_users = users::pack_many_users(runtime, &user_ids, auth).await?;
    let viewer = viewer_id(auth)?;
    Ok(Value::Array(
        futures_util::future::try_join_all(
            clips
                .iter()
                .map(|clip| pack_clip(runtime, clip, &packed_users, viewer)),
        )
        .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_clip_dates_follow_observed_ts_errors() {
        let oracle: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/clip-date-oracle.json")).unwrap();
        for case in oracle["cases"].as_array().unwrap() {
            let Err(Error::Legacy { message, code, .. }) =
                Clip::decode(&Value::from_json(&case["row"]), "aidx")
            else {
                panic!("observed legacy error required: {}", case["name"]);
            };
            assert_eq!(message, case["error"]["message"].as_str().unwrap());
            assert_eq!(code, case["error"]["code"].as_str().unwrap());
        }
    }

    fn clip_layout() -> HashMap<String, Vec<EntityColumn>> {
        HashMap::from([(
            "clip".to_owned(),
            SHOW_PROPERTIES
                .iter()
                .map(|(property, kind)| EntityColumn {
                    property_name: (*property).to_owned(),
                    database_name: (*property).to_owned(),
                    kind: *kind,
                })
                .collect(),
        )])
    }

    #[test]
    fn show_layout_fails_closed_for_missing_unknown_duplicate_and_wrong_kind() {
        assert!(matches!(
            Queries::new(&HashMap::new()),
            Err(FailureKind::MissingLayout)
        ));
        let layout = clip_layout();
        let columns = &layout["clip"];
        assert_eq!(
            validate_show_columns(&columns[1..]),
            Err(FailureKind::MissingProperty),
        );
        let mut bad = columns.clone();
        bad[0].property_name = "unconsumed".to_owned();
        assert_eq!(
            validate_show_columns(&bad),
            Err(FailureKind::UnknownProperty)
        );
        let mut bad = columns.clone();
        bad.push(bad[0].clone());
        assert_eq!(
            validate_show_columns(&bad),
            Err(FailureKind::DuplicateProperty)
        );
        let mut bad = columns.clone();
        bad[0].kind = ColumnKind::Date;
        assert_eq!(validate_show_columns(&bad), Err(FailureKind::HydrationKind));
        let mut bad = columns.clone();
        bad[0].database_name = "bad\0identifier".to_owned();
        assert_eq!(
            validate_show_columns(&bad),
            Err(FailureKind::InvalidIdentifier)
        );
    }

    #[test]
    fn show_query_uses_quoted_metadata_identifiers_without_replacing_inserted_braces() {
        let mut layout = clip_layout();
        let columns = layout.get_mut("clip").unwrap();
        columns[0].database_name = "select\"{table}".to_owned();
        columns[2].database_name = "date column".to_owned();
        let queries = Queries::new(&layout).unwrap();
        assert!(
            queries
                .show
                .contains(r#"FROM "clip" c WHERE c."select""{table}"=$1 LIMIT 1"#)
        );
        assert!(queries.show.contains(r#"'id', c."select""{table}""#));
        assert!(queries.show.contains(
            r#"'lastClippedAt', floor(extract(epoch FROM c."date column")*1000)::double precision"#,
        ));
    }

    fn assert_no_such_clip(result: Result<&Value, Error>) {
        match result {
            Err(Error::Fixed(crate::response::ApiFailure::NoSuchClip)) => {}
            _ => panic!("expected the endpoint-specific missing-clip error"),
        }
    }

    #[test]
    fn show_missing_and_private_denials_share_the_endpoint_error() {
        let private = Value::object([
            ("isPublic", Value::Boolean(false)),
            ("userId", Value::string("owner")),
        ]);
        for viewer in [None, Some("other"), Some("root")] {
            assert_no_such_clip(visible_clip(None, viewer));
            assert_no_such_clip(visible_clip(Some(&private), viewer));
        }
        assert!(visible_clip(Some(&private), Some("owner")).is_ok());
    }

    #[test]
    fn show_acl_reads_no_unrelated_payload_before_denial() {
        // These rows intentionally omit every date, author and content field.
        let private = Value::object([("isPublic", Value::Boolean(false))]);
        assert_no_such_clip(visible_clip(Some(&private), None));
        let public = Value::object([("isPublic", Value::Boolean(true))]);
        for viewer in [None, Some("owner"), Some("other"), Some("root")] {
            assert!(visible_clip(Some(&public), viewer).is_ok());
        }
    }

    #[test]
    fn show_private_denial_precedes_invalid_created_at_interpretation() {
        let private = Value::object([
            ("id", Value::string("zzzzzzzzzzzzzzzz")),
            ("isPublic", Value::Boolean(false)),
            ("userId", Value::string("owner")),
        ]);
        assert_no_such_clip(visible_clip(Some(&private), None));
        assert_no_such_clip(visible_clip(Some(&private), Some("other")));
        let owner_row = visible_clip(Some(&private), Some("owner")).unwrap();
        // Candidate assertion using the existing observed antenna error only;
        // this does not establish a clips/show TS runtime baseline.
        match Clip::decode(owner_row, "objectid") {
            Err(Error::Legacy { message, code, .. }) => {
                assert_eq!(message, crate::response::INVALID_DATE.message);
                assert_eq!(code, crate::response::INVALID_DATE.code);
            }
            _ => panic!("expected the shared invalid-date error"),
        }
    }

    #[test]
    fn show_reuses_clip_field_order_and_keeps_zero_false_null_and_empty_text() {
        let mut clip = clip(Value::Null);
        clip.description = Some(String::new());
        let packed = clip.pack(Value::Null, 0., Some(false), Some(0.));
        assert_eq!(
            packed.stringify().unwrap(),
            r#"{"id":"0000000000000001","createdAt":"2000-01-01T00:00:00.000Z","lastClippedAt":null,"userId":"owner","user":null,"name":"clip 😀","description":"","isPublic":true,"favoritedCount":0,"isFavorited":false,"notesCount":0}"#,
        );
    }

    fn clip(last_clipped_at: Value) -> Clip {
        Clip::decode(
            &Value::object([
                ("id", Value::string("0000000000000001")),
                ("userId", Value::string("owner")),
                ("lastClippedAt", last_clipped_at),
                ("name", Value::string("clip 😀")),
                ("description", Value::Null),
                ("isPublic", Value::Boolean(true)),
            ]),
            "aidx",
        )
        .unwrap()
    }

    #[test]
    fn null_and_epoch_last_clipped_dates_are_distinct() {
        assert_eq!(clip(Value::Null).last_clipped_at, None);
        assert_eq!(
            clip(Value::Number(0.)).last_clipped_at.as_deref(),
            Some("1970-01-01T00:00:00.000Z")
        );
        assert_eq!(
            clip(Value::Number(-1.)).last_clipped_at.as_deref(),
            Some("1969-12-31T23:59:59.999Z")
        );
    }

    #[test]
    fn anonymous_fields_are_omitted_and_nullable_fields_remain_null() {
        let packed = clip(Value::Null).pack(Value::Null, 0., None, None);
        let json = packed.stringify().unwrap();
        assert!(!json.contains("isFavorited"));
        assert!(!json.contains("notesCount"));
        assert!(json.contains(r#""lastClippedAt":null"#));
        assert!(json.contains(r#""description":null"#));
        assert_eq!(
            packed.get("favoritedCount").and_then(Value::as_number),
            Some(0.)
        );
    }

    #[test]
    fn authenticated_false_and_owner_zero_count_stay_present() {
        let packed = clip(Value::Null).pack(Value::Null, 0., Some(false), Some(0.));
        assert!(
            packed
                .stringify()
                .unwrap()
                .contains(r#""isFavorited":false"#)
        );
        assert_eq!(
            packed.get("notesCount").and_then(Value::as_number),
            Some(0.)
        );
    }

    #[test]
    fn aggregate_counts_keep_javascript_number_projection() {
        let rounded = Value::parse_json("9007199254740993").unwrap();
        assert_eq!(scalar_count(&[rounded]).unwrap(), 9_007_199_254_740_992.);
        assert!(!scalar_exists(&[Value::Boolean(false)]).unwrap());
        assert!(scalar_count(&[Value::string("1")]).is_err());
        assert!(scalar_exists(&[Value::Number(0.)]).is_err());
    }

    #[test]
    fn viewer_is_derived_from_a_typed_auth_principal() {
        let anonymous = Context {
            user: None,
            token: None,
            access_token_reduced: false,
        };
        assert_eq!(viewer_id(&anonymous).unwrap(), None);
        let malformed = Context {
            user: Some(serde_json::json!({"id": 1})),
            token: None,
            access_token_reduced: false,
        };
        assert!(viewer_id(&malformed).is_err());
    }
}
