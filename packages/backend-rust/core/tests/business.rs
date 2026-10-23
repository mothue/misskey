use misskey_backend_core::{Catalog, Endpoint, Property, echo, ping_at};
use std::time::{Duration, UNIX_EPOCH};

#[test]
fn ping_uses_signed_safe_milliseconds() {
    assert_eq!(ping_at(UNIX_EPOCH + Duration::from_millis(1234)), Ok(1234));
    assert_eq!(ping_at(UNIX_EPOCH - Duration::from_millis(1234)), Ok(-1234));
    assert!(ping_at(UNIX_EPOCH + Duration::from_millis(9_007_199_254_740_992)).is_err());
}

#[test]
fn echo_preserves_arbitrary_text_without_parsing() {
    let input = "{\"z\":-0,\"surrogate\":\"\\ud800\"} nonsense";
    assert_eq!(echo(input), input);
}

#[test]
fn catalog_preserves_utf16_order_types_and_first_match() {
    let catalog = Catalog::new(vec![
        Endpoint {
            name: vec![0xd800],
            properties: vec![
                Property {
                    name: vec![98],
                    kind: None,
                },
                Property {
                    name: vec![0xdc00],
                    kind: Some("integer".into()),
                },
                Property {
                    name: vec![97],
                    kind: Some("string".into()),
                },
            ],
        },
        Endpoint {
            name: vec![0xd800],
            properties: vec![],
        },
    ]);
    let props = catalog.lookup(&[0xd800]).unwrap();
    assert_eq!(
        props,
        vec![
            (vec![98], "string".into()),
            (vec![0xdc00], "Integer".into()),
            (vec![97], "String".into())
        ]
    );
    assert_eq!(catalog.lookup(&[]), None);
    assert_eq!(catalog.lookup(&[0xd801]), None);
}
