//! Complete note reads and their native public representations.
use super::{Error, iso_date, parse_id_date, rows, text};
use crate::Runtime;
use misskey_api_validation::Value;
use misskey_backend_api_auth::Context;
use std::collections::{HashMap, HashSet};

pub(crate) mod config;
mod entities;
mod files;
mod partial;
mod reactions;
mod row;
mod users;
pub(crate) use config::Config;
pub(super) use partial::show_partial_bulk;
use row::{Note, Visibility, boolean, field, null_string, optional_string, string, string_array};

struct Graph {
    notes: HashMap<String, Note>,
    users: HashMap<String, Value>,
    detailed: HashSet<String>,
}

async fn load_notes(runtime: &Runtime, ids: &[String]) -> Result<Vec<Value>, Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    rows(&runtime.database,true,
        "SELECT json_build_object('note',row_to_json(n),'user',CASE WHEN u.id IS NULL THEN NULL ELSE (to_jsonb(u)-'lastActiveDate') || jsonb_build_object('lastActiveDate',floor(extract(epoch FROM u.\"lastActiveDate\")*1000)::double precision) END)::text FROM note n LEFT JOIN \"user\" u ON u.id=n.\"userId\" WHERE n.id=ANY($1::varchar[])",
        &[&ids],
    ).await
}

fn insert_nodes(graph: &mut Graph, values: Vec<Value>) -> Result<(), Error> {
    for value in values {
        let note = Note::decode(field(&value, "note")?)?;
        let user = field(&value, "user")?;
        if !matches!(user, Value::Object(_)) || string(user, "id")? != note.user_id {
            return Err(Error::Internal);
        }
        graph.users.insert(note.user_id.clone(), user.clone());
        graph.notes.insert(note.id.clone(), note);
    }
    Ok(())
}

async fn load_graph(
    runtime: &Runtime,
    root: &str,
    auth: &Context,
    visitor_policy: &str,
) -> Result<Graph, Error> {
    let mut graph = Graph {
        notes: HashMap::new(),
        users: HashMap::new(),
        detailed: HashSet::from([root.to_owned()]),
    };
    let root_rows = load_notes(runtime, &[root.to_owned()]).await?;
    let Some(root_row) = root_rows.first() else {
        return Err(api_error(
            "No such note.",
            "NO_SUCH_NOTE",
            "24fcbfc6-2e37-42b6-8388-c29b3861a08d",
        ));
    };
    root_access(root_row, auth.user.is_none(), visitor_policy)?;
    insert_nodes(&mut graph, root_rows)?;
    let mut expanded = HashSet::new();
    let mut attempted = HashSet::from([root.to_owned()]);
    loop {
        let to_expand: Vec<_> = graph
            .detailed
            .iter()
            .filter(|id| !expanded.contains(*id))
            .cloned()
            .collect();
        if to_expand.is_empty() {
            break;
        }
        let mut fetch = Vec::new();
        for id in to_expand {
            expanded.insert(id.clone());
            let Some(note) = graph.notes.get(&id) else {
                continue;
            };
            for child in [&note.reply_id, &note.renote_id].into_iter().flatten() {
                if attempted.insert(child.clone()) {
                    fetch.push(child.clone());
                }
            }
            if let Some(id) = &note.renote_id {
                graph.detailed.insert(id.clone());
            }
        }
        insert_nodes(&mut graph, load_notes(runtime, &fetch).await?)?;
    }
    Ok(graph)
}

// Read only the fields used by the endpoint's ordered root access checks.
// Payload decoding must not replace a denied request with an unrelated error.
fn root_access(root: &Value, anonymous: bool, visitor_policy: &str) -> Result<(), Error> {
    if !anonymous {
        return Ok(());
    }
    if boolean(field(root, "user")?, "requireSigninToViewContents")? {
        return Err(api_error(
            "Content restricted by user. Please sign in to view.",
            "CONTENT_RESTRICTED_BY_USER",
            "fbcc002d-37d9-4944-a6b0-d9e29f2d33ab",
        ));
    }
    if visitor_policy == "none"
        || (visitor_policy == "local"
            && row::nullable_string(field(root, "note")?, "userHost")?.is_some())
    {
        return Err(api_error(
            "Content restricted by server settings. Please sign in to view.",
            "CONTENT_RESTRICTED_BY_SERVER",
            "145f88d2-b03d-4087-8143-a78928883c4b",
        ));
    }
    Ok(())
}

fn api_error(message: &'static str, code: &'static str, id: &'static str) -> Error {
    Error::Api {
        message,
        code,
        id,
        kind: "client",
        status: None,
        info: None,
    }
}

struct ReactionState {
    reactions: Value,
    mine: Option<String>,
}

async fn load_reactions(
    runtime: &Runtime,
    graph: &Graph,
    viewer: Option<&str>,
    buffered: bool,
) -> Result<HashMap<String, ReactionState>, Error> {
    let ids: Vec<_> = graph.notes.keys().cloned().collect();
    let buffers = if buffered {
        runtime
            .redis
            .reaction_buffers(&ids)
            .await
            .map_err(|_| Error::Internal)?
    } else {
        HashMap::new()
    };
    let mut states = HashMap::new();
    let mut mine_ids = Vec::new();
    for (id, note) in &graph.notes {
        let buffer = buffers.get(id);
        let reactions = reactions::merge(&note.reactions, buffer.map(|b| &b.deltas))?;
        let mut pairs = note.reaction_pairs.clone();
        if let Some(buffer) = buffer {
            pairs.extend(
                buffer
                    .pairs
                    .iter()
                    .map(|(user, reaction)| format!("{user}/{reaction}")),
            );
        }
        let mut mine = None;
        if let Some(viewer) = viewer.filter(|_| graph.detailed.contains(id)) {
            match reactions::cached_mine(&reactions, &pairs, viewer)? {
                Some(found) => mine = found,
                None => {
                    let created = parse_id_date(&runtime.business_config.id_method, id)
                        .ok_or(Error::Internal)?;
                    if created + 2000. <= crate::now_ms() as f64 {
                        mine_ids.push(id.clone());
                    }
                }
            }
        }
        states.insert(id.clone(), ReactionState { reactions, mine });
    }
    if let Some(viewer) = viewer.filter(|_| !mine_ids.is_empty()) {
        for result in rows(&runtime.database,true,"SELECT row_to_json(r)::text FROM note_reaction r WHERE r.\"userId\"=$1 AND r.\"noteId\"=ANY($2::varchar[])",&[&viewer,&mine_ids]).await? {
            let id=string(&result,"noteId")?;
            let Some(state)=states.get_mut(&id) else { return Err(Error::Internal); };
            state.mine=Some(reactions::convert(&string(&result,"reaction")?));
        }
    }
    Ok(states)
}

async fn load_polls(
    runtime: &Runtime,
    graph: &Graph,
    viewer: Option<&str>,
) -> Result<HashMap<String, Value>, Error> {
    let ids: Vec<_> = graph
        .notes
        .values()
        .filter(|n| n.has_poll && graph.detailed.contains(&n.id))
        .map(|n| n.id.clone())
        .collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let polls=rows(&runtime.database,true,"SELECT (to_jsonb(p)||jsonb_build_object('expiresAt',floor(extract(epoch FROM p.\"expiresAt\")*1000)::double precision))::text FROM poll p WHERE p.\"noteId\"=ANY($1::varchar[])",&[&ids]).await?;
    let mut votes: HashMap<String, Vec<usize>> = HashMap::new();
    if let Some(viewer) = viewer {
        for vote in rows(&runtime.database,true,"SELECT row_to_json(v)::text FROM poll_vote v WHERE v.\"userId\"=$1 AND v.\"noteId\"=ANY($2::varchar[])",&[&viewer,&ids]).await? {
            let choice=field(&vote,"choice")?.as_number().ok_or(Error::Internal)?;
            if choice.fract()!=0. || choice<0. || choice>i32::MAX as f64 { return Err(Error::Internal); }
            votes.entry(string(&vote,"noteId")?).or_default().push(choice as usize);
        }
    }
    let mut output = HashMap::new();
    for poll in polls {
        let id = string(&poll, "noteId")?;
        let empty = Vec::new();
        output.insert(
            id.clone(),
            pack_poll(&poll, votes.get(&id).unwrap_or(&empty))?,
        );
    }
    if ids.iter().any(|id| !output.contains_key(id)) {
        return Err(Error::Internal);
    }
    Ok(output)
}

fn pack_poll(poll: &Value, my_choices: &[usize]) -> Result<Value, Error> {
    let texts = row::strings(poll, "choices")?;
    let Value::Array(votes) = field(poll, "votes")? else {
        return Err(Error::Internal);
    };
    let multiple = boolean(poll, "multiple")?;
    let mut choices = Vec::new();
    for text in &texts {
        let index = texts
            .iter()
            .position(|candidate| candidate == text)
            .ok_or(Error::Internal)?;
        let vote = votes.get(index).cloned().unwrap_or(Value::Undefined);
        if !matches!(vote, Value::Undefined | Value::Number(_)) {
            return Err(Error::Internal);
        }
        choices.push(Value::object([
            ("text", Value::string(text)),
            ("votes", vote),
            ("isVoted", Value::Boolean(false)),
        ]));
    }
    for &index in my_choices
        .iter()
        .take(if multiple { usize::MAX } else { 1 })
    {
        choices
            .get_mut(index)
            .ok_or(Error::Internal)?
            .set("isVoted", Value::Boolean(true));
    }
    let expires = match field(poll, "expiresAt")? {
        Value::Null => Value::Null,
        Value::Number(ms) => Value::string(&iso_date(*ms)?),
        _ => return Err(Error::Internal),
    };
    Ok(Value::object([
        ("multiple", Value::Boolean(multiple)),
        ("expiresAt", expires),
        ("choices", Value::Array(choices)),
    ]))
}

async fn load_channels(runtime: &Runtime, graph: &Graph) -> Result<HashMap<String, Value>, Error> {
    let ids: Vec<_> = graph
        .notes
        .values()
        .filter_map(|n| n.channel_id.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut result = HashMap::new();
    for channel in rows(
        &runtime.database,
        true,
        "SELECT row_to_json(c)::text FROM channel c WHERE c.id=ANY($1::varchar[])",
        &[&ids],
    )
    .await?
    {
        let id = string(&channel, "id")?;
        let mut packed = Value::object(std::iter::empty::<(&str, Value)>());
        for key in [
            "id",
            "name",
            "color",
            "isSensitive",
            "allowRenoteToExternal",
            "userId",
        ] {
            packed.set(key, field(&channel, key)?.clone());
        }
        result.insert(id, packed);
    }
    Ok(result)
}

struct PackedDependencies {
    users: HashMap<String, Value>,
    files: HashMap<String, Value>,
    channels: HashMap<String, Value>,
    polls: HashMap<String, Value>,
    reactions: HashMap<String, ReactionState>,
    emojis: HashMap<String, (Value, Value)>,
    following: HashSet<String>,
}

async fn dependencies(
    runtime: &Runtime,
    graph: &Graph,
    auth: &Context,
    viewer: Option<&str>,
    buffered: bool,
) -> Result<PackedDependencies, Error> {
    let file_ids: Vec<_> = graph
        .notes
        .values()
        .flat_map(|n| n.file_ids.iter().cloned())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let (reactions, polls, channels, packed_files) = tokio::try_join!(
        load_reactions(runtime, graph, viewer, buffered),
        load_polls(runtime, graph, viewer),
        load_channels(runtime, graph),
        files::pack_files(runtime, &file_ids),
    )?;
    let Value::Array(file_values) = &packed_files else {
        return Err(Error::Internal);
    };
    let mut files = HashMap::new();
    for value in file_values {
        files.insert(string(value, "id")?, value.clone());
    }
    let packed_users =
        futures_util::future::try_join_all(graph.users.iter().map(|(id, user)| async move {
            Ok::<_, Error>((id.clone(), users::pack_user(runtime, user, auth).await?))
        }))
        .await?;
    let users: HashMap<_, _> = packed_users.into_iter().collect();
    let mut emojis = HashMap::new();
    for (id, note) in &graph.notes {
        let state = reactions.get(id).ok_or(Error::Internal)?;
        let names = reactions::emoji_names(&state.reactions)?;
        let (reaction_emojis, note_emojis) = tokio::try_join!(
            entities::populate_emojis(runtime, &names, note.user_host.as_deref()),
            async {
                if note.user_host.is_some() {
                    entities::populate_emojis(runtime, &note.emojis, note.user_host.as_deref())
                        .await
                } else {
                    Ok(Value::Undefined)
                }
            }
        )?;
        emojis.insert(id.clone(), (reaction_emojis, note_emojis));
    }
    // Match the existing cache path only when a follower check can be required.
    let mut following = HashSet::new();
    if let Some(viewer) = viewer {
        let needs = graph.notes.values().any(|note| {
            if note.user_id == viewer {
                return false;
            }
            let Some(user) = users.get(&note.user_id) else {
                return true;
            };
            let created =
                parse_id_date(&runtime.business_config.id_method, &note.id).unwrap_or(f64::NAN);
            note.visibility == Visibility::Followers
                || (matches!(note.visibility, Visibility::Public | Visibility::Home)
                    && hide_by_time(
                        user.get("makeNotesFollowersOnlyBefore"),
                        created,
                        crate::now_ms() as f64,
                    ))
        });
        if needs {
            following = runtime
                .redis
                .following_ids(viewer, &runtime.database)
                .await
                .map_err(|_| Error::Internal)?;
        }
    }
    Ok(PackedDependencies {
        users,
        files,
        channels,
        polls,
        reactions,
        emojis,
        following,
    })
}

/// Preserve the endpoint's eager root checks, then pack each nested state once.
pub(super) async fn show(
    runtime: &Runtime,
    params: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    let id = text(params, "noteId")?;
    let meta = runtime.auth.meta().await;
    let visitor = meta
        .as_value()
        .get("ugcVisibilityForVisitor")
        .and_then(serde_json::Value::as_str)
        .ok_or(Error::Internal)?;
    let buffered = meta
        .as_value()
        .get("enableReactionsBuffering")
        .and_then(serde_json::Value::as_bool)
        .ok_or(Error::Internal)?;
    let viewer = auth
        .user
        .as_ref()
        .map(|u| {
            u.get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or(Error::Internal)
        })
        .transpose()?;
    let graph = load_graph(runtime, &id, auth, visitor).await?;
    let deps = dependencies(runtime, &graph, auth, viewer, buffered).await?;
    pack_graph(runtime, &id, &graph, &deps, viewer, visitor)
}

fn pack_graph(
    runtime: &Runtime,
    root: &str,
    graph: &Graph,
    deps: &PackedDependencies,
    viewer: Option<&str>,
    visitor: &str,
) -> Result<Value, Error> {
    enum Task {
        Visit(String, bool),
        Finish(String, bool),
    }
    let mut tasks = vec![Task::Visit(root.to_owned(), true)];
    let mut active = HashSet::new();
    let mut packed = HashMap::new();
    while let Some(task) = tasks.pop() {
        let (id, detail, finish) = match task {
            Task::Visit(id, d) => (id, d, false),
            Task::Finish(id, d) => (id, d, true),
        };
        let key = (id.clone(), detail);
        if packed.contains_key(&key) {
            continue;
        }
        let Some(note) = graph.notes.get(&id) else {
            packed.insert(key, Value::Null);
            continue;
        };
        if !finish {
            if !active.insert(key.clone()) {
                return Err(Error::Internal);
            }
            tasks.push(Task::Finish(id.clone(), detail));
            if detail {
                if let Some(renote) = &note.renote_id {
                    tasks.push(Task::Visit(renote.clone(), true));
                }
                if let Some(reply) = &note.reply_id {
                    tasks.push(Task::Visit(reply.clone(), false));
                }
            }
            continue;
        }
        let child = |id: &Option<String>, detail| {
            id.as_ref()
                .map(|id| {
                    packed
                        .get(&(id.clone(), detail))
                        .cloned()
                        .ok_or(Error::Internal)
                })
                .transpose()
                .map(|v| v.unwrap_or(Value::Undefined))
        };
        let reply = if detail {
            child(&note.reply_id, false)?
        } else {
            Value::Undefined
        };
        let renote = if detail {
            child(&note.renote_id, true)?
        } else {
            Value::Undefined
        };
        let value = pack_one(runtime, note, deps, detail, reply, renote, viewer, visitor)?;
        active.remove(&key);
        packed.insert(key, value);
    }
    packed
        .remove(&(root.to_owned(), true))
        .ok_or(Error::Internal)
}

#[allow(clippy::too_many_arguments)]
fn pack_one(
    runtime: &Runtime,
    note: &Note,
    deps: &PackedDependencies,
    detail: bool,
    reply: Value,
    renote: Value,
    viewer: Option<&str>,
    visitor: &str,
) -> Result<Value, Error> {
    let user = deps.users.get(&note.user_id).ok_or(Error::Internal)?;
    let reactions = deps.reactions.get(&note.id).ok_or(Error::Internal)?;
    let (reaction_emojis, emojis) = deps.emojis.get(&note.id).ok_or(Error::Internal)?;
    let created =
        parse_id_date(&runtime.business_config.id_method, &note.id).ok_or(Error::Internal)?;
    let text = if note.name.as_ref().is_some_and(|s| !s.is_empty())
        && note
            .url
            .as_ref()
            .or(note.uri.as_ref())
            .is_some_and(|s| !s.is_empty())
    {
        let trim = note
            .text
            .as_deref()
            .unwrap_or("")
            .trim_matches(js_whitespace);
        Value::string(&format!(
            "【{}】\n{trim}\n\n{}",
            note.name.as_deref().unwrap_or(""),
            note.url
                .as_ref()
                .or(note.uri.as_ref())
                .ok_or(Error::Internal)?
        ))
    } else {
        null_string(note.text.as_deref())
    };
    let channel = note
        .channel_id
        .as_ref()
        .and_then(|id| deps.channels.get(id))
        .cloned()
        .unwrap_or(Value::Undefined);
    let files = Value::Array(
        note.file_ids
            .iter()
            .filter_map(|id| deps.files.get(id).cloned())
            .collect(),
    );
    let mut visibility = note.visibility;
    if matches!(visibility, Visibility::Public | Visibility::Home)
        && hide_by_time(
            user.get("makeNotesFollowersOnlyBefore"),
            created,
            crate::now_ms() as f64,
        )
    {
        visibility = Visibility::Followers;
    }
    let mut packed = Value::object([
        ("id", Value::string(&note.id)),
        ("createdAt", Value::string(&iso_date(created)?)),
        ("userId", Value::string(&note.user_id)),
        ("user", user.clone()),
        ("text", text),
        ("cw", null_string(note.cw.as_deref())),
        ("visibility", Value::string(visibility.name())),
        ("localOnly", Value::Boolean(note.local_only)),
        (
            "reactionAcceptance",
            null_string(note.reaction_acceptance.as_deref()),
        ),
        (
            "visibleUserIds",
            if note.visibility == Visibility::Specified {
                string_array(&note.visible_user_ids)
            } else {
                Value::Undefined
            },
        ),
        ("renoteCount", Value::Number(f64::from(note.renote_count))),
        ("repliesCount", Value::Number(f64::from(note.replies_count))),
        ("reactionCount", reactions::count(&reactions.reactions)?),
        ("reactions", reactions.reactions.clone()),
        ("reactionEmojis", reaction_emojis.clone()),
        ("emojis", emojis.clone()),
        (
            "tags",
            if note.tags.is_empty() {
                Value::Undefined
            } else {
                string_array(&note.tags)
            },
        ),
        ("fileIds", string_array(&note.file_ids)),
        ("files", files),
        ("replyId", null_string(note.reply_id.as_deref())),
        ("renoteId", null_string(note.renote_id.as_deref())),
        ("channelId", optional_string(note.channel_id.as_deref())),
        ("channel", channel),
        (
            "mentions",
            if note.mentions.is_empty() {
                Value::Undefined
            } else {
                string_array(&note.mentions)
            },
        ),
        (
            "hasPoll",
            if note.has_poll {
                Value::Boolean(true)
            } else {
                Value::Undefined
            },
        ),
        ("uri", optional_string(note.uri.as_deref())),
        ("url", optional_string(note.url.as_deref())),
    ]);
    if detail {
        packed.set("clippedCount", Value::Number(f64::from(note.clipped_count)));
        packed.set("reply", reply);
        packed.set("renote", renote);
        if note.has_poll {
            packed.set(
                "poll",
                deps.polls.get(&note.id).cloned().ok_or(Error::Internal)?,
            );
        }
        if viewer.is_some()
            && let Some(mine) = &reactions.mine
        {
            packed.set("myReaction", Value::string(mine));
        }
    }
    if should_hide(
        note,
        user,
        visibility,
        packed.get("reply"),
        viewer,
        visitor,
        &deps.following,
        created,
        crate::now_ms() as f64,
    )? {
        hide(&mut packed);
    }
    Ok(packed)
}

fn js_whitespace(c: char) -> bool {
    matches!(c,'\u{9}'..='\u{d}'|'\u{20}'|'\u{a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}'|'\u{feff}')
}

fn hide_by_time(before: Option<&Value>, created: f64, now: f64) -> bool {
    let Some(before) = before.and_then(Value::as_number) else {
        return false;
    };
    if before <= 0. {
        (now - created) / 1000. >= before.abs()
    } else {
        created / 1000. <= before
    }
}

#[allow(clippy::too_many_arguments)]
fn should_hide(
    note: &Note,
    user: &Value,
    visibility: Visibility,
    reply: Option<&Value>,
    viewer: Option<&str>,
    visitor: &str,
    following: &HashSet<String>,
    created: f64,
    now: f64,
) -> Result<bool, Error> {
    if viewer == Some(note.user_id.as_str()) {
        return Ok(false);
    }
    // UserLite deliberately omits this property when the stored flag is false.
    // The raw root row above still requires an actual database boolean.
    let require_signin = match user.get("requireSigninToViewContents") {
        None | Some(Value::Undefined) => false,
        Some(Value::Boolean(value)) => *value,
        _ => return Err(Error::Internal),
    };
    if viewer.is_none() && (visitor == "none" || require_signin) {
        return Ok(true);
    }
    if hide_by_time(user.get("makeNotesHiddenBefore"), created, now) {
        return Ok(true);
    }
    match visibility {
        Visibility::Specified => {
            Ok(viewer.is_none_or(|id| !note.visible_user_ids.iter().any(|u| u == id)))
        }
        Visibility::Followers => {
            let Some(viewer) = viewer else {
                return Ok(true);
            };
            if reply
                .and_then(|r| r.get("userId"))
                .and_then(Value::to_utf8)
                .as_deref()
                == Some(viewer)
                || note.mentions.iter().any(|id| id == viewer)
            {
                return Ok(false);
            }
            Ok(!following.contains(&note.user_id))
        }
        _ => Ok(false),
    }
}

fn hide(note: &mut Value) {
    note.set("visibleUserIds", Value::Undefined);
    note.set("fileIds", Value::Array(Vec::new()));
    note.set("files", Value::Array(Vec::new()));
    note.set("text", Value::Null);
    note.set("poll", Value::Undefined);
    note.set("cw", Value::Null);
    note.set("isHidden", Value::Boolean(true));
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(visibility: &str) -> Note {
        Note::decode(&Value::from_json(&serde_json::json!({
            "id":"note", "userId":"author", "userHost":null,
            "replyId":null,"renoteId":null,"text":"secret","name":null,
            "cw":"cw","uri":null,"url":null,"visibility":visibility,
            "localOnly":false,"reactionAcceptance":null,"renoteCount":0,
            "repliesCount":0,"clippedCount":0,"reactions":{},
            "reactionAndUserPairCache":[],"fileIds":[],"visibleUserIds":["recipient"],
            "mentions":[],"emojis":[],"tags":[],"hasPoll":false,"channelId":null
        })))
        .unwrap()
    }
    #[test]
    fn root_restrictions_precede_unrelated_payload_decoding() {
        let mut root = Value::parse_json(
            r#"{"user":{"requireSigninToViewContents":true},"note":{"userHost":null,"reactions":{"👍":"1"}}}"#,
        ).unwrap();
        assert!(matches!(
            root_access(&root, true, "none"),
            Err(Error::Api {
                code: "CONTENT_RESTRICTED_BY_USER",
                ..
            })
        ));
        root.get_mut("user")
            .unwrap()
            .set("requireSigninToViewContents", Value::Boolean(false));
        assert!(matches!(
            root_access(&root, true, "none"),
            Err(Error::Api {
                code: "CONTENT_RESTRICTED_BY_SERVER",
                ..
            })
        ));
        root.get_mut("note")
            .unwrap()
            .set("userHost", Value::string("remote.test"));
        assert!(matches!(
            root_access(&root, true, "local"),
            Err(Error::Api {
                code: "CONTENT_RESTRICTED_BY_SERVER",
                ..
            })
        ));
        assert!(root_access(&root, true, "all").is_ok());
        assert!(root_access(&root, false, "none").is_ok());
    }

    fn visible_user() -> Value {
        Value::from_json(
            &serde_json::json!({"requireSigninToViewContents":false,"makeNotesHiddenBefore":null,"makeNotesFollowersOnlyBefore":null}),
        )
    }
    #[test]
    fn ordinary_anonymous_public_note_accepts_omitted_false_user_signin_flag() {
        let note = fixture("public");
        let mut user = visible_user();
        user.set("requireSigninToViewContents", Value::Undefined);
        assert!(
            !should_hide(
                &note,
                &user,
                note.visibility,
                None,
                None,
                "all",
                &HashSet::new(),
                0.,
                1000.
            )
            .unwrap()
        );
        assert!(
            should_hide(
                &note,
                &user,
                note.visibility,
                None,
                None,
                "none",
                &HashSet::new(),
                0.,
                1000.
            )
            .unwrap()
        );
    }
    #[test]
    fn specified_visibility_allows_only_author_or_explicit_recipient() {
        let note = fixture("specified");
        let user = visible_user();
        for (viewer, hidden) in [
            (None, true),
            (Some("stranger"), true),
            (Some("recipient"), false),
            (Some("author"), false),
        ] {
            assert_eq!(
                should_hide(
                    &note,
                    &user,
                    note.visibility,
                    None,
                    viewer,
                    "all",
                    &HashSet::new(),
                    0.,
                    1000.
                )
                .unwrap(),
                hidden
            );
        }
    }
    #[test]
    fn followers_visibility_keeps_reply_mentions_and_following_exceptions() {
        let mut note = fixture("followers");
        let user = visible_user();
        let reply = Value::object([("userId", Value::string("viewer"))]);
        assert!(
            !should_hide(
                &note,
                &user,
                note.visibility,
                Some(&reply),
                Some("viewer"),
                "all",
                &HashSet::new(),
                0.,
                1000.
            )
            .unwrap()
        );
        // A detail=false packed note has no reply object, even if replyId exists.
        assert!(
            should_hide(
                &note,
                &user,
                note.visibility,
                None,
                Some("viewer"),
                "all",
                &HashSet::new(),
                0.,
                1000.
            )
            .unwrap()
        );
        note.mentions.push("viewer".into());
        assert!(
            !should_hide(
                &note,
                &user,
                note.visibility,
                None,
                Some("viewer"),
                "all",
                &HashSet::new(),
                0.,
                1000.
            )
            .unwrap()
        );
        note.mentions.clear();
        assert!(
            !should_hide(
                &note,
                &user,
                note.visibility,
                None,
                Some("viewer"),
                "all",
                &HashSet::from(["author".into()]),
                0.,
                1000.
            )
            .unwrap()
        );
        assert!(
            should_hide(
                &note,
                &user,
                note.visibility,
                None,
                None,
                "all",
                &HashSet::from(["author".into()]),
                0.,
                1000.
            )
            .unwrap()
        );
    }
    #[test]
    fn hidden_before_precedes_follower_exemptions_but_owner_always_sees_content() {
        let mut note = fixture("followers");
        note.mentions.push("viewer".into());
        let mut user = visible_user();
        user.set("makeNotesHiddenBefore", Value::Number(-1.));
        assert!(
            should_hide(
                &note,
                &user,
                note.visibility,
                None,
                Some("viewer"),
                "all",
                &HashSet::new(),
                0.,
                1000.
            )
            .unwrap()
        );
        assert!(
            !should_hide(
                &note,
                &user,
                note.visibility,
                None,
                Some("author"),
                "none",
                &HashSet::new(),
                0.,
                1000.
            )
            .unwrap()
        );
        user.set("makeNotesHiddenBefore", Value::Null);
        user.set("requireSigninToViewContents", Value::Boolean(true));
        assert!(
            should_hide(
                &note,
                &user,
                Visibility::Public,
                None,
                None,
                "all",
                &HashSet::new(),
                0.,
                0.
            )
            .unwrap()
        );
        assert!(
            !should_hide(
                &note,
                &user,
                Visibility::Public,
                None,
                Some("viewer"),
                "all",
                &HashSet::new(),
                0.,
                0.
            )
            .unwrap()
        );
    }
    #[test]
    fn time_visibility_keeps_both_inclusive_boundaries_and_zero() {
        assert!(!hide_by_time(Some(&Value::Number(-10.)), 1000., 10_999.));
        assert!(hide_by_time(Some(&Value::Number(-10.)), 1000., 11_000.));
        assert!(hide_by_time(Some(&Value::Number(10.)), 10_000., 0.));
        assert!(!hide_by_time(Some(&Value::Number(10.)), 10_001., 0.));
        assert!(hide_by_time(Some(&Value::Number(0.)), 1000., 1000.));
        assert!(!hide_by_time(Some(&Value::Null), 0., 1000.));
    }
    #[test]
    fn hidden_note_remains_an_object_with_null_text_and_omitted_poll() {
        let mut note=Value::parse_json(r#"{"text":"secret","cw":"secret","poll":{},"visibleUserIds":["a"],"fileIds":["f"],"files":[{}],"hasPoll":true,"reactions":{"like":1}}"#).unwrap();
        hide(&mut note);
        assert_eq!(note.get("text"), Some(&Value::Null));
        assert_eq!(note.get("poll"), Some(&Value::Undefined));
        assert_eq!(note.get("hasPoll"), Some(&Value::Boolean(true)));
        assert!(note.get("reactions").is_some());
    }
    #[test]
    fn duplicate_poll_choices_use_first_text_count_but_distinct_vote_positions() {
        let poll = Value::parse_json(
            r#"{"multiple":true,"expiresAt":null,"choices":["a","a"],"votes":[2,8]}"#,
        )
        .unwrap();
        let packed = pack_poll(&poll, &[1]).unwrap();
        let Some(Value::Array(choices)) = packed.get("choices") else {
            panic!()
        };
        assert_eq!(choices[0].get("votes"), Some(&Value::Number(2.)));
        assert_eq!(choices[1].get("votes"), Some(&Value::Number(2.)));
        assert_eq!(choices[0].get("isVoted"), Some(&Value::Boolean(false)));
        assert_eq!(choices[1].get("isVoted"), Some(&Value::Boolean(true)));
    }
    #[test]
    fn js_trim_excludes_next_line_and_includes_bom() {
        assert_eq!("\u{85}x\u{85}".trim_matches(js_whitespace), "\u{85}x\u{85}");
        assert_eq!("\u{feff} x\u{a0}".trim_matches(js_whitespace), "x");
    }
}
