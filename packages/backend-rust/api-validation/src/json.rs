use crate::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError;

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid JSON")
    }
}

impl std::error::Error for ParseError {}

impl Value {
    /// JSON.parse's JSON grammar, including escaped lone surrogates and
    /// numeric overflow. serde_json rejects those two valid JavaScript inputs.
    pub fn parse_json(input: &str) -> Result<Self, ParseError> {
        Self::parse_json_utf16(&input.encode_utf16().collect::<Vec<_>>())
    }

    pub fn parse_json_utf16(input: &[u16]) -> Result<Self, ParseError> {
        let mut parser = Parser { input, cursor: 0 };
        let value = parser.value()?;
        parser.space();
        if parser.cursor == input.len() {
            Ok(value)
        } else {
            Err(ParseError)
        }
    }
}

struct Parser<'a> {
    input: &'a [u16],
    cursor: usize,
}

impl Parser<'_> {
    fn byte(&self) -> Option<u8> {
        self.input
            .get(self.cursor)
            .copied()
            .and_then(|unit| u8::try_from(unit).ok())
    }

    fn space(&mut self) {
        while self
            .byte()
            .is_some_and(|c| matches!(c, b' ' | b'\t' | b'\r' | b'\n'))
        {
            self.cursor += 1
        }
    }

    fn expect(&mut self, value: u8) -> Result<(), ParseError> {
        if self.byte() != Some(value) {
            return Err(ParseError);
        }
        self.cursor += 1;
        Ok(())
    }

    fn value(&mut self) -> Result<Value, ParseError> {
        enum Frame {
            Array(Vec<Value>),
            Object(Vec<(Vec<u16>, Value)>, HashMap<Vec<u16>, usize>, Vec<u16>),
        }
        let mut frames = Vec::new();
        loop {
            self.space();
            let mut value = match self.byte() {
                Some(b'n') => self.literal("null", Value::Null)?,
                Some(b't') => self.literal("true", Value::Boolean(true))?,
                Some(b'f') => self.literal("false", Value::Boolean(false))?,
                Some(b'"') => Value::String(self.string()?),
                Some(b'-' | b'0'..=b'9') => self.number()?,
                Some(b'[') => {
                    self.cursor += 1;
                    self.space();
                    if self.byte() == Some(b']') {
                        self.cursor += 1;
                        Value::Array(Vec::new())
                    } else {
                        frames.push(Frame::Array(Vec::new()));
                        continue;
                    }
                }
                Some(b'{') => {
                    self.cursor += 1;
                    self.space();
                    if self.byte() == Some(b'}') {
                        self.cursor += 1;
                        Value::Object(Vec::new())
                    } else {
                        let key = self.string()?;
                        self.space();
                        self.expect(b':')?;
                        frames.push(Frame::Object(Vec::new(), HashMap::new(), key));
                        continue;
                    }
                }
                _ => return Err(ParseError),
            };
            loop {
                self.space();
                let Some(frame) = frames.pop() else {
                    return Ok(value);
                };
                match frame {
                    Frame::Array(mut values) => {
                        values.push(value);
                        if self.byte() == Some(b']') {
                            self.cursor += 1;
                            value = Value::Array(values);
                        } else {
                            self.expect(b',')?;
                            frames.push(Frame::Array(values));
                            break;
                        }
                    }
                    Frame::Object(mut object, mut keys, key) => {
                        // JSON.parse accepts repeated keys: keep the first
                        // insertion position and replace its value. A hash index
                        // avoids quadratic work on a public 1 MiB JSON object.
                        if let Some(&index) = keys.get(&key) {
                            object[index].1 = value;
                        } else {
                            keys.insert(key.clone(), object.len());
                            object.push((key, value));
                        }
                        if self.byte() == Some(b'}') {
                            self.cursor += 1;
                            value = Value::ordered_object(object);
                        } else {
                            self.expect(b',')?;
                            self.space();
                            let key = self.string()?;
                            self.space();
                            self.expect(b':')?;
                            frames.push(Frame::Object(object, keys, key));
                            break;
                        }
                    }
                }
            }
        }
    }

    fn literal(&mut self, literal: &str, result: Value) -> Result<Value, ParseError> {
        if !self.input[self.cursor..].starts_with(&literal.encode_utf16().collect::<Vec<_>>()) {
            return Err(ParseError);
        }
        self.cursor += literal.len();
        Ok(result)
    }

    fn string(&mut self) -> Result<Vec<u16>, ParseError> {
        self.expect(b'"')?;
        let mut result = Vec::new();
        loop {
            match self.input.get(self.cursor).copied().ok_or(ParseError)? {
                0x22 => {
                    self.cursor += 1;
                    return Ok(result);
                }
                0x5c => {
                    self.cursor += 1;
                    let escaped = self.byte().ok_or(ParseError)?;
                    self.cursor += 1;
                    result.push(match escaped {
                        b'"' => u16::from(b'"'),
                        b'\\' => u16::from(b'\\'),
                        b'/' => u16::from(b'/'),
                        b'b' => 8,
                        b'f' => 12,
                        b'n' => 10,
                        b'r' => 13,
                        b't' => 9,
                        b'u' => {
                            let mut unit = 0;
                            for _ in 0..4 {
                                let digit = match self.byte().ok_or(ParseError)? {
                                    b'0'..=b'9' => self.byte().expect("present") - b'0',
                                    b'a'..=b'f' => self.byte().expect("present") - b'a' + 10,
                                    b'A'..=b'F' => self.byte().expect("present") - b'A' + 10,
                                    _ => return Err(ParseError),
                                };
                                unit = unit * 16 + u16::from(digit);
                                self.cursor += 1;
                            }
                            unit
                        }
                        _ => return Err(ParseError),
                    });
                }
                0..=0x1f => return Err(ParseError),
                _ => {
                    result.push(self.input[self.cursor]);
                    self.cursor += 1;
                }
            }
        }
    }

    fn number(&mut self) -> Result<Value, ParseError> {
        let start = self.cursor;
        if self.byte() == Some(b'-') {
            self.cursor += 1
        }
        match self.byte() {
            Some(b'0') => self.cursor += 1,
            Some(b'1'..=b'9') => {
                self.cursor += 1;
                while self.byte().is_some_and(|c| c.is_ascii_digit()) {
                    self.cursor += 1
                }
            }
            _ => return Err(ParseError),
        }
        if self.byte() == Some(b'.') {
            self.cursor += 1;
            let digits = self.cursor;
            while self.byte().is_some_and(|c| c.is_ascii_digit()) {
                self.cursor += 1
            }
            if self.cursor == digits {
                return Err(ParseError);
            }
        }
        if self.byte().is_some_and(|c| matches!(c, b'e' | b'E')) {
            self.cursor += 1;
            if self.byte().is_some_and(|c| matches!(c, b'+' | b'-')) {
                self.cursor += 1
            }
            let digits = self.cursor;
            while self.byte().is_some_and(|c| c.is_ascii_digit()) {
                self.cursor += 1
            }
            if self.cursor == digits {
                return Err(ParseError);
            }
        }
        Ok(Value::Number(
            String::from_utf16(&self.input[start..self.cursor])
                .map_err(|_| ParseError)?
                .parse()
                .map_err(|_| ParseError)?,
        ))
    }
}
