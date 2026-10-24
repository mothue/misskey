//! Native visible-note reaction polling, sharing note reaction/emoji packing.
use super::{
    Error, Value, entities, reactions,
    row::{Visibility, field, nullable_string, string, strings},
    rows,
};
use crate::Runtime;
use misskey_backend_api_auth::Context;
use std::collections::HashSet;
use tokio_postgres::types::ToSql;

struct PartialNote {
    id: String,
    user_id: String,
    user_host: Option<String>,
    visibility: Visibility,
    visible_user_ids: Vec<Option<String>>,
    mentions: Vec<Option<String>>,
    reply_user_id: Option<String>,
    reactions: Value,
}

impl PartialNote {
    fn decode(value: &Value) -> Result<Self, Error> {
        let visibility = match string(value, "visibility")?.as_str() {
            "public" => Visibility::Public,
            "home" => Visibility::Home,
            "followers" => Visibility::Followers,
            "specified" => Visibility::Specified,
            _ => return Err(Error::Internal),
        };
        Ok(Self {
            id: string(value, "id")?,
            user_id: string(value, "userId")?,
            user_host: nullable_string(value, "userHost")?,
            visibility,
            visible_user_ids: membership_ids(value, "visibleUserIds")?,
            mentions: membership_ids(value, "mentions")?,
            reply_user_id: nullable_string(value, "replyUserId")?,
            // Payload interpretation belongs to packing after visibility,
            // matching fetchDiffs; a hidden payload cannot change the response.
            reactions: field(value, "reactions")?.clone(),
        })
    }
}

// PostgreSQL varchar[] permits NULL members. They cannot match an authenticated
// string ID, just as Array.includes(viewerId) in the existing TS path.
fn membership_ids(value: &Value, key: &str) -> Result<Vec<Option<String>>, Error> {
    let Value::Array(ids) = field(value, key)? else {
        return Err(Error::Internal);
    };
    ids.iter()
        .map(|id| match id {
            Value::Null => Ok(None),
            id => id.to_utf8().map(Some).ok_or(Error::Internal),
        })
        .collect()
}

/// isVisibleForMe is deliberately distinct from shouldHideNote: this polling
/// API does not apply author-time/signin restrictions or rewrite visibility.
/// None means the existing followers branch must query following + viewer.
fn visibility_without_following(note: &PartialNote, viewer: Option<&str>) -> Option<bool> {
    match note.visibility {
        Visibility::Public | Visibility::Home => Some(true),
        Visibility::Specified => Some(viewer.is_some_and(|viewer| {
            viewer == note.user_id
                || note
                    .visible_user_ids
                    .iter()
                    .any(|id| id.as_deref() == Some(viewer))
        })),
        Visibility::Followers => match viewer {
            None => Some(false),
            Some(viewer)
                if viewer == note.user_id
                    || note.reply_user_id.as_deref() == Some(viewer)
                    || note.mentions.iter().any(|id| id.as_deref() == Some(viewer)) =>
            {
                Some(true)
            }
            Some(_) => None,
        },
    }
}

fn visible(
    note: &PartialNote,
    viewer: Option<&str>,
    following: &HashSet<String>,
    viewer_remote: bool,
) -> bool {
    visibility_without_following(note, viewer).unwrap_or_else(|| {
        following.contains(&note.user_id) || (note.user_host.is_some() && viewer_remote)
    })
}

fn placeholders(count: usize, first: usize) -> String {
    (first..first + count)
        .map(|position| format!("${position}"))
        .collect::<Vec<_>>()
        .join(", ")
}

async fn load_notes(runtime: &Runtime, ids: &[String]) -> Result<Vec<PartialNote>, Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // Keep TypeORM's IN predicate and lack of ORDER BY; preserve the database's
    // returned sequence, including its natural deduplication of input IDs.
    let sql = format!(
        "SELECT json_build_object('id',n.id,'userId',n.\"userId\",'userHost',n.\"userHost\",'visibility',n.visibility,'visibleUserIds',n.\"visibleUserIds\",'mentions',n.mentions,'replyUserId',n.\"replyUserId\",'reactions',n.reactions,'reactionAndUserPairCache',n.\"reactionAndUserPairCache\")::text FROM note n WHERE n.id IN ({})",
        placeholders(ids.len(), 1)
    );
    let params: Vec<&(dyn ToSql + Sync)> = ids.iter().map(|id| id as _).collect();
    rows(&runtime.database, true, &sql, &params)
        .await?
        .iter()
        .map(PartialNote::decode)
        .collect()
}

async fn following_context(
    runtime: &Runtime,
    notes: &[PartialNote],
    viewer: Option<&str>,
) -> Result<(HashSet<String>, bool), Error> {
    let required: Vec<_> = notes
        .iter()
        .filter(|note| visibility_without_following(note, viewer).is_none())
        .map(|note| &note.user_id)
        .collect();
    if required.is_empty() {
        return Ok((HashSet::new(), false));
    }
    let viewer = viewer.ok_or(Error::Internal)?;
    let sql = format!(
        "SELECT json_build_object('followeeId',\"followeeId\")::text FROM following WHERE \"followerId\"=$1 AND \"followeeId\" IN ({})",
        placeholders(required.len(), 2)
    );
    let mut params: Vec<&(dyn ToSql + Sync)> = vec![&viewer];
    params.extend(required.iter().map(|id| *id as &(dyn ToSql + Sync)));
    let viewer_params: [&(dyn ToSql + Sync); 1] = [&viewer];
    // This operation's original visibility path reads the DB directly. Do not
    // substitute the separate notes/show follower cache or cached auth host.
    let (followings, users) = tokio::try_join!(
        rows(&runtime.database, true, &sql, &params),
        rows(
            &runtime.database,
            true,
            "SELECT json_build_object('host',host)::text FROM \"user\" WHERE id=$1 LIMIT 1",
            &viewer_params
        ),
    )?;
    let Some(user) = users.first() else {
        // TypeORM findOneByOrFail's ordinary error is retained, including its
        // stable class/message; correlation IDs remain request-specific.
        let encoded = serde_json::to_string(viewer).map_err(|_| Error::Internal)?;
        return Err(Error::legacy(
            format!(
                "Could not find any entity of type \"MiUser\" matching: {{\n    \"id\": {encoded}\n}}"
            ),
            "EntityNotFoundError",
        ));
    };
    let remote = nullable_string(user, "host")?.is_some();
    let followings = followings
        .iter()
        .map(|record| string(record, "followeeId"))
        .collect::<Result<_, _>>()?;
    Ok((followings, remote))
}

pub(in crate::business) async fn show_partial_bulk(
    runtime: &Runtime,
    params: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    let ids = strings(params, "noteIds")?;
    if ids.is_empty() {
        return Ok(Value::Array(Vec::new()));
    }
    let viewer = auth
        .user
        .as_ref()
        .map(|user| {
            user.get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or(Error::Internal)
        })
        .transpose()?;
    let meta = runtime.auth.meta().await;
    let visitor_policy = meta
        .get("ugcVisibilityForVisitor")
        .and_then(serde_json::Value::as_str)
        .ok_or(Error::Internal)?;
    if viewer.is_none() && visitor_policy == "none" {
        return Ok(Value::Array(Vec::new()));
    }
    let notes = load_notes(runtime, &ids).await?;
    let (following, viewer_remote) = following_context(runtime, &notes, viewer).await?;
    let visible_notes: Vec<_> = notes
        .iter()
        .filter(|note| visible(note, viewer, &following, viewer_remote))
        .collect();
    let buffered = meta
        .get("enableReactionsBuffering")
        .and_then(serde_json::Value::as_bool)
        .ok_or(Error::Internal)?;
    // TS getMany reads original IDs, including duplicates and hidden/missing
    // notes. The shared buffer helper preserves the same last-map-entry rule.
    let buffers = if buffered {
        runtime
            .redis
            .reaction_buffers(&ids)
            .await
            .map_err(|_| Error::Internal)?
    } else {
        Default::default()
    };
    let packed = futures_util::future::try_join_all(visible_notes.into_iter().map(|note| {
        let buffers = &buffers;
        async move {
            let reactions = reactions::merge(
                &note.reactions,
                buffers.get(&note.id).map(|buffer| &buffer.deltas),
            )?;
            let names = reactions::emoji_names(&reactions)?;
            let emojis =
                entities::populate_emojis(runtime, &names, note.user_host.as_deref()).await?;
            Ok::<_, Error>(Value::object([
                ("id", Value::string(&note.id)),
                ("reactions", reactions),
                ("reactionEmojis", emojis),
            ]))
        }
    }))
    .await?;
    Ok(Value::Array(packed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_payload_is_not_interpreted_before_visibility() {
        let row = Value::parse_json(
            r#"{"id":"note","userId":"author","userHost":null,"visibility":"specified","visibleUserIds":[null,"recipient"],"mentions":[null],"replyUserId":null,"reactions":{"👍":"1"}}"#,
        )
        .unwrap();
        let note = PartialNote::decode(&row).unwrap();
        assert_eq!(visibility_without_following(&note, None), Some(false));
        assert!(!visible(&note, Some("other"), &HashSet::new(), false));
        assert!(visible(&note, Some("recipient"), &HashSet::new(), false));
        assert!(visible(&note, Some("author"), &HashSet::new(), false));
    }

    fn note(visibility: Visibility) -> PartialNote {
        PartialNote {
            id: "note".into(),
            user_id: "author".into(),
            user_host: None,
            visibility,
            visible_user_ids: vec![Some("recipient".into())],
            mentions: vec![Some("mentioned".into())],
            reply_user_id: Some("reply-author".into()),
            reactions: Value::object(std::iter::empty::<(&str, Value)>()),
        }
    }

    #[test]
    fn public_and_home_polling_do_not_require_a_follow_lookup() {
        for visibility in [Visibility::Public, Visibility::Home] {
            for viewer in [None, Some("author"), Some("other")] {
                assert_eq!(
                    visibility_without_following(&note(visibility), viewer),
                    Some(true)
                );
            }
        }
    }

    #[test]
    fn specified_polling_accepts_only_owner_or_recipient() {
        let note = note(Visibility::Specified);
        for (viewer, expected) in [
            (None, false),
            (Some("other"), false),
            (Some("author"), true),
            (Some("recipient"), true),
        ] {
            assert_eq!(visible(&note, viewer, &HashSet::new(), true), expected);
        }
    }

    #[test]
    fn followers_polling_keeps_reply_and_mention_exemptions() {
        let note = note(Visibility::Followers);
        assert_eq!(visibility_without_following(&note, None), Some(false));
        for id in ["author", "mentioned", "reply-author"] {
            assert_eq!(visibility_without_following(&note, Some(id)), Some(true));
        }
        assert_eq!(visibility_without_following(&note, Some("other")), None);
        assert!(!visible(&note, Some("other"), &HashSet::new(), false));
        assert!(visible(
            &note,
            Some("other"),
            &HashSet::from(["author".into()]),
            false
        ));
    }

    #[test]
    fn remote_following_assumption_requires_both_remote_parties() {
        let mut note = note(Visibility::Followers);
        assert!(!visible(&note, Some("other"), &HashSet::new(), true));
        note.user_host = Some("remote.example".into());
        assert!(!visible(&note, Some("other"), &HashSet::new(), false));
        assert!(visible(&note, Some("other"), &HashSet::new(), true));
        assert!(!visible(&note, None, &HashSet::new(), true));
    }
}
