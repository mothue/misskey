use super::*;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tracing::{
    Event, Metadata, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<BTreeMap<String, String>>>>);

struct Fields(BTreeMap<String, String>);
impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }
}
impl Subscriber for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut fields = Fields(BTreeMap::new());
        event.record(&mut fields);
        self.0.lock().unwrap().push(fields.0);
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

fn uuid_v4(id: &str) {
    assert_eq!(id.len(), 36);
    for (index, byte) in id.bytes().enumerate() {
        if matches!(index, 8 | 13 | 18 | 23) {
            assert_eq!(byte, b'-');
        } else {
            assert!(byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        }
    }
    assert_eq!(id.as_bytes()[14], b'4');
    assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
}

#[tokio::test]
async fn postgres_business_error_preserves_legacy_public_shape_and_correlates_redacted_log() {
    let error = Error::query(QueryError::Server {
        message: "OFFSET must not be negative".into(),
    });
    let Error::Legacy { id, .. } = &error else {
        panic!("legacy error expected")
    };
    let id = id.clone();
    uuid_v4(&id);
    let capture = Capture::default();
    let response = tracing::subscriber::with_default(capture.clone(), || error.response());
    assert_eq!(response.status(), 500);
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"error": {
            "message": "Internal error occurred. Please contact us if the error persists.",
            "code": "INTERNAL_ERROR", "id": "5d37dbcb-891e-41ca-a3d6-e690c97775ac", "kind": "server",
            "info": {"e": {"message": "OFFSET must not be negative", "code": "QueryFailedError", "id": id}}
        }})
    );
    let events = capture.0.lock().unwrap();
    assert_eq!(events.len(), 1);
    let fields = &events[0];
    assert_eq!(
        fields.keys().map(String::as_str).collect::<Vec<_>>(),
        ["error.code", "error.id", "event_name", "message"]
    );
    assert_eq!(fields["error.id"].trim_matches('"'), id);
    assert_eq!(fields["error.code"].trim_matches('"'), "QueryFailedError");
    assert_eq!(
        fields["event_name"].trim_matches('"'),
        "api.endpoint.failed"
    );
    assert_eq!(fields["message"].trim_matches('"'), "endpoint failed");
    assert!(!format!("{fields:?}").contains("OFFSET"));
}

#[test]
fn each_query_failure_has_a_fresh_uuid_and_query_timeout_uses_node_pg_message() {
    let first = Error::query(QueryError::QueryTimeout);
    let second = Error::query(QueryError::QueryTimeout);
    let Error::Legacy {
        message,
        code,
        id: first,
    } = first
    else {
        panic!("legacy error expected")
    };
    let Error::Legacy { id: second, .. } = second else {
        panic!("legacy error expected")
    };
    assert_eq!(message, "Query read timeout");
    assert_eq!(code, "QueryFailedError");
    uuid_v4(&first);
    uuid_v4(&second);
    assert_ne!(first, second);
    assert!(matches!(
        Error::query(QueryError::Unavailable),
        Error::Internal
    ));
}
