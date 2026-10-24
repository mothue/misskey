use misskey_backend_core::{
    Catalog, Endpoint, Property, echo, online_users_count, online_users_cutoff, ping_at,
};
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

#[test]
fn catalog_names_preserves_order_duplicates_and_every_utf16_unit() {
    let names = vec![
        vec![122],
        vec![],
        vec![0xd800],
        vec![0xdc00],
        vec![0xd83d, 0xde00],
        vec![0, 0xffff],
        vec![97],
        vec![122],
    ];
    let catalog = Catalog::new(
        names
            .iter()
            .map(|name| Endpoint {
                name: name.clone(),
                properties: vec![],
            })
            .collect(),
    );

    assert_eq!(catalog.names(), names);
    // Callers receive names without gaining mutable access to the catalog.
    let mut returned = catalog.names();
    returned[0].clear();
    assert_eq!(catalog.names(), names);
}

#[test]
fn empty_catalog_has_no_names() {
    assert!(Catalog::new(vec![]).names().is_empty());
}

#[test]
fn online_cutoff_uses_one_millisecond_clock_and_ten_minutes() {
    assert_eq!(
        online_users_cutoff(UNIX_EPOCH + Duration::from_micros(1_234_567)),
        Ok(-598_766)
    );
    assert_eq!(online_users_cutoff(UNIX_EPOCH), Ok(-600_000));
    assert!(
        online_users_cutoff(UNIX_EPOCH + Duration::from_millis(9_007_199_254_740_992)).is_err()
    );
}

#[test]
fn online_count_keeps_the_existing_js_number_rounding() {
    for (count, number) in [
        (0, 0.0),
        (1, 1.0),
        (9_007_199_254_740_991, 9_007_199_254_740_991.0),
        (9_007_199_254_740_992, 9_007_199_254_740_992.0),
        (9_007_199_254_740_993, 9_007_199_254_740_992.0),
        (i64::MAX, 9_223_372_036_854_775_808.0),
    ] {
        assert_eq!(online_users_count(count), Ok(number));
    }
    assert!(online_users_count(-1).is_err());
}
