use serde_json::{Map, Value, json};
use std::{borrow::Borrow, collections::HashMap};

#[path = "policy_aggregation.rs"]
mod policy_aggregation;
use policy_aggregation::{Aggregation, policy_aggregation};

/// Mirrors RoleService.evalCond, including false for unknown/malformed formulas.
pub fn evaluate_condition<R: Borrow<Value>>(
    user: &Value,
    assigned_roles: &[R],
    formula: &Value,
    id_method: &str,
    now_ms: i64,
) -> bool {
    enum Frame<'a> {
        And(&'a [Value], usize),
        Or(&'a [Value], usize),
        Not,
    }
    let mut stack = Vec::new();
    let mut current = formula;
    loop {
        let mut result = match current.get("type").and_then(Value::as_str) {
            Some("and" | "or") => {
                let and = current["type"] == "and";
                if let Some(values) = current.get("values").and_then(Value::as_array) {
                    if let Some(first) = values.first() {
                        stack.push(if and {
                            Frame::And(values, 1)
                        } else {
                            Frame::Or(values, 1)
                        });
                        current = first;
                        continue;
                    }
                    and
                } else {
                    false
                }
            }
            Some("not") => {
                stack.push(Frame::Not);
                current = current.get("value").unwrap_or(&Value::Null);
                continue;
            }
            _ => evaluate_leaf(user, assigned_roles, current, id_method, now_ms),
        };
        loop {
            match stack.pop() {
                None => return result,
                Some(Frame::Not) => result = !result,
                Some(Frame::And(values, next)) if result && next < values.len() => {
                    stack.push(Frame::And(values, next + 1));
                    current = &values[next];
                    break;
                }
                Some(Frame::Or(values, next)) if !result && next < values.len() => {
                    stack.push(Frame::Or(values, next + 1));
                    current = &values[next];
                    break;
                }
                _ => {}
            }
        }
    }
}

fn evaluate_leaf<R: Borrow<Value>>(
    user: &Value,
    assigned: &[R],
    formula: &Value,
    id_method: &str,
    now_ms: i64,
) -> bool {
    match formula.get("type").and_then(Value::as_str) {
        Some("roleAssignedTo") => assigned
            .iter()
            .any(|role| role.borrow().get("id") == formula.get("roleId")),
        Some("isLocal") => user.get("host").is_none_or(Value::is_null),
        Some("isRemote") => user.get("host").is_some_and(|v| !v.is_null()),
        Some("isSuspended") => truthy(&user["isSuspended"]),
        Some("isLocked") => truthy(&user["isLocked"]),
        Some("isBot") => truthy(&user["isBot"]),
        Some("isCat") => truthy(&user["isCat"]),
        Some("isExplorable") => truthy(&user["isExplorable"]),
        Some("createdLessThan" | "createdMoreThan") => {
            let created = user
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| parse_id_date(id_method, id));
            let seconds = formula.get("sec").map(number).unwrap_or(f64::NAN);
            created.is_some_and(|date| {
                if formula["type"] == "createdLessThan" {
                    date > now_ms as f64 - seconds * 1000.0
                } else {
                    date < now_ms as f64 - seconds * 1000.0
                }
            })
        }
        Some(
            "followersLessThanOrEq"
            | "followersMoreThanOrEq"
            | "followingLessThanOrEq"
            | "followingMoreThanOrEq"
            | "notesLessThanOrEq"
            | "notesMoreThanOrEq",
        ) => {
            let kind = formula["type"].as_str().unwrap_or("");
            let field = if kind.starts_with("followers") {
                "followersCount"
            } else if kind.starts_with("following") {
                "followingCount"
            } else {
                "notesCount"
            };
            let actual = user.get(field).map(number).unwrap_or(f64::NAN);
            let expected = formula.get("value").map(number).unwrap_or(f64::NAN);
            if kind.ends_with("LessThanOrEq") {
                actual <= expected
            } else {
                actual >= expected
            }
        }
        _ => false,
    }
}

/// ID parsers intentionally do not validate the regex: TS parseInt accepts an
/// initial digit prefix, and malformed values produce Invalid Date/false tests.
pub fn parse_id_date(method: &str, id: &str) -> Option<f64> {
    let units: Vec<_> = id.encode_utf16().collect();
    let prefix = |start: usize, len: usize, radix: u32| {
        parse_int(
            &units[start.min(units.len())..(start + len).min(units.len())],
            radix,
        )
    };
    let result = match method {
        "aid" | "aidx" => prefix(0, 8, 36)? + 946_684_800_000.0,
        "meid" => prefix(0, 12, 16)? - 140_737_488_355_328.0,
        "meidg" => prefix(1, 11, 16)?,
        "objectid" => prefix(0, 8, 16)? * 1000.0,
        "ulid" => {
            let chars = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
            let mut date = 0.0;
            for ch in units.iter().take(10) {
                let digit = chars.iter().position(|c| u16::from(*c) == *ch);
                date = date * 32.0 + digit.map_or(-1.0, |d| d as f64);
            }
            date
        }
        _ => return None,
    };
    // ECMAScript TimeClip truncates fractional ms and rejects out-of-range dates.
    (result.is_finite() && result.abs() <= 8_640_000_000_000_000.0).then(|| result.trunc())
}

fn parse_int(units: &[u16], radix: u32) -> Option<f64> {
    let mut chars = units.iter().copied().peekable();
    while chars.peek().is_some_and(|ch| {
        char::from_u32(u32::from(*ch)).is_some_and(|c| c.is_whitespace() || c == '\u{feff}')
    }) {
        chars.next();
    }
    let sign = match chars.peek() {
        Some(0x2d) => {
            chars.next();
            -1.0
        }
        Some(0x2b) => {
            chars.next();
            1.0
        }
        _ => 1.0,
    };
    if radix == 16 {
        let mut candidate = chars.clone();
        if candidate.next() == Some(0x30) && matches!(candidate.next(), Some(0x78 | 0x58)) {
            chars = candidate;
        }
    }
    let mut seen = false;
    let mut result = 0.0;
    for ch in chars {
        let Some(digit) = char::from_u32(u32::from(ch)).and_then(|c| c.to_digit(radix)) else {
            break;
        };
        seen = true;
        result = result * f64::from(radix) + f64::from(digit);
    }
    seen.then_some(result * sign)
}

pub(crate) fn aggregate_policies<R: Borrow<Value>>(
    base: &Map<String, Value>,
    roles: &[R],
    max_file_size: u64,
) -> Result<crate::PolicySnapshot, crate::Failure> {
    let mut result = Map::new();
    let mut numeric = HashMap::new();
    for (name, base_value) in base {
        // getUserPolicies(user) returns only DEFAULT_POLICIES' known keys, even
        // if meta.policies has extra properties. Its anonymous branch spreads.
        let Some(aggregation) = policy_aggregation(name) else {
            continue;
        };
        let values: Vec<Option<&Value>> = if roles.is_empty() {
            vec![Some(base_value)]
        } else {
            let policies: Vec<_> = roles
                .iter()
                .map(|r| &r.borrow()["policies"][name])
                .collect();
            let tier = if policies
                .iter()
                .any(|p| p.get("priority").and_then(Value::as_f64) == Some(2.0))
            {
                Some(2)
            } else if policies
                .iter()
                .any(|p| p.get("priority").and_then(Value::as_f64) == Some(1.0))
            {
                Some(1)
            } else {
                None
            };
            policies
                .into_iter()
                .filter(|p| {
                    tier.is_none_or(|tier| {
                        p.get("priority").and_then(Value::as_f64) == Some(tier as f64)
                    })
                })
                .map(|p| {
                    if p.is_null() || truthy(&p["useDefault"]) {
                        Some(base_value)
                    } else {
                        p.get("value")
                    }
                })
                .collect()
        };
        let value = match aggregation {
            Aggregation::Any => Value::Bool(values.iter().any(|v| *v == Some(&Value::Bool(true)))),
            Aggregation::Max | Aggregation::MaxFileSize => {
                let mut maximum = f64::NEG_INFINITY;
                for value in values {
                    let next = value.map(policy_number).transpose()?.unwrap_or(f64::NAN);
                    if next.is_nan() {
                        maximum = f64::NAN;
                        // Math.max still coerces the remaining arguments. A
                        // later selected value may throw during ToPrimitive.
                        continue;
                    }
                    // Math.max chooses +0 when either equal zero is +0.
                    // Rust's max does not guarantee that signed-zero choice.
                    if next > maximum || (next == 0.0 && maximum == 0.0 && next.is_sign_positive())
                    {
                        maximum = next;
                    }
                }
                if matches!(aggregation, Aggregation::MaxFileSize) && !maximum.is_nan() {
                    let cap = (max_file_size / (1024 * 1024)) as f64;
                    // Math.min retains a -0 aggregate when the server cap is +0.
                    if cap < maximum {
                        maximum = cap;
                    }
                }
                numeric.insert(name.clone(), maximum);
                json!(maximum)
            }
            Aggregation::Chat => {
                if values
                    .iter()
                    .any(|v| v.and_then(Value::as_str) == Some("available"))
                {
                    json!("available")
                } else if values
                    .iter()
                    .any(|v| v.and_then(Value::as_str) == Some("readonly"))
                {
                    json!("readonly")
                } else {
                    json!("unavailable")
                }
            }
            Aggregation::FileTypes => {
                let mut types = Vec::<String>::new();
                for value in values {
                    let value = value.ok_or(crate::Failure)?;
                    let items: Vec<String> = if let Some(items) = value.as_array() {
                        items
                            .iter()
                            .map(|item| item.as_str().map(str::to_owned).ok_or(crate::Failure))
                            .collect::<Result<_, _>>()?
                    } else if let Some(text) = value.as_str() {
                        text.chars().map(|c| c.to_string()).collect()
                    } else {
                        return Err(crate::Failure);
                    };
                    for text in items {
                        let text = text.trim_matches(js_whitespace).to_owned();
                        if !text.is_empty() && !types.contains(&text) {
                            types.push(text);
                        }
                    }
                }
                json!(types)
            }
        };
        result.insert(name.clone(), value);
    }
    Ok(crate::PolicySnapshot::aggregated(
        Value::Object(result),
        numeric,
    ))
}

pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn number(value: &Value) -> f64 {
    match value {
        Value::Null => 0.0,
        Value::Bool(true) => 1.0,
        Value::Bool(false) => 0.0,
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => number_string(s),
        Value::Array(_) => {
            // Array ToPrimitive uses join/toString before Number conversion.
            // A one-child chain can be traversed without recursive JS coercion.
            let mut leaf = value;
            while let Value::Array(items) = leaf {
                if items.is_empty() {
                    return 0.0;
                }
                if items.len() != 1 {
                    return f64::NAN;
                }
                leaf = &items[0];
            }
            match leaf {
                Value::Null => 0.0,
                Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
                Value::String(s) => number_string(s),
                _ => f64::NAN,
            }
        }
        _ => f64::NAN,
    }
}

/// Checked ToNumber for stored JSON policy values. JSON cannot carry callable
/// own methods, so an own `toString` blocks both Object.prototype conversions.
/// Plain objects still convert to "[object Object]" and then NaN. Array join
/// visits every element, including nested arrays, before Number sees its text.
/// Keep condition evaluation's separate catch-as-false path unchanged.
fn policy_number(value: &Value) -> Result<f64, crate::Failure> {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(properties) if properties.contains_key("toString") => {
                return Err(crate::Failure);
            }
            Value::Array(items) => pending.extend(items.iter().rev()),
            _ => {}
        }
    }
    let result = number(value);
    if result == 0.0 && matches!(value, Value::Array(_)) {
        let mut leaf = value;
        while let Value::Array(items) = leaf {
            if items.len() != 1 {
                break;
            }
            leaf = &items[0];
        }
        // Array join stringifies numeric -0 as "0". A string "-0" instead
        // remains "-0", so its subsequent Number conversion keeps the sign.
        if matches!(leaf, Value::Number(_)) {
            return Ok(0.0);
        }
    }
    Ok(result)
}

fn number_string(text: &str) -> f64 {
    let text = text.trim_matches(js_whitespace);
    if text.is_empty() {
        return 0.0;
    }
    match text {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    for (prefix, base) in [
        ("0x", 16),
        ("0X", 16),
        ("0b", 2),
        ("0B", 2),
        ("0o", 8),
        ("0O", 8),
    ] {
        if let Some(digits) = text.strip_prefix(prefix) {
            if digits.is_empty() {
                return f64::NAN;
            }
            let mut result = 0.0;
            for ch in digits.chars() {
                let Some(digit) = ch.to_digit(base) else {
                    return f64::NAN;
                };
                result = result * f64::from(base) + f64::from(digit);
            }
            return result;
        }
    }
    let bytes = text.as_bytes();
    let mut at = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        at += 1;
    }
    let mut digits = 0;
    while bytes.get(at).is_some_and(u8::is_ascii_digit) {
        at += 1;
        digits += 1;
    }
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return f64::NAN;
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        let start = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        if at == start {
            return f64::NAN;
        }
    }
    if at != bytes.len() {
        return f64::NAN;
    }
    text.parse().unwrap_or(f64::NAN)
}

fn js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'..='\u{000d}'
            | '\u{0020}'
            | '\u{00a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// JSON.stringify(Date) emits this ISO form for Redis event dates. This parser
/// accepts the exact emitted form, including extended years, without a JS VM.
pub(crate) fn parse_iso_date(text: &str) -> Option<i64> {
    if !text.is_ascii() {
        return None;
    }
    let year_len = if text.starts_with(['+', '-']) { 7 } else { 4 };
    if text.len() != year_len + 20 || !text.ends_with('Z') {
        return None;
    }
    let year: i64 = text.get(..year_len)?.parse().ok()?;
    let rest = text.get(year_len..)?;
    if &rest[0..1] != "-"
        || &rest[3..4] != "-"
        || &rest[6..7] != "T"
        || &rest[9..10] != ":"
        || &rest[12..13] != ":"
        || &rest[15..16] != "."
    {
        return None;
    }
    let month: i64 = rest[1..3].parse().ok()?;
    let day: i64 = rest[4..6].parse().ok()?;
    let hour: i64 = rest[7..9].parse().ok()?;
    let minute: i64 = rest[10..12].parse().ok()?;
    let second: i64 = rest[13..15].parse().ok()?;
    let millis: i64 = rest[16..19].parse().ok()?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return None,
    };
    if !(1..=days_in_month).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some((((days * 24 + hour) * 60 + minute) * 60 + second) * 1000 + millis)
}
