//! serde_json's ordinary Clone and Drop recurse. Auth/role data can contain
//! legitimately deep condition trees, so their ownership must be iterative too.
use serde::{Serialize, Serializer};
use serde_json::Value;
use std::{
    borrow::Borrow,
    ops::{Deref, DerefMut},
};

pub struct SafeJson(Value);
impl SafeJson {
    pub fn new(value: Value) -> Self {
        Self(value)
    }
    pub(crate) fn into_value(mut self) -> Value {
        std::mem::replace(&mut self.0, Value::Null)
    }
}
impl Clone for SafeJson {
    fn clone(&self) -> Self {
        Self(clone_json(&self.0))
    }
}
impl Deref for SafeJson {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.0
    }
}
impl DerefMut for SafeJson {
    fn deref_mut(&mut self) -> &mut Value {
        &mut self.0
    }
}
impl Borrow<Value> for SafeJson {
    fn borrow(&self) -> &Value {
        &self.0
    }
}
impl Serialize for SafeJson {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl Drop for SafeJson {
    fn drop(&mut self) {
        drop_json(std::mem::replace(&mut self.0, Value::Null));
    }
}

pub fn drop_json(value: Value) {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(values) => pending.extend(values),
            Value::Object(values) => pending.extend(values.into_values()),
            _ => {}
        }
    }
}

pub fn clone_json(value: &Value) -> Value {
    enum Work<'a> {
        Visit(&'a Value),
        Array(usize),
        Object(Vec<String>),
    }
    let mut pending = vec![Work::Visit(value)];
    let mut values = Vec::new();
    while let Some(work) = pending.pop() {
        match work {
            Work::Visit(value) => match value {
                Value::Array(items) => {
                    pending.push(Work::Array(items.len()));
                    pending.extend(items.iter().rev().map(Work::Visit));
                }
                Value::Object(items) => {
                    pending.push(Work::Object(items.keys().cloned().collect()));
                    pending.extend(items.values().rev().map(Work::Visit));
                }
                _ => values.push(value.clone()),
            },
            Work::Array(len) => {
                let items = values.split_off(values.len() - len);
                values.push(Value::Array(items));
            }
            Work::Object(keys) => {
                let items = values.split_off(values.len() - keys.len());
                values.push(Value::Object(keys.into_iter().zip(items).collect()));
            }
        }
    }
    values.pop().expect("one JSON root")
}
