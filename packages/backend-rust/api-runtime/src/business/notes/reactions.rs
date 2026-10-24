//! Reaction compatibility is applied after merging the native Redis buffer.
use super::{Error, Value};

pub(super) fn convert(reaction: &str) -> String {
    let decoded = decode(reaction);
    match decoded.as_str() {
        "like" => "👍",
        "love" => "❤",
        "laugh" => "😆",
        "hmm" => "🤔",
        "surprise" => "😮",
        "congrats" => "🎉",
        "angry" => "💢",
        "confused" => "😥",
        "rip" => "😇",
        "pudding" => "🍮",
        "star" => "⭐",
        _ => return decoded,
    }
    .to_owned()
}

// Mirrors /^:([\w+-]+)(?:@([\w.-]+))?:$/; JavaScript \w is ASCII here.
fn decode(reaction: &str) -> String {
    let candidate = reaction
        .strip_suffix(['\n', '\r', '\u{2028}', '\u{2029}'])
        .unwrap_or(reaction);
    let Some(inner) = candidate
        .strip_prefix(':')
        .and_then(|v| v.strip_suffix(':'))
    else {
        return reaction.to_owned();
    };
    let (name, host) = inner
        .split_once('@')
        .map_or((inner, None), |(n, h)| (n, Some(h)));
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-'))
        || host.is_some_and(|h| {
            h.is_empty()
                || !h
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
        })
    {
        return reaction.to_owned();
    }
    format!(":{name}@{}:", host.unwrap_or("."))
}

pub(super) fn merge(src: &Value, delta: Option<&Value>) -> Result<Value, Error> {
    let mut merged = src.clone();
    let Value::Object(_) = &merged else {
        return Err(Error::Internal);
    };
    if let Some(delta) = delta {
        let Value::Object(entries) = delta else {
            return Err(Error::Internal);
        };
        for (key, count) in entries {
            let count = count.as_number().ok_or(Error::Internal)?;
            let previous = merged
                .get_units(key)
                .and_then(Value::as_number)
                .unwrap_or(0.);
            merged.set_units(key.clone(), Value::Number(previous + count));
        }
    }
    let Value::Object(entries) = &merged else {
        return Err(Error::Internal);
    };
    let mut converted = Value::object(std::iter::empty::<(&str, Value)>());
    for (key, count) in entries {
        let count = count.as_number().ok_or(Error::Internal)?;
        if count <= 0. || count.is_nan() {
            continue;
        }
        let key = convert(&String::from_utf16(key).map_err(|_| Error::Internal)?);
        let previous = converted.get(&key).and_then(Value::as_number).unwrap_or(0.);
        converted.set(&key, Value::Number(previous + count));
    }
    Ok(converted)
}
pub(super) fn count(reactions: &Value) -> Result<f64, Error> {
    let Value::Object(entries) = reactions else {
        return Err(Error::Internal);
    };
    entries.iter().try_fold(0., |sum, (_, v)| {
        v.as_number().map(|n| sum + n).ok_or(Error::Internal)
    })
}
pub(super) fn emoji_names(reactions: &Value) -> Result<Vec<String>, Error> {
    let Value::Object(entries) = reactions else {
        return Err(Error::Internal);
    };
    entries
        .iter()
        .filter_map(|(key, _)| {
            let key = String::from_utf16(key).ok()?;
            (key.starts_with(':') && key.contains('@') && !key.contains("@."))
                .then(|| decode(&key).replace(':', ""))
        })
        .map(Ok)
        .collect()
}
pub(super) fn cached_mine(
    reactions: &Value,
    pairs: &[String],
    viewer: &str,
) -> Result<Option<Option<String>>, Error> {
    let count = count(reactions)?;
    if count == 0. {
        return Ok(Some(None));
    }
    if count <= pairs.len() as f64 {
        return Ok(Some(
            pairs
                .iter()
                .find(|p| p.starts_with(viewer))
                .and_then(|p| p.split('/').nth(1))
                .map(convert),
        ));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn merge_precedes_zero_filter_and_legacy_key_coalescing() {
        let src = Value::parse_json(r#"{"like":1,"👍":2,":foo:":1,":foo@.:":2,"gone":1,"new":0}"#)
            .unwrap();
        let delta = Value::parse_json(r#"{"like":-1,"gone":-1,"new":2}"#).unwrap();
        let out = merge(&src, Some(&delta)).unwrap();
        assert_eq!(out.get("👍").and_then(Value::as_number), Some(2.));
        assert_eq!(out.get(":foo@.:").and_then(Value::as_number), Some(3.));
        assert_eq!(out.get("new").and_then(Value::as_number), Some(2.));
        assert!(out.get("gone").is_none());
    }
    #[test]
    fn custom_and_legacy_decoding_preserves_nonmatching_strings() {
        for (src, expected) in [
            (":a:", ":a@.:"),
            (":a+b@host.test:", ":a+b@host.test:"),
            (":a:\n", ":a@.:"),
            (":あ:", ":あ:"),
            ("love", "❤"),
            (":bad@:", ":bad@:"),
        ] {
            assert_eq!(convert(src), expected);
        }
    }
    #[test]
    fn pair_cache_uses_the_existing_prefix_and_first_reaction_component() {
        let reactions = Value::parse_json(r#"{"like":1}"#).unwrap();
        assert_eq!(
            cached_mine(&reactions, &["viewer/like/ignored".into()], "view").unwrap(),
            Some(Some("👍".into()))
        );
        assert_eq!(
            cached_mine(&reactions, &["other/like".into()], "view").unwrap(),
            Some(None)
        );
        assert_eq!(cached_mine(&reactions, &[], "view").unwrap(), None);
    }
}
