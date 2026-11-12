//! Loopback-only static launcher for the local reception UI, plus the read-only snapshot gate.
//!
//! Boundaries: binds 127.0.0.1 only; answers `GET`/`HEAD` for an exact route table (the request
//! target is only ever compared to constants, never turned into a path); `/api/snapshot` serves
//! the bytes of one `--snapshot` file that was read and validated once at startup. It never
//! reads the journal, a daemon socket, the environment or keys, and never opens a browser or
//! makes an outbound connection.
use crate::monitor::{BranchSpec, Checkpoint, Receipt};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::{
    collections::HashSet,
    fmt, fs,
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const SNAPSHOT_SCHEMA: &str = "h2o-snapshot/1";
pub const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
pub const MAX_BRANCHES: usize = 50;
pub const MAX_CHECKPOINTS_PER_BRANCH: usize = 1000;

const MAX_HEAD_BYTES: usize = 8 * 1024;
const HEAD_DEADLINE: Duration = Duration::from_secs(3);
const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// (request target, asset file name, content type). The whole serving surface besides the snapshot.
const ROUTES: [(&str, &str, &str); 4] = [
    ("/", "index.html", "text/html; charset=utf-8"),
    ("/app.css", "app.css", "text/css; charset=utf-8"),
    ("/core.js", "core.js", "text/javascript; charset=utf-8"),
    ("/app.js", "app.js", "text/javascript; charset=utf-8"),
];

// ---- snapshot --------------------------------------------------------------------------------

#[derive(Debug)]
pub enum SnapshotError {
    Io(String),
    TooLarge,
    NotUtf8,
    Invalid(String),
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "cannot read snapshot: {e}"),
            Self::TooLarge => write!(f, "snapshot is larger than 1 MiB"),
            Self::NotUtf8 => write!(f, "snapshot is not valid UTF-8"),
            Self::Invalid(e) => write!(f, "invalid snapshot: {e}"),
        }
    }
}

/// `receipt` and `needs_reconciliation` must be present; `null` is kept as "unknown".
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    schema: String,
    #[allow(dead_code)]
    captured_at: Option<String>,
    branches: Vec<BranchView>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BranchView {
    #[allow(dead_code)]
    spec: BranchSpec,
    checkpoints: Vec<Checkpoint>,
    receipts: Vec<ReceiptView>,
    #[allow(dead_code)]
    #[serde(deserialize_with = "present")]
    needs_reconciliation: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptView {
    sequence: u64,
    #[allow(dead_code)]
    #[serde(deserialize_with = "present")]
    receipt: Option<Receipt>,
}

/// Derived `Deserialize` also accepts sequence form (`["h2o-snapshot/1", null, []]`), so the JSON
/// object shape of the envelope, branches, specs, checkpoints and receipts is required up front.
fn require_objects(doc: &Value) -> Result<(), String> {
    let root = doc.as_object().ok_or("snapshot must be a JSON object")?;
    if root.get("captured_at").is_some_and(|c| !c.is_string()) {
        return Err("captured_at must be a string or omitted".into());
    }
    let branches = root.get("branches").and_then(Value::as_array);
    for (i, branch) in branches.into_iter().flatten().enumerate() {
        if !branch.is_object() {
            return Err(format!("branch {i} must be a JSON object"));
        }
        if branch.get("spec").is_some_and(|spec| !spec.is_object()) {
            return Err(format!("branch {i}: spec must be a JSON object"));
        }
        for key in ["checkpoints", "receipts"] {
            let items = branch.get(key).and_then(Value::as_array);
            for (j, item) in items.into_iter().flatten().enumerate() {
                if !item.is_object() {
                    return Err(format!("branch {i}: {key}[{j}] must be a JSON object"));
                }
            }
        }
    }
    Ok(())
}

/// Validates snapshot JSON against the real monitor types and the envelope rules.
pub fn validate_snapshot(bytes: &[u8]) -> Result<(), SnapshotError> {
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err(SnapshotError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| SnapshotError::NotUtf8)?;
    let invalid = |m: String| SnapshotError::Invalid(m);
    let value: Value = serde_json::from_str(text).map_err(|e| invalid(e.to_string()))?;
    require_objects(&value).map_err(invalid)?;
    let doc: Envelope = serde_json::from_value(value).map_err(|e| invalid(e.to_string()))?;
    if doc.schema != SNAPSHOT_SCHEMA {
        return Err(invalid(format!("unsupported schema, expected {SNAPSHOT_SCHEMA}")));
    }
    if doc.branches.len() > MAX_BRANCHES {
        return Err(invalid(format!("more than {MAX_BRANCHES} branches")));
    }
    for (i, branch) in doc.branches.iter().enumerate() {
        if branch.checkpoints.len() > MAX_CHECKPOINTS_PER_BRANCH {
            return Err(invalid(format!(
                "branch {i}: more than {MAX_CHECKPOINTS_PER_BRANCH} checkpoints"
            )));
        }
        let mut listed = HashSet::new();
        for checkpoint in &branch.checkpoints {
            if !listed.insert(checkpoint.sequence) {
                return Err(invalid(format!(
                    "branch {i}: duplicate checkpoint sequence {}",
                    checkpoint.sequence
                )));
            }
        }
        for entry in &branch.receipts {
            if !listed.contains(&entry.sequence) {
                return Err(invalid(format!(
                    "branch {i}: receipt for unlisted sequence {}",
                    entry.sequence
                )));
            }
        }
    }
    Ok(())
}

/// Reads one file (bounded) and validates it. The returned bytes are the file bytes, unchanged.
pub fn load_snapshot(path: &Path) -> Result<Vec<u8>, SnapshotError> {
    let io_err = |e: io::Error| SnapshotError::Io(e.to_string());
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(io_err)?
        .take(MAX_SNAPSHOT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_err)?;
    validate_snapshot(&bytes)?;
    Ok(bytes)
}

// ---- server ----------------------------------------------------------------------------------

#[derive(Debug)]
pub enum StartError {
    Snapshot(SnapshotError),
    Io(io::Error),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Snapshot(e) => e.fmt(f),
            Self::Io(e) => write!(f, "cannot listen on 127.0.0.1: {e}"),
        }
    }
}

struct Shared {
    assets: PathBuf,
    snapshot: Option<Vec<u8>>,
    port: u16,
}

pub struct Launcher {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl Launcher {
    /// Binds `127.0.0.1:port` (0 = any free port) and serves until stopped or dropped.
    pub fn start(
        assets: impl Into<PathBuf>,
        snapshot: Option<&Path>,
        port: u16,
    ) -> Result<Self, StartError> {
        let snapshot = snapshot
            .map(load_snapshot)
            .transpose()
            .map_err(StartError::Snapshot)?;
        let listener = TcpListener::bind(("127.0.0.1", port)).map_err(StartError::Io)?;
        let addr = listener.local_addr().map_err(StartError::Io)?;
        let shared = Arc::new(Shared {
            assets: assets.into(),
            snapshot,
            port: addr.port(),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        // ponytail: one thread per connection, bounded only by the head deadline; add a cap if
        // this is ever exposed beyond a single local user.
        let accept = thread::spawn(move || {
            for stream in listener.incoming() {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(stream) = stream {
                    let shared = shared.clone();
                    thread::spawn(move || handle(stream, &shared));
                }
            }
        });
        Ok(Self {
            addr,
            stop,
            accept: Some(accept),
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn stop(self) {}
}

impl Drop for Launcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking accept so it observes the flag.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1));
        if let Some(handle) = self.accept.take() {
            let _ = handle.join();
        }
    }
}

struct Reply {
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
    allow: bool,
}

fn text(status: &'static str) -> Reply {
    Reply {
        status,
        content_type: "text/plain; charset=utf-8",
        body: status.as_bytes().to_vec(),
        allow: false,
    }
}

/// A read-stage error reply; `HEAD` requests (identifiable from the first bytes) get no body.
fn refuse(status: &'static str, bytes: &[u8]) -> (Reply, bool) {
    (text(status), bytes.starts_with(b"HEAD "))
}

/// Reads the request head only (no body is ever read). `Err` is a ready reply (+ head-only flag).
/// The 8 KiB limit applies to the head up to and including its terminator, wherever in a read the
/// terminator arrives; body bytes that came in the same read are not counted.
fn read_head(stream: &mut TcpStream) -> Result<String, (Reply, bool)> {
    let deadline = Instant::now() + HEAD_DEADLINE;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            if end + 4 > MAX_HEAD_BYTES {
                return Err(refuse("431 Request Header Fields Too Large", &buf));
            }
            buf.truncate(end);
            return String::from_utf8(buf).map_err(|e| refuse("400 Bad Request", e.as_bytes()));
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(refuse("431 Request Header Fields Too Large", &buf));
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
            return Err(refuse("408 Request Timeout", &buf));
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Err(refuse("400 Bad Request", &buf)),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return Err(refuse("408 Request Timeout", &buf)),
        }
    }
}

fn route(head: &str, shared: &Shared) -> (Reply, bool) {
    let mut lines = head.split("\r\n");
    let mut parts = lines.next().unwrap_or("").split(' ');
    let head_only = head.starts_with("HEAD ");
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return (text("400 Bad Request"), head_only);
    };
    if !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return (text("400 Bad Request"), head_only);
    }

    // DNS-rebinding guard: exactly one Host header naming this loopback port.
    let hosts: Vec<&str> = lines
        .filter_map(|l| l.split_once(':'))
        .filter(|(k, _)| k.eq_ignore_ascii_case("host"))
        .map(|(_, v)| v.trim())
        .collect();
    let allowed = [format!("127.0.0.1:{}", shared.port), format!("localhost:{}", shared.port)];
    if hosts.len() != 1 || !allowed.iter().any(|a| a.eq_ignore_ascii_case(hosts[0])) {
        return (text("403 Forbidden"), head_only);
    }
    if method != "GET" && method != "HEAD" {
        return (
            Reply {
                allow: true,
                ..text("405 Method Not Allowed")
            },
            false,
        );
    }

    // The raw request-target is compared to constants only: a query or fragment is just a 404.
    let reply = if target == "/api/snapshot" {
        match &shared.snapshot {
            Some(bytes) => Reply {
                status: "200 OK",
                content_type: "application/json; charset=utf-8",
                body: bytes.clone(),
                allow: false,
            },
            None => text("404 Not Found"),
        }
    } else if let Some((_, file, content_type)) = ROUTES.iter().find(|(p, _, _)| *p == target) {
        match fs::read(shared.assets.join(file)) {
            Ok(body) => Reply {
                status: "200 OK",
                content_type,
                body,
                allow: false,
            },
            Err(_) => text("404 Not Found"),
        }
    } else {
        text("404 Not Found")
    };
    (reply, head_only)
}

fn handle(mut stream: TcpStream, shared: &Shared) {
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let (reply, head_only) = match read_head(&mut stream) {
        Ok(head) => route(&head, shared),
        Err(refused) => refused,
    };
    let mut out = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
X-Content-Type-Options: nosniff\r\nContent-Security-Policy: {CSP}\r\nConnection: close\r\n",
        reply.status,
        reply.content_type,
        reply.body.len()
    );
    if reply.allow {
        out.push_str("Allow: GET, HEAD\r\n");
    }
    out.push_str("\r\n");
    let mut bytes = out.into_bytes();
    if !head_only {
        bytes.extend_from_slice(&reply.body);
    }
    let _ = stream.write_all(&bytes);
    // Drain a little unread input so closing does not reset the connection before the client
    // has read the reply.
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let mut sink = [0u8; 4096];
    for _ in 0..16 {
        if !matches!(stream.read(&mut sink), Ok(n) if n > 0) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Shared {
        Shared {
            assets: PathBuf::from("does-not-exist"),
            snapshot: Some(b"{}".to_vec()),
            port: 5000,
        }
    }

    /// `route` on a head, bypassing TCP (and any loopback filter that rewrites headers).
    fn status(head: &str) -> (&'static str, bool) {
        let (reply, head_only) = route(head, &shared());
        (reply.status, head_only)
    }

    #[test]
    fn duplicate_host_headers_are_403_for_every_method() {
        let good = "Host: 127.0.0.1:5000";
        let variants = [
            vec![good, good],
            vec![good, "Host: evil.example"],
            vec!["Host: evil.example", good],
            vec![good, "Host: localhost:5000"],
            vec![good, "host: 127.0.0.1:5000"],
            vec!["HOST: 127.0.0.1:5000", good],
            vec![good, good, good],
        ];
        for method in ["GET", "HEAD", "POST", "XGET"] {
            for target in ["/", "/not-a-route", "/api/snapshot"] {
                for hosts in &variants {
                    let head = format!("{method} {target} HTTP/1.1\r\n{}\r\nConnection: close", hosts.join("\r\n"));
                    assert_eq!(status(&head), ("403 Forbidden", method == "HEAD"), "{head:?}");
                }
            }
        }
        // Exactly one allowed Host is fine, in either spelling and case.
        for host in ["Host: 127.0.0.1:5000", "host: localhost:5000", "HOST:   LOCALHOST:5000  "] {
            let head = format!("GET /not-a-route HTTP/1.1\r\n{host}\r\nConnection: close");
            assert_eq!(status(&head).0, "404 Not Found", "{head:?}");
        }
        assert_eq!(status("GET / HTTP/1.1\r\nConnection: close").0, "403 Forbidden");
    }

    #[test]
    fn route_matches_the_raw_target_exactly() {
        for target in ["/?", "/?x", "/app.css?x", "/app.css?", "/api/snapshot?anything", "/#", "/app.js#x"] {
            for method in ["GET", "HEAD"] {
                let head = format!("{method} {target} HTTP/1.1\r\nHost: 127.0.0.1:5000");
                assert_eq!(status(&head), ("404 Not Found", method == "HEAD"), "{head:?}");
            }
        }
        assert_eq!(status("GET /api/snapshot HTTP/1.1\r\nHost: 127.0.0.1:5000").0, "200 OK");
    }

    #[test]
    fn head_is_identified_on_every_route_stage_error() {
        assert_eq!(status("HEAD / HTTP/9.9\r\nHost: 127.0.0.1:5000"), ("400 Bad Request", true));
        assert_eq!(status("HEAD /"), ("400 Bad Request", true));
        assert_eq!(status("GET / HTTP/9.9\r\nHost: 127.0.0.1:5000"), ("400 Bad Request", false));
    }
}
