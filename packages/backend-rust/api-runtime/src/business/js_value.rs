//! Shared JS primitive conversion used by legacy payload arithmetic.
use super::Error;
use misskey_api_validation::Value;

pub(super) fn primitive(source: &Value) -> Result<Value, Error> {
    match source {
        Value::Object(_) => {
            if source.get("toString").is_some() {
                // Preserve the pinned Bun runtime's actual TypeError message.
                return Err(Error::legacy("No default value".into(), "TypeError"));
            }
            Ok(Value::string("[object Object]"))
        }
        Value::Array(_) => {
            enum Task<'a> {
                Value(&'a Value),
                Comma,
            }
            let mut tasks = vec![Task::Value(source)];
            let mut units = Vec::new();
            while let Some(task) = tasks.pop() {
                match task {
                    Task::Comma => units.push(u16::from(b',')),
                    Task::Value(Value::Array(items)) => {
                        for (index, item) in items.iter().enumerate().rev() {
                            tasks.push(Task::Value(item));
                            if index != 0 {
                                tasks.push(Task::Comma);
                            }
                        }
                    }
                    Task::Value(Value::Null | Value::Undefined) => {}
                    Task::Value(value) => units.extend(primitive_text(&primitive(value)?)),
                }
            }
            Ok(Value::String(units))
        }
        _ => Ok(source.clone()),
    }
}

pub(super) fn primitive_text(value: &Value) -> Vec<u16> {
    match value {
        Value::String(units) => return units.clone(),
        Value::Undefined => "undefined".into(),
        Value::Null => "null".into(),
        Value::Boolean(true) => "true".into(),
        Value::Boolean(false) => "false".into(),
        Value::Number(number) if *number == 0. => "0".into(),
        Value::Number(number) => ryu_js::Buffer::new().format(*number).to_owned(),
        // Callers pass only the result of primitive(), never a raw container.
        Value::Array(_) | Value::Object(_) => unreachable!("primitive value"),
    }
    .encode_utf16()
    .collect()
}

pub(super) fn number(value: &Value) -> Result<f64, Error> {
    match value {
        Value::Undefined => Ok(f64::NAN),
        Value::Null => Ok(0.),
        Value::Boolean(value) => Ok(if *value { 1. } else { 0. }),
        Value::Number(value) => Ok(*value),
        Value::String(units) => {
            let Ok(text) = String::from_utf16(units) else {
                return Ok(f64::NAN);
            };
            Ok(number_text(&text))
        }
        _ => number(&primitive(value)?),
    }
}

fn number_text(text: &str) -> f64 {
    let text = text.trim_matches(super::notes::js_whitespace);
    if text.is_empty() {
        return 0.;
    }
    match text {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    for (prefix, radix) in [
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
            let mut result = 0.;
            for digit in digits.chars() {
                let Some(digit) = digit.to_digit(radix) else {
                    return f64::NAN;
                };
                result = result * f64::from(radix) + f64::from(digit);
            }
            return result;
        }
    }
    let bytes = text.as_bytes();
    let mut at = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
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

pub(super) fn add(left: &Value, right: &Value) -> Result<Value, Error> {
    let left = primitive(left)?;
    let right = primitive(right)?;
    if matches!(left, Value::String(_)) || matches!(right, Value::String(_)) {
        let mut text = primitive_text(&left);
        text.extend(primitive_text(&right));
        Ok(Value::String(text))
    } else {
        Ok(Value::Number(number(&left)? + number(&right)?))
    }
}
