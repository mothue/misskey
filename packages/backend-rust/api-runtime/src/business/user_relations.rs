//! Whole native relationship reads; no profile lookup or persistent cache.
use super::{Error, rows};
use crate::Runtime;
use misskey_api_validation::Value;
use misskey_backend_api_auth::Context;
use std::collections::{HashMap, HashSet};

/// Exact JS UTF-16 strings, used both for ID keys and nullable payload fields.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct JsString(Vec<u16>);

impl JsString {
    fn decode(value: &Value) -> Result<Self, Error> {
        match value {
            Value::String(value) => Ok(Self(value.clone())),
            _ => Err(Error::Internal),
        }
    }

    fn value(&self) -> Value {
        Value::String(self.0.clone())
    }
}

fn string_field(row: &Value, key: &str) -> Result<JsString, Error> {
    JsString::decode(row.get(key).ok_or(Error::Internal)?)
}

fn nullable_string(row: &Value, key: &str) -> Result<Option<JsString>, Error> {
    match row.get(key) {
        Some(Value::Null) => Ok(None),
        Some(value @ Value::String(_)) => Ok(Some(JsString::decode(value)?)),
        _ => Err(Error::Internal),
    }
}

fn boolean_field(row: &Value, key: &str) -> Result<bool, Error> {
    match row.get(key) {
        Some(Value::Boolean(value)) => Ok(*value),
        _ => Err(Error::Internal),
    }
}

fn optional_value(value: &Option<JsString>) -> Value {
    value.as_ref().map_or(Value::Null, JsString::value)
}

/// The twelve persisted Following fields, without joined entity properties.
struct Following {
    id: JsString,
    followee_id: JsString,
    follower_id: JsString,
    is_follower_hibernated: bool,
    with_replies: bool,
    notify: Option<JsString>,
    follower_host: Option<JsString>,
    follower_inbox: Option<JsString>,
    follower_shared_inbox: Option<JsString>,
    followee_host: Option<JsString>,
    followee_inbox: Option<JsString>,
    followee_shared_inbox: Option<JsString>,
}

impl Following {
    fn decode(row: &Value) -> Result<Self, Error> {
        Ok(Self {
            id: string_field(row, "id")?,
            followee_id: string_field(row, "followeeId")?,
            follower_id: string_field(row, "followerId")?,
            is_follower_hibernated: boolean_field(row, "isFollowerHibernated")?,
            with_replies: boolean_field(row, "withReplies")?,
            // SQL permits any varchar; do not narrow this to the TS annotation.
            notify: nullable_string(row, "notify")?,
            follower_host: nullable_string(row, "followerHost")?,
            follower_inbox: nullable_string(row, "followerInbox")?,
            follower_shared_inbox: nullable_string(row, "followerSharedInbox")?,
            followee_host: nullable_string(row, "followeeHost")?,
            followee_inbox: nullable_string(row, "followeeInbox")?,
            followee_shared_inbox: nullable_string(row, "followeeSharedInbox")?,
        })
    }

    fn value(&self) -> Value {
        Value::object([
            ("id", self.id.value()),
            ("followeeId", self.followee_id.value()),
            ("followerId", self.follower_id.value()),
            (
                "isFollowerHibernated",
                Value::Boolean(self.is_follower_hibernated),
            ),
            ("withReplies", Value::Boolean(self.with_replies)),
            ("notify", optional_value(&self.notify)),
            ("followerHost", optional_value(&self.follower_host)),
            ("followerInbox", optional_value(&self.follower_inbox)),
            (
                "followerSharedInbox",
                optional_value(&self.follower_shared_inbox),
            ),
            ("followeeHost", optional_value(&self.followee_host)),
            ("followeeInbox", optional_value(&self.followee_inbox)),
            (
                "followeeSharedInbox",
                optional_value(&self.followee_shared_inbox),
            ),
        ])
    }
}

#[derive(Default)]
struct RelationFlags {
    is_followed: bool,
    pending_from_you: bool,
    pending_to_you: bool,
    is_blocking: bool,
    is_blocked: bool,
    is_muted: bool,
    is_renote_muted: bool,
}

fn relation_value(id: &JsString, following: Option<&Following>, flags: RelationFlags) -> Value {
    Value::object([
        ("id", id.value()),
        ("following", following.map_or(Value::Null, Following::value)),
        ("isFollowing", Value::Boolean(following.is_some())),
        ("isFollowed", Value::Boolean(flags.is_followed)),
        (
            "hasPendingFollowRequestFromYou",
            Value::Boolean(flags.pending_from_you),
        ),
        (
            "hasPendingFollowRequestToYou",
            Value::Boolean(flags.pending_to_you),
        ),
        ("isBlocking", Value::Boolean(flags.is_blocking)),
        ("isBlocked", Value::Boolean(flags.is_blocked)),
        ("isMuted", Value::Boolean(flags.is_muted)),
        ("isRenoteMuted", Value::Boolean(flags.is_renote_muted)),
    ])
}

/// Request-local reusable relationship data, never a cross-request cache.
struct RelationshipSet {
    following: HashMap<JsString, Following>,
    followed: HashSet<JsString>,
    pending_from_you: HashSet<JsString>,
    pending_to_you: HashSet<JsString>,
    blocking: HashSet<JsString>,
    blocked: HashSet<JsString>,
    muted: HashSet<JsString>,
    renote_muted: HashSet<JsString>,
}

fn id_set(values: &[Value]) -> Result<HashSet<JsString>, Error> {
    values.iter().map(JsString::decode).collect()
}

impl RelationshipSet {
    fn decode(values: &[Vec<Value>; 8]) -> Result<Self, Error> {
        let mut following = HashMap::new();
        for row in &values[0] {
            let row = Following::decode(row)?;
            // JS Map keeps the last matching value at its first insertion slot.
            following.insert(row.followee_id.clone(), row);
        }
        Ok(Self {
            following,
            followed: id_set(&values[1])?,
            pending_from_you: id_set(&values[2])?,
            pending_to_you: id_set(&values[3])?,
            blocking: id_set(&values[4])?,
            blocked: id_set(&values[5])?,
            muted: id_set(&values[6])?,
            renote_muted: id_set(&values[7])?,
        })
    }

    fn relation(&self, id: &JsString) -> Value {
        relation_value(
            id,
            self.following.get(id),
            RelationFlags {
                is_followed: self.followed.contains(id),
                pending_from_you: self.pending_from_you.contains(id),
                pending_to_you: self.pending_to_you.contains(id),
                is_blocking: self.blocking.contains(id),
                is_blocked: self.blocked.contains(id),
                is_muted: self.muted.contains(id),
                is_renote_muted: self.renote_muted.contains(id),
            },
        )
    }

    fn values(&self, targets: &[JsString]) -> Value {
        let mut seen = HashSet::new();
        Value::Array(
            targets
                .iter()
                .filter(|id| seen.insert((*id).clone()))
                .map(|id| self.relation(id))
                .collect(),
        )
    }
}

// Full entity projection, without relying on row_to_json's future DB columns.
const FOLLOWING_FIELDS: &str = "json_build_object('id',f.id,'followeeId',f.\"followeeId\",'followerId',f.\"followerId\",'isFollowerHibernated',f.\"isFollowerHibernated\",'withReplies',f.\"withReplies\",'notify',f.notify,'followerHost',f.\"followerHost\",'followerInbox',f.\"followerInbox\",'followerSharedInbox',f.\"followerSharedInbox\",'followeeHost',f.\"followeeHost\",'followeeInbox',f.\"followeeInbox\",'followeeSharedInbox',f.\"followeeSharedInbox\")";
const SCALAR_FLAGS: [&str; 7] = [
    "SELECT to_json(EXISTS(SELECT 1 FROM following WHERE \"followerId\"=$2 AND \"followeeId\"=$1 LIMIT 1))::text",
    "SELECT to_json(EXISTS(SELECT 1 FROM follow_request WHERE \"followerId\"=$1 AND \"followeeId\"=$2 LIMIT 1))::text",
    "SELECT to_json(EXISTS(SELECT 1 FROM follow_request WHERE \"followerId\"=$2 AND \"followeeId\"=$1 LIMIT 1))::text",
    "SELECT to_json(EXISTS(SELECT 1 FROM blocking WHERE \"blockerId\"=$1 AND \"blockeeId\"=$2 LIMIT 1))::text",
    "SELECT to_json(EXISTS(SELECT 1 FROM blocking WHERE \"blockerId\"=$2 AND \"blockeeId\"=$1 LIMIT 1))::text",
    "SELECT to_json(EXISTS(SELECT 1 FROM muting WHERE \"muterId\"=$1 AND \"muteeId\"=$2 LIMIT 1))::text",
    "SELECT to_json(EXISTS(SELECT 1 FROM renote_muting WHERE \"muterId\"=$1 AND \"muteeId\"=$2 LIMIT 1))::text",
];
const ARRAY_IDS: [&str; 7] = [
    "SELECT to_json(\"followerId\")::text FROM following WHERE \"followeeId\"=$1",
    "SELECT to_json(\"followeeId\")::text FROM follow_request WHERE \"followerId\"=$1",
    "SELECT to_json(\"followerId\")::text FROM follow_request WHERE \"followeeId\"=$1",
    "SELECT to_json(\"blockeeId\")::text FROM blocking WHERE \"blockerId\"=$1",
    "SELECT to_json(\"blockerId\")::text FROM blocking WHERE \"blockeeId\"=$1",
    "SELECT to_json(\"muteeId\")::text FROM muting WHERE \"muterId\"=$1",
    "SELECT to_json(\"muteeId\")::text FROM renote_muting WHERE \"muterId\"=$1",
];

async fn read(
    runtime: &Runtime,
    sql: &str,
    viewer: &str,
    target: Option<&str>,
) -> Result<Vec<Value>, Error> {
    match target {
        Some(target) => rows(&runtime.database, true, sql, &[&viewer, &target]).await,
        None => rows(&runtime.database, true, sql, &[&viewer]).await,
    }
}

async fn read_all(
    runtime: &Runtime,
    viewer: &str,
    target: Option<&str>,
) -> Result<[Vec<Value>; 8], Error> {
    let following = if target.is_some() {
        format!(
            "SELECT {FOLLOWING_FIELDS}::text FROM following f WHERE f.\"followerId\"=$1 AND f.\"followeeId\"=$2 LIMIT 1"
        )
    } else {
        format!("SELECT {FOLLOWING_FIELDS}::text FROM following f WHERE f.\"followerId\"=$1")
    };
    let flags = if target.is_some() {
        &SCALAR_FLAGS
    } else {
        &ARRAY_IDS
    };
    // Even [] initiates all eight independent read-lane statements, as TS does.
    let (
        following,
        followed,
        pending_from_you,
        pending_to_you,
        blocking,
        blocked,
        muted,
        renote_muted,
    ) = futures_util::try_join!(
        read(runtime, &following, viewer, target),
        read(runtime, flags[0], viewer, target),
        read(runtime, flags[1], viewer, target),
        read(runtime, flags[2], viewer, target),
        read(runtime, flags[3], viewer, target),
        read(runtime, flags[4], viewer, target),
        read(runtime, flags[5], viewer, target),
        read(runtime, flags[6], viewer, target),
    )?;
    Ok([
        following,
        followed,
        pending_from_you,
        pending_to_you,
        blocking,
        blocked,
        muted,
        renote_muted,
    ])
}

fn scalar_boolean(values: &[Value]) -> Result<bool, Error> {
    match values {
        [Value::Boolean(value)] => Ok(*value),
        _ => Err(Error::Internal),
    }
}

fn scalar_value(target: &JsString, values: &[Vec<Value>; 8]) -> Result<Value, Error> {
    let following = match values[0].as_slice() {
        [] => None,
        [row] => Some(Following::decode(row)?),
        _ => return Err(Error::Internal),
    };
    Ok(Value::Array(vec![relation_value(
        target,
        following.as_ref(),
        RelationFlags {
            is_followed: scalar_boolean(&values[1])?,
            pending_from_you: scalar_boolean(&values[2])?,
            pending_to_you: scalar_boolean(&values[3])?,
            is_blocking: scalar_boolean(&values[4])?,
            is_blocked: scalar_boolean(&values[5])?,
            is_muted: scalar_boolean(&values[6])?,
            is_renote_muted: scalar_boolean(&values[7])?,
        },
    )]))
}

pub(super) async fn relation(
    runtime: &Runtime,
    params: &Value,
    auth: &Context,
) -> Result<Value, Error> {
    let viewer = auth
        .user
        .as_ref()
        .and_then(|user| user.get("id"))
        .and_then(serde_json::Value::as_str)
        .ok_or(Error::Internal)?;
    match params.get("userId") {
        Some(Value::String(value)) => {
            let target = JsString(value.clone());
            // Shared misskey:id validation guarantees ASCII here; keep exact case.
            let sql_id = String::from_utf16(value).map_err(|_| Error::Internal)?;
            scalar_value(&target, &read_all(runtime, viewer, Some(&sql_id)).await?)
        }
        Some(Value::Array(values)) => {
            let targets = values
                .iter()
                .map(JsString::decode)
                .collect::<Result<Vec<_>, _>>()?;
            let values = read_all(runtime, viewer, None).await?;
            Ok(RelationshipSet::decode(&values)?.values(&targets))
        }
        _ => Err(Error::Internal),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn following() -> Value {
        Value::parse_json(r#"{"id":"following","followeeId":"A","followerId":"viewer","isFollowerHibernated":false,"withReplies":true,"notify":null,"followerHost":null,"followerInbox":null,"followerSharedInbox":null,"followeeHost":"remote.test","followeeInbox":"https://remote.test/inbox","followeeSharedInbox":""}"#).unwrap()
    }

    #[test]
    fn following_preserves_twelve_fields_null_empty_and_open_notify_strings() {
        for notify in [Value::Null, Value::string(""), Value::string("unexpected")] {
            let mut row = following();
            row.set("notify", notify.clone());
            let result = Following::decode(&row).unwrap().value();
            assert_eq!(result, row);
            assert_eq!(result.get("notify"), Some(&notify));
            assert_eq!(result.get("followerHost"), Some(&Value::Null));
            assert_eq!(result.get("followeeSharedInbox"), Some(&Value::string("")));
            assert!(result.get("followee").is_none());
            assert!(result.get("follower").is_none());
        }
        let omitted = following()
            .stringify()
            .unwrap()
            .replace("\"notify\":null,", "");
        assert!(Following::decode(&Value::parse_json(&omitted).unwrap()).is_err());
    }

    #[test]
    fn following_rejects_wrong_nonnullable_nullable_and_boolean_types() {
        for (key, value) in [
            ("id", Value::Null),
            ("notify", Value::Boolean(false)),
            ("withReplies", Value::Number(1.)),
            ("isFollowerHibernated", Value::string("false")),
        ] {
            let mut row = following();
            row.set(key, value);
            assert!(Following::decode(&row).is_err());
        }
        assert!(JsString::decode(&Value::Number(1.)).is_err());
        assert!(scalar_boolean(&[Value::Number(1.)]).is_err());
        assert!(scalar_boolean(&[]).is_err());
    }

    #[test]
    fn all_eight_directions_are_independent_and_unknowns_have_explicit_null() {
        let mut data: [Vec<Value>; 8] = std::array::from_fn(|_| Vec::new());
        data[0].push(following());
        let keys = [
            "isFollowed",
            "hasPendingFollowRequestFromYou",
            "hasPendingFollowRequestToYou",
            "isBlocking",
            "isBlocked",
            "isMuted",
            "isRenoteMuted",
        ];
        for (index, values) in data.iter_mut().enumerate().skip(1) {
            values.push(Value::string(&format!("target{index}")));
        }
        let set = RelationshipSet::decode(&data).unwrap();
        let outgoing = set.relation(&JsString::decode(&Value::string("A")).unwrap());
        assert_eq!(outgoing.get("isFollowing"), Some(&Value::Boolean(true)));
        for index in 1..8 {
            let result =
                set.relation(&JsString::decode(&Value::string(&format!("target{index}"))).unwrap());
            assert_eq!(result.get("following"), Some(&Value::Null));
            assert_eq!(result.get("isFollowing"), Some(&Value::Boolean(false)));
            for (flag, key) in keys.iter().enumerate() {
                assert_eq!(result.get(key), Some(&Value::Boolean(flag + 1 == index)));
            }
        }
        let unknown = set.relation(&JsString::decode(&Value::string("missing")).unwrap());
        assert_eq!(unknown.get("following"), Some(&Value::Null));
        for key in keys {
            assert_eq!(unknown.get(key), Some(&Value::Boolean(false)));
        }
    }

    #[test]
    fn arrays_deduplicate_at_first_position_and_scalar_stays_an_array() {
        let empty: [Vec<Value>; 8] = std::array::from_fn(|_| Vec::new());
        let set = RelationshipSet::decode(&empty).unwrap();
        let targets = ["B", "A", "B"].map(|id| JsString::decode(&Value::string(id)).unwrap());
        let result = set.values(&targets);
        let Value::Array(values) = &result else {
            panic!("expected array")
        };
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].get("id"), Some(&Value::string("B")));
        assert_eq!(values[1].get("id"), Some(&Value::string("A")));
        assert_eq!(set.values(&[]), Value::Array(Vec::new()));
        let scalar: [Vec<Value>; 8] = std::array::from_fn(|index| {
            if index == 0 {
                Vec::new()
            } else {
                vec![Value::Boolean(false)]
            }
        });
        assert_eq!(
            scalar_value(&targets[0], &scalar).unwrap(),
            set.values(&targets[..1])
        );
    }
}
