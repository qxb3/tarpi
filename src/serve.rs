//! `tarpi serve`: share this machine's characters, chats, personas, lorebooks,
//! system prompts and API connections with other devices running
//! `tarpi connect`. It does not open the chat screen; it just answers requests
//! until you stop it (Ctrl+C).
//!
//! Each device keeps **one persistent TCP connection** (framing and messages:
//! see `wire`). Everything goes over it: file sync, token counts, log lines,
//! the note that a shared file changed, and a beat in both directions that
//! lets either side see at once when the other is gone. The server's token is
//! made the first time and kept in `~/.config/tarpi/serve-token`; a device
//! proves it knows it with `auth` before anything else is answered. With
//! `serve_trust_all: true` in `config.yaml` no token is asked for at all.
//!
//! Requests (the `"op"` of a message from the device):
//!
//! | op | Does |
//! | --- | --- |
//! | `hello` | `{"proto"}`: answered even before `auth`: `{"tarpi": version, "proto": 2, "token": bool}` (does this server want a token) |
//! | `auth` | `{"token"}`: `ok`, or `401` and the server closes the link |
//! | `manifest` | every shared file: path, size, modification time, hash (the body is the JSON list) |
//! | `get` | `{"path"}`: the file's bytes in the body, its time in `mtime` |
//! | `put` | `{"path", "base", "mtime"}` + the bytes: write the file, but only if it still has hash `base` (empty: only if it does not exist yet); otherwise `409` |
//! | `delete` | `{"path", "base"}`: delete it, same rule |
//! | `tokenize` | `{"text", "connection"}`: token count from this machine's backend |
//! | `event` | `{"text"}` (no reply): a line about what the device just did (opened a chat, ...), printed in this machine's log |
//!
//! The server sends `{"op": "changed"}` (no reply wanted) to every
//! authenticated device when a shared file changes here or through another
//! device, so each syncs at once.
//!
//! Only the files listed in `mirror` can be reached; any other path is
//! refused. The link is plain TCP: use it on a network you trust, or with
//! `--local` through an SSH tunnel.

use crate::config;
use crate::mirror::{self, Entry, Manifest, Scanner, DEFAULT_PORT, PROTO};
use crate::wire::{self, Message};
use anyhow::{anyhow, Context, Result};
use clap::Parser;
use serde_json::json;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};

#[derive(Parser, Debug)]
#[command(
    name = "tarpi serve",
    about = "Share your characters, chats and settings with other devices running `tarpi connect`",
    long_about = "Serve the data in ~/.config/tarpi (characters, chats, personas, lorebooks, \
                  system prompts and the API connections) to other devices, which \
                  keep a copy of it in step with `tarpi connect`. Nothing is drawn on the \
                  screen: the command just runs until you stop it with Ctrl+C. By default \
                  it listens on every network interface; --local limits it to this \
                  machine. Clients need the token it prints (kept in \
                  ~/.config/tarpi/serve-token), unless `serve_trust_all: true` is set in \
                  config.yaml: then anyone who can reach the port is let in. The \
                  connection is a plain, unencrypted TCP link, so only use it on a network you trust.",
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

const MAX_BODY: usize = wire::MAX_FILE;

/// How long a device may be silent (it beats every 50 ms) before the server
/// lets go of it. Longer than the device's own limit: the server only has to
/// free what is dead, the device is the one that has to react quickly.
const SILENCE: Duration = Duration::from_secs(5);

struct State {
    root: PathBuf,
    token: String,
    /// `serve_trust_all`: no token is asked for.
    trust_all: bool,
    scanner: std::sync::Mutex<Scanner>,
    /// Held while a file is checked and changed, so two clients cannot both
    /// pass the "still the version you saw" check.
    write_lock: tokio::sync::Mutex<()>,
    /// Changes whenever a shared file does (see `monitor`); every connected
    /// device is told at once and syncs.
    changes: tokio::sync::watch::Sender<u64>,
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
            session(sock, peer, st).await;
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

/// One device, from its greeting to its goodbye.
async fn session(mut sock: TcpStream, peer: SocketAddr, st: Arc<State>) {
    if !wire::server_handshake(&mut sock).await {
        return;
    }
    let (tx, mut inbox) = wire::start(sock, SILENCE);
    let mut authed = st.trust_all;
    let mut since: Option<std::time::Instant> = None;
    let mut changes = st.changes.subscribe();
    // The state at the start is what the device is about to sync to.
    changes.borrow_and_update();
    if authed {
        println!("{}  {}  connected", stamp(), peer.ip());
        since = Some(std::time::Instant::now());
    }
    loop {
        tokio::select! {
            m = inbox.recv() => {
                let Some(m) = m else {
                    break;
                };
                let op = m.op().to_string();
                match op.as_str() {
                    // Always answered, so the device learns whether a token is wanted.
                    "hello" => {
                        tx.send(Message::ok(
                            m.id,
                            json!({ "tarpi": env!("CARGO_PKG_VERSION"), "proto": PROTO, "token": !st.trust_all }),
                        ));
                    }
                    "auth" => {
                        if authed || token_ok(&st.token, m.str("token")) {
                            if !authed {
                                authed = true;
                                println!("{}  {}  connected", stamp(), peer.ip());
                                since = Some(std::time::Instant::now());
                            }
                            tx.send(Message::ok(m.id, json!({})));
                        } else {
                            println!("{}  {}  refused: missing or wrong token", stamp(), peer.ip());
                            tx.send(Message::fail(m.id, 401, "missing or wrong token"));
                            tx.close(Duration::from_millis(500)).await;
                            break;
                        }
                    }
                    _ if !authed => {
                        println!("{}  {}  refused: missing or wrong token", stamp(), peer.ip());
                        tx.send(Message::fail(m.id, 401, "missing or wrong token"));
                        tx.close(Duration::from_millis(500)).await;
                        break;
                    }
                    _ => {
                        // Answered apart from the loop, so a slow request
                        // (a token count) does not hold up the others.
                        let (st2, tx2) = (st.clone(), tx.clone());
                        tokio::spawn(async move {
                            if let Some(r) = route(&st2, m, peer).await {
                                tx2.send(r);
                            }
                        });
                    }
                }
            }
            // A shared file changed: sync now.
            c = changes.changed(), if authed => {
                if c.is_err() || !tx.send(Message::new(0, json!({ "op": "changed" }), Vec::new())) {
                    break;
                }
            }
        }
    }
    if let Some(since) = since {
        println!("{}  {}  disconnected after {}", stamp(), peer.ip(), how_long(since.elapsed()));
    }
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

/// `event`: print a line about what a `tarpi connect` device did.
fn event(m: &Message, peer: SocketAddr) {
    let Some(text) = m.str("text") else {
        return;
    };
    let text = clean_event(text);
    if !text.is_empty() {
        println!("{}  {}  {}", stamp(), peer.ip(), text);
    }
}

fn stamp() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// Count `text` with this machine's own backend, for a `tarpi connect` device
/// that cannot reach it. The connection is the named one, else the active one.
async fn tokenize(m: &Message) -> Message {
    let Some(text) = m.str("text") else {
        return Message::fail(m.id, 400, "missing text");
    };
    let name = m.str("connection").unwrap_or("");
    let store = match crate::conn::Store::load() {
        Ok(s) => s,
        Err(e) => return Message::fail(m.id, 500, format!("{:#}", e)),
    };
    let conn = store
        .connections
        .iter()
        .find(|c| !name.is_empty() && c.name == name)
        .cloned()
        .or_else(|| store.active_connection());
    let Some(conn) = conn else {
        return Message::fail(m.id, 409, "this server has no API connection");
    };
    match crate::conn::count_tokens(&conn, text).await {
        Ok(n) => Message::ok(m.id, json!({ "tokens": n })),
        Err(e) => Message::fail(m.id, 502, format!("{:#}", e)),
    }
}

/// The answer to one request of an authenticated device (`None`: a note that
/// wants none).
async fn route(st: &Arc<State>, m: Message, peer: SocketAddr) -> Option<Message> {
    let id = m.id;
    let reply = match m.op() {
        "event" => {
            event(&m, peer);
            return None;
        }
        // Anything else without a number cannot be answered.
        _ if id == 0 => return None,
        "manifest" => manifest(st, id).await,
        "tokenize" => tokenize(&m).await,
        "get" => get_file(st, &m),
        "put" => put_file(st, &m, peer).await,
        "delete" => delete_file(st, &m, peer).await,
        other => Message::fail(id, 404, format!("unknown operation {}", other)),
    };
    Some(reply)
}

async fn manifest(st: &Arc<State>, id: u32) -> Message {
    let st2 = st.clone();
    let scanned = tokio::task::spawn_blocking(move || {
        st2.scanner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .scan()
    })
    .await;
    match scanned {
        Ok(files) => {
            let files: Vec<Entry> = files;
            Message::ok(id, json!({})).with_body(serde_json::to_vec(&Manifest { files }).unwrap_or_default())
        }
        Err(_) => Message::fail(id, 500, "scan failed"),
    }
}

/// The `path` of a request, if it names a shared file.
fn shared_path(m: &Message) -> std::result::Result<String, Message> {
    match m.str("path") {
        Some(p) if mirror::valid_path(p) => Ok(p.to_string()),
        _ => Err(Message::fail(m.id, 400, "not a shared file")),
    }
}

fn get_file(st: &Arc<State>, m: &Message) -> Message {
    let rel = match shared_path(m) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let path = mirror::to_fs(&st.root, &rel);
    let Ok(bytes) = std::fs::read(&path) else {
        return Message::fail(m.id, 404, "no such file");
    };
    if bytes.len() > MAX_BODY {
        return Message::fail(m.id, 413, "file too large");
    }
    let mtime = std::fs::metadata(&path)
        .map(|meta| mirror::mtime_ms(&meta))
        .unwrap_or(0);
    Message::ok(m.id, json!({ "mtime": mtime })).with_body(bytes)
}

/// The hash of the file as it is now, `None` when there is none.
fn current_hash(path: &std::path::Path) -> Option<String> {
    std::fs::read(path).ok().map(|b| mirror::hash_bytes(&b))
}

/// `base` of a request: `None` means "the file must not exist".
fn base_of(m: &Message) -> Option<&str> {
    m.str("base").filter(|s| !s.is_empty())
}

/// A refused write or delete: the file is not the version the device saw.
fn changed(id: u32, now: Option<String>) -> Message {
    let mut r = Message::fail(id, 409, "changed");
    r.head["hash"] = json!(now);
    r
}

async fn put_file(st: &Arc<State>, m: &Message, peer: SocketAddr) -> Message {
    let rel = match shared_path(m) {
        Ok(p) => p,
        Err(r) => return r,
    };
    if m.body.len() > MAX_BODY {
        return Message::fail(m.id, 413, "file too large");
    }
    let path = mirror::to_fs(&st.root, &rel);
    let mtime: Option<u64> = m.head.get("mtime").and_then(|v| v.as_u64());

    let _guard = st.write_lock.lock().await;
    let now = current_hash(&path);
    if now.as_deref() != base_of(m) {
        println!("{}  {}  refused {}: it changed here in the meantime", stamp(), peer.ip(), mirror::describe(&rel));
        return changed(m.id, now);
    }
    if let Err(e) = mirror::write_atomic(&path, &m.body, mtime) {
        return Message::fail(m.id, 500, format!("{:#}", e));
    }
    // A file that did not exist yet is a new chat, a new character (imported
    // on the device) and so on; otherwise it is a change to an existing one.
    let verb = if now.is_none() { "added" } else { "updated" };
    println!("{}  {}  {} {} (uploaded)", stamp(), peer.ip(), verb, mirror::describe(&rel));
    Message::ok(m.id, json!({ "hash": mirror::hash_bytes(&m.body), "mtime": mtime }))
}

async fn delete_file(st: &Arc<State>, m: &Message, peer: SocketAddr) -> Message {
    let rel = match shared_path(m) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let path = mirror::to_fs(&st.root, &rel);

    let _guard = st.write_lock.lock().await;
    let Some(now) = current_hash(&path) else {
        return Message::ok(m.id, json!({ "deleted": false }));
    };
    if Some(now.as_str()) != base_of(m) {
        return changed(m.id, Some(now));
    }
    if let Err(e) = std::fs::remove_file(&path) {
        return Message::fail(m.id, 500, format!("{}", e));
    }
    println!("{}  {}  deleted {}", stamp(), peer.ip(), mirror::describe(&rel));
    Message::ok(m.id, json!({ "deleted": true }))
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

    #[test]
    fn tokens_are_compared_whole() {
        assert!(token_ok("abc", Some("abc")));
        assert!(!token_ok("abc", Some("abd")));
        assert!(!token_ok("abc", Some("ab")));
        assert!(!token_ok("abc", None));
    }
}
