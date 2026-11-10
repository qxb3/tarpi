//! `tarpi connect [token]`: use the chats, characters, personas, system
//! prompts, formatting and API connection of the machine that runs `tarpi
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
                  system prompts, formatting and API connection from this device. A copy of \
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

/// The background sync of a running chat screen.
pub struct Session {
    client: Arc<tokio::sync::Mutex<Client>>,
    task: tokio::task::JoinHandle<()>,
}

impl Session {
    /// Stop the background sync and sync one last time (after the screen has
    /// been closed, so a failure may be printed).
    pub async fn finish(self) {
        self.task.abort();
        let mut c = self.client.lock().await;
        match c.cycle().await {
            Ok(_) => {}
            Err(e) => eprintln!(
                "tarpi: the last sync with {} failed: {:#}\n(your changes are kept here and are sent next time you connect)",
                c.base, e
            ),
        }
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

/// Connect, bring the local copy up to date, and start keeping it in step.
/// Returns the running sync; the caller then opens the chat as usual and calls
/// `Session::finish` after it.
pub async fn run() -> Result<Session> {
    // argv is `tarpi connect ...`; drop the program name so "connect" plays
    // the part of the binary name in clap's messages.
    let cli = ConnectCli::parse_from(std::env::args().skip(1));
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
    config::set_api_local(cli.local_api);

    let mut client = Client::new(base.clone(), String::new(), root.clone(), !cli.local_api)?;

    // Ask the server whether it wants a token. If not, none is asked for or
    // sent. If so: the one given now, else the one remembered, else ask.
    let token_file = root.join(".token");
    let (token, is_new) = if !client.needs_token().await? {
        (String::new(), false)
    } else {
        match cli.token.clone().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
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
    // The shared connection's address belongs to the server: it counts tokens,
    // and a loopback / 0.0.0.0 address is read as the server's host.
    if !cli.local_api {
        crate::conn::set_remote_tokenizer(base.clone(), token.clone());
        crate::conn::set_remote_host(&base);
    }
    if is_new {
        let _ = std::fs::write(&token_file, format!("{}\n", token));
        private(&token_file);
    }

    println!("Syncing with {} ...", base);
    let stats = client.cycle().await?;
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

    let client = Arc::new(tokio::sync::Mutex::new(client));
    let bg = client.clone();
    let log_root = root.clone();
    let task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(SYNC_EVERY)).await;
            let res = bg.lock().await.cycle().await;
            log_status(&log_root, &res);
        }
    });

    Ok(Session { client, task })
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
