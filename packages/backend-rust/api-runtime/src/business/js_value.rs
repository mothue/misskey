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
            return radix_number(digits, radix);
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

/// JavaScriptCore's js*IntegerLiteral retries values >= 2^53 through
/// runtime/ParseInt.h::parseIntOverflow, summing digits from the least significant end.
/// Source: https://github.com/WebKit/WebKit/blob/main/Source/JavaScriptCore/runtime/ParseInt.h
/// Keep that rounding order: an exact integer-to-f64 conversion differs from
/// the pinned Bun runtime for values such as 0xfffffffffffffbff.
fn radix_number(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() {
        return f64::NAN;
    }
    let mut forward = 0.;
    for digit in digits.chars() {
        let Some(digit) = digit.to_digit(radix) else {
            return f64::NAN;
        };
        forward = forward * f64::from(radix) + f64::from(digit);
    }
    const MANTISSA_OVERFLOW_LOWER_BOUND: f64 = 9_007_199_254_740_992.;
    if forward < MANTISSA_OVERFLOW_LOWER_BOUND {
        return forward;
    }
    let mut result = 0.;
    let mut multiplier = 1_f64;
    // All characters have already been validated, including suffixes after the
    // forward accumulator overflowed. Invalid trailing data must remain NaN.
    for digit in digits.chars().rev() {
        let Some(digit) = digit.to_digit(radix) else {
            return f64::NAN;
        };
        if multiplier.is_infinite() {
            if digit != 0 {
                return f64::INFINITY;
            }
        } else {
            result += f64::from(digit) * multiplier;
        }
        multiplier *= f64::from(radix);
    }
    result
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radix_rounding_matches_pinned_bun_number_oracle() {
        // Actual Bun 1.4.4 Number(string) outputs, including the reported birthday
        // month and boundaries where String and BigInt conversion round differently.
        let cases = [
            (
                "string-bigint-rounding-difference",
                "0xfffffffffffffbff",
                0x43f0000000000000_u64,
            ),
            (
                "reported-month",
                "0x03b7a7cc3f68a79d386f",
                0x448dbd3e61fb453d_u64,
            ),
            (
                "0b-tie-even-down",
                "0b100000000000000000000000000000000000000000000000000001",
                0x4340000000000000_u64,
            ),
            (
                "0b-tie-odd-up",
                "0b100000000000000000000000000000000000000000000000000011",
                0x4340000000000002_u64,
            ),
            (
                "0b-carry",
                "0b111111111111111111111111111111111111111111111111111111",
                0x4350000000000000_u64,
            ),
            (
                "0b-sticky",
                "0b10000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000001",
                0x4630000000000001_u64,
            ),
            (
                "0o-tie-even-down",
                "0o400000000000000001",
                0x4340000000000000_u64,
            ),
            (
                "0o-tie-odd-up",
                "0o400000000000000003",
                0x4340000000000002_u64,
            ),
            ("0o-carry", "0o777777777777777777", 0x4350000000000000_u64),
            (
                "0o-sticky",
                "0o2000000000000000004000000000000001",
                0x4630000000000001_u64,
            ),
            (
                "0x-tie-even-down",
                "0x20000000000001",
                0x4340000000000000_u64,
            ),
            ("0x-tie-odd-up", "0x20000000000003", 0x4340000000000002_u64),
            ("0x-carry", "0x3fffffffffffff", 0x4350000000000000_u64),
            (
                "0x-sticky",
                "0x10000000000000800000000001",
                0x4630000000000001_u64,
            ),
            (
                "below-overflow",
                "0xfffffffffffffbffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                0x7ff0000000000000_u64,
            ),
            (
                "at-overflow",
                "0xfffffffffffffc00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
                0x7ff0000000000000_u64,
            ),
            (
                "above-overflow",
                "0xfffffffffffffc00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001",
                0x7ff0000000000000_u64,
            ),
            ("syntax-0x0", "0x0", 0x0000000000000000_u64),
            ("syntax-0b00001", "0b00001", 0x3ff0000000000000_u64),
            ("syntax-0o17", "0o17", 0x402e000000000000_u64),
            ("syntax-+0x1", "+0x1", 0x7ff8000000000000_u64),
            ("syntax--0x1", "-0x1", 0x7ff8000000000000_u64),
            ("syntax-0x", "0x", 0x7ff8000000000000_u64),
            ("syntax-0o8", "0o8", 0x7ff8000000000000_u64),
            ("syntax-0b2", "0b2", 0x7ff8000000000000_u64),
        ];
        for (label, input, expected_bits) in cases {
            let actual = number_text(input);
            let expected = f64::from_bits(expected_bits);
            if expected.is_nan() {
                assert!(actual.is_nan(), "{label}");
            } else {
                assert_eq!(actual.to_bits(), expected_bits, "{label}");
            }
        }
    }

    #[test]
    fn long_radix_inputs_keep_leading_zero_and_invalid_suffix_behavior() {
        // Same Bun oracle; generate long strings at runtime rather than fixtures
        // containing a megabyte of repeated digits.
        let cases = [
            (
                "long-leading-zero",
                "0x",
                "0",
                1048576,
                "20000000000001",
                0x4340000000000000_u64,
            ),
            (
                "long-overflow",
                "0x",
                "f",
                1048576,
                "",
                0x7ff0000000000000_u64,
            ),
            (
                "invalid-after-overflow",
                "0x",
                "f",
                1048576,
                "g",
                0x7ff8000000000000_u64,
            ),
        ];
        for (label, prefix, digit, repeats, suffix, expected_bits) in cases {
            let input = format!("{prefix}{}{suffix}", digit.repeat(repeats));
            let actual = number_text(&input);
            let expected = f64::from_bits(expected_bits);
            if expected.is_nan() {
                assert!(actual.is_nan(), "{label}");
            } else {
                assert_eq!(actual.to_bits(), expected_bits, "{label}");
            }
        }
    }
}
