use super::*;
use misskey_backend_core::{Endpoint as CatalogEndpoint, Property};

fn definitions() -> Vec<EndpointDefinition> {
    serde_json::from_value(json!([
        {"name":"z\u{0}😀", "meta":{}, "params":{
            "type":"object", "properties":{
                "later":{"type":"integer"},
                "fallback":{},
                "😀\u{0}":{"type":"string"}
            },
            "allOf":[{"properties":{"nested":{"type":"boolean"}}}]
        }},
        {"name":"a", "meta":{}, "params":{
            "anyOf":[{"properties":{"nested":{"type":"number"}}}]
        }}
    ]))
    .unwrap()
}

fn catalog() -> Catalog {
    Catalog::new(vec![
        CatalogEndpoint {
            name: "z\0😀".encode_utf16().collect(),
            properties: vec![
                Property {
                    name: "later".encode_utf16().collect(),
                    kind: Some("integer".into()),
                },
                Property {
                    name: "fallback".encode_utf16().collect(),
                    kind: None,
                },
                Property {
                    name: "😀\0".encode_utf16().collect(),
                    kind: Some("string".into()),
                },
            ],
        },
        CatalogEndpoint {
            name: vec![97],
            properties: vec![],
        },
    ])
}

#[test]
fn schema_view_matches_ordered_utf16_catalog_without_expanding_compositions() {
    let catalog = catalog();
    assert!(validate_catalog(&definitions(), &catalog).is_ok());
    assert_eq!(
        catalog.lookup(&"z\0😀".encode_utf16().collect::<Vec<_>>()),
        Some(vec![
            ("later".encode_utf16().collect(), "Integer".into()),
            ("fallback".encode_utf16().collect(), "string".into()),
            ("😀\0".encode_utf16().collect(), "String".into()),
        ])
    );
    assert!(validate_catalog(&[], &Catalog::new(vec![])).is_ok());
}

#[test]
fn schema_view_rejects_every_endpoint_and_property_mismatch() {
    let catalog = catalog();
    for case in [
        "missing endpoint",
        "extra endpoint",
        "endpoint order",
        "endpoint name",
        "missing property",
        "extra property",
        "property order",
        "property name",
        "property type",
        "absent type becomes explicit string",
        "explicit type becomes absent",
        "non-string type",
        "invalid properties",
    ] {
        let mut definitions = definitions();
        match case {
            "missing endpoint" => {
                definitions.pop();
            }
            "extra endpoint" => definitions.push(EndpointDefinition {
                name: "extra".into(),
                meta: json!({}),
                params: json!({}),
            }),
            "endpoint order" => definitions.swap(0, 1),
            "endpoint name" => definitions[0].name = "z\0�".into(),
            "missing property" => {
                definitions[0].params["properties"]
                    .as_object_mut()
                    .unwrap()
                    .remove("later");
            }
            "extra property" => {
                definitions[0].params["properties"]["extra"] = json!({});
            }
            "property order" => {
                definitions[0].params["properties"] = json!({
                    "fallback":{}, "later":{"type":"integer"}, "😀\u{0}":{"type":"string"}
                });
            }
            "property name" => {
                definitions[0].params["properties"] = json!({
                    "later":{"type":"integer"}, "fallback":{}, "�\u{0}":{"type":"string"}
                });
            }
            "property type" => {
                definitions[0].params["properties"]["later"]["type"] = json!("number");
            }
            "absent type becomes explicit string" => {
                definitions[0].params["properties"]["fallback"]["type"] = json!("string");
            }
            "explicit type becomes absent" => {
                definitions[0].params["properties"]["😀\u{0}"] = json!({});
            }
            "non-string type" => {
                definitions[0].params["properties"]["fallback"]["type"] = json!(["string"]);
            }
            "invalid properties" => {
                definitions[0].params["properties"] = json!([]);
            }
            _ => unreachable!(),
        }
        let failure = validate_catalog(&definitions, &catalog).expect_err(case);
        assert_eq!(failure.stage(), FailureStage::EndpointRegistry, "{case}");
    }
}

#[tokio::test]
async fn mismatching_api_catalog_fails_before_auth_store_or_redis_startup() {
    let config: ApiBootstrap = serde_json::from_value(json!({
        "callback":{"url":"http://127.0.0.1:1/execute", "token":"a".repeat(64)},
        "endpoints":[{"name":"endpoint", "meta":{}, "params":{"type":"object"}}],
        "maxFileSize":1, "enableIpRateLimit":false, "environment":"test",
        "uploadDirectory":"/unused", "host":"must-not-connect.invalid", "postgresPrimary":null,
        "redis":{"host":"must-not-connect.invalid", "port":1, "db":0, "keyPrefix":"", "connectTimeoutMs":1},
        "redisPubsub":{"host":"must-not-connect.invalid", "port":1, "db":0, "keyPrefix":"", "connectTimeoutMs":1},
        "auth":{"idMethod":"aidx", "maxFileSize":1, "defaultPolicies":{}, "meta":{}, "entityLayouts":{}}
    }))
    .unwrap();
    let result = Runtime::new(
        config,
        Arc::new(Catalog::new(vec![])),
        Arc::new(OnlineUsers::new(None)),
    )
    .await;
    match result {
        Err(failure) => assert_eq!(failure.stage(), FailureStage::EndpointRegistry),
        Ok(_) => panic!("inconsistent registry became ready"),
    }
}
