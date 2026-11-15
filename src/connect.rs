//! `tarpi connect [token]`: use the chats, characters, personas, lorebooks,
//! system prompts and API connection of the machine that runs `tarpi
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
//! - a file that one side changed and the other deleted is never deleted;
//! - a file that cannot be read (here or on the server) is not "deleted" either:
//!   it is left alone for that round (see `mirror::Scanner::unreadable`), and a
//!   copy that lists no files at all is filled from the server again instead of
//!   emptying the server;
//! - a file changed here after the listing is not replaced or deleted by the
//!   round that made the listing (see `copy_unchanged`).
//!
//! What was in step the last time is remembered in `.sync-state.json` inside
//! the copy. `config.yaml`, the command history and the spell-check dictionary
//! stay on each device; `--local-api` keeps `api.yaml` on this device too (a
//! phone usually cannot reach a model at the server's `127.0.0.1`).
//!
//! An open chat is not reloaded when another device changes it: saving it here
//! is then refused, as when another terminal changed the file (reopen the chat).
//!
//! Everything goes over one persistent TCP connection (see `wire`): the sync
//! requests, the token counts, the log lines for the server, the server's
//! "a file changed" notes and a beat in both directions. When that link goes
//! (the server stops, the network drops) the chat screen quits at once.

use crate::config;
use crate::mirror::{
    hash_bytes, in_scope, is_skipped, mtime_ms, now_ms, plan, to_fs, write_atomic, Action, Entry,
    Manifest, Scanner, DEFAULT_PORT, PROTO,
};
use crate::wire::{self, Message};
use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

#[derive(Parser, Debug)]
#[command(
    name = "tarpi connect",
    about = "Chat with the characters and chats of the machine named by `serve_url` (under `sync:`) in config.yaml",
    long_about = "Connect to the machine that runs `tarpi serve` (its address is `serve_url` \
                  in ~/.config/tarpi/config.yaml) and use its characters, chats, personas, \
                  lorebooks, system prompts, vector memory settings and API connection from this device (formatting stays on each device). A copy of \
                  them is kept in ~/.config/tarpi/remote/ and brought up to date before the \
                  chat opens, every few seconds while it is open, and when you quit. Then \
                  you pick a character, as in plain `tarpi`. The token is the one `tarpi \
                  serve` prints; it is remembered for next time. tarpi asks the server whether \
                  it needs one, so with `serve_trust_all: true` (under `sync:`) on the server no token is asked for.",
    after_help = concat!(
        "EXAMPLES:\n",
        "  tarpi connect                 serve_url is set; the token is remembered (or not needed)\n",
        "  tarpi connect 1a2b3c...       give the token (first time, or after --new-token)\n",
        "  tarpi connect --local-api     keep this device's own API connection\n",
        "\n",
        "In ~/.config/tarpi/config.yaml:\n",
        "  sync:\n    serve_url: 192.168.1.20:7878"
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

/// The server's address as `host:port` (the default port is added when there
/// is none; a leading `http://` or `tcp://`, as older versions of this setting
/// were written, is ignored).
fn normalize(address: &str) -> String {
    let t = address.trim().trim_end_matches('/');
    let rest = t.split_once("://").map(|(_, r)| r).unwrap_or(t);
    let host = rest.split('/').next().unwrap_or("");
    let has_port = if host.starts_with('[') {
        host.contains("]:")
    } else {
        host.contains(':')
    };
    if has_port {
        host.to_string()
    } else {
        format!("{}:{}", host, DEFAULT_PORT)
    }
}

/// A folder name for a server: `192.168.1.20:7878` becomes `192.168.1.20-7878`.
fn mirror_name(addr: &str) -> String {
    let host = addr.split('/').next().unwrap_or("");
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
/// that does not know the note (an older tarpi) or is gone is ignored. Lines
/// are sent in the order they were reported.
pub fn report(text: impl Into<String>) {
    if let Some(tx) = REPORTER.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = tx.send(text.into());
    }
}

/// How long one request may wait for its answer.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// How long the server's beats may stay away before the link counts as dead:
/// with the beat every 50 ms, a link that goes quiet (wifi dropped) is
/// noticed within this time.
const BEAT_TIMEOUT: Duration = Duration::from_millis(300);

struct RpcInner {
    tx: wire::Sender,
    /// The binding of an encrypted link (what the token proofs are tied to).
    binding: Option<[u8; 32]>,
    /// Requests waiting for their answer, by number.
    pending: std::sync::Mutex<HashMap<u32, oneshot::Sender<Message>>>,
    next: AtomicU32,
}

/// The persistent connection to `tarpi serve`: requests go out numbered and
/// their answers find their way back to whoever asked, so a sync, a token
/// count and a log line can all use the one link at the same time. Cheap to
/// clone.
#[derive(Clone)]
pub struct Rpc(Arc<RpcInner>);

impl Rpc {
    /// Open the connection, do the greeting and start listening. The server's
    /// "a file changed" notes wake the sync; the end of the link sets [`LOST`].
    async fn connect(addr: &str, encrypted: bool) -> Result<Rpc> {
        let mut sock = tokio::time::timeout(Duration::from_secs(10), tokio::net::TcpStream::connect(addr))
            .await
            .map_err(|_| anyhow!("timed out reaching tarpi serve at {}", addr))?
            .with_context(|| format!("could not reach tarpi serve at {}", addr))?;
        let link = match wire::client_handshake(&mut sock, encrypted).await {
            Ok(link) => link,
            Err(e) => {
                // Two tarpis that are set differently about `encrypted` did
                // answer like tarpi serve: say what to change instead.
                if let Some(m) = e.downcast_ref::<wire::EncryptionMismatch>() {
                    bail!("{}", m.explain(addr));
                }
                return Err(e.context(format!("{} did not answer like tarpi serve", addr)));
            }
        };
        let binding = link.binding();
        let (tx, mut inbox) = wire::start(sock, BEAT_TIMEOUT, link);
        let inner = Arc::new(RpcInner {
            tx,
            binding,
            pending: std::sync::Mutex::new(HashMap::new()),
            next: AtomicU32::new(1),
        });
        let dispatch = inner.clone();
        tokio::spawn(async move {
            while let Some(m) = inbox.recv().await {
                if m.id == 0 {
                    if m.op() == "changed" {
                        WAKE.notify_one();
                    }
                    continue;
                }
                let waiter = dispatch.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&m.id);
                if let Some(w) = waiter {
                    let _ = w.send(m);
                }
            }
            // The link is gone: nothing more will be answered.
            LOST.store(true, Ordering::Relaxed);
            dispatch.pending.lock().unwrap_or_else(|e| e.into_inner()).clear();
        });
        Ok(Rpc(inner))
    }

    fn forget(&self, id: u32) {
        self.0.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    }

    /// Send a request and wait for its answer (which may be a refusal: see [`Rpc::checked`]).
    async fn call(&self, head: Value, body: Vec<u8>) -> Result<Message> {
        // While a request is out, a stall (a big file over wifi) is not yet a dead link.
        let _busy = self.0.tx.busy();
        let (tx, rx) = oneshot::channel();
        let id = loop {
            let id = self.0.next.fetch_add(1, Ordering::Relaxed);
            if id != 0 {
                break id;
            }
        };
        self.0.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(id, tx);
        if !self.0.tx.send(Message::new(id, head, body)) {
            self.forget(id);
            bail!("lost the connection to the server");
        }
        match tokio::time::timeout(CALL_TIMEOUT, rx).await {
            Ok(Ok(m)) => Ok(m),
            Ok(Err(_)) => bail!("lost the connection to the server"),
            Err(_) => {
                self.forget(id);
                bail!("the server did not answer in time")
            }
        }
    }

    /// Tell the server something that needs no answer (queued, never waits).
    fn notify(&self, head: Value) {
        let _ = self.0.tx.send(Message::new(0, head, Vec::new()));
    }

    /// Turn a refused request into an error that says why.
    fn checked(m: Message) -> Result<Message> {
        if m.is_ok() {
            return Ok(m);
        }
        if m.code() == 401 {
            bail!(
                "the server wants a valid token: run `tarpi connect <token>` with the one \
                 `tarpi serve` prints (or set `serve_trust_all: true` under `sync:` on the serving machine)"
            );
        }
        bail!("the server answered {}: {}", m.code(), m.error())
    }

    /// Token count of `text` from the serving machine's backend. `None` when
    /// that server does not know the request (an older tarpi): count directly instead.
    pub async fn tokenize(&self, text: &str, connection: &str) -> Result<Option<usize>> {
        let m = self
            .call(json!({ "op": "tokenize", "text": text, "connection": connection }), Vec::new())
            .await?;
        if !m.is_ok() && m.code() == 404 {
            return Ok(None);
        }
        let m = Rpc::checked(m)?;
        m.head
            .get("tokens")
            .and_then(|n| n.as_u64())
            .map(|n| Some(n as usize))
            .ok_or_else(|| anyhow!("the server gave no token count"))
    }

    /// Send what is still queued and hang up.
    async fn close(&self) {
        self.0.tx.close(Duration::from_secs(2)).await;
    }
}

/// Is the file at `path` still what the last listing saw (`scanned` is its
/// hash there; `None`: the listing found no such file)? Asked right before a
/// step that replaces or deletes this device's copy, because the chat screen
/// may have saved the file since the listing was made. A file that cannot be
/// read now counts as changed.
fn copy_unchanged(path: &std::path::Path, scanned: Option<&str>) -> bool {
    match std::fs::read(path) {
        Ok(bytes) => scanned.map_or(false, |h| h == hash_bytes(&bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => scanned.is_none(),
        Err(_) => false,
    }
}

/// The state of one connection: where the server is, where the copy is, and
/// what both sides agreed on last time.
pub struct Client {
    rpc: Rpc,
    /// The server as `host:port`.
    addr: String,
    root: PathBuf,
    api: bool,
    state: HashMap<String, String>,
    scanner: Arc<std::sync::Mutex<Scanner>>,
}

impl Client {
    fn new(addr: String, rpc: Rpc, root: PathBuf, api: bool) -> Client {
        let state = std::fs::read_to_string(root.join(".sync-state.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<HashMap<String, String>>(&t).ok())
            .unwrap_or_default();
        Client {
            rpc,
            addr,
            scanner: Arc::new(std::sync::Mutex::new(Scanner::new(root.clone(), api))),
            root,
            api,
            state,
        }
    }

    /// The server's files, and what it has but could not read (those are not
    /// in the first list, and must not be taken for deleted).
    async fn remote_manifest(&self) -> Result<(HashMap<String, Entry>, Vec<String>)> {
        let m = self
            .rpc
            .call(json!({ "op": "manifest" }), Vec::new())
            .await
            .with_context(|| format!("could not reach tarpi serve at {}", self.addr))?;
        let m = Rpc::checked(m)?;
        let m: Manifest = serde_json::from_slice(&m.body).context("reading the server's file list")?;
        let unreadable = m.unreadable;
        let files = m
            .files
            .into_iter()
            .filter(|e| in_scope(&e.path, self.api))
            .map(|e| (e.path.clone(), e))
            .collect();
        Ok((files, unreadable))
    }

    /// A file's bytes and modification time.
    async fn download(&self, rel: &str) -> Result<(Vec<u8>, u64)> {
        let m = self
            .rpc
            .call(json!({ "op": "get", "path": rel }), Vec::new())
            .await
            .with_context(|| format!("downloading {}", rel))?;
        let m = Rpc::checked(m).with_context(|| format!("downloading {}", rel))?;
        let mtime = m.head.get("mtime").and_then(|v| v.as_u64()).unwrap_or_else(now_ms);
        Ok((m.body, mtime))
    }

    /// Send a file, provided the server's copy still has hash `base` (`None`:
    /// provided it has none). `false` when it changed in the meantime.
    async fn upload(&self, rel: &str, bytes: Vec<u8>, base: Option<&str>, mtime: u64) -> Result<bool> {
        if bytes.len() > wire::MAX_FILE {
            bail!("{} is too large to share (over {} MB)", rel, wire::MAX_FILE / (1024 * 1024));
        }
        let m = self
            .rpc
            .call(
                json!({ "op": "put", "path": rel, "base": base.unwrap_or(""), "mtime": mtime }),
                bytes,
            )
            .await
            .with_context(|| format!("uploading {}", rel))?;
        if !m.is_ok() && m.code() == 409 {
            return Ok(false);
        }
        Rpc::checked(m).with_context(|| format!("uploading {}", rel))?;
        Ok(true)
    }

    /// Delete a file on the server, provided it still has hash `base`.
    async fn delete_remote(&self, rel: &str, base: Option<&str>) -> Result<bool> {
        let m = self
            .rpc
            .call(json!({ "op": "delete", "path": rel, "base": base.unwrap_or("") }), Vec::new())
            .await
            .with_context(|| format!("deleting {}", rel))?;
        if !m.is_ok() && m.code() == 409 {
            return Ok(false);
        }
        Rpc::checked(m).with_context(|| format!("deleting {}", rel))?;
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
                // The listing is a moment old (the server's copy may take a
                // while to arrive): if this device changed the file since, the
                // download would wipe that change out. Leave it; the next
                // round sees both versions and settles it as a conflict.
                if !copy_unchanged(&to_fs(&self.root, &p), local.get(&p).map(|e| e.hash.as_str())) {
                    return Ok(());
                }
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
                // Same reason as for a download: never delete a file that was
                // changed here after the listing (a chat that just got a new message).
                if !copy_unchanged(&to_fs(&self.root, &p), local.get(&p).map(|e| e.hash.as_str())) {
                    return Ok(());
                }
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
        let (remote, remote_unreadable) = self.remote_manifest().await?;
        if remote.is_empty() && !self.state.is_empty() {
            bail!(
                "the server lists no files, but this copy has been in step with it before; \
                 nothing is deleted (is tarpi serve using another config folder?)"
            );
        }
        let sc = self.scanner.clone();
        let (found, local_unreadable) = tokio::task::spawn_blocking(move || {
            let mut scanner = sc.lock().unwrap_or_else(|e| e.into_inner());
            let found = scanner.scan();
            (found, scanner.unreadable())
        })
        .await
        .context("listing the local files")?;
        let local: HashMap<String, Entry> =
            found.into_iter().map(|e| (e.path.clone(), e)).collect();

        // This side lists nothing at all, yet it has been in step with the
        // server before: the shared files are gone from here (the folder was
        // wiped or moved, a storage was not mounted), not deleted one by one.
        // Passing that on would delete everything on the server, and from
        // there on every device. Forget what was agreed on, so that the
        // server's files are simply downloaded again.
        if local.is_empty() && local_unreadable.is_empty() && !self.state.is_empty() {
            self.state.clear();
        }

        // Files either side has but could not read are neither deleted nor
        // overwritten this round: not being in a list is no sign of a delete.
        let mut skip = local_unreadable;
        skip.extend(remote_unreadable);
        let mut actions = plan(&self.state, &local, &remote);
        actions.retain(|a| !is_skipped(&skip, a.path()));
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

/// Set when the link to `tarpi serve` has ended: the server stopped, the
/// network dropped, or it went silent (see `Rpc::connect`).
static LOST: AtomicBool = AtomicBool::new(false);

/// `true` once the server this device is connected to has gone away. The chat
/// screen checks it and quits.
pub fn server_lost() -> bool {
    LOST.load(Ordering::Relaxed)
}

/// Set when the server says a shared file changed, so the sync runs at once.
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// How long the sync may go without a reason: the server's notes and the
/// local check (every 200 ms) make it run at once, this is only a safety net.
const SYNC_EVERY: Duration = Duration::from_secs(30);

/// The background sync of a running chat screen.
pub struct Session {
    client: Arc<tokio::sync::Mutex<Client>>,
    rpc: Rpc,
    task: tokio::task::JoinHandle<()>,
    reporter: tokio::task::JoinHandle<()>,
}

impl Session {
    /// Stop the background sync and sync one last time (after the screen has
    /// been closed, so a failure may be printed).
    pub async fn finish(self) {
        self.task.abort();
        if server_lost() {
            self.reporter.abort();
            let c = self.client.lock().await;
            eprintln!(
                "tarpi: lost the connection to {} (is `tarpi serve` still running?); quit.\n\
                 (your changes are kept here and are sent next time you connect)",
                c.addr
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
                    c.addr, e
                ),
            }
        }
        // Let the server hear what is still queued, then close the link (the
        // server logs that the device left).
        *REPORTER.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let _ = tokio::time::timeout(Duration::from_secs(3), self.reporter).await;
        self.rpc.close().await;
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

/// Find the server named by `serve_url`, make sure of the token, set the copy
/// in `~/.config/tarpi/remote/` as the data folder and check the server answers.
/// The returned client holds the open connection.
async fn open(token_arg: Option<String>, local_api: bool) -> Result<Client> {
    let cfg = config::load()?;
    let sync = cfg.sync();
    let address = sync
        .serve_url
        .clone()
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .ok_or_else(|| {
            anyhow!(
                "no server address: set `serve_url: host:port` under `sync:` in {}",
                config::config_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "config.yaml".to_string())
            )
        })?;
    let addr = normalize(&address);

    let local = config::local_dir().ok_or_else(|| anyhow!("no config directory on this system"))?;
    let root = local.join("remote").join(mirror_name(&addr));
    std::fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;

    // From here on every store reads and writes the copy.
    config::set_data_dir(root.clone());
    config::set_api_local(local_api);

    let rpc = Rpc::connect(&addr, sync.encrypted).await?;

    // The greeting is answered without a token: it says which protocol the
    // server speaks and whether it wants a token.
    let hello = rpc
        .call(json!({ "op": "hello", "proto": PROTO }), Vec::new())
        .await?;
    let hello = Rpc::checked(hello).with_context(|| format!("{} did not answer like tarpi serve", addr))?;
    let proto = hello.head.get("proto").and_then(|p| p.as_u64()).unwrap_or(0);
    if proto != PROTO as u64 {
        bail!(
            "the server speaks protocol {} and this tarpi {}: update the older one",
            proto,
            PROTO
        );
    }
    let wants_token = hello.head.get("token").and_then(|t| t.as_bool()).unwrap_or(true);

    // If the server wants no token, none is asked for or sent. If it does:
    // the one given now, else the one remembered, else ask.
    if wants_token {
        let token_file = root.join(".token");
        let (token, is_new) = match token_arg.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
            Some(t) => (t, true),
            None => match std::fs::read_to_string(&token_file)
                .ok()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
            {
                Some(t) => (t, false),
                None => (ask_token()?, true),
            },
        };
        // On an encrypted link the token is proved, not sent, and the server has
        // to prove it knows it too: someone in the middle can do neither.
        let request = match rpc.0.binding {
            Some(b) => json!({ "op": "auth", "proof": wire::proof(&token, &b, "client") }),
            None => json!({ "op": "auth", "token": token }),
        };
        let answer = rpc.call(request, Vec::new()).await?;
        let answer = Rpc::checked(answer)?;
        if let Some(b) = rpc.0.binding {
            let want = wire::proof(&token, &b, "server");
            if answer.str("proof") != Some(want.as_str()) {
                bail!("the server could not prove that it knows the token: it may not be the tarpi serve you meant");
            }
        }
        if is_new {
            let _ = std::fs::write(&token_file, format!("{}\n", token));
            private(&token_file);
        }
    }
    Ok(Client::new(addr, rpc, root, !local_api))
}

/// Connect, bring the local copy up to date, and start keeping it in step.
/// Returns the running sync; the caller then opens the chat as usual and calls
/// `Session::finish` after it.
pub async fn run() -> Result<Session> {
    // argv is `tarpi connect ...`; drop the program name so "connect" plays
    // the part of the binary name in clap's messages.
    // `tarpi c` is short for `tarpi connect`; usage texts should say the long name.
    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    if let Some(first) = argv.first_mut() {
        if first == "c" {
            *first = "connect".to_string();
        }
    }
    let cli = ConnectCli::parse_from(argv);
    let mut client = open(cli.token.clone(), cli.local_api).await?;
    let root = client.root.clone();
    let addr = client.addr.clone();
    let rpc = client.rpc.clone();
    // The shared connection's address belongs to the server: it counts tokens,
    // and a loopback / 0.0.0.0 address is read as the server's host.
    if !cli.local_api {
        crate::conn::set_remote_tokenizer(rpc.clone());
        crate::conn::set_remote_host(&addr);
    }

    // What this device does is told to the server's log (see `report`).
    let (report_tx, mut report_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    *REPORTER.lock().unwrap_or_else(|e| e.into_inner()) = Some(report_tx);
    let reporter = {
        let rpc = rpc.clone();
        tokio::spawn(async move {
            while let Some(text) = report_rx.recv().await {
                rpc.notify(json!({ "op": "event", "text": text }));
            }
        })
    };

    println!("Syncing with {} ...", addr);
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
    // the server (it tells us over the connection), and every so often as a
    // safety net.
    let task = tokio::spawn(async move {
        let mut last_err: Option<String> = None;
        let mut last = local_digest(&scanner).await;
        let mut since = std::time::Instant::now();
        loop {
            tokio::select! {
                _ = WAKE.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(200)) => {
                    if local_digest(&scanner).await == last && since.elapsed() < SYNC_EVERY {
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

    Ok(Session { client, rpc, task, reporter })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_get_a_port() {
        assert_eq!(normalize("192.168.1.20:9000"), "192.168.1.20:9000");
        assert_eq!(normalize("192.168.1.20"), "192.168.1.20:7878");
        assert_eq!(normalize("http://host:1/"), "host:1");
        assert_eq!(normalize("tcp://host"), "host:7878");
        assert_eq!(normalize("[::1]:5"), "[::1]:5");
        assert_eq!(normalize("[::1]"), "[::1]:7878");
    }

    fn temp_file(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("tarpi-test-{}-{}", tag, uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&d).unwrap();
        d.join("f.jsonl")
    }

    #[test]
    fn a_copy_changed_since_the_listing_is_not_replaced() {
        let p = temp_file("unchanged");
        std::fs::write(&p, b"one").unwrap();
        let listed = hash_bytes(b"one");
        assert!(copy_unchanged(&p, Some(listed.as_str())));
        // The chat screen saved a new message after the listing.
        std::fs::write(&p, b"one two").unwrap();
        assert!(!copy_unchanged(&p, Some(listed.as_str())));
        // The listing saw no file, but there is one now.
        assert!(!copy_unchanged(&p, None));
        // Deleted since the listing: fine to treat as unchanged only if none was listed.
        std::fs::remove_file(&p).unwrap();
        assert!(!copy_unchanged(&p, Some(listed.as_str())));
        assert!(copy_unchanged(&p, None));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn mirror_folders_are_plain_names() {
        assert_eq!(mirror_name("192.168.1.20:7878"), "192.168.1.20-7878");
        assert_eq!(mirror_name("[::1]:5"), "_--1_-5");
        assert_eq!(mirror_name("phone.local:1"), "phone.local-1");
    }
}
