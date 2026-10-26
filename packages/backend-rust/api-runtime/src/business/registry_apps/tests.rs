use super::*;

fn value(json: &str) -> Value {
    Value::parse_json(json).unwrap()
}

fn result(value: Result<Value, Error>) -> Value {
    value.unwrap_or_else(|_| panic!("unexpected business error"))
}

#[test]
fn registry_projection_preserves_numbers_types_assignment_and_key_order() {
    let rows = vec![
        value(r#"{"key":"z","value":null}"#),
        value(r#"{"key":"10","value":[1]}"#),
        value(r#"{"key":"2","value":9007199254740993}"#),
        value(r#"{"key":"__proto__","value":{"secret":true}}"#),
        value(r#"{"key":"z","value":false}"#),
        value(r#"{"key":"constructor","value":"ordinary"}"#),
        value(r#"{"key":"nested","value":{"__proto__":{"kept":true}}}"#),
    ];
    assert_eq!(
        result(registry_result("i/registry/get-all", &rows))
            .stringify()
            .unwrap(),
        r#"{"2":9007199254740992,"10":[1],"z":false,"constructor":"ordinary","nested":{"__proto__":{"kept":true}}}"#
    );
    assert_eq!(
        result(registry_result("i/registry/keys-with-type", &rows))
            .stringify()
            .unwrap(),
        r#"{"2":"number","10":"array","z":"boolean","constructor":"string","nested":"object"}"#
    );
    assert_eq!(
        result(registry_result("i/registry/keys", &rows))
            .stringify()
            .unwrap(),
        r#"["z","10","2","__proto__","z","constructor","nested"]"#
    );
}

#[test]
fn registry_missing_errors_keep_endpoint_specific_ids_and_null_is_a_value() {
    for (name, id) in [
        ("i/registry/get", "ac3ed68a-62f0-422b-a7bc-d5e09e8f6a6a"),
        (
            "i/registry/get-detail",
            "97a1e8e7-c0f7-47d2-957a-92e61256e01a",
        ),
    ] {
        assert!(
            matches!(registry_result(name, &[]), Err(Error::Api { code: "NO_SUCH_KEY", id: actual, .. }) if actual == id)
        );
    }
    assert_eq!(
        result(registry_result(
            "i/registry/get",
            &[value(r#"{"value":null}"#)]
        )),
        Value::Null
    );
    assert_eq!(
        result(registry_result(
            "i/registry/get-detail",
            &[value(r#"{"value":null,"updatedAt":-1}"#)]
        ))
        .stringify()
        .unwrap(),
        r#"{"updatedAt":"1969-12-31T23:59:59.999Z","value":null}"#
    );
}

#[test]
fn duplicate_proto_assignment_observes_the_replaced_prototype() {
    for first in ["null", r#"{"__proto__":"shadowed"}"#] {
        let items = [
            value(&format!(r#"{{"key":"__proto__","value":{first}}}"#)),
            value(r#"{"key":"__proto__","value":{"now":"own"}}"#),
        ];
        assert_eq!(
            result(registry_result("i/registry/get-all", &items))
                .stringify()
                .unwrap(),
            r#"{"__proto__":{"now":"own"}}"#
        );
        assert_eq!(
            result(registry_result("i/registry/keys-with-type", &items))
                .stringify()
                .unwrap(),
            "{}"
        );
    }
}

#[test]
fn scope_projection_preserves_first_seen_order_and_join_deduplication() {
    let items = vec![
        value(r#"{"domain":null,"scope":["a","b"]}"#),
        value(r#"{"domain":"","scope":[]}"#),
        value(r#"{"domain":null,"scope":["a.b"]}"#),
        value(r#"{"domain":"second","scope":["b","a"]}"#),
        value(r#"{"domain":null,"scope":["b","a"]}"#),
        value(r#"{"domain":"","scope":[null]}"#),
    ];
    assert_eq!(
        result(scope_result(&items)).stringify().unwrap(),
        r#"[{"domain":null,"scopes":[["a","b"],["b","a"]]},{"domain":"","scopes":[[]]},{"domain":"second","scopes":[["b","a"]]}]"#
    );
    assert_eq!(
        joined_scope(&value(r#"[["a",null],["b","c"]]"#))
            .ok()
            .unwrap(),
        "a,.b,c".encode_utf16().collect::<Vec<_>>()
    );
}

#[test]
fn registry_domain_keeps_null_empty_and_token_boundaries() {
    let native = Context {
        user: None,
        token: None,
        access_token_reduced: false,
    };
    assert_eq!(domain(&value("{}"), &native).ok(), Some(None));
    assert_eq!(
        domain(&value(r#"{"domain":null}"#), &native).ok(),
        Some(None)
    );
    assert_eq!(
        domain(&value(r#"{"domain":""}"#), &native).ok(),
        Some(Some(String::new()))
    );
    assert_eq!(
        domain(&value(r#"{"domain":"\ud800"}"#), &native).ok(),
        Some(Some("�".into()))
    );
    let application = Context {
        user: None,
        token: Some(serde_json::json!({"id":"token-id"})),
        access_token_reduced: true,
    };
    assert_eq!(
        domain(&value(r#"{"domain":"ignored"}"#), &application).ok(),
        Some(Some("token-id".into()))
    );
    for invalid in [
        r#"{"domain":false}"#,
        r#"{"domain":1}"#,
        r#"{"domain":[]}"#,
        r#"{"domain":{}}"#,
    ] {
        assert!(domain(&value(invalid), &native).is_err());
    }
}

#[test]
fn token_projection_uses_nullish_app_fallback_and_omits_absent_fields() {
    let native = value(
        r#"{"id":"0000000000000000","name":null,"lastUsedAt":null,"permission":["read:account"],"iconUrl":null,"description":null,"app":null}"#,
    );
    assert_eq!(
        result(token_result(&native, "aidx")).stringify().unwrap(),
        r#"{"id":"0000000000000000","createdAt":"2000-01-01T00:00:00.000Z","permission":["read:account"],"iconUrl":null,"description":null}"#
    );
    let application = value(
        r#"{"id":"0000000000000000","name":"","lastUsedAt":-1,"permission":["ignored"],"iconUrl":"icon","description":null,"app":{"name":"application","description":"fallback","permission":["read:drive"]}}"#,
    );
    assert_eq!(
        result(token_result(&application, "aidx"))
            .stringify()
            .unwrap(),
        r#"{"id":"0000000000000000","name":"","createdAt":"2000-01-01T00:00:00.000Z","lastUsedAt":"1969-12-31T23:59:59.999Z","permission":["read:drive"],"iconUrl":"icon","description":"fallback"}"#
    );
    for invalid in ["null", "true", "1", "[]", "{}"] {
        assert!(token_result(&value(invalid), "aidx").is_err());
    }
}

#[test]
fn pagination_keeps_large_negative_and_zero_js_number_literals() {
    for (offset, literal) in [
        ("-1", "-1"),
        ("-0", "0"),
        ("1e21", "1e+21"),
        ("9007199254740993", "9007199254740992"),
    ] {
        assert_eq!(
            page(&value(&format!(r#"{{"limit":10,"offset":{offset}}}"#)))
                .ok()
                .unwrap(),
            format!(" LIMIT 10 OFFSET {literal}")
        );
    }
    for invalid in ["null", "true", "[]", r#""10""#] {
        assert!(page(&value(&format!(r#"{{"limit":{invalid},"offset":0}}"#))).is_err());
    }
}

#[test]
fn deleted_authorized_app_keeps_typeorm_criteria_and_correlation_id() {
    for (app_id, json) in [
        ("0000000000000000", r#""0000000000000000""#),
        ("a\"b\nc", r#""a\"b\nc""#),
        ("あ😀", r#""あ😀""#),
    ] {
        let Error::Legacy { message, code, id } = missing_authorized_app(app_id) else {
            panic!("expected EntityNotFoundError");
        };
        assert_eq!(code, "EntityNotFoundError");
        assert_eq!(
            message,
            format!(
                "Could not find any entity of type \"MiApp\" matching: {{\n    \"id\": {json}\n}}"
            )
        );
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
    }
}
