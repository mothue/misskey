use misskey_api_validation::{Schema, Value, utf16};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize)]
struct Batch {
    name: String,
    schema: serde_json::Value,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
struct Case {
    id: String,
    input: Value,
    expected: serde_json::Value,
}

#[test]
fn captured_ajv_first_errors_and_default_mutations() {
    let batches: Vec<Batch> = serde_json::from_str(include_str!("fixtures/ajv.json")).unwrap();
    let mut count = 0;
    for batch in batches {
        let schema = Schema::compile(batch.schema).unwrap();
        for case in batch.cases {
            let mut value = case.input;
            let result = schema.validate(&mut value);
            let actual = json!({"valid":result.is_ok(),"error":result.err(),"value":value});
            assert_eq!(actual, case.expected, "{} case {}", batch.name, case.id);
            count += 1;
        }
    }
    assert_eq!(count, 125);
}

#[test]
fn captured_ajv_primitive_uniqueness_preserves_indices_and_inherited_keys() {
    let batches: Vec<Batch> =
        serde_json::from_str(include_str!("fixtures/unique-items-ajv.json")).unwrap();
    let mut count = 0;
    for batch in batches {
        let schema = Schema::compile(batch.schema).unwrap();
        for case in batch.cases {
            let mut value = case.input;
            let result = schema.validate(&mut value);
            let actual = json!({"valid":result.is_ok(),"error":result.err(),"value":value});
            assert_eq!(actual, case.expected, "{} case {}", batch.name, case.id);
            count += 1;
        }
    }
    assert_eq!(count, 272);
}

#[test]
fn large_unique_id_array_and_distant_duplicate_keep_the_same_contract() {
    // These APIs have no maxItems bound. This correctness case exercises an
    // ordinary request-sized array without asserting a duration or benchmark.
    let schema = Schema::compile(json!({
        "type":"array","uniqueItems":true,"items":{"type":"string","format":"misskey:id"}
    }))
    .unwrap();
    let mut values = Value::Array(
        (0..50_000)
            .map(|i| Value::string(&format!("user{i}")))
            .collect(),
    );
    schema.validate(&mut values).unwrap();
    if let Value::Array(values) = &mut values {
        values.push(Value::string("user0"));
    }
    let error = schema.validate(&mut values).unwrap_err();
    assert_eq!(error.schema_path, "#/uniqueItems");
    assert_eq!(
        error.message,
        "must NOT have duplicate items (items ## 50000 and 0 are identical)"
    );
}

#[test]
fn query_conversion_is_json_parse_then_validation() {
    let schema = Schema::compile(json!({"type":"object","properties":{
        "a":{"type":"integer"},"b":{"type":"boolean"},"string":{"type":"string"}
    }}))
    .unwrap();
    for literal in ["null", "true", "[]", "{}", "1e400", "-0", "1.5", "\"x\""] {
        let mut value = Value::object([
            ("a", Value::string(literal)),
            ("string", Value::string("1")),
        ]);
        schema.cast_query(&mut value).unwrap();
        assert_eq!(value.get("a"), Some(&Value::parse_json(literal).unwrap()));
        assert_eq!(value.get("string"), Some(&Value::string("1")));
    }
    for literal in ["", "undefined", "1x", "01", "+1", "NaN", "Infinity"] {
        let mut value = Value::object([("a", Value::string(literal))]);
        let error = schema.cast_query(&mut value).unwrap_err();
        assert_eq!(error.param, "a");
        assert_eq!(error.reason, "cannot cast to integer");
    }
    let mut value = Value::object([("a", Value::String(vec![0x22, 0xd800, 0x22]))]);
    schema.cast_query(&mut value).unwrap();
    assert_eq!(value.get("a"), Some(&Value::String(vec![0xd800])));
    assert_eq!(
        schema.validate(&mut value).unwrap_err().schema_path,
        "#/properties/a/type"
    );
}

#[test]
fn json_parse_and_stringify_preserve_javascript_values() {
    let value = Value::parse_json(r#"{"b":1,"10":2,"2":3,"b":4,"__proto__":{"x":1},"surrogates":"\ud800x\udfff","pair":"\ud83d\ude00","overflow":1e400,"negative":-0}"#).unwrap();
    assert_eq!(
        value.stringify().unwrap(),
        "{\"2\":3,\"10\":2,\"b\":4,\"__proto__\":{\"x\":1},\"surrogates\":\"\\ud800x\\udfff\",\"pair\":\"😀\",\"overflow\":null,\"negative\":0}"
    );
    assert!(
        value
            .get("negative")
            .unwrap()
            .as_number()
            .unwrap()
            .is_sign_negative()
    );
    assert_eq!(
        value.get("overflow").unwrap().as_number(),
        Some(f64::INFINITY)
    );
    assert_eq!(Value::Undefined.stringify(), None);
    assert_eq!(
        Value::object([
            ("missing", Value::Undefined),
            (
                "array",
                Value::Array(vec![Value::Undefined, Value::Number(f64::NAN)])
            )
        ])
        .stringify()
        .unwrap(),
        "{\"array\":[null,null]}"
    );
    assert_eq!(Value::Number(1e21).stringify().unwrap(), "1e+21");
    assert_eq!(Value::Number(1e-6).stringify().unwrap(), "0.000001");
    assert_eq!(Value::Number(1e-7).stringify().unwrap(), "1e-7");
}

#[test]
fn malformed_json_is_rejected_without_coercion() {
    for invalid in [
        "",
        " ",
        "NaN",
        "Infinity",
        "+1",
        "01",
        "1.",
        ".1",
        "1e",
        "--1",
        "nullx",
        "[1,]",
        "{\"a\":1,}",
        "{a:1}",
        "\"a\nb\"",
        "\"\\x20\"",
        "\"\\u123\"",
        "\u{feff}null",
    ] {
        assert!(Value::parse_json(invalid).is_err(), "{invalid:?}");
    }
}

#[test]
fn own_property_order_and_default_clone_are_stable() {
    let schema = Schema::compile(json!({"type":"object","properties":{
        "b":{"type":"object","default":{"x":[1]}},"1":{"type":"integer","default":2}
    }}))
    .unwrap();
    let mut a = Value::object([("a", Value::Boolean(true))]);
    let mut b = Value::Object(Vec::new());
    schema.validate(&mut a).unwrap();
    schema.validate(&mut b).unwrap();
    a.get_mut("b").unwrap().set("x", Value::Null);
    assert_eq!(
        a.stringify().unwrap(),
        "{\"1\":2,\"a\":true,\"b\":{\"x\":null}}"
    );
    assert_eq!(b.stringify().unwrap(), "{\"1\":2,\"b\":{\"x\":[1]}}");
    let mut object = Value::Object(Vec::new());
    object.set("4294967295", Value::Null);
    object.set("02", Value::Null);
    object.set("2", Value::Null);
    object.set("0", Value::Null);
    assert_eq!(
        object.stringify().unwrap(),
        "{\"0\":null,\"2\":null,\"4294967295\":null,\"02\":null}"
    );
}

#[test]
fn opaque_deep_json_has_no_recursive_parser_clone_equality_or_drop() {
    let raw = format!("{}0{}", "[".repeat(12_000), "]".repeat(12_000));
    let value = Value::parse_json(&raw).unwrap();
    let cloned = value.clone();
    assert!(value == cloned);
    assert_eq!(value.stringify().unwrap(), raw);
    drop(cloned);
    drop(value);
}

#[test]
fn deep_wire_round_trip_uses_stack_growing_serde_and_iterative_cleanup() {
    let raw = format!("{}0{}", "[".repeat(12_000), "]".repeat(12_000));
    let value = Value::parse_json(&raw).unwrap();
    let mut wire = Vec::new();
    let mut serializer = serde_json::Serializer::new(&mut wire);
    value
        .serialize(serde_stacker::Serializer::new(&mut serializer))
        .unwrap();
    let mut deserializer = serde_json::Deserializer::from_slice(&wire);
    deserializer.disable_recursion_limit();
    let decoded = Value::deserialize(serde_stacker::Deserializer::new(&mut deserializer)).unwrap();
    deserializer.end().unwrap();
    assert!(value == decoded);
    assert_eq!(decoded.stringify().unwrap(), raw);
}

#[test]
fn wire_frames_preserve_nonfinite_numbers_and_reject_malformed_tags() {
    for value in [
        Value::Number(-0.0),
        Value::Number(f64::INFINITY),
        Value::Number(f64::NEG_INFINITY),
        Value::Number(f64::NAN),
        Value::String(vec![0xd800]),
        Value::Object(vec![(utf16("__proto__"), Value::Null)]),
    ] {
        let encoded = serde_json::to_string(&value).unwrap();
        let decoded: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value, decoded);
        if let Value::Number(number) = &value
            && *number == 0.0
        {
            assert!(decoded.as_number().unwrap().is_sign_negative())
        }
    }
    for invalid in [
        r#"{"t":"null","v":null}"#,
        r#"{"t":"undefined","v":null}"#,
        r#"{"t":"unknown"}"#,
        r#"{"t":"string","v":[65536]}"#,
        r#"{"t":"number","v":1}"#,
        r#"{"t":"number","v":"+1"}"#,
        r#"{"t":"boolean","v":0}"#,
        r#"{"t":"null","extra":0}"#,
        r#"{"t":"object","v":[[[97],{"t":"null"}],[[97],{"t":"null"}]]}"#,
    ] {
        assert!(serde_json::from_str::<Value>(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn unsupported_schema_semantics_fail_at_compile_time() {
    for unsupported in [
        json!({"$ref":"#/definitions/X"}),
        json!({"type":"string","format":"date-time"}),
        json!({"type":"string","pattern":"["}),
        json!({"patternProperties":{"a":{}}}),
        json!({"type":"any"}),
    ] {
        assert!(Schema::compile(unsupported).is_err());
    }
}
