//! Execution metadata is decoded once at bootstrap, before serving requests.
//! Documentation and response schemas remain the registry's responsibility.
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EndpointMeta {
    #[serde(default)]
    pub(crate) allow_get: bool,
    #[serde(default)]
    pub(crate) require_credential: bool,
    #[serde(default)]
    pub(crate) require_moderator: bool,
    #[serde(default)]
    pub(crate) require_admin: bool,
    #[serde(default)]
    pub(crate) secure: bool,
    #[serde(default)]
    pub(crate) require_file: bool,
    #[serde(default)]
    pub(crate) prohibit_moved: bool,
    pub(crate) cache_sec: Option<f64>,
    pub(crate) kind: Option<String>,
    pub(crate) required_role_policy: Option<String>,
    #[serde(default, deserialize_with = "deserialize_limit")]
    pub(crate) limit: Option<RateLimit>,
}

fn deserialize_limit<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<RateLimit>, D::Error> {
    // serde's struct visitor also accepts positional JSON arrays. Registry
    // metadata is an object contract, so reject arrays at this boundary.
    Option::<serde_json::Value>::deserialize(deserializer)?
        .map(|value| {
            if !value.is_object() {
                return Err(serde::de::Error::custom("rate limit must be an object"));
            }
            serde_json::from_value(value).map_err(serde::de::Error::custom)
        })
        .transpose()
}

impl EndpointMeta {
    pub(crate) fn credential_required(&self) -> bool {
        self.require_credential || self.require_moderator || self.require_admin
    }
}

/// Missing and null limiter members have the same meaning in the Node limiter.
/// Keep numbers as JavaScript doubles: truncating durations, ranks or factors
/// would change Redis command errors and fractional-limit behavior.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RateLimit {
    pub(crate) key: Option<String>,
    pub(crate) duration: Option<f64>,
    pub(crate) max: Option<f64>,
    pub(crate) min_interval: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn defaults_and_documentation_do_not_change_execution_policy() {
        let meta: EndpointMeta = serde_json::from_value(json!({
            "tags": ["notes"],
            "description": "Metadata owned by the endpoint registry",
            "errors": {"sample": {"message": "sample", "code": "SAMPLE", "id": "sample"}},
            "res": {"type": "object", "properties": {}}
        }))
        .unwrap();
        assert!(!meta.allow_get);
        assert!(!meta.require_file);
        assert!(!meta.secure);
        assert!(!meta.prohibit_moved);
        assert!(!meta.credential_required());
        assert!(meta.limit.is_none());
        assert!(meta.cache_sec.is_none());
        assert!(meta.kind.is_none());
        assert!(meta.required_role_policy.is_none());
        for field in ["requireCredential", "requireModerator", "requireAdmin"] {
            let meta: EndpointMeta = serde_json::from_value(json!({field: true})).unwrap();
            assert!(meta.credential_required());
        }
    }

    #[test]
    fn malformed_execution_metadata_is_rejected_before_admission() {
        for field in [
            "allowGet",
            "requireCredential",
            "requireModerator",
            "requireAdmin",
            "secure",
            "requireFile",
            "prohibitMoved",
        ] {
            for value in [Value::Null, json!(1), json!("true"), json!([]), json!({})] {
                assert!(
                    serde_json::from_value::<EndpointMeta>(json!({field: value})).is_err(),
                    "{field}"
                );
            }
        }
        for field in ["kind", "requiredRolePolicy"] {
            for value in [json!(false), json!(1), json!([]), json!({})] {
                assert!(
                    serde_json::from_value::<EndpointMeta>(json!({field: value})).is_err(),
                    "{field}"
                );
            }
        }
        for value in [json!("60"), json!(true), json!([]), json!({})] {
            assert!(serde_json::from_value::<EndpointMeta>(json!({"cacheSec": value})).is_err());
        }
        for value in [
            json!(false),
            json!(1),
            json!("limit"),
            json!([]),
            json!([null, null, null, null]),
        ] {
            assert!(serde_json::from_value::<EndpointMeta>(json!({"limit": value})).is_err());
        }
        for field in ["duration", "max", "minInterval"] {
            assert!(
                serde_json::from_value::<EndpointMeta>(json!({"limit": {field: "1"}})).is_err(),
                "{field}"
            );
        }
        assert!(serde_json::from_value::<EndpointMeta>(json!({"limit": {"key": 1}})).is_err());
        assert!(serde_json::from_value::<EndpointMeta>(json!([])).is_err());
        assert!(serde_json::from_value::<EndpointMeta>(Value::Null).is_err());
    }

    #[test]
    fn nullable_limiter_members_and_double_precision_are_preserved() {
        let meta: EndpointMeta = serde_json::from_value(json!({
            "kind": null, "requiredRolePolicy": null, "cacheSec": null,
            "limit": {"key": null, "duration": null, "max": null, "minInterval": null}
        }))
        .unwrap();
        let limit = meta.limit.unwrap();
        assert!(limit.key.is_none());
        assert!(limit.duration.is_none());
        assert!(limit.max.is_none());
        assert!(limit.min_interval.is_none());
        let meta: EndpointMeta = serde_json::from_value(json!({
            "cacheSec": -0.25,
            "limit": {"key": "", "duration": -0.5, "max": 0.125, "minInterval": 0}
        }))
        .unwrap();
        assert_eq!(meta.cache_sec, Some(-0.25));
        let limit = meta.limit.unwrap();
        assert_eq!(limit.key.as_deref(), Some(""));
        assert_eq!(limit.duration, Some(-0.5));
        assert_eq!(limit.max, Some(0.125));
        assert_eq!(limit.min_interval, Some(0.));
    }

    /// Run against a fresh export of the TypeScript registry rather than
    /// checking in a second, eventually stale endpoint list:
    /// MISSKEY_ENDPOINT_METADATA_JSON=/path/to/export.json cargo test \
    ///   -p misskey-api-runtime registered_endpoint_metadata -- --ignored
    #[test]
    #[ignore = "requires a fresh TypeScript registry export in MISSKEY_ENDPOINT_METADATA_JSON"]
    fn registered_endpoint_metadata_decodes_without_losing_execution_fields() {
        #[derive(Deserialize)]
        struct RegisteredEndpoint {
            name: String,
            meta: Value,
        }
        let path = std::env::var("MISSKEY_ENDPOINT_METADATA_JSON")
            .expect("set MISSKEY_ENDPOINT_METADATA_JSON to [{name, meta}, ...]");
        let endpoints: Vec<RegisteredEndpoint> =
            serde_json::from_slice(&std::fs::read(path).expect("read endpoint metadata export"))
                .expect("decode endpoint registry export");
        assert!(!endpoints.is_empty());
        let mut names = std::collections::HashSet::new();
        let mut flags = [false; 7];
        let mut typed_members = [false; 4];
        for endpoint in endpoints {
            assert!(
                names.insert(endpoint.name.clone()),
                "duplicate endpoint {}",
                endpoint.name
            );
            assert!(
                endpoint.meta.is_object(),
                "metadata object for {}",
                endpoint.name
            );
            let raw = endpoint.meta;
            let meta: EndpointMeta = serde_json::from_value(raw.clone())
                .unwrap_or_else(|error| panic!("{} metadata: {error}", endpoint.name));
            for (index, (key, flag)) in [
                ("allowGet", meta.allow_get),
                ("requireCredential", meta.require_credential),
                ("requireModerator", meta.require_moderator),
                ("requireAdmin", meta.require_admin),
                ("secure", meta.secure),
                ("requireFile", meta.require_file),
                ("prohibitMoved", meta.prohibit_moved),
            ]
            .into_iter()
            .enumerate()
            {
                assert_eq!(
                    flag,
                    raw.get(key).and_then(Value::as_bool) == Some(true),
                    "{} {key}",
                    endpoint.name
                );
                flags[index] |= flag;
            }
            assert_eq!(meta.cache_sec, raw.get("cacheSec").and_then(Value::as_f64));
            assert_eq!(
                meta.kind.as_deref(),
                raw.get("kind").and_then(Value::as_str)
            );
            assert_eq!(
                meta.required_role_policy.as_deref(),
                raw.get("requiredRolePolicy").and_then(Value::as_str)
            );
            typed_members[0] |= meta.cache_sec.is_some();
            typed_members[1] |= meta.kind.is_some();
            typed_members[2] |= meta.required_role_policy.is_some();
            typed_members[3] |= meta.limit.is_some();
            if let Some(limit) = meta.limit {
                let raw = &raw["limit"];
                assert_eq!(limit.key.as_deref(), raw.get("key").and_then(Value::as_str));
                assert_eq!(limit.duration, raw.get("duration").and_then(Value::as_f64));
                assert_eq!(limit.max, raw.get("max").and_then(Value::as_f64));
                assert_eq!(
                    limit.min_interval,
                    raw.get("minInterval").and_then(Value::as_f64)
                );
            }
        }
        assert!(
            flags.into_iter().all(|present| present),
            "registry must cover each execution flag"
        );
        assert!(
            typed_members.into_iter().all(|present| present),
            "registry must cover optional execution members"
        );
        eprintln!(
            "Validated native metadata for {} registered endpoints",
            names.len()
        );
    }
}
