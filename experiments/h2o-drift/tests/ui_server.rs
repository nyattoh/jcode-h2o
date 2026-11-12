#![cfg(feature = "ui")]
//! Loopback launcher and read-only snapshot boundary. No daemon, no network beyond 127.0.0.1,
//! no keys; the server is pointed at a temporary assets directory with fake files.

use h2o_drift_prototype::ui::{Launcher, SnapshotError, StartError, load_snapshot};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

mod support;
use support::TestDir;

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

const ASSETS: [(&str, &str, &str, &str); 4] = [
    ("/", "index.html", "text/html; charset=utf-8", "<!doctype html><p>fake index</p>"),
    ("/app.css", "app.css", "text/css; charset=utf-8", "body{color:#fff}"),
    ("/core.js", "core.js", "text/javascript; charset=utf-8", "var core=1;"),
    ("/app.js", "app.js", "text/javascript; charset=utf-8", "var app=2;"),
];

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

fn assets_dir() -> TestDir {
    let dir = TestDir::new();
    for (_, file, _, body) in ASSETS {
        fs::write(dir.path.join(file), body).unwrap();
    }
    // Decoys that exist on disk but are not routes.
    fs::write(dir.path.join("secret.txt"), "SECRET").unwrap();
    fs::create_dir_all(dir.path.join("ui")).unwrap();
    fs::write(dir.path.join("ui").join("index.html"), "NESTED").unwrap();
    dir
}

fn raw(addr: SocketAddr, request: &[u8]) -> Response {
    let mut stream = TcpStream::connect(addr).expect("connect to launcher");
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.write_all(request).unwrap();
    let mut bytes = Vec::new();
    let _ = stream.read_to_end(&mut bytes);
    parse(&bytes)
}

fn parse(bytes: &[u8]) -> Response {
    let split = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("no response head in {:?}", String::from_utf8_lossy(bytes)));
    let head = std::str::from_utf8(&bytes[..split]).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    Response {
        status,
        headers,
        body: bytes[split + 4..].to_vec(),
    }
}

fn request(addr: SocketAddr, method: &str, target: &str) -> Response {
    let host = format!("127.0.0.1:{}", addr.port());
    raw(
        addr,
        format!("{method} {target} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
    )
}

fn start(assets: &TestDir, snapshot: Option<&Path>) -> Launcher {
    Launcher::start(&assets.path, snapshot, 0).unwrap_or_else(|e| panic!("start failed: {e:?}"))
}

fn assert_security_headers(r: &Response) {
    assert_eq!(r.header("cache-control"), Some("no-store"));
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(r.header("content-security-policy"), Some(CSP));
}

#[test]
fn routes_serve_exact_assets_with_content_types_and_security_headers() {
    let dir = assets_dir();
    let launcher = start(&dir, None);
    for (route, _, content_type, body) in ASSETS {
        let get = request(launcher.addr(), "GET", route);
        assert_eq!(get.status, 200, "GET {route}");
        assert_eq!(get.header("content-type"), Some(content_type), "{route}");
        assert_eq!(get.body, body.as_bytes(), "{route}");
        assert_eq!(get.header("content-length"), Some(body.len().to_string().as_str()));
        assert_security_headers(&get);

        let head = request(launcher.addr(), "HEAD", route);
        assert_eq!(head.status, 200, "HEAD {route}");
        assert!(head.body.is_empty(), "HEAD must have no body: {route}");
        assert_eq!(head.header("content-length"), Some(body.len().to_string().as_str()));
        assert_eq!(head.header("content-type"), Some(content_type));
        assert_security_headers(&head);
    }
}

#[test]
fn binds_loopback_only_and_stops() {
    let dir = assets_dir();
    let launcher = start(&dir, None);
    let addr = launcher.addr();
    assert_eq!(addr.ip().to_string(), "127.0.0.1");
    assert_ne!(addr.port(), 0);
    launcher.stop();
    assert!(
        TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_err(),
        "stopped launcher must refuse connections"
    );
}

#[test]
fn only_exact_routes_are_served_never_a_request_derived_path() {
    let dir = assets_dir();
    let launcher = start(&dir, None);
    let targets = [
        "/../Cargo.toml",
        "/%2e%2e/Cargo.toml",
        "/%2e%2e/",
        "/%2E%2E%2Fsecret.txt",
        "//",
        "//app.css",
        "/ui/index.html",
        "/index.html",
        "/secret.txt",
        "/app.css/",
        "/APP.CSS",
        "/app.css%00",
        "/app.css%2f..%2fsecret.txt",
        "/..\\secret.txt",
        "/\\app.css",
        "/.",
        "/./app.css",
        "/api/snapshot/",
        "/api",
        "*",
        "http://127.0.0.1/app.js",
        "",
    ];
    for target in targets {
        let r = request(launcher.addr(), "GET", target);
        assert!(
            r.status == 404 || r.status == 400,
            "{target:?} must be 404/400, got {}",
            r.status
        );
        assert!(!String::from_utf8_lossy(&r.body).contains("SECRET"), "{target:?}");
        assert!(!String::from_utf8_lossy(&r.body).contains("NESTED"), "{target:?}");
        assert_security_headers(&r);
    }
    // 404 specifically for well-formed but unknown targets.
    for target in ["/ui/index.html", "/index.html", "/secret.txt", "//", "/%2e%2e/"] {
        assert_eq!(request(launcher.addr(), "GET", target).status, 404, "{target}");
    }
}

#[test]
fn route_match_is_exact_on_the_raw_target_so_query_and_fragment_forms_are_404() {
    let dir = assets_dir();
    let snap = write(&dir, "snap.json", serde_json::to_vec(&envelope(vec![])).unwrap());
    let launcher = start(&dir, Some(&snap));
    let targets = [
        "/app.css?x",
        "/app.css?x=1",
        "/app.css?",
        "/app.css?/../secret.txt",
        "/app.js?v=2",
        "/core.js?",
        "/?",
        "/?..",
        "/?../../Cargo.toml",
        "/?/app.js",
        "/api/snapshot?",
        "/api/snapshot?anything",
        "/secret.txt?/app.css",
        "/#",
        "/#x",
        "/app.css#x",
        "/api/snapshot#x",
    ];
    for method in ["GET", "HEAD"] {
        for target in targets {
            let r = request(launcher.addr(), method, target);
            assert_eq!(r.status, 404, "{method} {target}");
            assert!(!String::from_utf8_lossy(&r.body).contains("fake index"), "{method} {target}");
            assert!(!String::from_utf8_lossy(&r.body).contains("body{"), "{method} {target}");
            if method == "HEAD" {
                assert!(r.body.is_empty(), "HEAD {target} must have no body");
            }
            assert_security_headers(&r);
        }
    }
    // The plain routes still work next to the rejected forms.
    assert_eq!(request(launcher.addr(), "GET", "/app.css").status, 200);
    assert_eq!(request(launcher.addr(), "GET", "/api/snapshot").status, 200);
}

#[test]
fn methods_other_than_get_and_head_are_405() {
    let dir = assets_dir();
    let launcher = start(&dir, None);
    for method in ["POST", "PUT", "DELETE", "PATCH", "OPTIONS"] {
        for target in ["/", "/app.js", "/api/snapshot", "/nope"] {
            let r = request(launcher.addr(), method, target);
            assert_eq!(r.status, 405, "{method} {target}");
            assert_eq!(r.header("allow"), Some("GET, HEAD"));
            assert_security_headers(&r);
        }
    }
}

#[test]
fn wrong_or_missing_host_is_403() {
    let dir = assets_dir();
    let launcher = start(&dir, None);
    let port = launcher.addr().port();
    let get = |host: Option<String>| {
        let host = host.map(|h| format!("Host: {h}\r\n")).unwrap_or_default();
        raw(
            launcher.addr(),
            format!("GET / HTTP/1.1\r\n{host}Connection: close\r\n\r\n").as_bytes(),
        )
    };
    for host in [
        "evil.example".to_string(),
        format!("evil.example:{port}"),
        format!("127.0.0.1.evil.example:{port}"),
        format!("127.0.0.1:{}", port.wrapping_add(1)),
        "127.0.0.1".to_string(),
        "localhost".to_string(),
        format!("[::1]:{port}"),
        format!("0.0.0.0:{port}"),
        String::new(),
    ] {
        let r = get(Some(host.clone()));
        assert_eq!(r.status, 403, "Host {host:?}");
        assert_security_headers(&r);
    }
    assert_eq!(get(None).status, 403, "missing Host");
    // POST with a bad Host is still refused as a bad Host first.
    let r = raw(
        launcher.addr(),
        b"POST / HTTP/1.1\r\nHost: evil.example\r\nContent-Length: 0\r\n\r\n",
    );
    assert_eq!(r.status, 403);
    // Both loopback spellings with the real port are accepted.
    for host in [format!("127.0.0.1:{port}"), format!("localhost:{port}")] {
        let r = get(Some(host.clone()));
        assert_eq!(r.status, 200, "Host {host}");
    }
}

#[test]
fn oversized_request_head_is_refused_and_stalled_clients_time_out() {
    let dir = assets_dir();
    let launcher = start(&dir, None);
    let port = launcher.addr().port();
    let mut big = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Pad: ").into_bytes();
    big.extend(std::iter::repeat_n(b'a', 20 * 1024));
    big.extend_from_slice(b"\r\n\r\n");
    assert_eq!(raw(launcher.addr(), &big).status, 431);

    // A client that never finishes its head is dropped, and does not block others.
    let mut stalled = TcpStream::connect(launcher.addr()).unwrap();
    stalled.write_all(b"GET / HTTP/1.1\r\n").unwrap();
    stalled.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    assert_eq!(request(launcher.addr(), "GET", "/").status, 200);
    let started = Instant::now();
    let mut buf = Vec::new();
    let _ = stalled.read_to_end(&mut buf);
    assert!(started.elapsed() < Duration::from_secs(10), "stalled client held open");
    if !buf.is_empty() {
        assert_eq!(parse(&buf).status, 408);
    }
}

/// Request whose head (request line .. final CRLFCRLF, inclusive) is exactly `head_len` bytes.
fn request_with_head_len(method: &str, port: u16, head_len: usize) -> Vec<u8> {
    let base = format!("{method} /not-a-route HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Pad: ");
    let fixed = base.len() + 4; // plus the final CRLFCRLF
    assert!(head_len > fixed);
    let mut req = base.into_bytes();
    req.extend(std::iter::repeat_n(b'a', head_len - fixed));
    req.extend_from_slice(b"\r\n\r\n");
    assert_eq!(req.len(), head_len);
    req
}

#[test]
fn head_limit_is_8192_bytes_measured_at_the_terminator() {
    let dir = assets_dir();
    let launcher = start(&dir, None);
    let port = launcher.addr().port();
    for (len, expected) in [(8000, 404), (8191, 404), (8192, 404), (8193, 431), (8194, 431), (9000, 431)] {
        let r = raw(launcher.addr(), &request_with_head_len("GET", port, len));
        assert_eq!(r.status, expected, "head of {len} bytes");
        assert_security_headers(&r);
    }
    // Body bytes arriving in the same read are not part of the head.
    for (len, expected) in [(8192, 404), (8193, 431)] {
        let mut req = request_with_head_len("GET", port, len);
        req.extend(std::iter::repeat_n(b'b', 600));
        assert_eq!(raw(launcher.addr(), &req).status, expected, "head of {len} bytes plus body");
    }
    // A small head followed by a large body in the same write is fine.
    let mut req = request_with_head_len("GET", port, 100);
    req.extend(std::iter::repeat_n(b'b', 12 * 1024));
    assert_eq!(raw(launcher.addr(), &req).status, 404);
}

#[test]
fn head_requests_never_get_a_body_even_on_read_stage_errors() {
    let dir = assets_dir();
    let launcher = start(&dir, None);
    let port = launcher.addr().port();
    // 431: oversized head, and the terminator in the final chunk.
    for len in [9000, 8193] {
        let r = raw(launcher.addr(), &request_with_head_len("HEAD", port, len));
        assert_eq!(r.status, 431, "{len}");
        assert!(r.body.is_empty(), "HEAD 431 had a body: {:?}", r.body);
        assert_security_headers(&r);
    }
    // 400: non-UTF-8 header value.
    let mut bad = format!("HEAD /not-a-route HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Bad: ").into_bytes();
    bad.extend_from_slice(&[0xff, 0xfe]);
    bad.extend_from_slice(b"\r\n\r\n");
    let r = raw(launcher.addr(), &bad);
    assert_eq!(r.status, 400);
    assert!(r.body.is_empty(), "HEAD 400 had a body: {:?}", r.body);
    assert_security_headers(&r);
    // 400: bad version on a HEAD request line.
    let r = raw(launcher.addr(), format!("HEAD / HTTP/9.9\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes());
    assert_eq!(r.status, 400);
    assert!(r.body.is_empty());
    // GET keeps its body on the same error.
    let mut bad_get = bad.clone();
    bad_get.splice(0..4, *b"GET ");
    let r = raw(launcher.addr(), &bad_get);
    assert_eq!(r.status, 400);
    assert!(!r.body.is_empty());
    assert_eq!(r.header("content-length"), Some(r.body.len().to_string().as_str()));
}

#[test]
fn head_request_that_stalls_gets_408_without_a_body() {
    let dir = assets_dir();
    let launcher = start(&dir, None);
    let mut stalled = TcpStream::connect(launcher.addr()).unwrap();
    // No line ending: a loopback filter on some machines withholds a complete first line from the
    // server until the head ends, which would leave the server nothing to identify as HEAD.
    stalled.write_all(b"HEAD / HTTP/1.1").unwrap();
    stalled.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    let mut buf = Vec::new();
    let _ = stalled.read_to_end(&mut buf);
    let r = parse(&buf);
    assert_eq!(r.status, 408);
    assert!(r.body.is_empty(), "HEAD 408 had a body: {:?}", r.body);
}

/// Requests whose Host headers are not exactly one loopback `Host` for this port.
fn duplicate_host_requests(port: u16, method: &str, target: &str) -> Vec<(&'static str, Vec<u8>)> {
    let good = format!("Host: 127.0.0.1:{port}");
    let other = format!("Host: localhost:{port}");
    let evil = "Host: evil.example".to_string();
    let cases = [
        ("identical twice", vec![good.clone(), good.clone()]),
        ("good then evil", vec![good.clone(), evil.clone()]),
        ("evil then good", vec![evil, good.clone()]),
        ("two allowed spellings", vec![good.clone(), other]),
        ("lowercase twin", vec![good.clone(), good.to_ascii_lowercase()]),
        ("uppercase twin", vec![good.to_ascii_uppercase(), good.clone()]),
        ("three times", vec![good.clone(), good.clone(), good.clone()]),
    ];
    cases
        .into_iter()
        .map(|(name, hosts)| {
            let mut req = format!("{method} {target} HTTP/1.1\r\n");
            for h in hosts {
                req.push_str(&h);
                req.push_str("\r\n");
            }
            req.push_str("Connection: close\r\n\r\n");
            (name, req.into_bytes())
        })
        .collect()
}

/// Some Windows machines run a loopback HTTP filter that silently drops every `Host` header after
/// the first on requests with a standard method (reproduced with a plain Python socket server, so
/// it is not this launcher). Probe a plain listener to learn whether duplicates survive transport.
fn transport_keeps_duplicate_headers() -> bool {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut got = Vec::new();
        let mut chunk = [0u8; 512];
        while !got.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut chunk) {
                Ok(n) if n > 0 => got.extend_from_slice(&chunk[..n]),
                _ => break,
            }
        }
        String::from_utf8_lossy(&got).into_owned()
    });
    let mut client = TcpStream::connect(addr).unwrap();
    client.write_all(b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n").unwrap();
    server.join().unwrap().matches("Host:").count() == 2
}

fn assert_duplicate_hosts_are_403(addr: SocketAddr) {
    // `XGET` is not a standard method, so even a filtering network layer leaves it alone; the Host
    // check runs before method dispatch, so a duplicate must give 403 (a single Host gives 405).
    let mut methods = vec!["XGET"];
    if transport_keeps_duplicate_headers() {
        methods.extend(["GET", "HEAD", "POST"]);
    } else {
        eprintln!("note: loopback filter collapses duplicate Host headers; GET/HEAD/POST variants are covered in-process by ui::tests");
    }
    assert_eq!(request(addr, "XGET", "/").status, 405, "one Host passes the Host gate");
    for method in methods {
        for target in ["/not-a-route", "/", "/api/snapshot"] {
            for (name, req) in duplicate_host_requests(addr.port(), method, target) {
                let r = raw(addr, &req);
                assert_eq!(r.status, 403, "{method} {target}: {name}");
                assert!(!String::from_utf8_lossy(&r.body).contains("fake index"), "{name}");
                if method == "HEAD" {
                    assert!(r.body.is_empty(), "HEAD {target}: {name}");
                }
                assert_security_headers(&r);
            }
        }
    }
    // The same requests with exactly one Host are not refused for their Host.
    assert_eq!(request(addr, "GET", "/not-a-route").status, 404);
    assert_eq!(request(addr, "GET", "/").status, 200);
}

#[test]
fn library_duplicate_host_headers_are_403() {
    let dir = assets_dir();
    let launcher = start(&dir, None);
    assert_duplicate_hosts_are_403(launcher.addr());
}

// ---- snapshot --------------------------------------------------------------------------------

fn goal() -> Value {
    json!({"purpose":"Verify","constraints":[],"success_conditions":["ok"],"owned_paths":[],"unfinished":[]})
}

fn spec(branch: &str) -> Value {
    json!({"run_id":"run-1","branch_id":branch,"owner_session_id":"root","session_id":"worker-a",
           "goal":goal(),"context_ids":[],"max_corrections":1})
}

fn checkpoint(sequence: u64) -> Value {
    json!({"sequence":sequence,
           "input":{"goal":goal(),
                    "observation":{"call_id":"c","tool":"bash","output":"private","error":null},
                    "context":[{"id":"s","bytes":[1,2,3]}]},
           "assessment":{"signal":"Aligned","reason":"fine"},
           "decision":"Continue","action":"Continue"})
}

fn branch(id: &str, sequences: &[u64], receipts: Vec<Value>, reconcile: Value) -> Value {
    json!({"spec":spec(id),
           "checkpoints":sequences.iter().map(|s| checkpoint(*s)).collect::<Vec<_>>(),
           "receipts":receipts,
           "needs_reconciliation":reconcile})
}

fn envelope(branches: Vec<Value>) -> Value {
    json!({"schema":"h2o-snapshot/1","captured_at":"2026-11-12 manual","branches":branches})
}

fn write(dir: &TestDir, name: &str, bytes: impl AsRef<[u8]>) -> std::path::PathBuf {
    let path = dir.path.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

fn rejected(dir: &TestDir, name: &str, bytes: impl AsRef<[u8]>, needle: &str) {
    let path = write(dir, name, bytes);
    let err = match load_snapshot(&path) {
        Ok(_) => panic!("{name}: invalid snapshot was accepted"),
        Err(e) => e,
    };
    let message = err.to_string();
    assert!(message.contains(needle), "{name}: {message:?} lacks {needle:?}");
    match Launcher::start(&dir.path, Some(&path), 0) {
        Err(StartError::Snapshot(_)) => {}
        Err(other) => panic!("{name}: wrong start error {other:?}"),
        Ok(_) => panic!("{name}: launcher started with an invalid snapshot"),
    }
}

#[test]
fn valid_snapshot_is_served_as_the_exact_file_bytes_including_unknown_states() {
    let dir = assets_dir();
    let doc = envelope(vec![
        branch(
            "a",
            &[1, 2, 3],
            vec![
                json!({"sequence":1,"receipt":{"acknowledged":true}}),
                json!({"sequence":2,"receipt":null}),
            ],
            Value::Null,
        ),
        branch("b", &[], vec![], json!(true)),
        branch("c", &[7], vec![], json!(false)),
    ]);
    // Deliberately odd formatting: serving must not re-serialise.
    let mut bytes = b"  \n".to_vec();
    bytes.extend(serde_json::to_vec_pretty(&doc).unwrap());
    bytes.extend_from_slice(b"\n\n");
    let path = write(&dir, "snap.json", &bytes);

    assert!(load_snapshot(&path).unwrap() == bytes, "load_snapshot must return the file bytes unchanged");
    let launcher = start(&dir, Some(&path));
    for method in ["GET", "HEAD"] {
        let r = request(launcher.addr(), method, "/api/snapshot");
        assert_eq!(r.status, 200, "{method}");
        assert_eq!(r.header("content-type"), Some("application/json; charset=utf-8"));
        assert_security_headers(&r);
        if method == "GET" {
            assert!(r.body == bytes, "served bytes differ from the file ({} vs {})", r.body.len(), bytes.len());
            let served: Value = serde_json::from_slice(&r.body).unwrap();
            assert!(served["branches"][0]["receipts"][1]["receipt"].is_null());
            assert!(served["branches"][0]["needs_reconciliation"].is_null());
        } else {
            assert!(r.body.is_empty());
            assert_eq!(r.header("content-length"), Some(bytes.len().to_string().as_str()));
        }
    }
    // The snapshot is read once at startup: later changes to the file are not served.
    fs::write(&path, b"{}").unwrap();
    assert!(request(launcher.addr(), "GET", "/api/snapshot").body == bytes, "snapshot re-read after startup");
}

#[test]
fn snapshot_without_optional_captured_at_is_accepted() {
    let dir = assets_dir();
    let path = write(&dir, "s.json", br#"{"schema":"h2o-snapshot/1","branches":[]}"#);
    assert!(load_snapshot(&path).is_ok());
}

#[test]
fn snapshot_route_is_404_without_a_snapshot() {
    let dir = assets_dir();
    let launcher = start(&dir, None);
    for method in ["GET", "HEAD"] {
        assert_eq!(request(launcher.addr(), method, "/api/snapshot").status, 404, "{method}");
    }
}

#[test]
fn invalid_snapshots_are_rejected_with_specific_messages() {
    let dir = assets_dir();
    let ok = |v: Value| serde_json::to_vec(&v).unwrap();

    let mut wrong = envelope(vec![]);
    wrong["schema"] = json!("h2o-snapshot/2");
    rejected(&dir, "schema.json", ok(wrong), "schema");

    let mut extra = envelope(vec![]);
    extra["extra"] = json!(1);
    rejected(&dir, "top.json", ok(extra), "unknown field");

    let mut extra_branch = branch("a", &[], vec![], json!(false));
    extra_branch["surprise"] = json!(1);
    rejected(&dir, "branch.json", ok(envelope(vec![extra_branch])), "unknown field");

    rejected(
        &dir,
        "dup.json",
        ok(envelope(vec![branch("a", &[1, 2, 1], vec![], json!(false))])),
        "duplicate",
    );
    rejected(
        &dir,
        "orphan.json",
        ok(envelope(vec![branch(
            "a",
            &[1],
            vec![json!({"sequence":9,"receipt":{"acknowledged":true}})],
            json!(false),
        )])),
        "receipt",
    );
    rejected(
        &dir,
        "badreceipt.json",
        ok(envelope(vec![branch(
            "a",
            &[1],
            vec![json!({"sequence":1,"receipt":{"acknowledged":"yes"}})],
            json!(false),
        )])),
        "boolean",
    );
    rejected(
        &dir,
        "badspec.json",
        ok(envelope(vec![json!({"spec":{"run_id":"x"},"checkpoints":[],"receipts":[],"needs_reconciliation":null})])),
        "branch_id",
    );
    rejected(
        &dir,
        "badflag.json",
        ok(envelope(vec![branch("a", &[], vec![], json!("maybe"))])),
        "boolean",
    );
    rejected(&dir, "notobj.json", b"[]", "invalid");

    // JSON object shape is required at every level (serde would also accept sequence form).
    rejected(&dir, "arr-env.json", br#"["h2o-snapshot/1",null,[]]"#, "object");
    rejected(&dir, "arr-env2.json", br#"["h2o-snapshot/1",[]]"#, "object");
    rejected(&dir, "top-string.json", br#""h2o-snapshot/1""#, "object");
    rejected(&dir, "top-null.json", b"null", "object");
    rejected(&dir, "top-num.json", b"7", "object");
    let b = branch("a", &[1], vec![], json!(false));
    let arr_branch = json!([b["spec"], b["checkpoints"], b["receipts"], b["needs_reconciliation"]]);
    rejected(&dir, "arr-branch.json", ok(envelope(vec![arr_branch])), "object");
    rejected(
        &dir,
        "arr-receipt.json",
        ok(envelope(vec![branch("a", &[1], vec![json!([1, null])], json!(false))])),
        "object",
    );
    rejected(
        &dir,
        "arr-receipt2.json",
        ok(envelope(vec![branch("a", &[1], vec![json!([1, {"acknowledged": true}])], json!(false))])),
        "object",
    );
    let mut arr_spec = branch("a", &[], vec![], json!(false));
    arr_spec["spec"] = json!(["run-1", "a", "root", "worker-a", goal(), [], 1]);
    rejected(&dir, "arr-spec.json", ok(envelope(vec![arr_spec])), "object");
    let mut arr_cp = branch("a", &[], vec![], json!(false));
    arr_cp["checkpoints"] = json!([[1, {}, {}, "Continue", "Continue"]]);
    rejected(&dir, "arr-cp.json", ok(envelope(vec![arr_cp])), "object");

    // captured_at is free text or omitted: null and non-strings are rejected.
    for (i, bad) in [json!(null), json!(5), json!(true), json!({}), json!(["x"])].into_iter().enumerate() {
        let mut doc = envelope(vec![]);
        doc["captured_at"] = bad;
        rejected(&dir, &format!("captured-{i}.json"), ok(doc), "captured_at");
    }
    rejected(&dir, "notjson.json", b"not json", "invalid");
    rejected(&dir, "empty.json", b"", "invalid");

    let many: Vec<Value> = (0..51).map(|i| branch(&format!("b{i}"), &[], vec![], json!(false))).collect();
    rejected(&dir, "many.json", ok(envelope(many)), "50");
    let fifty: Vec<Value> = (0..50).map(|i| branch(&format!("b{i}"), &[], vec![], json!(false))).collect();
    assert!(load_snapshot(&write(&dir, "fifty.json", ok(envelope(fifty)))).is_ok());

    let seqs: Vec<u64> = (0..1001).collect();
    rejected(&dir, "cps.json", ok(envelope(vec![branch("a", &seqs, vec![], json!(false))])), "1000");

    // Non-UTF-8 JSON text.
    let mut bad = br#"{"schema":"h2o-snapshot/1","captured_at":""#.to_vec();
    bad.extend_from_slice(&[0xff, 0xfe]);
    bad.extend_from_slice(br#"","branches":[]}"#);
    rejected(&dir, "latin.json", bad, "UTF-8");

    // Over 1 MiB, even though it is otherwise valid.
    let mut big = envelope(vec![]);
    big["captured_at"] = json!("x".repeat(1024 * 1024));
    rejected(&dir, "big.json", ok(big), "1 MiB");

    // Exactly 1 MiB is within the limit.
    let head = br#"{"schema":"h2o-snapshot/1","captured_at":""#;
    let tail = br#"","branches":[]}"#;
    let mut exact = head.to_vec();
    exact.extend(std::iter::repeat_n(b'x', 1024 * 1024 - head.len() - tail.len()));
    exact.extend_from_slice(tail);
    assert_eq!(exact.len(), 1024 * 1024);
    assert!(load_snapshot(&write(&dir, "exact.json", exact)).is_ok());

    // Missing file and a directory are I/O errors, not panics.
    assert!(matches!(load_snapshot(&dir.path.join("absent.json")), Err(SnapshotError::Io(_))));
    assert!(load_snapshot(&dir.path).is_err());
}

// ---- CLI -------------------------------------------------------------------------------------

fn ui(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_h2o-ui"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn cli_bad_arguments_exit_2_with_usage_and_serve_nothing() {
    let dir = assets_dir();
    let assets = dir.path.to_str().unwrap();
    let missing = dir.path.join("missing");
    let cases: Vec<Vec<&str>> = vec![
        vec!["--bogus"],
        vec!["positional"],
        vec!["--port"],
        vec!["--port", "0", "--port", "0"],
        vec!["--port", "abc"],
        vec!["--port", "70000"],
        vec!["--port", "-1"],
        vec!["--assets", missing.to_str().unwrap(), "--port", "0"],
        vec!["--assets", assets, "--assets", assets, "--port", "0"],
        vec!["--assets", assets, "--snapshot", "--port", "0"],
        vec!["--host", "0.0.0.0"],
    ];
    for args in cases {
        let out = ui(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?} printed a URL: {:?}", out.stdout);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("usage: h2o-ui"), "{args:?}: {err}");
    }
}

#[test]
fn cli_invalid_snapshot_exits_2_with_a_specific_message_and_nothing_is_served() {
    let dir = assets_dir();
    let bad = write(&dir, "bad.json", br#"{"schema":"nope","branches":[]}"#);
    let out = ui(&["--port", "0", "--assets", dir.path.to_str().unwrap(), "--snapshot", bad.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("schema"));

    let gone = dir.path.join("gone.json");
    let out = ui(&["--port", "0", "--assets", dir.path.to_str().unwrap(), "--snapshot", gone.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}

/// Starts the real binary on a free port, runs `check` against its address, always kills it.
fn with_cli(
    dir: &TestDir,
    snapshot: Option<&Path>,
    check: impl FnOnce(SocketAddr) + std::panic::UnwindSafe,
) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_h2o-ui"));
    command.args(["--port", "0", "--assets", dir.path.to_str().unwrap()]);
    if let Some(snapshot) = snapshot {
        command.args(["--snapshot", snapshot.to_str().unwrap()]);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let url = line.trim().to_string();
    let result = std::panic::catch_unwind(|| {
        let rest = url.strip_prefix("http://127.0.0.1:").expect("loopback URL");
        let port: u16 = rest.strip_suffix('/').expect("trailing slash").parse().unwrap();
        assert_ne!(port, 0);
        check(([127, 0, 0, 1], port).into());
    });
    let _ = child.kill();
    let _ = child.wait();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn cli_prints_loopback_url_and_serves_assets_and_snapshot() {
    let dir = assets_dir();
    let snap = write(&dir, "snap.json", serde_json::to_vec(&envelope(vec![])).unwrap());
    with_cli(&dir, Some(&snap), |addr| {
        let index = request(addr, "GET", "/");
        assert_eq!(index.status, 200);
        assert_eq!(index.body, ASSETS[0].3.as_bytes());
        let snapshot = request(addr, "GET", "/api/snapshot");
        assert_eq!(snapshot.status, 200);
        assert_eq!(snapshot.body, fs::read(&snap).unwrap());
    });
}

#[test]
fn cli_duplicate_host_headers_are_403() {
    let dir = assets_dir();
    let snap = write(&dir, "snap.json", serde_json::to_vec(&envelope(vec![])).unwrap());
    with_cli(&dir, Some(&snap), assert_duplicate_hosts_are_403);
}

#[test]
fn cli_route_matching_head_limit_and_head_bodies_match_the_library() {
    let dir = assets_dir();
    let snap = write(&dir, "snap.json", serde_json::to_vec(&envelope(vec![])).unwrap());
    with_cli(&dir, Some(&snap), |addr| {
        for method in ["GET", "HEAD"] {
            for target in ["/app.css?x", "/?..", "/api/snapshot?anything", "/app.js#f"] {
                assert_eq!(request(addr, method, target).status, 404, "{method} {target}");
            }
        }
        let port = addr.port();
        assert_eq!(raw(addr, &request_with_head_len("GET", port, 8192)).status, 404);
        assert_eq!(raw(addr, &request_with_head_len("GET", port, 8193)).status, 431);
        let r = raw(addr, &request_with_head_len("HEAD", port, 8193));
        assert_eq!(r.status, 431);
        assert!(r.body.is_empty());
        let mut bad = format!("HEAD /x HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX: ").into_bytes();
        bad.extend_from_slice(&[0xff, b'\r', b'\n', b'\r', b'\n']);
        let r = raw(addr, &bad);
        assert_eq!(r.status, 400);
        assert!(r.body.is_empty());
    });
}
