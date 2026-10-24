use misskey_api_validation::Value;

#[test]
fn many_distinct_members_and_duplicate_numeric_keys_preserve_json_semantics() {
    // Near the public body limit. This also exercises the private tagged decode
    // path; neither path should linearly scan all previous keys on insertion.
    let mut source = String::from("{");
    for index in 0..70_000 {
        if index != 0 {
            source.push(',');
        }
        source.push_str(&format!("\"k{index}\":{index}"));
    }
    source.push_str(",\"2\":1,\"1\":2,\"2\":3}");
    let value = Value::parse_json(&source).unwrap();
    assert_eq!(value.get("k69999").and_then(Value::as_number), Some(69999.));
    let text = value.stringify().unwrap();
    assert!(text.starts_with("{\"1\":2,\"2\":3,\"k0\":0,"));
    let wire = serde_json::to_string(&value).unwrap();
    let restored: Value = serde_json::from_str(&wire).unwrap();
    assert_eq!(restored.stringify(), Some(text));
}
