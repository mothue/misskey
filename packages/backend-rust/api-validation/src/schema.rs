use crate::{Value, value::number_string};
use regress::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValidationError {
    pub schema_path: String,
    pub message: String,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.schema_path, self.message)
    }
}
impl std::error::Error for ValidationError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CastError {
    pub param: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileError(pub String);
impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CompileError {}

#[derive(Clone)]
pub struct Schema {
    root: Json,
    patterns: HashMap<String, Regex>,
    id_format: Regex,
}

impl Schema {
    /// Unsupported schema semantics are rejected at startup, rather than
    /// accepting inputs the old AJV implementation would reject.
    pub fn compile(root: Json) -> Result<Self, CompileError> {
        let mut patterns = HashMap::new();
        compile(&root, "#", &mut patterns)?;
        Ok(Self {
            root,
            patterns,
            id_format: Regex::new("^[a-zA-Z0-9]+$").expect("constant regex"),
        })
    }

    pub fn validate(&self, value: &mut Value) -> Result<(), ValidationError> {
        self.validate_at(&self.root, value, "#", false)
    }

    /// The existing GET/multipart conversion is JSON.parse, not conversion to
    /// the declared type. Thus "null", "[]" and "{}" parse successfully and
    /// only fail later validation. Only direct top-level typed properties cast.
    pub fn cast_query(&self, value: &mut Value) -> Result<(), CastError> {
        let Some(properties) = self.root.get("properties").and_then(Json::as_object) else {
            return Ok(());
        };
        for (key, schema) in properties {
            let Some(kind @ ("boolean" | "integer" | "number")) =
                schema.get("type").and_then(Json::as_str)
            else {
                continue;
            };
            let Some(Value::String(units)) = value.get(key) else {
                continue;
            };
            let parsed = Value::parse_json_utf16(units).map_err(|_| ());
            match parsed {
                Ok(parsed) => value.set(key, parsed),
                Err(()) => {
                    return Err(CastError {
                        param: key.clone(),
                        reason: format!("cannot cast to {kind}"),
                    });
                }
            }
        }
        Ok(())
    }

    fn validate_at(
        &self,
        schema: &Json,
        value: &mut Value,
        path: &str,
        composite: bool,
    ) -> Result<(), ValidationError> {
        if schema == &Json::Bool(true) {
            return Ok(());
        }
        if schema == &Json::Bool(false) {
            return Err(error(path, "false schema", "boolean schema is false"));
        }
        let schema = schema.as_object().expect("compiled schema");
        let types = types(schema);
        let delayed = types.len() == 1 && has_group_rules(schema, &types[0]);
        if !types.is_empty() && !delayed && !types.iter().any(|kind| is_type(value, kind)) {
            return Err(type_error(path, schema));
        }

        // AJV's untyped rule group runs before number/string/array/object
        // groups. In particular, allOf and if happen before this object's
        // defaults and required checks.
        if let Some(expected) = schema.get("const")
            && value != &Value::from_json(expected)
        {
            return Err(error(path, "const", "must be equal to constant"));
        }
        if let Some(allowed) = schema.get("enum").and_then(Json::as_array)
            && !allowed
                .iter()
                .any(|expected| value == &Value::from_json(expected))
        {
            return Err(error(
                path,
                "enum",
                "must be equal to one of the allowed values",
            ));
        }
        if let Some(not) = schema.get("not")
            && self
                .validate_at(not, value, &child(path, "not"), true)
                .is_ok()
        {
            return Err(error(path, "not", "must NOT be valid"));
        }
        if let Some(schemas) = schema.get("anyOf").and_then(Json::as_array) {
            let mut first = None;
            let mut valid = false;
            for (i, schema) in schemas.iter().enumerate() {
                match self.validate_at(schema, value, &format!("{path}/anyOf/{i}"), true) {
                    Ok(()) => {
                        valid = true;
                        break;
                    }
                    Err(error) => {
                        if first.is_none() {
                            first = Some(error)
                        }
                    }
                }
            }
            if !valid {
                return Err(
                    first.unwrap_or_else(|| error(path, "anyOf", "must match a schema in anyOf"))
                );
            }
        }
        if let Some(schemas) = schema.get("oneOf").and_then(Json::as_array) {
            let mut first = None;
            let mut passing = 0;
            for (i, schema) in schemas.iter().enumerate() {
                match self.validate_at(schema, value, &format!("{path}/oneOf/{i}"), true) {
                    Ok(()) => {
                        passing += 1;
                        if passing == 2 {
                            break;
                        }
                    }
                    Err(error) => {
                        if first.is_none() {
                            first = Some(error)
                        }
                    }
                }
            }
            if passing != 1 {
                return Err(first.unwrap_or_else(|| {
                    error(path, "oneOf", "must match exactly one schema in oneOf")
                }));
            }
        }
        if let Some(schemas) = schema.get("allOf").and_then(Json::as_array) {
            for (i, schema) in schemas.iter().enumerate() {
                self.validate_at(schema, value, &format!("{path}/allOf/{i}"), composite)?;
            }
        }
        if let Some(condition) = schema.get("if") {
            let matched = self
                .validate_at(condition, value, &child(path, "if"), true)
                .is_ok();
            let keyword = if matched { "then" } else { "else" };
            if let Some(schema) = schema.get(keyword) {
                self.validate_at(schema, value, &child(path, keyword), composite)?
            }
        }

        // Typed rule groups follow AJV's registration order, independent of the
        // source schema object's insertion order.
        if has_group_rules(schema, "number") {
            if let Value::Number(number) = value {
                if number.is_finite() {
                    for (keyword, operator) in [
                        ("maximum", "<="),
                        ("minimum", ">="),
                        ("exclusiveMaximum", "<"),
                        ("exclusiveMinimum", ">"),
                    ] {
                        if let Some(limit) = schema.get(keyword).and_then(Json::as_f64) {
                            let valid = match keyword {
                                "maximum" => *number <= limit,
                                "minimum" => *number >= limit,
                                "exclusiveMaximum" => *number < limit,
                                _ => *number > limit,
                            };
                            if !valid {
                                return Err(error(
                                    path,
                                    keyword,
                                    &format!("must be {operator} {}", number_string(limit)),
                                ));
                            }
                        }
                    }
                    if let Some(divisor) = schema.get("multipleOf").and_then(Json::as_f64)
                        && (*number / divisor).fract() != 0.0
                    {
                        return Err(error(
                            path,
                            "multipleOf",
                            &format!("must be multiple of {}", number_string(divisor)),
                        ));
                    }
                } else if delayed && types[0] == "number" {
                    return Err(type_error(path, schema));
                }
            } else if delayed && types[0] == "number" {
                return Err(type_error(path, schema));
            }
        }
        if has_group_rules(schema, "string") {
            if let Value::String(units) = value {
                let length = unicode_length(units);
                for (keyword, maximum, adjective) in
                    [("maxLength", true, "more"), ("minLength", false, "fewer")]
                {
                    if let Some(limit) = schema.get(keyword).and_then(Json::as_u64)
                        && ((maximum && length > limit) || (!maximum && length < limit))
                    {
                        return Err(error(
                            path,
                            keyword,
                            &format!("must NOT have {adjective} than {limit} characters"),
                        ));
                    }
                }
                if let Some(pattern) = schema.get("pattern").and_then(Json::as_str)
                    && self.patterns[pattern]
                        .find_from_utf16(units, 0)
                        .next()
                        .is_none()
                {
                    return Err(error(
                        path,
                        "pattern",
                        &format!("must match pattern \"{pattern}\""),
                    ));
                }
                if schema.get("format").and_then(Json::as_str) == Some("misskey:id")
                    && self.id_format.find_from_utf16(units, 0).next().is_none()
                {
                    return Err(error(path, "format", "must match format \"misskey:id\""));
                }
            } else if delayed && types[0] == "string" {
                return Err(type_error(path, schema));
            }
        }
        if has_group_rules(schema, "array") {
            if let Value::Array(values) = value {
                if !composite && let Some(items) = schema.get("items").and_then(Json::as_array) {
                    for (i, item) in items.iter().enumerate() {
                        if let Some(default) = item.get("default") {
                            if i >= values.len() {
                                values.resize(i + 1, Value::Undefined)
                            }
                            if matches!(values[i], Value::Undefined) {
                                values[i] = Value::from_json(default)
                            }
                        }
                    }
                }
                for (keyword, maximum, adjective) in
                    [("maxItems", true, "more"), ("minItems", false, "fewer")]
                {
                    if let Some(limit) = schema.get(keyword).and_then(Json::as_u64)
                        && ((maximum && values.len() as u64 > limit)
                            || (!maximum && (values.len() as u64) < limit))
                    {
                        return Err(error(
                            path,
                            keyword,
                            &format!("must NOT have {adjective} than {limit} items"),
                        ));
                    }
                }
                if let Some(items) = schema.get("items") {
                    if let Some(items) = items.as_array() {
                        for (i, (item, value)) in items.iter().zip(values.iter_mut()).enumerate() {
                            self.validate_at(item, value, &format!("{path}/items/{i}"), composite)?;
                        }
                    } else {
                        for value in values.iter_mut() {
                            self.validate_at(items, value, &child(path, "items"), composite)?
                        }
                    }
                }
                if schema.get("uniqueItems") == Some(&Json::Bool(true))
                    && let Some((i, j)) = duplicate(values, schema.get("items"))
                {
                    return Err(error(
                        path,
                        "uniqueItems",
                        &format!(
                            "must NOT have duplicate items (items ## {j} and {i} are identical)"
                        ),
                    ));
                }
            } else if delayed && types[0] == "array" {
                return Err(type_error(path, schema));
            }
        }
        if has_group_rules(schema, "object") {
            if matches!(value, Value::Object(_)) {
                let properties = schema.get("properties").and_then(Json::as_object);
                if !composite && let Some(properties) = properties {
                    for (key, property) in properties {
                        if let Some(default) = property.get("default")
                            && value
                                .get(key)
                                .is_none_or(|value| matches!(value, Value::Undefined))
                        {
                            value.set(key, Value::from_json(default));
                        }
                    }
                }
                let count = match value {
                    Value::Object(values) => values.len() as u64,
                    _ => unreachable!(),
                };
                for (keyword, maximum, adjective) in [
                    ("maxProperties", true, "more"),
                    ("minProperties", false, "fewer"),
                ] {
                    if let Some(limit) = schema.get(keyword).and_then(Json::as_u64)
                        && ((maximum && count > limit) || (!maximum && count < limit))
                    {
                        return Err(error(
                            path,
                            keyword,
                            &format!("must NOT have {adjective} than {limit} properties"),
                        ));
                    }
                }
                if let Some(required) = schema.get("required").and_then(Json::as_array) {
                    for key in required.iter().filter_map(Json::as_str) {
                        if value
                            .get(key)
                            .is_none_or(|value| matches!(value, Value::Undefined))
                        {
                            return Err(error(
                                path,
                                "required",
                                &format!("must have required property '{key}'"),
                            ));
                        }
                    }
                }
                if let Some(additional) = schema.get("additionalProperties")
                    && additional != &Json::Bool(true)
                {
                    let Value::Object(values) = value else {
                        unreachable!()
                    };
                    for (key, value) in values {
                        let known = String::from_utf16(key).ok().is_some_and(|key| {
                            properties.is_some_and(|properties| properties.contains_key(&key))
                        });
                        if !known {
                            self.validate_at(
                                additional,
                                value,
                                &child(path, "additionalProperties"),
                                composite,
                            )
                            .map_err(|error| {
                                if additional == &Json::Bool(false) {
                                    super::schema::error(
                                        path,
                                        "additionalProperties",
                                        "must NOT have additional properties",
                                    )
                                } else {
                                    error
                                }
                            })?
                        }
                    }
                }
                if let Some(properties) = properties {
                    for (key, property) in properties {
                        if let Some(value) = value
                            .get_mut(key)
                            .filter(|value| !matches!(value, Value::Undefined))
                        {
                            self.validate_at(
                                property,
                                value,
                                &format!("{path}/properties/{}", fragment(key)),
                                composite,
                            )?;
                        }
                    }
                }
            } else if delayed && types[0] == "object" {
                return Err(type_error(path, schema));
            }
        }
        Ok(())
    }
}

fn child(path: &str, keyword: &str) -> String {
    format!("{path}/{keyword}")
}
fn error(path: &str, keyword: &str, message: &str) -> ValidationError {
    ValidationError {
        schema_path: child(path, keyword),
        message: message.into(),
    }
}
fn type_error(path: &str, schema: &Map<String, Json>) -> ValidationError {
    let declared = match schema.get("type") {
        Some(Json::String(kind)) => kind.clone(),
        Some(Json::Array(kinds)) => kinds
            .iter()
            .filter_map(Json::as_str)
            .collect::<Vec<_>>()
            .join(","),
        _ => String::new(),
    };
    error(path, "type", &format!("must be {declared}"))
}
fn types(schema: &Map<String, Json>) -> Vec<String> {
    let mut result = match schema.get("type") {
        Some(Json::String(kind)) => vec![kind.clone()],
        Some(Json::Array(types)) => types
            .iter()
            .filter_map(Json::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    };
    if schema.get("nullable") == Some(&Json::Bool(true))
        && !result.iter().any(|kind| kind == "null")
    {
        result.push("null".into())
    }
    result
}
fn is_type(value: &Value, kind: &str) -> bool {
    match (value, kind) {
        (Value::Null, "null")
        | (Value::Boolean(_), "boolean")
        | (Value::String(_), "string")
        | (Value::Array(_), "array")
        | (Value::Object(_), "object") => true,
        (Value::Number(number), "number") => number.is_finite(),
        (Value::Number(number), "integer") => number.is_finite() && number.fract() == 0.0,
        _ => false,
    }
}
fn has_group_rules(schema: &Map<String, Json>, kind: &str) -> bool {
    let keywords: &[&str] = match kind {
        "number" => &[
            "maximum",
            "minimum",
            "exclusiveMaximum",
            "exclusiveMinimum",
            "multipleOf",
        ],
        "string" => &["maxLength", "minLength", "pattern", "format"],
        "array" => &["maxItems", "minItems", "items", "uniqueItems"],
        "object" => &[
            "maxProperties",
            "minProperties",
            "required",
            "additionalProperties",
            "properties",
        ],
        _ => &[],
    };
    keywords.iter().any(|keyword| schema.contains_key(*keyword))
}

fn unicode_length(units: &[u16]) -> u64 {
    let mut count = 0;
    let mut i = 0;
    while i < units.len() {
        if (0xd800..=0xdbff).contains(&units[i])
            && units
                .get(i + 1)
                .is_some_and(|next| (0xdc00..=0xdfff).contains(next))
        {
            i += 1
        }
        count += 1;
        i += 1;
    }
    count
}

fn duplicate(values: &[Value], items: Option<&Json>) -> Option<(usize, usize)> {
    let item_types = items
        .and_then(Json::as_object)
        .map(types)
        .unwrap_or_default();
    let optimized = !item_types.is_empty()
        && !item_types
            .iter()
            .any(|kind| kind == "object" || kind == "array");
    if optimized {
        #[derive(Hash, PartialEq, Eq)]
        enum Primitive<'a> {
            Null,
            Boolean(bool),
            Number(u64),
            String(&'a [u16]),
        }
        let mut indices = HashMap::new();
        for i in (0..values.len()).rev() {
            if !item_types.iter().any(|kind| is_type(&values[i], kind)) {
                continue;
            }
            let key = match &values[i] {
                Value::Null => Primitive::Null,
                Value::Boolean(value) => Primitive::Boolean(*value),
                Value::Number(value) => {
                    // JS property keys stringify either sign of zero as "0".
                    Primitive::Number(if *value == 0.0 { 0 } else { value.to_bits() })
                }
                Value::String(value) => {
                    // AJV uses an ordinary JS object for its index. In the
                    // single-string-type specialization, assigning a numeric
                    // index to __proto__ invokes the inherited setter and is
                    // ignored. Union specializations append '_' to strings,
                    // so their exceptional input is instead "__proto_".
                    const PROTOTYPE: &[u16] = &[95, 95, 112, 114, 111, 116, 111, 95, 95];
                    let ignored = if item_types.len() == 1 {
                        PROTOTYPE
                    } else {
                        &PROTOTYPE[..PROTOTYPE.len() - 1]
                    };
                    if value == ignored {
                        continue;
                    }
                    Primitive::String(value)
                }
                _ => continue,
            };
            if let Some(j) = indices.insert(key, i) {
                return Some((i, j));
            }
        }
    } else {
        for i in (0..values.len()).rev() {
            for j in (0..i).rev() {
                if values[i] == values[j] {
                    return Some((i, j));
                }
            }
        }
    }
    None
}

fn fragment(key: &str) -> String {
    use std::fmt::Write;
    let escaped = key.replace('~', "~0").replace('/', "~1");
    let mut result = String::new();
    for byte in escaped.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            result.push(char::from(byte))
        } else {
            let _ = write!(result, "%{byte:02X}");
        }
    }
    result
}

fn compile(
    schema: &Json,
    path: &str,
    patterns: &mut HashMap<String, Regex>,
) -> Result<(), CompileError> {
    if schema.is_boolean() {
        return Ok(());
    }
    let Some(schema) = schema.as_object() else {
        return Err(CompileError(format!(
            "{path}: schema must be object or boolean"
        )));
    };
    for keyword in schema.keys() {
        if !matches!(
            keyword.as_str(),
            "type"
                | "nullable"
                | "const"
                | "enum"
                | "not"
                | "anyOf"
                | "oneOf"
                | "allOf"
                | "if"
                | "then"
                | "else"
                | "default"
                | "description"
                | "title"
                | "maximum"
                | "minimum"
                | "exclusiveMaximum"
                | "exclusiveMinimum"
                | "multipleOf"
                | "maxLength"
                | "minLength"
                | "pattern"
                | "format"
                | "maxItems"
                | "minItems"
                | "items"
                | "uniqueItems"
                | "maxProperties"
                | "minProperties"
                | "required"
                | "additionalProperties"
                | "properties"
        ) {
            return Err(CompileError(format!(
                "{path}: unsupported keyword {keyword}"
            )));
        }
    }
    for keyword in [
        "minLength",
        "maxLength",
        "minItems",
        "maxItems",
        "minProperties",
        "maxProperties",
    ] {
        if let Some(bound) = schema.get(keyword)
            && bound.as_u64().is_none()
        {
            return Err(CompileError(format!("{path}: invalid {keyword}")));
        }
    }
    for keyword in [
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "multipleOf",
    ] {
        if let Some(bound) = schema.get(keyword)
            && (bound.as_f64().is_none()
                || (keyword == "multipleOf" && bound.as_f64().unwrap_or(0.0) <= 0.0))
        {
            return Err(CompileError(format!("{path}: invalid {keyword}")));
        }
    }
    for keyword in ["nullable", "uniqueItems"] {
        if let Some(value) = schema.get(keyword)
            && !value.is_boolean()
        {
            return Err(CompileError(format!("{path}: invalid {keyword}")));
        }
    }
    if schema.contains_key("nullable") && !schema.contains_key("type") {
        return Err(CompileError(format!("{path}: nullable requires a type")));
    }
    if let Some(required) = schema.get("required") {
        let required = required
            .as_array()
            .ok_or_else(|| CompileError(format!("{path}: invalid required")))?;
        let mut seen = std::collections::HashSet::new();
        if required
            .iter()
            .any(|key| key.as_str().is_none_or(|key| !seen.insert(key)))
        {
            return Err(CompileError(format!("{path}: invalid required")));
        }
    }
    if let Some(values) = schema.get("enum")
        && values.as_array().is_none_or(Vec::is_empty)
    {
        return Err(CompileError(format!("{path}: invalid enum")));
    }
    if let Some(kind) = schema.get("type") {
        let kinds = match kind {
            Json::String(_) => vec![kind],
            Json::Array(kinds) => kinds.iter().collect(),
            _ => return Err(CompileError(format!("{path}: invalid type"))),
        };
        if kinds.is_empty()
            || kinds.iter().any(|kind| {
                !matches!(
                    kind.as_str(),
                    Some("null" | "boolean" | "integer" | "number" | "string" | "object" | "array")
                )
            })
        {
            return Err(CompileError(format!("{path}: unsupported type")));
        }
    }
    if let Some(format) = schema.get("format")
        && format.as_str() != Some("misskey:id")
    {
        return Err(CompileError(format!("{path}: unsupported format")));
    }
    if let Some(pattern) = schema.get("pattern") {
        let pattern = pattern
            .as_str()
            .ok_or_else(|| CompileError(format!("{path}: invalid pattern")))?;
        patterns.entry(pattern.into()).or_insert(
            Regex::with_flags(pattern, "u")
                .map_err(|_| CompileError(format!("{path}: unsupported pattern")))?,
        );
    }
    for (key, value) in schema {
        match key.as_str() {
            "properties" => {
                let properties = value
                    .as_object()
                    .ok_or_else(|| CompileError(format!("{path}: invalid properties")))?;
                for (name, schema) in properties {
                    compile(
                        schema,
                        &format!("{path}/properties/{}", fragment(name)),
                        patterns,
                    )?
                }
            }
            "allOf" | "anyOf" | "oneOf" => {
                let schemas = value
                    .as_array()
                    .ok_or_else(|| CompileError(format!("{path}: invalid {key}")))?;
                if schemas.is_empty() {
                    return Err(CompileError(format!("{path}: invalid {key}")));
                }
                for (i, schema) in schemas.iter().enumerate() {
                    compile(schema, &format!("{path}/{key}/{i}"), patterns)?
                }
            }
            "not" | "if" | "then" | "else" | "additionalProperties" => {
                compile(value, &child(path, key), patterns)?
            }
            "items" => {
                if let Some(schemas) = value.as_array() {
                    for (i, schema) in schemas.iter().enumerate() {
                        compile(schema, &format!("{path}/items/{i}"), patterns)?
                    }
                } else {
                    compile(value, &child(path, key), patterns)?
                }
            }
            _ => {}
        }
    }
    Ok(())
}
