//! `tarpi serve`: share this machine's characters, chats, personas, system
//! prompts and API connections with other devices running
//! `tarpi connect`. It does not open the chat screen; it just answers requests
//! until you stop it (Ctrl+C).
//!
//! The protocol is a handful of HTTP/1.1 requests (hand-written on top of
//! tokio, so no web framework is needed). Every request must carry the
//! server's token (`Authorization: Bearer <token>`); the token is made the
//! first time and kept in `~/.config/tarpi/serve-token`. With
//! `serve_trust_all: true` in `config.yaml` no token is asked for at all.
//!
//! | Request | Does |
//! | --- | --- |
//! | `GET /v1/ping` | `{"tarpi": version, "proto": 1}` (checks the token too) |
//! | `GET /v1/manifest` | every shared file: path, size, modification time, hash |
//! | `GET /v1/file?path=P` | the file's bytes (its time is in `X-Tarpi-Mtime`) |
//! | `PUT /v1/file?path=P&base=H&mtime=M` | write the file, but only if it still has hash `H` (empty: only if it does not exist yet); otherwise `409` |
//! | `DELETE /v1/file?path=P&base=H` | delete it, same rule |
//! | `GET /v1/watch` | stays open; a byte every 50 ms (both ways), so the other side sees at once when this one is gone |
//! | `POST /v1/tokenize` | `{"text", "connection"}`: token count from this machine's backend |
//! | `POST /v1/event` | `{"text"}`: a line about what the device just did (opened a chat, ...), printed in this machine's log |
//!
//! Only the files listed in `mirror` can be reached; any other path is
//! refused. The connection is plain HTTP: use it on a network you trust, or
//! with `--local` through an SSH tunnel.

use crate::config;
use crate::mirror::{self, Entry, Manifest, Scanner, DEFAULT_PORT, PROTO};
use anyhow::{anyhow, Context, Result};
use clap::Parser;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Parser, Debug)]
#[command(
    name = "tarpi serve",
    about = "Share your characters, chats and settings with other devices running `tarpi connect`",
    long_about = "Serve the data in ~/.config/tarpi (characters, chats, personas, system \
                  prompts and the API connections) to other devices, which \
                  keep a copy of it in step with `tarpi connect`. Nothing is drawn on the \
                  screen: the command just runs until you stop it with Ctrl+C. By default \
                  it listens on every network interface; --local limits it to this \
                  machine. Clients need the token it prints (kept in \
                  ~/.config/tarpi/serve-token), unless `serve_trust_all: true` is set in \
                  config.yaml: then anyone who can reach the port is let in. The \
                  connection is plain HTTP, so only use it on a network you trust.",
    after_help = concat!(
        "EXAMPLES:\n",
        "  tarpi serve                 share on the network, port 7878\n",
        "  tarpi serve --local         this machine only (e.g. behind an SSH tunnel)\n",
        "  tarpi serve -p 9000         another port\n",
        "  tarpi serve --new-token     replace the token (connected devices must be given the new one)"
    )
)]
struct ServeCli {
    /// Listen on localhost only, instead of on every network interface
    #[arg(long)]
    local: bool,

    /// Port to listen on
    #[arg(short, long, default_value_t = DEFAULT_PORT)]
    port: u16,

    /// Make a new token (the old one stops working)
    #[arg(long)]
    new_token: bool,
}

const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 256 * 1024 * 1024;

struct State {
    root: PathBuf,
    token: String,
    /// `serve_trust_all`: no token is asked for.
    trust_all: bool,
    scanner: std::sync::Mutex<Scanner>,
    /// Held while a file is checked and changed, so two clients cannot both
    /// pass the "still the version you saw" check.
    write_lock: tokio::sync::Mutex<()>,
    /// Changes whenever a shared file does (see `monitor`); every watching
    /// device is told at once and syncs.
    changes: tokio::sync::watch::Sender<u64>,
}

struct Req {
    method: String,
    path: String,
    query: HashMap<String, String>,
    bearer: Option<String>,
    body: Vec<u8>,
}

struct Reply {
    code: u16,
    ctype: &'static str,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
}

impl Reply {
    fn json<T: Serialize>(code: u16, v: &T) -> Reply {
        Reply {
            code,
            ctype: "application/json",
            body: serde_json::to_vec(v).unwrap_or_default(),
            headers: Vec::new(),
        }
    }

    fn text(code: u16, msg: &str) -> Reply {
        Reply {
            code,
            ctype: "text/plain; charset=utf-8",
            body: msg.as_bytes().to_vec(),
            headers: Vec::new(),
        }
    }
}

/// The token kept in `serve-token`, made first if there is none (or `renew`).
fn load_or_create_token(renew: bool) -> Result<String> {
    let path = config::serve_token_path().ok_or_else(|| anyhow!("no config directory on this system"))?;
    if !renew {
        if let Ok(t) = std::fs::read_to_string(&path) {
            let t = t.trim();
            if !t.is_empty() {
                return Ok(t.to_string());
            }
        }
    }
    let token = uuid::Uuid::new_v4().simple().to_string();
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    std::fs::write(&path, format!("{}\n", token))
        .with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(token)
}

/// The address of this machine on the local network, if there is one (asked
/// from the routing table; nothing is sent).
fn lan_ip() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    if ip.is_unspecified() || ip.is_loopback() {
        None
    } else {
        Some(ip.to_string())
    }
}

pub async fn run() -> Result<()> {
    // argv is `tarpi serve ...`; drop the program name so "serve" plays the
    // part of the binary name in clap's messages.
    // `tarpi s` is short for `tarpi serve`; usage texts should say the long name.
    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    if let Some(first) = argv.first_mut() {
        if first == "s" {
            *first = "serve".to_string();
        }
    }
    let cli = ServeCli::parse_from(argv);
    let cfg = config::load()?;
    let trust_all = cfg.serve_trust_all.unwrap_or(false);
    let root = config::data_root().ok_or_else(|| anyhow!("no config directory on this system"))?;
    std::fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
    // With `serve_trust_all` there is no token to make or to print.
    let token = if trust_all {
        String::new()
    } else {
        load_or_create_token(cli.new_token)?
    };

    let ip: [u8; 4] = if cli.local { [127, 0, 0, 1] } else { [0, 0, 0, 0] };
    let addr = SocketAddr::from((ip, cli.port));
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("cannot listen on {} (is another tarpi serve running?)", addr))?;

    let host = if cli.local {
        Some("127.0.0.1".to_string())
    } else {
        lan_ip()
    };
    let host = host.unwrap_or_else(|| "<this machine's address>".to_string());
    let scope = if cli.local { "  (this machine only)" } else { "" };
    if trust_all {
        println!("tarpi serve  {}:{}{}  [no token]", host, cli.port, scope);
        println!();
        println!("  connect  tarpi connect");
        println!("           serve_url: {}:{}  serve_trust_all: true", host, cli.port);
        println!("  warning  anyone who can reach this port can read and change everything shared");
    } else {
        println!("tarpi serve  {}:{}{}", host, cli.port, scope);
        println!();
        println!("  token    {}", token);
        println!("  connect  tarpi connect {}", token);
        println!("           serve_url: {}:{}", host, cli.port);
    }
    println!();
    println!("Ctrl+C to stop\n");

    let state = Arc::new(State {
        scanner: std::sync::Mutex::new(Scanner::new(root.clone(), true)),
        root,
        token,
        trust_all,
        write_lock: tokio::sync::Mutex::new(()),
        changes: tokio::sync::watch::channel(0u64).0,
    });
    tokio::spawn(monitor(state.clone()));

    loop {
        let (sock, peer) = match listener.accept().await {
            Ok(x) => x,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let st = state.clone();
        tokio::spawn(async move {
            handle(sock, peer, st).await;
        });
    }
}

/// Look at the shared files every 200 ms (hashes are kept, so only files whose
/// size or time changed are read again) and tell the watching devices when
/// something is different: a device's upload, or this machine's own tarpi.
async fn monitor(st: Arc<State>) {
    let mut last = 0u64;
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let st2 = st.clone();
        let d = tokio::task::spawn_blocking(move || {
            let files = st2.scanner.lock().unwrap_or_else(|e| e.into_inner()).scan();
            mirror::digest(&files)
        })
        .await;
        if let Ok(d) = d {
            if d != last {
                last = d;
                st.changes.send_replace(d);
            }
        }
    }
}

/// How often a `tarpi connect` device that watches this server is sent a byte.
const BEAT: Duration = Duration::from_millis(50);

async fn handle(mut sock: TcpStream, peer: SocketAddr, st: Arc<State>) {
    let limit = Duration::from_secs(300);
    let req = match tokio::time::timeout(limit, read_req(&mut sock)).await {
        Ok(Ok(req)) => req,
        Ok(Err(r)) => return write_reply(&mut sock, r).await,
        Err(_) => return,
    };
    // The one request that is not answered and closed: see `watch`.
    if req.method == "GET" && req.path == "/v1/watch" {
        return watch(sock, &req, &st, peer).await;
    }
    let reply = match tokio::time::timeout(limit, route(&st, req, peer)).await {
        Ok(r) => r,
        Err(_) => Reply::text(504, "timed out"),
    };
    write_reply(&mut sock, reply).await;
}

/// `GET /v1/watch`: a connection that stays open and gets a byte every
/// 50 ms ([`BEAT`]), so a `tarpi connect` device on the other end learns at once when
/// this machine or the link goes away (the connection closes or the bytes
/// stop). It ends when the device hangs up.
async fn watch(mut sock: TcpStream, req: &Req, st: &Arc<State>, peer: SocketAddr) {
    if !st.trust_all && !token_ok(&st.token, req.bearer.as_deref()) {
        println!("{}  {}  refused: missing or wrong token", stamp(), peer.ip());
        return write_reply(&mut sock, Reply::text(401, "missing or wrong token")).await;
    }
    let _ = sock.set_nodelay(true);
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n";
    if sock.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    println!("{}  {}  connected", stamp(), peer.ip());
    let since = std::time::Instant::now();
    let mut tick = tokio::time::interval(BEAT);
    let mut scratch = [0u8; 64];
    let mut changes = st.changes.subscribe();
    // The state at the start is what the device has just synced to.
    changes.borrow_and_update();
    loop {
        tokio::select! {
            _ = tick.tick() => {
                if sock.write_all(b"\n").await.is_err() {
                    break;
                }
            }
            // `c`: a shared file changed, sync now.
            c = changes.changed() => {
                if c.is_err() || sock.write_all(b"c").await.is_err() {
                    break;
                }
            }
            // The device never sends anything more; a read that ends means it left.
            r = sock.read(&mut scratch) => {
                if matches!(r, Ok(0) | Err(_)) {
                    break;
                }
            }
        }
    }
    println!("{}  {}  disconnected after {}", stamp(), peer.ip(), how_long(since.elapsed()));
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

async fn read_req(sock: &mut TcpStream) -> std::result::Result<Req, Reply> {
    let bad = |m: &str| Reply::text(400, m);

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = find_sub(&buf, b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return Err(Reply::text(431, "request headers too large"));
        }
        let n = sock.read(&mut chunk).await.map_err(|_| bad("read error"))?;
        if n == 0 {
            return Err(bad("connection closed"));
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let head = std::str::from_utf8(&buf[..head_end]).map_err(|_| bad("bad request"))?;
    let mut lines = head.split("\r\n");
    let first = lines.next().ok_or_else(|| bad("bad request"))?;
    let mut parts = first.split(' ');
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().ok_or_else(|| bad("bad request line"))?;

    let mut content_length: Option<usize> = None;
    let mut bearer: Option<String> = None;
    for line in lines {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        match k.as_str() {
            "content-length" => content_length = v.parse().ok(),
            "authorization" => bearer = v.strip_prefix("Bearer ").map(|t| t.trim().to_string()),
            _ => {}
        }
    }

    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), mirror::parse_query(q)),
        None => (target.to_string(), HashMap::new()),
    };

    let mut body: Vec<u8> = Vec::new();
    if method == "PUT" || method == "POST" {
        let len = content_length.ok_or_else(|| Reply::text(411, "Content-Length required"))?;
        if len > MAX_BODY {
            return Err(Reply::text(413, "file too large"));
        }
        body = buf[head_end + 4..].to_vec();
        if body.len() > len {
            body.truncate(len);
        } else if body.len() < len {
            let mut rest = vec![0u8; len - body.len()];
            sock.read_exact(&mut rest)
                .await
                .map_err(|_| bad("body cut short"))?;
            body.extend_from_slice(&rest);
        }
    }

    Ok(Req {
        method,
        path,
        query,
        bearer,
        body,
    })
}

async fn write_reply(sock: &mut TcpStream, r: Reply) {
    let reason = match r.code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Payload Too Large",
        504 => "Gateway Timeout",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    };
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        r.code,
        reason,
        r.ctype,
        r.body.len()
    );
    for (k, v) in &r.headers {
        head.push_str(&format!("{}: {}\r\n", k, v));
    }
    head.push_str("\r\n");
    let _ = sock.write_all(head.as_bytes()).await;
    let _ = sock.write_all(&r.body).await;
    let _ = sock.shutdown().await;
}

/// Compare without stopping at the first difference.
fn token_ok(want: &str, got: Option<&str>) -> bool {
    let Some(got) = got else {
        return false;
    };
    let (a, b) = (want.as_bytes(), got.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `5s`, `3m 04s`, `1h 02m`.
fn how_long(d: Duration) -> String {
    let t = d.as_secs();
    if t < 60 {
        format!("{}s", t)
    } else if t < 3600 {
        format!("{}m {:02}s", t / 60, t % 60)
    } else {
        format!("{}h {:02}m", t / 3600, (t % 3600) / 60)
    }
}

/// What a device says it did, made safe to print: no control characters
/// (nobody can move the cursor of this terminal), at most 300 characters.
fn clean_event(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(300).collect()
}

/// `POST /v1/event`: print a line about what a `tarpi connect` device did.
fn event(req: &Req, peer: SocketAddr) -> Reply {
    let v: serde_json::Value = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(_) => return Reply::text(400, "expected JSON"),
    };
    let Some(text) = v.get("text").and_then(|t| t.as_str()) else {
        return Reply::text(400, "missing text");
    };
    let text = clean_event(text);
    if !text.is_empty() {
        println!("{}  {}  {}", stamp(), peer.ip(), text);
    }
    Reply::json(200, &json!({ "ok": true }))
}

fn stamp() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// Count `text` with this machine's own backend, for a `tarpi connect` device
/// that cannot reach it. The connection is the named one, else the active one.
async fn tokenize(req: &Req) -> Reply {
    let v: serde_json::Value = match serde_json::from_slice(&req.body) {
        Ok(v) => v,
        Err(_) => return Reply::text(400, "expected JSON"),
    };
    let Some(text) = v.get("text").and_then(|t| t.as_str()) else {
        return Reply::text(400, "missing text");
    };
    let name = v.get("connection").and_then(|n| n.as_str()).unwrap_or("");
    let store = match crate::conn::Store::load() {
        Ok(s) => s,
        Err(e) => return Reply::text(500, &format!("{:#}", e)),
    };
    let conn = store
        .connections
        .iter()
        .find(|c| !name.is_empty() && c.name == name)
        .cloned()
        .or_else(|| store.active_connection());
    let Some(conn) = conn else {
        return Reply::text(409, "this server has no API connection");
    };
    match crate::conn::count_tokens(&conn, text).await {
        Ok(n) => Reply::json(200, &json!({ "tokens": n })),
        Err(e) => Reply::text(502, &format!("{:#}", e)),
    }
}

async fn route(st: &Arc<State>, req: Req, peer: SocketAddr) -> Reply {
    // A ping without a token is always answered, so `tarpi connect` can learn
    // whether this server wants one. A ping that does bring a token is checked.
    let open_ping = req.method == "GET" && req.path == "/v1/ping" && req.bearer.is_none();
    if !st.trust_all && !open_ping && !token_ok(&st.token, req.bearer.as_deref()) {
        println!("{}  {}  refused: missing or wrong token", stamp(), peer.ip());
        return Reply::text(401, "missing or wrong token");
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/v1/ping") => Reply::json(200, &json!({ "tarpi": env!("CARGO_PKG_VERSION"), "proto": PROTO, "token": !st.trust_all })),
        ("GET", "/v1/manifest") => {
            let st2 = st.clone();
            let files: Vec<Entry> = match tokio::task::spawn_blocking(move || {
                st2.scanner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .scan()
            })
            .await
            {
                Ok(f) => f,
                Err(_) => return Reply::text(500, "scan failed"),
            };
            Reply::json(200, &Manifest { files })
        }
        ("POST", "/v1/tokenize") => tokenize(&req).await,
        ("POST", "/v1/event") => event(&req, peer),
        ("GET", "/v1/file") => get_file(st, &req),
        ("PUT", "/v1/file") => put_file(st, &req, peer).await,
        ("DELETE", "/v1/file") => delete_file(st, &req, peer).await,
        ("GET", _) | ("PUT", _) | ("DELETE", _) => Reply::text(404, "not found"),
        _ => Reply::text(405, "method not allowed"),
    }
}

/// The `path` of a request, if it names a shared file.
fn shared_path(req: &Req) -> std::result::Result<String, Reply> {
    match req.query.get("path") {
        Some(p) if mirror::valid_path(p) => Ok(p.clone()),
        _ => Err(Reply::text(400, "not a shared file")),
    }
}

fn get_file(st: &Arc<State>, req: &Req) -> Reply {
    let rel = match shared_path(req) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let path = mirror::to_fs(&st.root, &rel);
    let Ok(bytes) = std::fs::read(&path) else {
        return Reply::text(404, "no such file");
    };
    let mtime = std::fs::metadata(&path)
        .map(|m| mirror::mtime_ms(&m))
        .unwrap_or(0);
    Reply {
        code: 200,
        ctype: "application/octet-stream",
        body: bytes,
        headers: vec![("X-Tarpi-Mtime".to_string(), mtime.to_string())],
    }
}

/// The hash of the file as it is now, `None` when there is none.
fn current_hash(path: &std::path::Path) -> Option<String> {
    std::fs::read(path).ok().map(|b| mirror::hash_bytes(&b))
}

/// `base` of a request: `None` means "the file must not exist".
fn base_of(req: &Req) -> Option<&str> {
    req.query.get("base").map(|s| s.as_str()).filter(|s| !s.is_empty())
}

async fn put_file(st: &Arc<State>, req: &Req, peer: SocketAddr) -> Reply {
    let rel = match shared_path(req) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let path = mirror::to_fs(&st.root, &rel);
    let mtime: Option<u64> = req.query.get("mtime").and_then(|m| m.parse().ok());

    let _guard = st.write_lock.lock().await;
    let now = current_hash(&path);
    if now.as_deref() != base_of(req) {
        println!("{}  {}  refused {}: it changed here in the meantime", stamp(), peer.ip(), mirror::describe(&rel));
        return Reply::json(409, &json!({ "error": "changed", "hash": now }));
    }
    if let Err(e) = mirror::write_atomic(&path, &req.body, mtime) {
        return Reply::text(500, &format!("{:#}", e));
    }
    // A file that did not exist yet is a new chat, a new character (imported
    // on the device) and so on; otherwise it is a change to an existing one.
    let verb = if now.is_none() { "added" } else { "updated" };
    println!("{}  {}  {} {} (uploaded)", stamp(), peer.ip(), verb, mirror::describe(&rel));
    Reply::json(
        200,
        &json!({ "hash": mirror::hash_bytes(&req.body), "mtime": mtime }),
    )
}

async fn delete_file(st: &Arc<State>, req: &Req, peer: SocketAddr) -> Reply {
    let rel = match shared_path(req) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let path = mirror::to_fs(&st.root, &rel);

    let _guard = st.write_lock.lock().await;
    let Some(now) = current_hash(&path) else {
        return Reply::json(200, &json!({ "deleted": false }));
    };
    if Some(now.as_str()) != base_of(req) {
        return Reply::json(409, &json!({ "error": "changed", "hash": now }));
    }
    if let Err(e) = std::fs::remove_file(&path) {
        return Reply::text(500, &format!("{}", e));
    }
    println!("{}  {}  deleted {}", stamp(), peer.ip(), mirror::describe(&rel));
    Reply::json(200, &json!({ "deleted": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_naturally() {
        assert_eq!(how_long(Duration::from_secs(5)), "5s");
        assert_eq!(how_long(Duration::from_secs(184)), "3m 04s");
        assert_eq!(how_long(Duration::from_secs(3720)), "1h 02m");
    }

    #[test]
    fn events_lose_control_characters_and_length() {
        assert_eq!(clean_event("opened\x1b[2J chat\n"), "opened[2J chat");
        assert_eq!(clean_event(&"a".repeat(500)).chars().count(), 300);
    }
}
