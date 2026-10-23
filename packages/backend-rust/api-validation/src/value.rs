use serde::{Deserialize, Serialize};

pub fn utf16(value: &str) -> Vec<u16> {
    value.encode_utf16().collect()
}

/// The transport representation intentionally does not use JSON numbers or
/// strings for their JavaScript counterparts.
#[derive(Debug, Serialize)]
#[serde(
    tag = "t",
    content = "v",
    rename_all = "lowercase",
    deny_unknown_fields
)]
pub enum Value {
    Undefined,
    Null,
    Boolean(bool),
    Number(#[serde(with = "number")] f64),
    String(Vec<u16>),
    Array(Vec<Value>),
    Object(Vec<(Vec<u16>, Value)>),
}

impl Clone for Value {
    fn clone(&self) -> Self {
        enum Task<'a> {
            Visit(&'a Value),
            Array(usize),
            Object(Vec<Vec<u16>>),
        }
        let mut tasks = vec![Task::Visit(self)];
        let mut results = Vec::new();
        while let Some(task) = tasks.pop() {
            match task {
                Task::Visit(value) => match value {
                    Self::Undefined => results.push(Self::Undefined),
                    Self::Null => results.push(Self::Null),
                    Self::Boolean(v) => results.push(Self::Boolean(*v)),
                    Self::Number(v) => results.push(Self::Number(*v)),
                    Self::String(v) => results.push(Self::String(v.clone())),
                    Self::Array(values) => {
                        tasks.push(Task::Array(values.len()));
                        for value in values.iter().rev() {
                            tasks.push(Task::Visit(value))
                        }
                    }
                    Self::Object(values) => {
                        tasks.push(Task::Object(
                            values.iter().map(|(key, _)| key.clone()).collect(),
                        ));
                        for (_, value) in values.iter().rev() {
                            tasks.push(Task::Visit(value))
                        }
                    }
                },
                Task::Array(len) => {
                    let values = results.split_off(results.len() - len);
                    results.push(Self::Array(values));
                }
                Task::Object(keys) => {
                    let values = results.split_off(results.len() - keys.len());
                    results.push(Self::Object(keys.into_iter().zip(values).collect()));
                }
            }
        }
        results.pop().expect("clone root")
    }
}

impl Drop for Value {
    fn drop(&mut self) {
        fn drain(value: &mut Value, pending: &mut Vec<Value>) {
            match value {
                Value::Array(values) => pending.append(values),
                Value::Object(values) => {
                    for (_, value) in std::mem::take(values) {
                        pending.push(value)
                    }
                }
                _ => {}
            }
        }
        let mut pending = Vec::new();
        drain(self, &mut pending);
        while let Some(mut value) = pending.pop() {
            drain(&mut value, &mut pending)
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        let mut pending = vec![(self, other)];
        while let Some((left, right)) = pending.pop() {
            match (left, right) {
                (Self::Undefined, Self::Undefined) | (Self::Null, Self::Null) => {}
                (Self::Boolean(a), Self::Boolean(b)) if a == b => {}
                (Self::Number(a), Self::Number(b)) if a == b || (a.is_nan() && b.is_nan()) => {}
                (Self::String(a), Self::String(b)) if a == b => {}
                (Self::Array(a), Self::Array(b)) if a.len() == b.len() => {
                    pending.extend(a.iter().zip(b))
                }
                (Self::Object(a), Self::Object(b)) if a.len() == b.len() => {
                    for (key, value) in a {
                        let Some((_, other)) = b.iter().find(|(other, _)| other == key) else {
                            return false;
                        };
                        pending.push((value, other));
                    }
                }
                _ => return false,
            }
        }
        true
    }
}

// serde's adjacent tagged unit variants accept a spurious `v:null`. Decode
// the complete envelope explicitly, including presence, duplicate keys and
// unknown fields. Iterative conversion also avoids another recursive Value
// deserialization pass after the JSON transport has been parsed.
impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        decode_wire(serde_json::Value::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

struct Raw(serde_json::Value);
impl Drop for Raw {
    fn drop(&mut self) {
        let mut pending = vec![std::mem::replace(&mut self.0, serde_json::Value::Null)];
        while let Some(value) = pending.pop() {
            match value {
                serde_json::Value::Array(values) => pending.extend(values),
                serde_json::Value::Object(values) => pending.extend(values.into_values()),
                _ => {}
            }
        }
    }
}

fn wire_units(value: &serde_json::Value) -> Result<Vec<u16>, String> {
    value
        .as_array()
        .ok_or("invalid UTF16 array")?
        .iter()
        .map(|unit| {
            unit.as_u64()
                .and_then(|unit| u16::try_from(unit).ok())
                .ok_or_else(|| "invalid UTF16 unit".into())
        })
        .collect()
}

fn decode_wire(root: serde_json::Value) -> Result<Value, String> {
    enum Task {
        Value(Raw),
        Array(usize),
        Object(Vec<Vec<u16>>),
    }
    let mut tasks = vec![Task::Value(Raw(root))];
    let mut results = Vec::new();
    while let Some(task) = tasks.pop() {
        match task {
            Task::Array(len) => {
                let values = results.split_off(results.len() - len);
                results.push(Value::Array(values));
            }
            Task::Object(keys) => {
                let values = results.split_off(results.len() - keys.len());
                results.push(Value::ordered_object(
                    keys.into_iter().zip(values).collect(),
                ));
            }
            Task::Value(mut raw) => {
                let fields = raw.0.as_object_mut().ok_or("invalid wire envelope")?;
                let tag = Raw(fields.remove("t").ok_or("missing wire tag")?);
                let content = fields.remove("v");
                let has_content = content.is_some();
                let mut content = Raw(content.unwrap_or(serde_json::Value::Null));
                if !fields.is_empty() {
                    return Err("unknown wire field".into());
                }
                match tag.0.as_str().ok_or("invalid wire tag")? {
                    "undefined" | "null" => {
                        if has_content {
                            return Err("unexpected wire content".into());
                        }
                        results.push(if tag.0.as_str() == Some("null") {
                            Value::Null
                        } else {
                            Value::Undefined
                        });
                    }
                    "boolean" => results.push(Value::Boolean(
                        content.0.as_bool().ok_or("invalid wire boolean")?,
                    )),
                    "number" => {
                        let number = content.0.as_str().ok_or("invalid wire number")?;
                        let value = match number {
                            "NaN" => f64::NAN,
                            "Infinity" | "+Infinity" => f64::INFINITY,
                            "-Infinity" => f64::NEG_INFINITY,
                            _ => {
                                if number.trim() != number {
                                    return Err("invalid wire number".into());
                                }
                                match Value::parse_json(number)
                                    .map_err(|_| "invalid wire number")?
                                {
                                    Value::Number(ref number) => *number,
                                    _ => return Err("invalid wire number".into()),
                                }
                            }
                        };
                        results.push(Value::Number(value));
                    }
                    "string" => results.push(Value::String(wire_units(&content.0)?)),
                    "array" => {
                        let values =
                            std::mem::take(content.0.as_array_mut().ok_or("invalid wire array")?);
                        tasks.push(Task::Array(values.len()));
                        for value in values.into_iter().rev() {
                            tasks.push(Task::Value(Raw(value)))
                        }
                    }
                    "object" => {
                        let entries =
                            std::mem::take(content.0.as_array_mut().ok_or("invalid wire object")?);
                        let entries: Vec<Raw> = entries.into_iter().map(Raw).collect();
                        let mut keys = Vec::new();
                        let mut values = Vec::new();
                        let mut seen = std::collections::HashSet::new();
                        for mut entry in entries {
                            let pair = entry.0.as_array_mut().ok_or("invalid wire property")?;
                            if pair.len() != 2 {
                                return Err("invalid wire property".into());
                            }
                            let key = wire_units(&pair[0])?;
                            if !seen.insert(key.clone()) {
                                return Err("duplicate wire object key".into());
                            }
                            keys.push(key);
                            values.push(Raw(pair.pop().expect("value")));
                        }
                        tasks.push(Task::Object(keys));
                        for value in values.into_iter().rev() {
                            tasks.push(Task::Value(value))
                        }
                    }
                    _ => return Err("unknown wire tag".into()),
                }
            }
        }
    }
    results.pop().ok_or_else(|| "missing wire value".into())
}

impl Value {
    /// Keys are unique and in insertion order; only array-index keys move.
    pub(crate) fn ordered_object(mut entries: Vec<(Vec<u16>, Self)>) -> Self {
        entries.sort_by_cached_key(|(key, _)| array_index(key).map_or((1, 0), |index| (0, index)));
        Self::Object(entries)
    }
    pub fn string(value: &str) -> Self {
        Self::String(utf16(value))
    }

    pub fn object(values: impl IntoIterator<Item = (impl AsRef<str>, Value)>) -> Self {
        let mut result = Self::Object(Vec::new());
        for (key, value) in values {
            result.set(key.as_ref(), value);
        }
        result
    }

    pub fn get(&self, key: &str) -> Option<&Self> {
        self.get_units(&utf16(key))
    }

    pub fn get_units(&self, key: &[u16]) -> Option<&Self> {
        match self {
            Self::Object(values) => values
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Self> {
        let key = utf16(key);
        match self {
            Self::Object(values) => values
                .iter_mut()
                .find(|(name, _)| name == &key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    pub fn set(&mut self, key: &str, value: Self) {
        self.set_units(utf16(key), value);
    }

    /// As for a JavaScript own property, replacing a value keeps its original
    /// insertion position and an array-index key enumerates before other keys.
    pub fn set_units(&mut self, key: Vec<u16>, value: Self) {
        let Self::Object(values) = self else { return };
        if let Some((_, current)) = values.iter_mut().find(|(name, _)| name == &key) {
            *current = value;
            return;
        }
        if let Some(index) = array_index(&key) {
            let at = values
                .iter()
                .position(|(other, _)| array_index(other).is_none_or(|i| i > index))
                .unwrap_or(values.len());
            values.insert(at, (key, value));
        } else {
            values.push((key, value));
        }
    }

    pub fn as_str(&self) -> Option<String> {
        match self {
            Self::String(value) => String::from_utf16(value).ok(),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    pub fn truthy(&self) -> bool {
        match self {
            Self::Undefined | Self::Null => false,
            Self::Boolean(v) => *v,
            Self::Number(v) => *v != 0.0 && !v.is_nan(),
            Self::String(v) => !v.is_empty(),
            Self::Array(_) | Self::Object(_) => true,
        }
    }

    pub fn from_json(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(value) => Self::Boolean(*value),
            serde_json::Value::Number(value) => Self::Number(value.as_f64().expect("JSON number")),
            serde_json::Value::String(value) => Self::string(value),
            serde_json::Value::Array(values) => {
                Self::Array(values.iter().map(Self::from_json).collect())
            }
            serde_json::Value::Object(values) => Self::ordered_object(
                values
                    .iter()
                    .map(|(key, value)| (utf16(key), Self::from_json(value)))
                    .collect(),
            ),
        }
    }

    /// JSON.stringify-compatible output. Object undefined members are omitted;
    /// array undefined values/nonfinite numbers become null. Root undefined is
    /// represented by None, matching JSON.stringify(undefined).
    pub fn to_json_string(&self) -> Option<String> {
        self.stringify()
    }

    pub fn stringify(&self) -> Option<String> {
        if matches!(self, Self::Undefined) {
            return None;
        }
        let mut output = String::new();
        stringify(self, &mut output);
        Some(output)
    }
}

pub(crate) fn number_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value == f64::INFINITY {
        return "Infinity".into();
    }
    if value == f64::NEG_INFINITY {
        return "-Infinity".into();
    }
    if value == 0.0 && value.is_sign_negative() {
        return "-0".into();
    }
    ryu_js::Buffer::new().format(value).to_owned()
}

fn array_index(units: &[u16]) -> Option<u32> {
    if units.is_empty() || (units.len() > 1 && units[0] == u16::from(b'0')) {
        return None;
    }
    let mut value = 0_u32;
    for &unit in units {
        if !(u16::from(b'0')..=u16::from(b'9')).contains(&unit) {
            return None;
        }
        value = value
            .checked_mul(10)?
            .checked_add(u32::from(unit - u16::from(b'0')))?;
    }
    (value != u32::MAX).then_some(value)
}

fn stringify(value: &Value, output: &mut String) {
    enum Task<'a> {
        Value(&'a Value),
        Char(char),
        Key(&'a [u16]),
    }
    let mut tasks = vec![Task::Value(value)];
    while let Some(task) = tasks.pop() {
        match task {
            Task::Char(c) => output.push(c),
            Task::Key(key) => quote(key, output),
            Task::Value(value) => match value {
                Value::Undefined | Value::Null => output.push_str("null"),
                Value::Boolean(value) => output.push_str(if *value { "true" } else { "false" }),
                Value::Number(value) => {
                    if !value.is_finite() {
                        output.push_str("null")
                    } else if *value == 0.0 {
                        output.push('0')
                    } else {
                        output.push_str(&number_string(*value))
                    }
                }
                Value::String(value) => quote(value, output),
                Value::Array(values) => {
                    output.push('[');
                    tasks.push(Task::Char(']'));
                    for (i, value) in values.iter().enumerate().rev() {
                        tasks.push(Task::Value(value));
                        if i != 0 {
                            tasks.push(Task::Char(','))
                        }
                    }
                }
                Value::Object(values) => {
                    output.push('{');
                    tasks.push(Task::Char('}'));
                    let mut last = true;
                    for (key, value) in values.iter().rev() {
                        if matches!(value, Value::Undefined) {
                            continue;
                        }
                        if !last {
                            tasks.push(Task::Char(','))
                        }
                        last = false;
                        tasks.push(Task::Value(value));
                        tasks.push(Task::Char(':'));
                        tasks.push(Task::Key(key));
                    }
                }
            },
        }
    }
}

fn quote(value: &[u16], output: &mut String) {
    use std::fmt::Write;
    output.push('"');
    let mut i = 0;
    while i < value.len() {
        let unit = value[i];
        match unit {
            0x22 => output.push_str("\\\""),
            0x5c => output.push_str("\\\\"),
            8 => output.push_str("\\b"),
            9 => output.push_str("\\t"),
            10 => output.push_str("\\n"),
            12 => output.push_str("\\f"),
            13 => output.push_str("\\r"),
            0..=0x1f => {
                let _ = write!(output, "\\u{unit:04x}");
            }
            0xd800..=0xdbff
                if value
                    .get(i + 1)
                    .is_some_and(|next| (0xdc00..=0xdfff).contains(next)) =>
            {
                let next = value[i + 1];
                output.push(
                    char::from_u32(
                        0x10000 + ((u32::from(unit) - 0xd800) << 10) + u32::from(next) - 0xdc00,
                    )
                    .expect("surrogate pair"),
                );
                i += 1;
            }
            0xd800..=0xdfff => {
                let _ = write!(output, "\\u{unit:04x}");
            }
            _ => output.push(char::from_u32(u32::from(unit)).expect("non-surrogate")),
        }
        i += 1;
    }
    output.push('"');
}

mod number {
    use serde::Serializer;
    pub fn serialize<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&super::number_string(*value))
    }
}
