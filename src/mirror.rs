//! What `tarpi serve` and `tarpi connect` have in common: which files are
//! shared, how they are listed and compared, and the rule that decides what to
//! do with each one.
//!
//! The shared files are the ones in `~/.config/tarpi/` that hold your data:
//! `characters/`, `chats/`, `persona/`, `system-prompt.jsonl`,
//! `formatting.yaml` and `api.yaml`. `config.yaml`, `command_history`,
//! `dictionary.txt` and `serve-token` belong to the machine and are never
//! shared.
//!
//! Files are identified by a path relative to that folder, always written with
//! `/`, and compared by a content hash (FNV-1a, 64 bit: this finds changes, it
//! is not a security measure).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Version of the serve / connect protocol (`GET /v1/ping`).
pub const PROTO: u32 = 1;
/// Port `tarpi serve` listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 7878;

/// Folders that are shared, whole.
const DIRS: &[&str] = &["characters", "chats", "persona"];
/// Single files that are shared.
const FILES: &[&str] = &["system-prompt.jsonl", "formatting.yaml", "api.yaml"];
const API_FILE: &str = "api.yaml";

/// One file in a listing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Relative path, `/` separated (`chats/Sofia/Sofia01.jsonl`).
    pub path: String,
    pub size: u64,
    /// Modification time, ms since the epoch.
    pub mtime: u64,
    pub hash: String,
}

/// The answer to `GET /v1/manifest`.
#[derive(Serialize, Deserialize)]
pub struct Manifest {
    pub files: Vec<Entry>,
}

/// FNV-1a over the bytes, as 16 hex digits.
pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:016x}", h)
}

/// Files tarpi (or this sync) is in the middle of writing, and the copies a
/// conflict leaves behind. Never listed, never shared.
fn is_temp_name(name: &str) -> bool {
    name.ends_with(".tmp") || name.ends_with(".tmp-sync") || name.contains(".conflict-")
}

/// Is `p` a path that may be shared? This is what keeps a client from
/// reading or writing anything outside the shared files (no `..`, no
/// absolute paths, no other top-level names).
pub fn valid_path(p: &str) -> bool {
    if p.is_empty() || p.len() > 400 || p.contains(['\\', '\0']) {
        return false;
    }
    let parts: Vec<&str> = p.split('/').collect();
    if parts.len() > 4 {
        return false;
    }
    if parts
        .iter()
        .any(|c| c.is_empty() || *c == "." || *c == ".." || c.chars().any(char::is_control))
    {
        return false;
    }
    if is_temp_name(parts[parts.len() - 1]) {
        return false;
    }
    if parts.len() == 1 {
        FILES.contains(&parts[0])
    } else {
        DIRS.contains(&parts[0])
    }
}

/// `valid_path`, and not `api.yaml` when the API connection is kept local.
pub fn in_scope(p: &str, api: bool) -> bool {
    valid_path(p) && (api || p != API_FILE)
}

/// The file a validated relative path stands for under `root`.
pub fn to_fs(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for part in rel.split('/') {
        p.push(part);
    }
    p
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn mtime_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Write `bytes` to `path` without ever leaving a half-written file there
/// (a temporary file next to it, then a rename), creating folders as needed.
/// `mtime` (ms) is given to the new file, so "latest chat" means the same on
/// every device.
pub fn write_atomic(path: &Path, bytes: &[u8], mtime: Option<u64>) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    let tmp = path.with_file_name(format!("{}.tmp-sync", name));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    if let Some(ms) = mtime {
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&tmp) {
            let _ = f.set_modified(UNIX_EPOCH + Duration::from_millis(ms));
        }
    }
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Lists the shared files under `root`. Hashes are kept between scans, and a
/// file is only read again when its size or modification time changed.
pub struct Scanner {
    root: PathBuf,
    api: bool,
    cache: HashMap<String, (u64, u64, String)>,
}

impl Scanner {
    pub fn new(root: PathBuf, api: bool) -> Self {
        Scanner {
            root,
            api,
            cache: HashMap::new(),
        }
    }

    pub fn scan(&mut self) -> Vec<Entry> {
        let mut found: Vec<(String, PathBuf)> = Vec::new();
        for f in FILES {
            if !self.api && *f == API_FILE {
                continue;
            }
            let p = self.root.join(f);
            if p.is_file() {
                found.push((f.to_string(), p));
            }
        }
        for d in DIRS {
            walk(&self.root.join(d), d, 1, &mut found);
        }

        let mut out = Vec::new();
        let mut seen: HashMap<String, (u64, u64, String)> = HashMap::new();
        for (rel, path) in found {
            if !valid_path(&rel) {
                continue;
            }
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let (size, mtime) = (meta.len(), mtime_ms(&meta));
            let hash = match self.cache.get(&rel) {
                Some((s, m, h)) if *s == size && *m == mtime => h.clone(),
                _ => match std::fs::read(&path) {
                    Ok(bytes) => hash_bytes(&bytes),
                    Err(_) => continue,
                },
            };
            seen.insert(rel.clone(), (size, mtime, hash.clone()));
            out.push(Entry {
                path: rel,
                size,
                mtime,
                hash,
            });
        }
        self.cache = seen;
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }
}

/// Collect the regular files below `dir` (symbolic links are not followed).
fn walk(dir: &Path, rel: &str, depth: usize, out: &mut Vec<(String, PathBuf)>) {
    if depth > 3 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else {
            continue;
        };
        let Some(name) = e.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let child = format!("{}/{}", rel, name);
        if ft.is_dir() {
            walk(&e.path(), &child, depth + 1, out);
        } else if ft.is_file() && !is_temp_name(&name) {
            out.push((child, e.path()));
        }
    }
}

// ---------------------------------------------------------------- planning

/// What one `connect` round does about one file.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Same on both sides (or gone from both): just remember it.
    Settled(String),
    /// Only the server changed it: take the server's.
    Pull(String),
    /// Only this side changed it: send it.
    Push(String),
    /// Deleted on the server, untouched here: delete it here too.
    DeleteLocal(String),
    /// Deleted here, untouched on the server: delete it there too.
    DeleteRemote(String),
    /// Changed on both sides and the server's is newer (or equal): this side's
    /// copy is kept as `<file>.conflict-<time>`, the server's is taken.
    ConflictPull(String),
    /// Changed on both sides and this side's is newer: the server's copy is
    /// kept here as `<file>.conflict-<time>`, this side's is sent.
    ConflictPush(String),
}

/// Compare this side (`local`), the server (`remote`) and what both agreed on
/// last time (`base`, path to hash) and say what to do with every file.
///
/// A change is only ever overwritten when it was made on both sides, and then
/// the losing copy is kept. A file that one side changed and the other
/// deleted is never deleted: the change wins.
pub fn plan(
    base: &HashMap<String, String>,
    local: &HashMap<String, Entry>,
    remote: &HashMap<String, Entry>,
) -> Vec<Action> {
    let mut paths: Vec<&String> = local
        .keys()
        .chain(remote.keys())
        .chain(base.keys())
        .collect();
    paths.sort();
    paths.dedup();

    let mut out = Vec::new();
    for p in paths {
        let l: Option<&str> = local.get(p.as_str()).map(|e| e.hash.as_str());
        let r: Option<&str> = remote.get(p.as_str()).map(|e| e.hash.as_str());
        let b: Option<&str> = base.get(p.as_str()).map(|s| s.as_str());
        let p = p.clone();
        if l == r {
            out.push(Action::Settled(p));
            continue;
        }
        let local_changed = l != b;
        let remote_changed = r != b;
        out.push(match (local_changed, remote_changed) {
            (false, _) => {
                if r.is_some() {
                    Action::Pull(p)
                } else {
                    Action::DeleteLocal(p)
                }
            }
            (true, false) => {
                if l.is_some() {
                    Action::Push(p)
                } else {
                    Action::DeleteRemote(p)
                }
            }
            (true, true) => match (l, r) {
                (None, Some(_)) => Action::Pull(p),
                (Some(_), None) => Action::Push(p),
                (Some(_), Some(_)) => {
                    let lm = local.get(p.as_str()).map(|e| e.mtime).unwrap_or(0);
                    let rm = remote.get(p.as_str()).map(|e| e.mtime).unwrap_or(0);
                    if lm > rm {
                        Action::ConflictPush(p)
                    } else {
                        Action::ConflictPull(p)
                    }
                }
                (None, None) => Action::Settled(p),
            },
        });
    }
    out
}

// -------------------------------------------------------------- url helpers

/// `%41` and `+` in a query string value. `None` when it is not valid UTF-8.
pub fn url_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let hex = s.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// `a=1&b=x%20y` as a map (later keys win).
pub fn parse_query(q: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for pair in q.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if let (Some(k), Some(v)) = (url_decode(k), url_decode(v)) {
            m.insert(k, v);
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(path: &str, hash: &str, mtime: u64) -> Entry {
        Entry {
            path: path.to_string(),
            size: 1,
            mtime,
            hash: hash.to_string(),
        }
    }

    fn map(items: &[(&str, &str, u64)]) -> HashMap<String, Entry> {
        items
            .iter()
            .map(|(p, h, m)| (p.to_string(), e(p, h, *m)))
            .collect()
    }

    fn base(items: &[(&str, &str)]) -> HashMap<String, String> {
        items
            .iter()
            .map(|(p, h)| (p.to_string(), h.to_string()))
            .collect()
    }

    #[test]
    fn only_shared_paths_are_valid() {
        assert!(valid_path("chats/Sofia/Sofia01.jsonl"));
        assert!(valid_path("characters/Sofia.png"));
        assert!(valid_path("persona/active"));
        assert!(valid_path("system-prompt.jsonl"));
        assert!(valid_path("formatting.yaml"));
        assert!(valid_path("api.yaml"));
        assert!(!valid_path("config.yaml"));
        assert!(!valid_path("serve-token"));
        assert!(!valid_path("command_history"));
        assert!(!valid_path("chats"));
        assert!(!valid_path("../etc/passwd"));
        assert!(!valid_path("chats/../../x"));
        assert!(!valid_path("/etc/passwd"));
        assert!(!valid_path("chats//x.jsonl"));
        assert!(!valid_path("chats\\x.jsonl"));
        assert!(!valid_path("chats/a/b/c/d.jsonl"));
        assert!(!valid_path("chats/x.tmp"));
        assert!(!valid_path("chats/x.jsonl.tmp-sync"));
        assert!(!valid_path("chats/x.jsonl.conflict-1"));
        assert!(!valid_path("system-prompt.jsonl/x"));
    }

    #[test]
    fn the_api_file_can_be_left_out() {
        assert!(in_scope("api.yaml", true));
        assert!(!in_scope("api.yaml", false));
        assert!(in_scope("formatting.yaml", false));
    }

    #[test]
    fn identical_files_are_settled() {
        let l = map(&[("chats/a/x.jsonl", "h1", 5)]);
        let r = map(&[("chats/a/x.jsonl", "h1", 9)]);
        let a = plan(&HashMap::new(), &l, &r);
        assert_eq!(a, vec![Action::Settled("chats/a/x.jsonl".into())]);
    }

    #[test]
    fn new_files_go_to_the_other_side() {
        let l = map(&[("chats/a/l.jsonl", "h1", 5)]);
        let r = map(&[("chats/a/r.jsonl", "h2", 5)]);
        let a = plan(&HashMap::new(), &l, &r);
        assert_eq!(
            a,
            vec![
                Action::Push("chats/a/l.jsonl".into()),
                Action::Pull("chats/a/r.jsonl".into())
            ]
        );
    }

    #[test]
    fn a_change_on_one_side_is_followed() {
        let b = base(&[("f", "h1")]);
        let a = plan(&b, &map(&[("f", "h1", 1)]), &map(&[("f", "h2", 2)]));
        assert_eq!(a, vec![Action::Pull("f".into())]);
        let a = plan(&b, &map(&[("f", "h2", 2)]), &map(&[("f", "h1", 1)]));
        assert_eq!(a, vec![Action::Push("f".into())]);
    }

    #[test]
    fn deletes_follow_when_the_other_side_did_not_change() {
        let b = base(&[("f", "h1")]);
        let a = plan(&b, &map(&[("f", "h1", 1)]), &HashMap::new());
        assert_eq!(a, vec![Action::DeleteLocal("f".into())]);
        let a = plan(&b, &HashMap::new(), &map(&[("f", "h1", 1)]));
        assert_eq!(a, vec![Action::DeleteRemote("f".into())]);
    }

    #[test]
    fn a_change_beats_a_delete() {
        let b = base(&[("f", "h1")]);
        let a = plan(&b, &HashMap::new(), &map(&[("f", "h2", 1)]));
        assert_eq!(a, vec![Action::Pull("f".into())]);
        let a = plan(&b, &map(&[("f", "h2", 1)]), &HashMap::new());
        assert_eq!(a, vec![Action::Push("f".into())]);
    }

    #[test]
    fn a_double_change_keeps_the_newer_and_flags_a_conflict() {
        let b = base(&[("f", "h1")]);
        let a = plan(&b, &map(&[("f", "h2", 10)]), &map(&[("f", "h3", 20)]));
        assert_eq!(a, vec![Action::ConflictPull("f".into())]);
        let a = plan(&b, &map(&[("f", "h2", 30)]), &map(&[("f", "h3", 20)]));
        assert_eq!(a, vec![Action::ConflictPush("f".into())]);
    }

    #[test]
    fn a_file_gone_from_both_is_forgotten() {
        let b = base(&[("f", "h1")]);
        let a = plan(&b, &HashMap::new(), &HashMap::new());
        assert_eq!(a, vec![Action::Settled("f".into())]);
    }

    #[test]
    fn query_values_are_decoded() {
        let q = parse_query("path=chats%2FS%C3%B3fia%2Fa+b.jsonl&base=&mtime=5");
        assert_eq!(q["path"], "chats/Sófia/a b.jsonl");
        assert_eq!(q["base"], "");
        assert_eq!(q["mtime"], "5");
        assert!(url_decode("%zz").is_none());
        assert!(url_decode("%").is_none());
    }

    #[test]
    fn hashes_differ_with_content() {
        assert_eq!(hash_bytes(b"abc"), hash_bytes(b"abc"));
        assert_ne!(hash_bytes(b"abc"), hash_bytes(b"abd"));
        assert_eq!(hash_bytes(b"").len(), 16);
    }
}
