/*
 * SPDX-FileCopyrightText: syuilo and misskey-project
 * SPDX-License-Identifier: AGPL-3.0-only
 */
//! Stored PostgreSQL text arrays and pg-compatible outer IN parameters.
use misskey_api_validation::Value;

#[derive(Debug)]
pub(super) enum TextArrayFailure {
    FieldType,
    ArrayElement,
    Utf16 { source: std::string::FromUtf16Error },
}
/// Validated PostgreSQL text-array data, including multiple dimensions.
/// Each Value clone/drop uses the existing iterative implementation; this type
/// adds no recursive Clone/Drop or unrestricted opaque JSON payload.
pub(super) struct TextArray {
    elements: Vec<Value>,
}
impl TextArray {
    pub(super) fn new(value: &Value) -> Result<Self, TextArrayFailure> {
        let Value::Array(elements) = value else {
            return Err(TextArrayFailure::FieldType);
        };
        let mut pending: Vec<_> = elements.iter().collect();
        while let Some(element) = pending.pop() {
            match element {
                Value::String(_) | Value::Null => {}
                Value::Array(children) => pending.extend(children),
                _ => return Err(TextArrayFailure::ArrayElement),
            }
        }
        Ok(Self {
            elements: elements.clone(),
        })
    }

    pub(super) fn elements(&self) -> &[Value] {
        &self.elements
    }

    pub(super) fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    pub(super) fn packed(&self) -> Value {
        Value::Array(self.elements.clone())
    }

    pub(super) fn pg_array_literal(elements: &[Value]) -> Result<String, TextArrayFailure> {
        // Actual pg/lib/utils.js prepareValue -> arrayString + escapeElement.
        // Generate the same text using an explicit stack instead of recursively
        // traversing a potentially deep public Value. Quotes/backslashes are the
        // only escaped text characters; SQL NULL is unquoted inside the literal.
        enum Emit<'a> {
            Array(&'a [Value]),
            Element(&'a Value),
            Ascii(u8),
        }
        let mut pending = vec![Emit::Array(elements)];
        let mut output = Vec::<u16>::new();
        while let Some(task) = pending.pop() {
            match task {
                Emit::Array(elements) => {
                    output.push(u16::from(b'{'));
                    pending.push(Emit::Ascii(b'}'));
                    for (index, element) in elements.iter().enumerate().rev() {
                        pending.push(Emit::Element(element));
                        if index > 0 {
                            pending.push(Emit::Ascii(b','));
                        }
                    }
                }
                Emit::Ascii(byte) => output.push(u16::from(byte)),
                Emit::Element(Value::Null) => output.extend("NULL".encode_utf16()),
                Emit::Element(Value::String(units)) => {
                    output.push(u16::from(b'"'));
                    for unit in units {
                        if *unit == u16::from(b'"') || *unit == u16::from(b'\\') {
                            output.push(u16::from(b'\\'));
                        }
                        output.push(*unit);
                    }
                    output.push(u16::from(b'"'));
                }
                Emit::Element(Value::Array(children)) => pending.push(Emit::Array(children)),
                Emit::Element(_) => return Err(TextArrayFailure::ArrayElement),
            }
        }
        String::from_utf16(&output).map_err(|source| TextArrayFailure::Utf16 { source })
    }

    pub(super) fn query_ids(&self) -> Result<Vec<Option<String>>, TextArrayFailure> {
        // TypeORM passes each OUTER item as one IN parameter. Scalar text stays
        // text, NULL stays NULL, and an array becomes pg's array-literal string.
        // Keeping [None] distinct from [] preserves the source file query even
        // when the original nonempty array contains only NULL elements.
        self.elements
            .iter()
            .map(|element| match element {
                Value::Null => Ok(None),
                Value::String(units) => String::from_utf16(units)
                    .map(Some)
                    .map_err(|source| TextArrayFailure::Utf16 { source }),
                Value::Array(children) => Self::pg_array_literal(children).map(Some),
                _ => Err(TextArrayFailure::ArrayElement),
            })
            .collect()
    }
}
