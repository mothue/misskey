//! Native transport parsing tests. These use neither a database nor Redis.
use axum::{
    body::{Body, to_bytes},
    http::Response,
};
use misskey_api_runtime::{RequestContext, request};
use misskey_api_validation::{Value, utf16};
use serde_json::json;

fn context(method: &str, path: &str, content_type: Option<&str>) -> RequestContext {
    let mut headers = serde_json::Map::new();
    if let Some(content_type) = content_type {
        headers.insert("content-type".into(), json!(content_type));
    }
    serde_json::from_value(json!({"v":1,"requestId":"a".repeat(64),"method":method,"path":path,"ip":"192.0.2.1","headers":headers})).unwrap()
}

async fn error_json(reply: Response<Body>) -> serde_json::Value {
    let bytes = to_bytes(reply.into_body(), 2 * 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

type Part<'a> = (&'a str, Option<&'a str>, Option<&'a str>, &'a [u8]);

fn multipart(parts: &[Part<'_>]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, filename, content_type, bytes) in parts {
        body.extend_from_slice(b"--native-boundary\r\n");
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"").as_bytes(),
        );
        if let Some(filename) = filename {
            body.extend_from_slice(format!("; filename=\"{filename}\"").as_bytes());
        }
        body.extend_from_slice(b"\r\n");
        if let Some(content_type) = content_type {
            body.extend_from_slice(format!("Content-Type: {content_type}\r\n").as_bytes());
        }
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(b"--native-boundary--\r\n");
    body
}

const MULTIPART: &str = "multipart/form-data; boundary=native-boundary";

#[tokio::test]
async fn get_query_preserves_duplicates_empty_values_unicode_and_key_order() {
    let input = request::parse(&context("GET", "/api/test?a=one+two&a=three&10=x&2=y&empty=&bare&utf=%F0%9F%90%88&__proto__=own&bad=%ZZ", None), Body::empty(), false, 64, "/unused").await.unwrap();
    assert_eq!(
        input.params.stringify().unwrap(),
        "{\"2\":\"y\",\"10\":\"x\",\"a\":[\"one two\",\"three\"],\"empty\":\"\",\"bare\":\"\",\"utf\":\"🐈\",\"__proto__\":\"own\",\"bad\":\"%ZZ\"}"
    );
    assert_eq!(
        input.params.get("a"),
        Some(&Value::Array(vec![
            Value::string("one two"),
            Value::string("three")
        ]))
    );
}

#[tokio::test]
async fn json_keeps_numeric_overflow_negative_zero_and_lone_surrogates() {
    let input = request::parse(
        &context("POST", "/api/test", Some("application/json; charset=utf-8")),
        Body::from(r#"{"negative":-0,"big":1e400,"surrogate":"\ud800","unknown":[null,true]}"#),
        false,
        64,
        "/unused",
    )
    .await
    .unwrap();
    assert!(
        input
            .params
            .get("negative")
            .unwrap()
            .as_number()
            .unwrap()
            .is_sign_negative()
    );
    assert_eq!(
        input.params.get("big").unwrap().as_number(),
        Some(f64::INFINITY)
    );
    assert_eq!(
        input.params.get("surrogate"),
        Some(&Value::String(vec![0xd800]))
    );
    assert_eq!(
        input.params.stringify().unwrap(),
        "{\"negative\":0,\"big\":null,\"surrogate\":\"\\ud800\",\"unknown\":[null,true]}"
    );
}

#[tokio::test]
async fn json_parser_does_not_impose_a_new_shallow_nesting_limit() {
    let raw = format!(
        "{{\"extra\":{}0{}}}",
        "[".repeat(12_000),
        "]".repeat(12_000)
    );
    let input = request::parse(
        &context("POST", "/api/ping", Some("application/json")),
        Body::from(raw.clone()),
        false,
        64,
        "/unused",
    )
    .await
    .unwrap();
    assert_eq!(input.params.stringify().unwrap(), raw);
}

#[tokio::test]
async fn json_body_errors_and_framework_limit_are_distinct() {
    for (body, expected) in [
        ("", "FST_ERR_CTP_EMPTY_JSON_BODY"),
        ("[1,]", "FST_ERR_CTP_INVALID_JSON_BODY"),
        ("undefined", "FST_ERR_CTP_INVALID_JSON_BODY"),
    ] {
        let reply = request::parse(
            &context("POST", "/api/test", Some("application/json")),
            Body::from(body),
            false,
            64,
            "/unused",
        )
        .await
        .err()
        .expect("invalid JSON");
        assert_eq!(reply.status(), 400);
        assert_eq!(error_json(*reply).await["code"], expected);
    }
    let body = format!("\"{}\"", "x".repeat(1024 * 1024));
    let reply = request::parse(
        &context("POST", "/api/test", Some("application/json")),
        Body::from(body),
        false,
        64,
        "/unused",
    )
    .await
    .err()
    .expect("body limit");
    assert_eq!(reply.status(), 413);
    assert_eq!(
        error_json(*reply).await["code"],
        "FST_ERR_CTP_BODY_TOO_LARGE"
    );
}

#[tokio::test]
async fn nested_json_prototype_poisoning_is_rejected_before_endpoint_validation() {
    for body in [
        r#"{"__proto__":{}}"#,
        r#"{"x":[{"constructor":{"prototype":{"polluted":true}}}]}"#,
    ] {
        let reply = request::parse(
            &context("POST", "/api/test", Some("application/json")),
            Body::from(body),
            false,
            64,
            "/unused",
        )
        .await
        .err()
        .expect("poisoned JSON");
        assert_eq!(reply.status(), 400);
    }
    let input = request::parse(
        &context("POST", "/api/test", Some("application/json")),
        Body::from(r#"{"constructor":"ordinary value"}"#),
        false,
        64,
        "/unused",
    )
    .await
    .unwrap();
    assert_eq!(
        input.params.get("constructor"),
        Some(&Value::string("ordinary value"))
    );
}

#[tokio::test]
async fn text_plain_and_unsupported_media_do_not_silently_become_objects() {
    let input = request::parse(
        &context("POST", "/api/test", Some("text/plain")),
        Body::from("true"),
        false,
        64,
        "/unused",
    )
    .await
    .unwrap();
    assert_eq!(input.params, Value::string("true"));
    let reply = request::parse(
        &context("POST", "/api/test", Some("application/octet-stream")),
        Body::from("data"),
        false,
        64,
        "/unused",
    )
    .await
    .err()
    .expect("unsupported media type");
    assert_eq!(reply.status(), 415);
    assert_eq!(
        error_json(*reply).await["code"],
        "FST_ERR_CTP_INVALID_MEDIA_TYPE"
    );
}

#[tokio::test]
async fn multipart_file_and_fields_have_one_owner_and_cleanup_on_drop() {
    let directory = tempfile::tempdir().unwrap();
    let body = multipart(&[
        ("before", None, None, b"one"),
        ("file", Some("upload.txt"), Some("text/plain"), b"payload"),
        ("after", None, None, b"two"),
        ("json", None, Some("application/json"), br#"{"a":1}"#),
    ]);
    let input = request::parse(
        &context("POST", "/api/drive/files/create", Some(MULTIPART)),
        Body::from(body),
        true,
        64,
        directory.path().to_str().unwrap(),
    )
    .await
    .unwrap();
    let upload = input.upload.as_ref().expect("file");
    assert_eq!(upload.name.as_deref(), Some("upload.txt"));
    let path = std::path::PathBuf::from(&upload.path);
    assert_eq!(std::fs::read(&path).unwrap(), b"payload");
    assert_eq!(input.params.get("before"), Some(&Value::string("one")));
    assert_eq!(input.params.get("after"), Some(&Value::string("two")));
    assert_eq!(
        input.params.get("json"),
        Some(&Value::object([("a", Value::Number(1.))]))
    );
    assert_eq!(input.params.get("file"), Some(&Value::Undefined));
    drop(input);
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn duplicate_multipart_fields_are_undefined_and_scalar_casts_remain_separate() {
    let directory = tempfile::tempdir().unwrap();
    let body = multipart(&[
        ("file", Some("a"), None, b"x"),
        ("required", None, None, b"true"),
        ("required", None, None, b"false"),
    ]);
    let input = request::parse(
        &context("POST", "/api/drive/files/create", Some(MULTIPART)),
        Body::from(body),
        true,
        64,
        directory.path().to_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(input.params.get("required"), Some(&Value::Undefined));
    assert!(
        matches!(&input.params,Value::Object(entries) if entries.iter().any(|(name,_)|name==&utf16("required")))
    );
}

#[tokio::test]
async fn multipart_limits_and_invalid_fields_remove_partial_uploads() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().to_str().unwrap();
    for (parts, limit, status) in [
        (vec![("file", Some("a"), None, b"12345".as_slice())], 4, 413),
        (
            vec![
                ("file", Some("a"), None, b"x".as_slice()),
                ("other", Some("b"), None, b"y".as_slice()),
            ],
            64,
            400,
        ),
        (vec![("field", None, None, b"x".as_slice())], 64, 400),
    ] {
        let reply = request::parse(
            &context("POST", "/api/drive/files/create", Some(MULTIPART)),
            Body::from(multipart(&parts)),
            true,
            limit,
            path,
        )
        .await
        .err()
        .expect("invalid multipart");
        assert_eq!(reply.status(), status);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn cancelled_multipart_reader_drops_its_temporary_file() {
    let directory = tempfile::tempdir().unwrap();
    let partial=b"--native-boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a\"\r\n\r\npayload";
    let stream =
        futures_util::stream::once(async move { Ok::<_, std::io::Error>(partial.as_slice()) });
    let stream = futures_util::StreamExt::chain(stream, futures_util::stream::pending());
    let path = directory.path().to_str().unwrap().to_owned();
    let task = tokio::spawn(async move {
        let context = context("POST", "/api/drive/files/create", Some(MULTIPART));
        request::parse(&context, Body::from_stream(stream), true, 64, &path).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while std::fs::read_dir(directory.path()).unwrap().count() == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("upload was created before cancellation");
    task.abort();
    assert!(task.await.err().expect("cancelled task").is_cancelled());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn multipart_bad_fields_are_omitted_without_rejecting_the_first_file() {
    let directory = tempfile::tempdir().unwrap();
    for after_file in [false, true] {
        let bad_fields: Vec<Part<'_>> = vec![
            ("invalid", None, Some("application/json"), b"{"),
            (
                "poison",
                None,
                Some("application/json"),
                br#"{"__proto__":{}}"#,
            ),
            ("constructor", None, None, b"ordinary field"),
            ("toString", None, None, b"ordinary field"),
        ];
        let file = ("file", Some("a"), None, b"payload".as_slice());
        let mut parts = bad_fields;
        if after_file {
            parts.insert(0, file)
        } else {
            parts.push(file)
        }
        let input = request::parse(
            &context("POST", "/api/drive/files/create", Some(MULTIPART)),
            Body::from(multipart(&parts)),
            true,
            64,
            directory.path().to_str().unwrap(),
        )
        .await
        .unwrap();
        for key in ["invalid", "poison", "constructor", "toString"] {
            assert!(input.params.get(key).is_none(), "{key}")
        }
        assert!(input.upload.is_some());
    }
}

#[tokio::test]
async fn multipart_json_prefix_and_truncated_json_follow_the_plugin_contract() {
    let directory = tempfile::tempdir().unwrap();
    let mut truncated_json = vec![b' '; 1024 * 1024 + 1];
    truncated_json[0] = b'0';
    let parts = [
        (
            "prefix",
            None,
            Some("application/json-patch+json"),
            br#"{"a":1}"#.as_slice(),
        ),
        (
            "truncated",
            None,
            Some("application/json"),
            truncated_json.as_slice(),
        ),
        ("file", Some("a"), None, b"payload".as_slice()),
    ];
    let input = request::parse(
        &context("POST", "/api/drive/files/create", Some(MULTIPART)),
        Body::from(multipart(&parts)),
        true,
        64,
        directory.path().to_str().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        input.params.get("prefix"),
        Some(&Value::object([("a", Value::Number(1.))]))
    );
    assert!(input.params.get("truncated").is_none());
}

#[tokio::test]
async fn prototype_file_names_fail_and_unnamed_binary_files_remain_files() {
    let directory = tempfile::tempdir().unwrap();
    let input = request::parse(
        &context("POST", "/api/drive/files/create", Some(MULTIPART)),
        Body::from(multipart(&[(
            "file",
            None,
            Some("application/octet-stream"),
            b"x",
        )])),
        true,
        64,
        directory.path().to_str().unwrap(),
    )
    .await
    .unwrap();
    assert!(input.upload.as_ref().unwrap().name.is_none());
    drop(input);
    for name in ["constructor", "__proto__", "toString"] {
        let reply = request::parse(
            &context("POST", "/api/drive/files/create", Some(MULTIPART)),
            Body::from(multipart(&[(name, Some("a"), None, b"x")])),
            true,
            64,
            directory.path().to_str().unwrap(),
        )
        .await
        .err()
        .expect("prototype file");
        assert_eq!(reply.status(), 400);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn completed_empty_file_keeps_the_baselines_late_limit_behavior() {
    let directory = tempfile::tempdir().unwrap();
    let input = request::parse(
        &context("POST", "/api/drive/files/create", Some(MULTIPART)),
        Body::from(multipart(&[
            ("file", Some("a"), None, b""),
            ("second", Some("b"), None, b"x"),
        ])),
        true,
        64,
        directory.path().to_str().unwrap(),
    )
    .await
    .unwrap();
    assert!(input.upload.is_some());
    assert_eq!(
        std::fs::read(input.upload.as_ref().unwrap().path.as_str()).unwrap(),
        b""
    );
}

#[tokio::test]
async fn completed_file_does_not_wait_for_late_multipart_fields() {
    let directory = tempfile::tempdir().unwrap();
    let partial=b"--native-boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a\"\r\n\r\npayload\r\n--native-boundary\r\nContent-Disposition: form-data; name=\"next\"\r\n\r\n";
    let stream =
        futures_util::stream::once(async move { Ok::<_, std::io::Error>(partial.as_slice()) });
    let stream = futures_util::StreamExt::chain(stream, futures_util::stream::pending());
    let context = context("POST", "/api/drive/files/create", Some(MULTIPART));
    let input = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        request::parse(
            &context,
            Body::from_stream(stream),
            true,
            64,
            directory.path().to_str().unwrap(),
        ),
    )
    .await
    .expect("first file completed")
    .unwrap();
    assert!(input.params.get("next").is_none());
    let path = input.upload.as_ref().unwrap().path.clone();
    assert_eq!(std::fs::read(&path).unwrap(), b"payload");
    drop(input);
    assert!(!std::path::Path::new(&path).exists());
}

#[tokio::test]
async fn body_media_types_are_exact_after_case_and_parameter_normalization() {
    for content_type in [
        "application/jsonx",
        "application/json-patch+json",
        "text/plainx",
    ] {
        let reply = request::parse(
            &context("POST", "/api/test", Some(content_type)),
            Body::from("{}"),
            false,
            64,
            "/unused",
        )
        .await
        .err()
        .expect("unsupported media type");
        assert_eq!(reply.status(), 415);
    }
    let input = request::parse(
        &context("POST", "/api/test", Some("Application/JSON; charset=utf-8")),
        Body::from("{}"),
        false,
        64,
        "/unused",
    )
    .await
    .unwrap();
    assert_eq!(input.params, Value::Object(Vec::new()));
}

#[tokio::test]
async fn trace_ignores_json_body_even_when_framed_and_delete_parses_without_framing() {
    for body in ["", "{", "{}"] {
        let trace = serde_json::from_value(json!({"v":1,"requestId":"a".repeat(64),"method":"TRACE","path":"/api/test","ip":"192.0.2.1","headers":{"content-type":"application/json","content-length":body.len().to_string()}})).unwrap();
        let input = request::parse(&trace, Body::from(body), false, 64, "/unused")
            .await
            .unwrap();
        assert_eq!(input.params, Value::Undefined);
    }
    for (body, code) in [
        ("", "FST_ERR_CTP_EMPTY_JSON_BODY"),
        ("{", "FST_ERR_CTP_INVALID_JSON_BODY"),
    ] {
        let reply = request::parse(
            &context("DELETE", "/api/test", Some("application/json")),
            Body::from(body),
            false,
            64,
            "/unused",
        )
        .await
        .err()
        .expect("DELETE runs its content type parser without CL or TE");
        assert_eq!(reply.status(), 400);
        assert_eq!(error_json(*reply).await["code"], code);
    }
    let input = request::parse(
        &context("DELETE", "/api/test", Some("application/json")),
        Body::from("{}"),
        false,
        64,
        "/unused",
    )
    .await
    .unwrap();
    assert_eq!(input.params, Value::Object(Vec::new()));
}

#[tokio::test]
async fn query_requires_content_type_and_framed_body_before_json_parsing() {
    for (content_type, length, transfer, body, expected) in [
        (
            None,
            Some("2"),
            None,
            "{}",
            "FST_ERR_ROUTE_MISSING_CONTENT_TYPE",
        ),
        (
            Some("application/json"),
            None,
            None,
            "",
            "FST_ERR_ROUTE_MISSING_CONTENT",
        ),
        (
            Some("application/json"),
            Some("0"),
            None,
            "",
            "FST_ERR_ROUTE_MISSING_CONTENT",
        ),
        (
            Some("application/json"),
            Some("1"),
            None,
            "{",
            "FST_ERR_CTP_INVALID_JSON_BODY",
        ),
        (
            Some("application/json"),
            None,
            Some("chunked"),
            "",
            "FST_ERR_CTP_EMPTY_JSON_BODY",
        ),
    ] {
        let mut headers = serde_json::Map::new();
        if let Some(content_type) = content_type {
            headers.insert("content-type".into(), json!(content_type));
        }
        if let Some(length) = length {
            headers.insert("content-length".into(), json!(length));
        }
        if let Some(transfer) = transfer {
            headers.insert("transfer-encoding".into(), json!(transfer));
        }
        let context = serde_json::from_value(json!({"v":1,"requestId":"a".repeat(64),"method":"QUERY","path":"/api/test","ip":"192.0.2.1","headers":headers})).unwrap();
        let reply = request::parse(&context, Body::from(body), false, 64, "/unused")
            .await
            .err()
            .expect("QUERY enforces body preconditions");
        assert_eq!(reply.status(), 400);
        let actual = error_json(*reply).await;
        assert_eq!(actual["code"], expected);
        if expected == "FST_ERR_ROUTE_MISSING_CONTENT" {
            assert_eq!(
                actual["message"],
                "Method 'QUERY' must provide a request body."
            );
        }
        if expected == "FST_ERR_ROUTE_MISSING_CONTENT_TYPE" {
            assert_eq!(
                actual["message"],
                "Method 'QUERY' must provide a 'Content-Type' header."
            );
        }
    }
    for framing in [
        json!({"content-length":"2"}),
        json!({"transfer-encoding":"chunked"}),
    ] {
        let mut headers = framing.as_object().unwrap().clone();
        headers.insert("content-type".into(), json!("application/json"));
        let context = serde_json::from_value(json!({"v":1,"requestId":"a".repeat(64),"method":"QUERY","path":"/api/test","ip":"192.0.2.1","headers":headers})).unwrap();
        let input = request::parse(&context, Body::from("{}"), false, 64, "/unused")
            .await
            .unwrap();
        assert_eq!(input.params, Value::Object(Vec::new()));
    }
}
