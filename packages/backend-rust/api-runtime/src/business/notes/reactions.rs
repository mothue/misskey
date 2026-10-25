//! Reaction compatibility is applied after merging the native Redis buffer.
use super::super::js_value::{add, number};
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

// Stored JSONB is not guaranteed to satisfy the TypeScript annotation. Keep
// the legacy spread, relational coercion and addition semantics explicitly at
// this payload boundary; authentication and visibility fields stay typed.
fn spread(source: &Value) -> Value {
    match source {
        Value::Object(_) => source.clone(),
        Value::Array(items) => Value::object(
            items
                .iter()
                .enumerate()
                .map(|(i, value)| (i.to_string(), value.clone())),
        ),
        Value::String(units) => Value::object(
            units
                .iter()
                .enumerate()
                .map(|(i, unit)| (i.to_string(), Value::String(vec![*unit]))),
        ),
        _ => Value::object(std::iter::empty::<(&str, Value)>()),
    }
}

// The two legacy TS accumulators are ordinary `{}` objects, not maps.
// Model inherited reads and the __proto__ setter explicitly without giving
// Rust payload objects a mutable prototype or treating inherited keys as own.
const PROTOTYPE_FUNCTIONS: &[&str] = &[
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "toLocaleString",
];

fn key_is(key: &[u16], name: &str) -> bool {
    key.len() == name.len()
        && key
            .iter()
            .zip(name.bytes())
            .all(|(unit, byte)| *unit == u16::from(byte))
}

fn plain_property<'a>(object: &'a Value, key: &[u16]) -> Option<std::borrow::Cow<'a, Value>> {
    use std::borrow::Cow;
    if let Some(own) = object.get_units(key) {
        return Some(Cow::Borrowed(own));
    }
    // Emoji/custom-emoji names cannot name Object.prototype properties.
    if !key.first().is_some_and(|first| {
        u8::try_from(*first)
            .is_ok_and(|first| matches!(first, b'_' | b'c' | b'h' | b'i' | b'p' | b't' | b'v'))
    }) {
        return None;
    }
    if key_is(key, "__proto__") {
        return Some(Cow::Owned(Value::object(
            std::iter::empty::<(&str, Value)>(),
        )));
    }
    let name = PROTOTYPE_FUNCTIONS.iter().find(|name| key_is(key, name))?;
    let function_name = if *name == "constructor" {
        "Object"
    } else {
        name
    };
    Some(Cow::Owned(Value::string(&format!(
        "function {function_name}() {{ [native code] }}"
    ))))
}

fn assign_plain_primitive(object: &mut Value, key: Vec<u16>, value: Value) {
    // Every caller's result is a primitive (JS addition or numeric delta).
    // With no own property, Object.prototype.__proto__ ignores that assignment.
    if object.get_units(&key).is_none() && key_is(&key, "__proto__") {
        return;
    }
    object.set_units(key, value);
}

pub(super) fn merge(src: &Value, delta: Option<&Value>) -> Result<Value, Error> {
    let mut merged = spread(src);
    if let Some(delta) = delta {
        let Value::Object(entries) = delta else {
            return Err(Error::Internal);
        };
        for (key, count) in entries {
            // Buffer deltas are an independently decoded numeric contract.
            count.as_number().ok_or(Error::Internal)?;
            let next = match plain_property(&merged, key) {
                Some(previous) if !matches!(previous.as_ref(), Value::Null | Value::Undefined) => {
                    add(&previous, count)?
                }
                _ => count.clone(),
            };
            assign_plain_primitive(&mut merged, key.clone(), next);
        }
    }
    let Value::Object(entries) = &merged else {
        return Err(Error::Internal);
    };
    let mut converted = Value::object(std::iter::empty::<(&str, Value)>());
    for (key, count) in entries {
        let positive = number(count)?;
        if positive <= 0. || positive.is_nan() {
            continue;
        }
        // A legacy key's non-ASCII UTF16 is preserved unless ASCII decoding
        // applies; invalid surrogate pairs must not become replacement text.
        let key = String::from_utf16(key)
            .map(|key| convert(&key).encode_utf16().collect())
            .unwrap_or_else(|_| key.clone());
        let zero = Value::Number(0.);
        let previous = plain_property(&converted, &key);
        let previous = previous
            .as_deref()
            .filter(|value| !matches!(value, Value::Null | Value::Undefined))
            .unwrap_or(&zero);
        let next = add(previous, count)?;
        assign_plain_primitive(&mut converted, key, next);
    }
    Ok(converted)
}

pub(super) fn count(reactions: &Value) -> Result<Value, Error> {
    let Value::Object(entries) = reactions else {
        return Err(Error::Internal);
    };
    entries
        .iter()
        .try_fold(Value::Number(0.), |sum, (_, value)| add(&sum, value))
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
    if matches!(count, Value::Number(0.)) {
        return Ok(Some(None));
    }
    if number(&count)? <= pairs.len() as f64 {
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
    fn legacy_json_counts_keep_javascript_coercion_and_addition() {
        for (src, expected, expected_count) in [
            (r#"{"👍":"1"}"#, r#"{"👍":"01"}"#, r#""001""#),
            (r#"{"👍":true}"#, r#"{"👍":1}"#, "1"),
            (r#"{"👍":null,"other":"bad"}"#, "{}", "0"),
            (r#"{"👍":[1]}"#, r#"{"👍":"01"}"#, r#""001""#),
            (r#"{"👍":{},"other":[]}"#, "{}", "0"),
            ("null", "{}", "0"),
            ("true", "{}", "0"),
            ("12", "{}", "0"),
            ("[1]", r#"{"0":1}"#, "1"),
            (r#""2""#, r#"{"0":"02"}"#, r#""002""#),
        ] {
            let source = Value::parse_json(src).unwrap();
            let before = source.clone();
            let packed = merge(&source, None).unwrap();
            assert_eq!(packed.stringify().as_deref(), Some(expected), "{src}");
            assert_eq!(
                count(&packed).unwrap().stringify().as_deref(),
                Some(expected_count),
                "{src}"
            );
            assert_eq!(source, before);
        }
    }

    #[test]
    fn buffered_addition_precedes_coercion_and_colliding_legacy_keys() {
        let source = Value::parse_json(r#"{"like":"1","👍":2,"gone":null}"#).unwrap();
        let delta = Value::parse_json(r#"{"like":1,"gone":1}"#).unwrap();
        let packed = merge(&source, Some(&delta)).unwrap();
        assert_eq!(
            packed.stringify().as_deref(),
            Some(r#"{"👍":"0112","gone":1}"#)
        );
        assert_eq!(
            count(&packed).unwrap().stringify().as_deref(),
            Some(r#""001121""#)
        );
    }

    #[test]
    fn string_relational_coercion_and_array_join_match_json_runtime_values() {
        for (value, expected) in [
            (r#"" 0x10 ""#, 16.),
            (r#""0b11""#, 3.),
            (r#""0o10""#, 8.),
            (r#""+1.e1""#, 10.),
            (r#""\ufeff1\u00a0""#, 1.),
            ("[[[1]]]", 1.),
            ("[null]", 0.),
            ("[]", 0.),
            ("-0", -0.),
        ] {
            assert_eq!(
                number(&Value::parse_json(value).unwrap()).unwrap(),
                expected,
                "{value}"
            );
        }
        for value in [r#""inf""#, r#""-0x1""#, r#""1_0""#, "[1,2]", "{}", "[true]"] {
            assert!(
                number(&Value::parse_json(value).unwrap()).unwrap().is_nan(),
                "{value}"
            );
        }
        assert!(matches!(
            number(&Value::parse_json(r#"{"toString":null}"#).unwrap()),
            Err(Error::Legacy { message, code: "TypeError", .. }) if message == "No default value"
        ));
    }

    #[test]
    fn keys_and_addition_preserve_utf16_and_javascript_enumeration() {
        let source = Value::parse_json(r#"{"z":1,"10":1,"2":1,"\ud800":1}"#).unwrap();
        let packed = merge(&source, None).unwrap();
        assert_eq!(
            packed.stringify().as_deref(),
            Some(r#"{"2":1,"10":1,"z":1,"\ud800":1}"#)
        );
        let left = Value::String(vec![0xd800]);
        let joined = add(&left, &Value::Number(1.)).unwrap();
        assert_eq!(
            joined.as_utf16(),
            Some([0xd800, u16::from(b'1')].as_slice())
        );
    }

    #[test]
    fn plain_accumulators_preserve_inherited_function_reads() {
        for name in PROTOTYPE_FUNCTIONS {
            let source = Value::object([(*name, Value::Number(1.))]);
            let packed = merge(&source, None).unwrap();
            let function_name = if *name == "constructor" {
                "Object"
            } else {
                name
            };
            let expected = format!("function {function_name}() {{ [native code] }}1");
            assert_eq!(
                packed.get(name).and_then(Value::to_utf8),
                Some(expected.clone()),
                "{name}"
            );
            assert_eq!(
                count(&packed).unwrap().to_utf8(),
                Some(format!("0{expected}")),
                "{name}"
            );
            // During merge, inherited function + numeric delta becomes a
            // nonnumeric string and is removed by the subsequent positive test.
            let delta = Value::object([(*name, Value::Number(1.))]);
            assert_eq!(
                merge(
                    &Value::object(std::iter::empty::<(&str, Value)>()),
                    Some(&delta)
                )
                .unwrap(),
                Value::object(std::iter::empty::<(&str, Value)>()),
                "{name}"
            );
        }
    }

    #[test]
    fn proto_setter_ignores_primitives_without_erasing_spread_own_properties() {
        let source = Value::parse_json(r#"{"__proto__":1,"👍":1}"#).unwrap();
        assert_eq!(
            merge(&source, None).unwrap().stringify().as_deref(),
            Some(r#"{"👍":1}"#)
        );
        let delta = Value::parse_json(r#"{"__proto__":2}"#).unwrap();
        assert_eq!(
            merge(&source, Some(&delta)).unwrap().stringify().as_deref(),
            Some(r#"{"👍":1}"#)
        );
        assert_eq!(
            merge(
                &Value::object(std::iter::empty::<(&str, Value)>()),
                Some(&delta)
            )
            .unwrap()
            .stringify()
            .as_deref(),
            Some("{}")
        );
        // The relational conversion still happens before assigning a key that
        // will be discarded: it must not suppress a TS TypeError.
        let invalid = Value::parse_json(r#"{"__proto__":{"toString":null}}"#).unwrap();
        assert!(merge(&invalid, None).is_err());
        let packed = merge(&Value::parse_json(r#"{"__proto__":1}"#).unwrap(), None).unwrap();
        assert_eq!(count(&packed).unwrap(), Value::Number(0.));
    }

    #[test]
    fn pair_cache_uses_numeric_comparison_of_legacy_string_totals() {
        let reactions = Value::parse_json(r#"{"👍":"01"}"#).unwrap();
        assert_eq!(
            cached_mine(&reactions, &["viewer/like".into()], "viewer").unwrap(),
            Some(Some("👍".into()))
        );
        assert_eq!(cached_mine(&reactions, &[], "viewer").unwrap(), None);
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
