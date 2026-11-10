//! `tarpi connect [token]`: use the chats, characters, personas, system
//! prompts and API connection of the machine that runs `tarpi
//! serve` (`serve_url` in `config.yaml`), from this device. It first asks the
//! server whether it wants a token (that is the server's `serve_trust_all`),
//! and only asks for one when it does.
//!
//! tarpi keeps working on plain files, so this device keeps a **copy** of the
//! server's data in `~/.config/tarpi/remote/<host>-<port>/` and the chat screen
//! runs on that copy exactly as it does on local data. The copy is brought up
//! to date before the screen opens, kept in step every few seconds while it is
//! open, and synced once more when you quit:
//!
//! - a file only the server changed is downloaded, a file only this device
//!   changed (or created) is uploaded, deletes are passed on;
//! - a file changed on both sides keeps the newer version, and the older one is
//!   kept next to it as `<file>.conflict-<time>` (not shown by tarpi, not shared);
//! - a file that one side changed and the other deleted is never deleted.
//!
//! What was in step the last time is remembered in `.sync-state.json` inside
//! the copy. `config.yaml`, the command history and the spell-check dictionary
//! stay on each device; `--local-api` keeps `api.yaml` on this device too (a
//! phone usually cannot reach a model at the server's `127.0.0.1`).
//!
//! An open chat is not reloaded when another device changes it: saving it here
//! is then refused, as when another terminal changed the file (reopen the chat).

use crate::config;
use crate::mirror::{
    hash_bytes, in_scope, mtime_ms, now_ms, plan, to_fs, write_atomic, Action, Entry, Manifest,
    Scanner, DEFAULT_PORT, PROTO,
};
use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use reqwest::{Method, StatusCode};
use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Seconds between two background syncs.
const SYNC_EVERY: u64 = 3;

#[derive(Parser, Debug)]
#[command(
    name = "tarpi connect",
    about = "Chat with the characters and chats of the machine named by `serve_url` in config.yaml",
    long_about = "Connect to the machine that runs `tarpi serve` (its address is `serve_url` \
                  in ~/.config/tarpi/config.yaml) and use its characters, chats, personas, \
                  system prompts and API connection from this device (formatting stays on each device). A copy of \
                  them is kept in ~/.config/tarpi/remote/ and brought up to date before the \
                  chat opens, every few seconds while it is open, and when you quit. Then \
                  you pick a character, as in plain `tarpi`. The token is the one `tarpi \
                  serve` prints; it is remembered for next time. tarpi asks the server whether \
                  it needs one, so with `serve_trust_all: true` on the server no token is asked for.",
    after_help = concat!(
        "EXAMPLES:\n",
        "  tarpi connect                 serve_url is set; the token is remembered (or not needed)\n",
        "  tarpi connect 1a2b3c...       give the token (first time, or after --new-token)\n",
        "  tarpi connect --local-api     keep this device's own API connection\n",
        "\n",
        "In ~/.config/tarpi/config.yaml:\n",
        "  serve_url: 192.168.1.20:7878"
    )
)]
struct ConnectCli {
    /// The token `tarpi serve` printed (remembered after the first time; not
    /// asked for when the server needs none)
    #[arg(env = "TARPI_TOKEN", hide_env_values = true)]
    token: Option<String>,

    /// Do not share api.yaml: this device keeps its own API connections
    #[arg(long)]
    local_api: bool,
}

/// `host:port` becomes `http://host:port` (and the default port is added when
/// there is none).
fn normalize(address: &str) -> String {
    let t = address.trim().trim_end_matches('/');
    let lower = t.to_lowercase();
    let (scheme, rest) = if lower.starts_with("http://") {
        ("http://", &t[7..])
    } else if lower.starts_with("https://") {
        ("https://", &t[8..])
    } else {
        ("http://", t)
    };
    let host = rest.split('/').next().unwrap_or("");
    let has_port = if host.starts_with('[') {
        host.contains("]:")
    } else {
        host.contains(':')
    };
    if has_port {
        format!("{}{}", scheme, rest)
    } else {
        format!("{}{}:{}", scheme, rest, DEFAULT_PORT)
    }
}

/// A folder name for a server: `192.168.1.20:7878` becomes `192.168.1.20-7878`.
fn mirror_name(base: &str) -> String {
    let rest = base.split_once("://").map(|(_, r)| r).unwrap_or(base);
    let host = rest.split('/').next().unwrap_or("");
    host.chars()
        .map(|c| match c {
            c if c.is_ascii_alphanumeric() || c == '.' || c == '-' => c,
            ':' => '-',
            _ => '_',
        })
        .collect()
}

#[cfg(unix)]
fn private(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn private(_path: &std::path::Path) {}

fn ask_token() -> Result<String> {
    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        bail!("no token: run `tarpi connect <token>` (or set TARPI_TOKEN); `tarpi serve` prints it");
    }
    print!("Token (printed by `tarpi serve`): ");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let t = line.trim().to_string();
    if t.is_empty() {
        bail!("no token given");
    }
    Ok(t)
}

pub struct Stats {
    pub pulled: usize,
    pub pushed: usize,
    pub deleted: usize,
    pub conflicts: usize,
}

impl Stats {
    /// "2 downloaded, 1 uploaded", `None` when the round changed nothing.
    fn describe(&self) -> Option<String> {
        let mut parts = Vec::new();
        for (n, what) in [
            (self.pulled, "downloaded"),
            (self.pushed, "uploaded"),
            (self.deleted, "deleted"),
            (self.conflicts, "changed on both sides"),
        ] {
            if n > 0 {
                parts.push(format!("{} {}", n, what));
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(", "))
        }
    }
}

/// Where [`report`] sends its lines; set while `tarpi connect` is running.
static REPORTER: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>> =
    std::sync::Mutex::new(None);

/// Tell the server's log what this device just did ("opened chat Sofia03 with
/// Sofia"). Does nothing outside `tarpi connect`, never waits, and a server
/// that does not know the route (an older tarpi) or is gone is ignored. Lines
/// are sent in the order they were reported.
pub fn report(text: impl Into<String>) {
    if let Some(tx) = REPORTER.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.send(text.into());
    }
}

/// The state of one connection: where the server is, where the copy is, and
/// what both sides agreed on last time.
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
    root: PathBuf,
    api: bool,
    state: HashMap<String, String>,
    scanner: Arc<std::sync::Mutex<Scanner>>,
}

/// Turn a refused request into an error that says why.
async fn ok(resp: reqwest::Response) -> Result<reqwest::Response> {
    let st = resp.status();
    if st.is_success() {
        return Ok(resp);
    }
    if st == StatusCode::UNAUTHORIZED {
        bail!(
            "the server wants a valid token: run `tarpi connect <token>` with the one \
             `tarpi serve` prints (or set `serve_trust_all: true` on the serving machine)"
        );
    }
    let body = resp.text().await.unwrap_or_default();
    bail!("the server answered {}: {}", st, body.trim())
}

impl Client {
    fn new(base: String, token: String, root: PathBuf, api: bool) -> Result<Client> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()
            .context("building HTTP client")?;
        let state = std::fs::read_to_string(root.join(".sync-state.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<HashMap<String, String>>(&t).ok())
            .unwrap_or_default();
        Ok(Client {
            http,
            base,
            token,
            scanner: Arc::new(std::sync::Mutex::new(Scanner::new(root.clone(), api))),
            root,
            api,
            state,
        })
    }

    fn req(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let r = self.http.request(method, format!("{}{}", self.base, path));
        // No token at all is what `serve_trust_all` is for.
        if self.token.is_empty() {
            r
        } else {
            r.bearer_auth(&self.token)
        }
    }

    /// Tell the server's log something (see `report`); a failure is ignored.
    pub async fn event(&self, text: &str) {
        let _ = self
            .req(Method::POST, "/v1/event")
            .timeout(Duration::from_secs(5))
            .json(&serde_json::json!({ "text": text }))
            .send()
            .await;
    }

    /// Ask the server, without a token, whether it wants one. A server that
    /// answers 401 wants one (older versions); a server that answers the ping
    /// says so itself (`token`, false with `serve_trust_all: true`).
    async fn needs_token(&self) -> Result<bool> {
        let resp = self
            .http
            .get(format!("{}/v1/ping", self.base))
            .send()
            .await
            .with_context(|| format!("could not reach tarpi serve at {}", self.base))?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            return Ok(true);
        }
        let v: serde_json::Value = ok(resp)
            .await?
            .json()
            .await
            .with_context(|| format!("{} did not answer like tarpi serve", self.base))?;
        Ok(v.get("token").and_then(|t| t.as_bool()).unwrap_or(false))
    }

    /// Reach the server and check the token and the protocol version.
    async fn ping(&self) -> Result<()> {
        let resp = self
            .req(Method::GET, "/v1/ping")
            .send()
            .await
            .with_context(|| format!("could not reach tarpi serve at {}", self.base))?;
        let v: serde_json::Value = ok(resp)
            .await?
            .json()
            .await
            .with_context(|| format!("{} did not answer like tarpi serve", self.base))?;
        let proto = v.get("proto").and_then(|p| p.as_u64()).unwrap_or(0);
        if proto != PROTO as u64 {
            bail!(
                "the server speaks protocol {} and this tarpi {}: update the older one",
                proto,
                PROTO
            );
        }
        Ok(())
    }

    async fn remote_manifest(&self) -> Result<HashMap<String, Entry>> {
        let resp = self
            .req(Method::GET, "/v1/manifest")
            .send()
            .await
            .with_context(|| format!("could not reach tarpi serve at {}", self.base))?;
        let m: Manifest = ok(resp)
            .await?
            .json()
            .await
            .context("reading the server's file list")?;
        Ok(m.files
            .into_iter()
            .filter(|e| in_scope(&e.path, self.api))
            .map(|e| (e.path.clone(), e))
            .collect())
    }

    /// A file's bytes and modification time.
    async fn download(&self, rel: &str) -> Result<(Vec<u8>, u64)> {
        let resp = self
            .req(Method::GET, "/v1/file")
            .query(&[("path", rel)])
            .send()
            .await
            .with_context(|| format!("downloading {}", rel))?;
        let resp = ok(resp).await?;
        let mtime = resp
            .headers()
            .get("x-tarpi-mtime")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or_else(now_ms);
        let bytes = resp
            .bytes()
            .await
            .with_context(|| format!("downloading {}", rel))?;
        Ok((bytes.to_vec(), mtime))
    }

    /// Send a file, provided the server's copy still has hash `base` (`None`:
    /// provided it has none). `false` when it changed in the meantime.
    async fn upload(&self, rel: &str, bytes: Vec<u8>, base: Option<&str>, mtime: u64) -> Result<bool> {
        let mt = mtime.to_string();
        let resp = self
            .req(Method::PUT, "/v1/file")
            .query(&[("path", rel), ("base", base.unwrap_or("")), ("mtime", mt.as_str())])
            .body(bytes)
            .send()
            .await
            .with_context(|| format!("uploading {}", rel))?;
        if resp.status() == StatusCode::CONFLICT {
            return Ok(false);
        }
        let _: serde_json::Value = ok(resp)
            .await?
            .json()
            .await
            .with_context(|| format!("uploading {}", rel))?;
        Ok(true)
    }

    /// Delete a file on the server, provided it still has hash `base`.
    async fn delete_remote(&self, rel: &str, base: Option<&str>) -> Result<bool> {
        let resp = self
            .req(Method::DELETE, "/v1/file")
            .query(&[("path", rel), ("base", base.unwrap_or(""))])
            .send()
            .await
            .with_context(|| format!("deleting {}", rel))?;
        if resp.status() == StatusCode::CONFLICT {
            return Ok(false);
        }
        ok(resp).await?;
        Ok(true)
    }

    async fn pull(&mut self, rel: &str) -> Result<()> {
        let (bytes, mtime) = self.download(rel).await?;
        write_atomic(&to_fs(&self.root, rel), &bytes, Some(mtime))?;
        self.state.insert(rel.to_string(), hash_bytes(&bytes));
        Ok(())
    }

    async fn push(&mut self, rel: &str, remote_hash: Option<String>) -> Result<()> {
        let path = to_fs(&self.root, rel);
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let mtime = std::fs::metadata(&path)
            .map(|m| mtime_ms(&m))
            .unwrap_or_else(|_| now_ms());
        let hash = hash_bytes(&bytes);
        if self.upload(rel, bytes, remote_hash.as_deref(), mtime).await? {
            self.state.insert(rel.to_string(), hash);
        }
        // Not accepted: the server's copy changed since the listing. The next
        // round sees both versions and settles it.
        Ok(())
    }

    /// Keep this device's version of a file before the server's replaces it.
    fn backup_local(&self, rel: &str) -> Result<()> {
        let path = to_fs(&self.root, rel);
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let copy = path.with_file_name(format!("{}.conflict-{}", name, now_ms()));
        std::fs::copy(&path, &copy)
            .with_context(|| format!("keeping a copy of {} before replacing it", rel))?;
        Ok(())
    }

    /// Keep the server's version of a file here before this device's replaces it.
    async fn backup_remote(&self, rel: &str) -> Result<()> {
        let (bytes, _) = self.download(rel).await?;
        let path = to_fs(&self.root, rel);
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let copy = path.with_file_name(format!("{}.conflict-{}", name, now_ms()));
        write_atomic(&copy, &bytes, None)
    }

    async fn apply(
        &mut self,
        action: Action,
        local: &HashMap<String, Entry>,
        remote: &HashMap<String, Entry>,
        stats: &mut Stats,
    ) -> Result<()> {
        match action {
            Action::Settled(p) => match local.get(&p) {
                Some(e) => {
                    self.state.insert(p, e.hash.clone());
                }
                None => {
                    self.state.remove(&p);
                }
            },
            Action::Pull(p) => {
                self.pull(&p).await?;
                stats.pulled += 1;
            }
            Action::ConflictPull(p) => {
                self.backup_local(&p)?;
                self.pull(&p).await?;
                stats.pulled += 1;
                stats.conflicts += 1;
            }
            Action::DeleteLocal(p) => {
                match std::fs::remove_file(to_fs(&self.root, &p)) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e).with_context(|| format!("deleting {}", p)),
                }
                self.state.remove(&p);
                stats.deleted += 1;
            }
            Action::Push(p) => {
                let rh = remote.get(&p).map(|e| e.hash.clone());
                self.push(&p, rh).await?;
                stats.pushed += 1;
            }
            Action::ConflictPush(p) => {
                self.backup_remote(&p).await?;
                let rh = remote.get(&p).map(|e| e.hash.clone());
                self.push(&p, rh).await?;
                stats.pushed += 1;
                stats.conflicts += 1;
            }
            Action::DeleteRemote(p) => {
                let rh = remote.get(&p).map(|e| e.hash.clone());
                if self.delete_remote(&p, rh.as_deref()).await? {
                    self.state.remove(&p);
                    stats.deleted += 1;
                }
            }
        }
        Ok(())
    }

    fn save_state(&self) {
        if let Ok(text) = serde_json::to_vec(&self.state) {
            let _ = write_atomic(&self.root.join(".sync-state.json"), &text, None);
        }
    }

    /// One round: compare, then bring both sides level. Carries on past a file
    /// that fails (and reports the first failure at the end).
    pub async fn cycle(&mut self) -> Result<Stats> {
        let remote = self.remote_manifest().await?;
        if remote.is_empty() && !self.state.is_empty() {
            bail!(
                "the server lists no files, but this copy has been in step with it before; \
                 nothing is deleted (is tarpi serve using another config folder?)"
            );
        }
        let sc = self.scanner.clone();
        let found = tokio::task::spawn_blocking(move || {
            sc.lock().unwrap_or_else(|e| e.into_inner()).scan()
        })
        .await
        .context("listing the local files")?;
        let local: HashMap<String, Entry> =
            found.into_iter().map(|e| (e.path.clone(), e)).collect();

        let actions = plan(&self.state, &local, &remote);
        let mut stats = Stats {
            pulled: 0,
            pushed: 0,
            deleted: 0,
            conflicts: 0,
        };
        let mut first_err: Option<anyhow::Error> = None;
        for a in actions {
            if let Err(e) = self.apply(a, &local, &remote, &mut stats).await {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
        self.save_state();
        match first_err {
            Some(e) => Err(e),
            None => Ok(stats),
        }
    }
}

/// Set by the watchdog when `tarpi serve` stopped answering (see `run`).
static LOST: AtomicBool = AtomicBool::new(false);

/// `true` once the server this device is connected to has gone away. The chat
/// screen checks it and quits.
pub fn server_lost() -> bool {
    LOST.load(Ordering::Relaxed)
}

/// How often the server is asked whether it is still there, how long one
/// answer may take, and how many misses in a row mean it is gone.
const WATCH_EVERY: Duration = Duration::from_secs(2);
const WATCH_TIMEOUT: Duration = Duration::from_secs(3);
const WATCH_MISSES: u32 = 3;
/// With the persistent connection (`watch`): the server sends a byte every
/// 50 ms and so does this device, so this long without one from the server
/// means the link is dead.
const BEAT_TIMEOUT: Duration = Duration::from_millis(300);
const BEAT_SEND: Duration = Duration::from_millis(50);

/// Set when the server says a shared file changed, so the sync runs at once.
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();
/// The persistent connection is up: the server tells us about changes, so the
/// slow poll is only a safety net.
static WATCH_ON: AtomicBool = AtomicBool::new(false);

enum Watched {
    /// The connection closed, failed or went silent.
    Lost,
    /// The server does not know `/v1/watch` (an older tarpi), or it is not plain HTTP.
    Unsupported,
}

/// Hold one TCP connection to `tarpi serve` open (`GET /v1/watch`) and return
/// when it is gone. A closed or reset connection is noticed at once; a link
/// that just goes quiet (wifi dropped) within [`BEAT_TIMEOUT`].
async fn watch_connection(base: &str, token: &str) -> Watched {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let Some(hostport) = base.strip_prefix("http://") else {
        return Watched::Unsupported;
    };
    let hostport = hostport.trim_end_matches('/');
    let Ok(Ok(mut sock)) = tokio::time::timeout(WATCH_TIMEOUT, tokio::net::TcpStream::connect(hostport)).await else {
        return Watched::Lost;
    };
    let _ = sock.set_nodelay(true);
    let mut req = format!("GET /v1/watch HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n", hostport);
    if !token.is_empty() {
        req.push_str(&format!("Authorization: Bearer {}\r\n", token));
    }
    req.push_str("\r\n");
    if sock.write_all(req.as_bytes()).await.is_err() {
        return Watched::Lost;
    }
    let mut head: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 256];
    let mut checked = false;
    loop {
        match tokio::time::timeout(BEAT_TIMEOUT.max(WATCH_TIMEOUT), sock.read(&mut chunk)).await {
            Ok(Ok(n)) if n > 0 => {
                if !checked {
                    head.extend_from_slice(&chunk[..n]);
                    if head.windows(4).any(|w| w == b"\r\n\r\n") {
                        // Anything but 200 (404 from an older server, 401 ...): not a watch.
                        if !head.starts_with(b"HTTP/1.1 200") {
                            return Watched::Unsupported;
                        }
                        checked = true;
                    }
                }
            }
            _ => return Watched::Lost,
        }
        if checked {
            break;
        }
    }
    // From here on: a byte each way every 50 ms. Writing fails at once when this
    // device has no route any more (its wifi went off); the server's bytes
    // stopping for BEAT_TIMEOUT or its end closing covers the rest.
    let (mut rd, mut wr) = sock.into_split();
    let reader = async {
        loop {
            match tokio::time::timeout(BEAT_TIMEOUT, rd.read(&mut chunk)).await {
                Ok(Ok(n)) if n > 0 => {
                    if chunk[..n].contains(&b'c') {
                        WAKE.notify_one();
                    }
                }
                _ => break,
            }
        }
    };
    let writer = async {
        loop {
            tokio::time::sleep(BEAT_SEND).await;
            if wr.write_all(b"\n").await.is_err() {
                break;
            }
        }
    };
    WATCH_ON.store(true, Ordering::Relaxed);
    tokio::select! {
        _ = reader => {}
        _ = writer => {}
    }
    WATCH_ON.store(false, Ordering::Relaxed);
    Watched::Lost
}

/// The background sync of a running chat screen.
pub struct Session {
    client: Arc<tokio::sync::Mutex<Client>>,
    task: tokio::task::JoinHandle<()>,
    watchdog: tokio::task::JoinHandle<()>,
    reporter: tokio::task::JoinHandle<()>,
}

impl Session {
    /// Stop the background sync and sync one last time (after the screen has
    /// been closed, so a failure may be printed).
    pub async fn finish(self) {
        self.task.abort();
        if server_lost() {
            self.watchdog.abort();
            self.reporter.abort();
            let c = self.client.lock().await;
            eprintln!(
                "tarpi: lost the connection to {} (is `tarpi serve` still running?); quit.\n\
                 (your changes are kept here and are sent next time you connect)",
                c.base
            );
            return;
        }
        {
            let mut c = self.client.lock().await;
            match c.cycle().await {
                Ok(s) => {
                    if let Some(t) = s.describe() {
                        report(format!("synced on quit: {}", t));
                    }
                }
                Err(e) => eprintln!(
                    "tarpi: the last sync with {} failed: {:#}\n(your changes are kept here and are sent next time you connect)",
                    c.base, e
                ),
            }
        }
        // Let the server hear what is still queued, then close the live link
        // (the server logs that the device left).
        *REPORTER.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let _ = tokio::time::timeout(Duration::from_secs(3), self.reporter).await;
        self.watchdog.abort();
    }
}

/// One line about how the last background sync went, for whoever wonders
/// (`~/.config/tarpi/remote/<server>/sync.log`). Nothing may be printed while
/// the chat screen is open.
fn log_status(root: &std::path::Path, res: &Result<Stats>) {
    let when = chrono::Local::now().format("%F %T");
    let line = match res {
        Ok(s) => format!(
            "{} ok: {} downloaded, {} uploaded, {} deleted, {} conflicts\n",
            when, s.pulled, s.pushed, s.deleted, s.conflicts
        ),
        Err(e) => format!("{} error: {:#}\n", when, e),
    };
    let _ = std::fs::write(root.join("sync.log"), line);
}

/// One number for the shared files here (see `mirror::digest`).
async fn local_digest(scanner: &Arc<std::sync::Mutex<Scanner>>) -> u64 {
    let sc = scanner.clone();
    tokio::task::spawn_blocking(move || {
        crate::mirror::digest(&sc.lock().unwrap_or_else(|e| e.into_inner()).scan())
    })
    .await
    .unwrap_or(0)
}

/// Connect, bring the local copy up to date, and start keeping it in step.
/// Returns the running sync; the caller then opens the chat as usual and calls
/// `Session::finish` after it.
/// A reached and authenticated server, with this device's copy of its data
/// set up as the folder every store reads and writes.
struct Link {
    client: Client,
    base: String,
    token: String,
}

/// Find the server named by `serve_url`, make sure of the token, set the copy
/// in `~/.config/tarpi/remote/` as the data folder and check the server answers.
async fn open(token_arg: Option<String>, local_api: bool) -> Result<Link> {
    let cfg = config::load()?;
    let address = cfg
        .serve_url
        .clone()
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .ok_or_else(|| {
            anyhow!(
                "no server address: set `serve_url: host:port` in {}",
                config::config_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "config.yaml".to_string())
            )
        })?;
    let base = normalize(&address);

    let local = config::local_dir().ok_or_else(|| anyhow!("no config directory on this system"))?;
    let root = local.join("remote").join(mirror_name(&base));
    std::fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;

    // From here on every store reads and writes the copy.
    config::set_data_dir(root.clone());
    config::set_api_local(local_api);

    let mut client = Client::new(base.clone(), String::new(), root.clone(), !local_api)?;

    // Ask the server whether it wants a token. If not, none is asked for or
    // sent. If so: the one given now, else the one remembered, else ask.
    let token_file = root.join(".token");
    let (token, is_new) = if !client.needs_token().await? {
        (String::new(), false)
    } else {
        match token_arg.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
            Some(t) => (t, true),
            None => match std::fs::read_to_string(&token_file)
                .ok()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
            {
                Some(t) => (t, false),
                None => (ask_token()?, true),
            },
        }
    };
    client.token = token.clone();
    client.ping().await?;
    if is_new {
        let _ = std::fs::write(&token_file, format!("{}\n", token));
        private(&token_file);
    }
    Ok(Link { client, base, token })
}

/// `tarpi import --remote`: reach the server, bring the local copy up to date
/// (so a card name that is taken there is noticed) and return the connection.
/// From here on the stores read and write the copy; the caller imports, then
/// calls `Client::cycle` to upload it.
pub async fn open_remote() -> Result<Client> {
    let token = std::env::var("TARPI_TOKEN").ok();
    let Link { mut client, base, .. } = open(token, false).await?;
    eprintln!("Syncing with {} ...", base);
    client.cycle().await?;
    Ok(client)
}

pub async fn run() -> Result<Session> {
    // argv is `tarpi connect ...`; drop the program name so "connect" plays
    // the part of the binary name in clap's messages.
    let cli = ConnectCli::parse_from(std::env::args().skip(1));
    let Link { mut client, base, token } = open(cli.token.clone(), cli.local_api).await?;
    let root = client.root.clone();
    // The shared connection's address belongs to the server: it counts tokens,
    // and a loopback / 0.0.0.0 address is read as the server's host.
    if !cli.local_api {
        crate::conn::set_remote_tokenizer(base.clone(), token.clone());
        crate::conn::set_remote_host(&base);
    }

    // What this device does is told to the server's log (see `report`).
    let (report_tx, mut report_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    *REPORTER.lock().unwrap_or_else(|e| e.into_inner()) = Some(report_tx);
    let reporter = {
        let (http, url, tok) = (client.http.clone(), format!("{}/v1/event", base), token.clone());
        tokio::spawn(async move {
            while let Some(text) = report_rx.recv().await {
                let mut rb = http
                    .post(&url)
                    .timeout(Duration::from_secs(5))
                    .json(&serde_json::json!({ "text": text }));
                if !tok.is_empty() {
                    rb = rb.bearer_auth(&tok);
                }
                let _ = rb.send().await;
            }
        })
    };

    println!("Syncing with {} ...", base);
    let stats = client.cycle().await?;
    report(match stats.describe() {
        Some(t) => format!("first sync: {}", t),
        None => "first sync: already in step".to_string(),
    });
    println!(
        "{} downloaded, {} uploaded, {} deleted{}",
        stats.pulled,
        stats.pushed,
        stats.deleted,
        if stats.conflicts > 0 {
            format!(
                ", {} changed on both sides (the older version is kept as <file>.conflict-<time>)",
                stats.conflicts
            )
        } else {
            String::new()
        }
    );

    let scanner = client.scanner.clone();
    let client = Arc::new(tokio::sync::Mutex::new(client));
    let bg = client.clone();
    let log_root = root.clone();
    // Sync as soon as something changed: here (looked at every 200 ms) or on
    // the server (it tells us over the watch connection). Without the watch
    // connection (an older server), and as a safety net with it, every so often.
    let task = tokio::spawn(async move {
        let mut last_err: Option<String> = None;
        let mut last = local_digest(&scanner).await;
        let mut since = std::time::Instant::now();
        loop {
            let every = if WATCH_ON.load(Ordering::Relaxed) {
                Duration::from_secs(30)
            } else {
                Duration::from_secs(SYNC_EVERY)
            };
            tokio::select! {
                _ = WAKE.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(200)) => {
                    if local_digest(&scanner).await == last && since.elapsed() < every {
                        continue;
                    }
                }
            }
            let res = bg.lock().await.cycle().await;
            log_status(&log_root, &res);
            match &res {
                Ok(s) => {
                    last_err = None;
                    if let Some(t) = s.describe() {
                        report(format!("synced: {}", t));
                    }
                }
                // The same failure every few seconds is told once.
                Err(e) => {
                    let m = format!("sync failed: {:#}", e);
                    if last_err.as_deref() != Some(m.as_str()) {
                        report(m.clone());
                    }
                    last_err = Some(m);
                }
            }
            // What the sync itself downloaded is not a new change.
            last = local_digest(&scanner).await;
            since = std::time::Instant::now();
        }
    });

    // Keep an eye on the server apart from the sync (which can sit in a long
    // request): when it stops answering, the chat screen quits.
    let ping_url = format!("{}/v1/ping", base);
    let (watch_base, watch_token) = (base.clone(), token.clone());
    let watchdog = tokio::spawn(async move {
        // The persistent connection first: it notices a loss at once. An older
        // server without it is watched by asking every few seconds instead.
        match watch_connection(&watch_base, &watch_token).await {
            Watched::Lost => {
                LOST.store(true, Ordering::Relaxed);
                return;
            }
            Watched::Unsupported => {}
        }
        let Ok(http) = reqwest::Client::builder().timeout(WATCH_TIMEOUT).build() else {
            return;
        };
        let mut misses = 0u32;
        loop {
            tokio::time::sleep(WATCH_EVERY).await;
            // Any answer, even a refusal, means the server is there.
            match http.get(&ping_url).send().await {
                Ok(_) => misses = 0,
                Err(_) => {
                    misses += 1;
                    if misses >= WATCH_MISSES {
                        LOST.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
        }
    });

    Ok(Session { client, task, watchdog, reporter })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_get_a_scheme_and_a_port() {
        assert_eq!(normalize("192.168.1.20:9000"), "http://192.168.1.20:9000");
        assert_eq!(normalize("192.168.1.20"), "http://192.168.1.20:7878");
        assert_eq!(normalize("http://host:1/"), "http://host:1");
        assert_eq!(normalize("https://host"), "https://host:7878");
        assert_eq!(normalize("[::1]:5"), "http://[::1]:5");
        assert_eq!(normalize("[::1]"), "http://[::1]:7878");
    }

    #[test]
    fn mirror_folders_are_plain_names() {
        assert_eq!(mirror_name("http://192.168.1.20:7878"), "192.168.1.20-7878");
        assert_eq!(mirror_name("http://[::1]:5"), "_--1_-5");
        assert_eq!(mirror_name("http://phone.local:1/x"), "phone.local-1");
    }
}
