//! Public clips containing an existing note, including viewer-specific fields.
use super::{
    Error, Value, api_error, iso_date, parse_id_date,
    row::{boolean, field, nullable_string, string},
    rows, users,
};
use crate::Runtime;
use misskey_backend_api_auth::Context;
use std::collections::HashMap;
use tokio_postgres::types::ToSql;

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
        let created_at = iso_date(parse_id_date(id_method, &id).ok_or(Error::Internal)?)?;
        let last_clipped_at = match field(row, "lastClippedAt")? {
            Value::Null => None,
            Value::Number(milliseconds) => Some(iso_date(*milliseconds)?),
            _ => return Err(Error::Internal),
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
                .ok_or(Error::Internal)
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
    // packMany has already packed the selected public clips' authors. Keep
    // clip payload decoding after that stage, just like ClipEntityService.
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
