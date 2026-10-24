//! serde_json's ordinary Clone and Drop recurse. JSON at the API boundary can
//! be legitimately deep, so its ownership must be iterative too.
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

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    pub fn as_value_mut(&mut self) -> &mut Value {
        &mut self.0
    }

    /// Transfer ownership to a caller that will use iterative cleanup too.
    pub fn into_value(mut self) -> Value {
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
        self.as_value()
    }
}
impl DerefMut for SafeJson {
    fn deref_mut(&mut self) -> &mut Value {
        self.as_value_mut()
    }
}
impl Borrow<Value> for SafeJson {
    fn borrow(&self) -> &Value {
        self.as_value()
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

#[cfg(test)]
mod tests {
    use super::{SafeJson, clone_json, drop_json};
    use serde_json::{Map, Value};

    #[test]
    fn deeply_nested_json_clone_and_drop_keep_every_container() {
        let mut nested = Value::Bool(true);
        for depth in 0..20_000 {
            nested = if depth % 2 == 0 {
                Value::Array(vec![nested])
            } else {
                Value::Object(Map::from_iter([("child".into(), nested)]))
            };
        }
        let original = SafeJson::new(nested);
        let copied = original.clone();
        let raw_copy = clone_json(copied.as_value());
        let mut value = &raw_copy;
        for depth in (0..20_000).rev() {
            value = if depth % 2 == 0 {
                let Value::Array(values) = value else {
                    panic!("expected array at depth {depth}")
                };
                assert_eq!(values.len(), 1);
                &values[0]
            } else {
                let Value::Object(values) = value else {
                    panic!("expected object at depth {depth}")
                };
                assert_eq!(values.len(), 1);
                &values["child"]
            };
        }
        assert_eq!(value.as_bool(), Some(true));
        drop_json(raw_copy);
        drop_json(copied.into_value());
        drop(original);
    }

    #[test]
    fn clone_keeps_property_insertion_order_and_exact_numbers() {
        let mut object = Map::new();
        object.insert("z".into(), Value::from(-0.0));
        object.insert("10".into(), Value::from(9_007_199_254_740_993_u64));
        object.insert("2".into(), Value::Null);
        object.insert("a".into(), Value::String("😀".into()));
        let original = SafeJson::new(Value::Object(object));
        let cloned = original.clone();
        let keys: Vec<_> = cloned
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["z", "10", "2", "a"]);
        assert_eq!(
            cloned["z"].as_f64().unwrap().to_bits(),
            (-0.0_f64).to_bits()
        );
        assert_eq!(cloned["10"].as_u64(), Some(9_007_199_254_740_993));
        assert_eq!(cloned["a"].as_str(), Some("😀"));
    }
}
