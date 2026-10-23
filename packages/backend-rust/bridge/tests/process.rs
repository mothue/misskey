use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
struct Server {
    child: Child,
    port: u16,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn spawn() -> Child {
    Command::new(env!("CARGO_BIN_EXE_misskey-backend-rust"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}
fn bootstrap() -> Value {
    json!({"v":1,"token":TOKEN,"endpoints":[{"nameUtf16":[55296],"properties":[{"nameUtf16":[56320],"type":null},{"nameUtf16":[97],"type":"integer"}]}]})
}
fn start() -> Server {
    start_with(bootstrap())
}
fn start_with(bootstrap: Value) -> Server {
    let mut child = spawn();
    writeln!(child.stdin.as_mut().unwrap(), "{bootstrap}").unwrap();
    let mut frame = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut frame)
        .unwrap();
    let ready: Value = serde_json::from_str(&frame).expect("ready frame");
    assert_eq!(ready["v"], 1);
    Server {
        child,
        port: ready["port"].as_u64().unwrap() as u16,
    }
}
fn request(server: &Server, auth: &str, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(stream,"POST /rpc HTTP/1.1\r\nHost: localhost\r\nAuthorization: {auth}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    (
        headers.split_whitespace().nth(1).unwrap().parse().unwrap(),
        body.to_owned(),
    )
}
fn rpc(server: &Server, value: Value) -> Value {
    let (status, body) = request(server, &format!("Bearer {TOKEN}"), &value.to_string());
    assert_eq!(status, 200, "{body}");
    serde_json::from_str(&body).unwrap()
}
#[test]
fn serves_exact_envelopes_and_utf16_catalog() {
    let server = start();
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let pong = rpc(&server, json!({"v":1,"op":"ping"}));
    assert_eq!(pong["v"], 1);
    assert_eq!(pong["op"], "ping");
    assert!(pong["pong"].as_u64().unwrap() as u128 >= before);
    let opaque = "{\"b\":-0,\"s\":\"\\ud800\",\"a\":1} arbitrary";
    assert_eq!(
        rpc(&server, json!({"v":1,"op":"test","paramsJson":opaque})),
        json!({"v":1,"op":"test","resultJson":opaque})
    );
    assert_eq!(
        rpc(
            &server,
            json!({"v":1,"op":"endpoint","endpointUtf16":[55296]})
        ),
        json!({"v":1,"op":"endpoint","result":{"params":[{"nameUtf16":[56320],"type":"string"},{"nameUtf16":[97],"type":"Integer"}]}})
    );
    assert_eq!(
        rpc(&server, json!({"v":1,"op":"endpoint","endpointUtf16":[]}))["result"],
        Value::Null
    );
    assert_eq!(
        rpc(&server, json!({"v":1,"op":"endpoints"})),
        json!({"v":1,"op":"endpoints","endpointsUtf16":[[55296]]})
    );
}
#[test]
fn endpoints_uses_bootstrapped_names_without_sorting_or_deduplicating() {
    let names = json!([
        [122],
        [],
        [55296],
        [56320],
        [55357, 56832],
        [0, 65535],
        [97],
        [122]
    ]);
    let definitions: Vec<_> = names
        .as_array()
        .unwrap()
        .iter()
        .map(|name| json!({"nameUtf16":name,"properties":[]}))
        .collect();
    let server = start_with(json!({"v":1,"token":TOKEN,"endpoints":definitions}));
    assert_eq!(
        rpc(&server, json!({"v":1,"op":"endpoints"})),
        json!({"v":1,"op":"endpoints","endpointsUtf16":names})
    );
}
#[test]
fn endpoints_returns_an_empty_array_for_an_empty_catalog() {
    let server = start_with(json!({"v":1,"token":TOKEN,"endpoints":[]}));
    assert_eq!(
        rpc(&server, json!({"v":1,"op":"endpoints"})),
        json!({"v":1,"op":"endpoints","endpointsUtf16":[]})
    );
}
#[test]
fn missing_database_configuration_only_disables_the_database_operation() {
    let server = start();
    let (status, body) = request(
        &server,
        &format!("Bearer {TOKEN}"),
        r#"{"v":1,"op":"get-online-users-count"}"#,
    );
    assert_eq!(status, 503);
    assert_eq!(body, "Rust backend unavailable");
    assert_eq!(rpc(&server, json!({"v":1,"op":"ping"}))["op"], "ping");
    assert_eq!(
        rpc(&server, json!({"v":1,"op":"endpoints"}))["endpointsUtf16"],
        json!([[55296]])
    );
}
#[test]
fn rejects_unauthorized_malformed_versions_ops_and_oversize() {
    let server = start();
    assert_eq!(
        request(&server, "Bearer wrong", r#"{"v":1,"op":"ping"}"#).0,
        401
    );
    assert_eq!(
        request(&server, "Bearer wrong", r#"{"v":1,"op":"endpoints"}"#).0,
        401
    );
    for body in [
        "{",
        r#"{"v":2,"op":"ping"}"#,
        r#"{"v":1,"op":"unknown"}"#,
        r#"{"v":1,"op":"test","paramsJson":7}"#,
        r#"{"v":1,"op":"endpoint","endpointUtf16":[65536]}"#,
        r#"{"v":1,"op":"ping","extra":1}"#,
        r#"{"v":2,"op":"endpoints"}"#,
        r#"{"v":"1","op":"endpoints"}"#,
        r#"{"op":"endpoints"}"#,
        r#"{"v":1,"op":"endpoints","endpointUtf16":[]}"#,
        r#"{"v":1,"op":"endpoints","extra":null}"#,
        r#"{"v":2,"op":"get-online-users-count"}"#,
        r#"{"v":1,"op":"get-online-users-count","extra":null}"#,
    ] {
        let (status, body) = request(&server, &format!("Bearer {TOKEN}"), body);
        assert_eq!(status, 400);
        assert!(!body.contains(TOKEN));
    }
    let prefix = r#"{"v":1,"op":"test","paramsJson":""#;
    let suffix = "\"}";
    let exact_limit = format!(
        "{prefix}{}{suffix}",
        "a".repeat(16 * 1024 * 1024 - prefix.len() - suffix.len())
    );
    let (status, response) = request(&server, &format!("Bearer {TOKEN}"), &exact_limit);
    assert_eq!(status, 200);
    assert_eq!(response.len(), 16 * 1024 * 1024);
    let body = format!(
        "{{\"v\":1,\"op\":\"test\",\"paramsJson\":\"{}\"}}",
        "a".repeat(16 * 1024 * 1024)
    );
    assert_eq!(request(&server, &format!("Bearer {TOKEN}"), &body).0, 413);
    assert_eq!(rpc(&server, json!({"v":1,"op":"ping"}))["op"], "ping");
}
fn wait_exit(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "child did not exit");
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn eof_exits_even_when_parent_does_not_send_a_signal() {
    let mut server = start();
    drop(server.child.stdin.take());
    assert!(wait_exit(&mut server.child).success());
}
#[test]
fn invalid_bootstrap_never_becomes_ready() {
    for frame in [
        r#"{"v":2,"token":"bad","endpoints":[]}"#.to_owned(),
        r#"{"v":1,"token":"bad","endpoints":[]}"#.to_owned(),
        format!(
            r#"{{"v":1,"token":"{TOKEN}","endpoints":[{{"nameUtf16":[],"properties":[{{"nameUtf16":[],"type":"unknown"}}]}}]}}"#
        ),
        format!(
            r#"{{"v":1,"token":"{TOKEN}","endpoints":[{{"nameUtf16":[],"properties":[{{"nameUtf16":[],"type":""}}]}}]}}"#
        ),
        format!(
            r#"{{"v":1,"token":"{TOKEN}","endpoints":[{{"nameUtf16":[],"properties":[{{"nameUtf16":[],"type":"日本語"}}]}}]}}"#
        ),
        format!(
            r#"{{"v":1,"token":"{TOKEN}","endpoints":[{{"nameUtf16":[],"properties":[{{"nameUtf16":[]}}]}}]}}"#
        ),
        "{\n".to_owned(),
        "x".repeat(16 * 1024 * 1024 + 1),
    ] {
        let mut child = spawn();
        let _ = writeln!(child.stdin.as_mut().unwrap(), "{frame}");
        drop(child.stdin.take());
        assert!(!wait_exit(&mut child).success());
        let mut output = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        assert!(output.is_empty());
    }
}

#[cfg(unix)]
#[test]
fn termination_signal_exits_with_open_stdin_and_incomplete_request() {
    let mut server = start();
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    write!(stream,"POST /rpc HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: 100\r\n\r\n{{").unwrap();
    thread::sleep(Duration::from_millis(50));
    let result = Command::new("kill")
        .args(["-TERM", &server.child.id().to_string()])
        .status()
        .unwrap();
    assert!(result.success());
    assert!(wait_exit(&mut server.child).success());
}

#[test]
fn eof_allows_in_flight_request_to_finish_before_exit() {
    let mut server = start();
    let body = r#"{"v":1,"op":"ping"}"#;
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(stream, "POST /rpc HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{{", body.len()).unwrap();
    thread::sleep(Duration::from_millis(50));
    drop(server.child.stdin.take());
    thread::sleep(Duration::from_millis(50));
    stream.write_all(&body.as_bytes()[1..]).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(wait_exit(&mut server.child).success());
}

#[test]
fn eof_bounds_shutdown_with_an_incomplete_request() {
    let mut server = start();
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    write!(stream,"POST /rpc HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nContent-Length: 100\r\n\r\n{{").unwrap();
    thread::sleep(Duration::from_millis(50));
    drop(server.child.stdin.take());
    assert!(wait_exit(&mut server.child).success());
}
