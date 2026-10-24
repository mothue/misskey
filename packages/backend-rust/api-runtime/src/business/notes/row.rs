//! Database records decoded into the types used by NoteEntityService.
use super::{Error, Value};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Visibility {
    Public,
    Home,
    Followers,
    Specified,
}
impl Visibility {
    pub fn name(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Home => "home",
            Self::Followers => "followers",
            Self::Specified => "specified",
        }
    }
}

pub(super) struct Note {
    pub id: String,
    pub user_id: String,
    pub user_host: Option<String>,
    pub reply_id: Option<String>,
    pub renote_id: Option<String>,
    pub text: Option<String>,
    pub name: Option<String>,
    pub cw: Option<String>,
    pub uri: Option<String>,
    pub url: Option<String>,
    pub visibility: Visibility,
    pub local_only: bool,
    pub reaction_acceptance: Option<String>,
    pub renote_count: i16,
    pub replies_count: i16,
    pub clipped_count: i16,
    pub reactions: Value,
    pub reaction_pairs: Vec<String>,
    pub file_ids: Vec<String>,
    pub visible_user_ids: Vec<String>,
    pub mentions: Vec<String>,
    pub emojis: Vec<String>,
    pub tags: Vec<String>,
    pub has_poll: bool,
    pub channel_id: Option<String>,
}
impl Note {
    pub fn decode(row: &Value) -> Result<Self, Error> {
        let visibility = match string(row, "visibility")?.as_str() {
            "public" => Visibility::Public,
            "home" => Visibility::Home,
            "followers" => Visibility::Followers,
            "specified" => Visibility::Specified,
            _ => return Err(Error::Internal),
        };
        let reactions = field(row, "reactions")?.clone();
        let Value::Object(entries) = &reactions else {
            return Err(Error::Internal);
        };
        if entries.iter().any(|(_, n)| n.as_number().is_none()) {
            return Err(Error::Internal);
        }
        let reaction_acceptance = nullable_string(row, "reactionAcceptance")?;
        if !matches!(
            reaction_acceptance.as_deref(),
            None | Some(
                "likeOnly"
                    | "likeOnlyForRemote"
                    | "nonSensitiveOnly"
                    | "nonSensitiveOnlyForLocalLikeOnlyForRemote"
            )
        ) {
            return Err(Error::Internal);
        }
        Ok(Self {
            id: string(row, "id")?,
            user_id: string(row, "userId")?,
            user_host: nullable_string(row, "userHost")?,
            reply_id: nullable_string(row, "replyId")?,
            renote_id: nullable_string(row, "renoteId")?,
            text: nullable_string(row, "text")?,
            name: nullable_string(row, "name")?,
            cw: nullable_string(row, "cw")?,
            uri: nullable_string(row, "uri")?,
            url: nullable_string(row, "url")?,
            visibility,
            local_only: boolean(row, "localOnly")?,
            reaction_acceptance,
            renote_count: integer16(row, "renoteCount")?,
            replies_count: integer16(row, "repliesCount")?,
            clipped_count: integer16(row, "clippedCount")?,
            reactions,
            reaction_pairs: strings(row, "reactionAndUserPairCache")?,
            file_ids: strings(row, "fileIds")?,
            visible_user_ids: strings(row, "visibleUserIds")?,
            mentions: strings(row, "mentions")?,
            emojis: strings(row, "emojis")?,
            tags: strings(row, "tags")?,
            has_poll: boolean(row, "hasPoll")?,
            channel_id: nullable_string(row, "channelId")?,
        })
    }
}

pub(super) fn field<'a>(row: &'a Value, name: &str) -> Result<&'a Value, Error> {
    row.get(name).ok_or(Error::Internal)
}
pub(super) fn string(row: &Value, name: &str) -> Result<String, Error> {
    field(row, name)?.to_utf8().ok_or(Error::Internal)
}
pub(super) fn nullable_string(row: &Value, name: &str) -> Result<Option<String>, Error> {
    match field(row, name)? {
        Value::Null => Ok(None),
        value => value.to_utf8().map(Some).ok_or(Error::Internal),
    }
}
pub(super) fn strings(row: &Value, name: &str) -> Result<Vec<String>, Error> {
    let Value::Array(values) = field(row, name)? else {
        return Err(Error::Internal);
    };
    values
        .iter()
        .map(|v| v.to_utf8().ok_or(Error::Internal))
        .collect()
}
pub(super) fn boolean(row: &Value, name: &str) -> Result<bool, Error> {
    match field(row, name)? {
        Value::Boolean(v) => Ok(*v),
        _ => Err(Error::Internal),
    }
}
fn integer16(row: &Value, name: &str) -> Result<i16, Error> {
    let n = field(row, name)?.as_number().ok_or(Error::Internal)?;
    if n.fract() != 0. || !(f64::from(i16::MIN)..=f64::from(i16::MAX)).contains(&n) {
        return Err(Error::Internal);
    }
    Ok(n as i16)
}

pub(super) fn optional_string(value: Option<&str>) -> Value {
    value.map(Value::string).unwrap_or(Value::Undefined)
}
pub(super) fn null_string(value: Option<&str>) -> Value {
    value.map(Value::string).unwrap_or(Value::Null)
}
pub(super) fn string_array(values: &[String]) -> Value {
    Value::Array(values.iter().map(|v| Value::string(v)).collect())
}
