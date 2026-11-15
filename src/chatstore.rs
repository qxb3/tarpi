//! Chats: tarpi's own folder, `~/.config/tarpi/chats/`.
//!
//! One folder per character (named like the card's avatar without `.png`, e.g.
//! `chats/Sofia/`), one `<chat>.jsonl` per chat. The files are the same
//! SillyTavern `.jsonl` format as before: the first line is the header
//! (`chat_metadata`, ...), every other line a message. Lines are kept as raw
//! JSON, so fields written by SillyTavern or extensions survive a round trip,
//! and a chat folder copied from SillyTavern's `data/<user>/chats/` works as is.
//!
//! Saving refuses to overwrite a file whose `chat_metadata.integrity` differs
//! from the one being saved (another terminal or program changed it), the same
//! check SillyTavern made.
//!
//! That id does not change when somebody only adds messages to the same chat,
//! so it cannot tell that another device did (`tarpi connect` replaces the file
//! with the newer copy, `tarpi serve` takes the upload of another device). So
//! this process also remembers the contents of each chat file as it last read
//! or wrote it (a hash, see `SEEN`), and a save is refused when the file on
//! disk is no longer that: the chat open here is out of date.

use crate::config;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Hash of each chat file (by path) as this process last loaded or wrote it.
/// `Option` only because a `HashMap` cannot be made in a `static` directly.
static SEEN: Mutex<Option<HashMap<PathBuf, String>>> = Mutex::new(None);

fn remember(path: &Path, text: &str) {
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    seen.get_or_insert_with(HashMap::new)
        .insert(path.to_path_buf(), crate::mirror::hash_bytes(text.as_bytes()));
}

fn forget(path: &Path) {
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = seen.as_mut() {
        m.remove(path);
    }
}

/// Forget every chat file below `dir` (its folder was deleted).
fn forget_under(dir: &Path) {
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = seen.as_mut() {
        m.retain(|p, _| !p.starts_with(dir));
    }
}

fn seen(path: &Path) -> Option<String> {
    let seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    seen.as_ref().and_then(|m| m.get(path).cloned())
}

fn root() -> Result<PathBuf> {
    config::chats_dir().ok_or_else(|| anyhow!("no config directory on this system"))
}

fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains(['/', '\\', '\0'])
        && s.chars().count() <= 200
}

fn char_dir(avatar: &str) -> Result<PathBuf> {
    let stem = avatar.strip_suffix(".png").unwrap_or(avatar);
    if !safe_name(stem) {
        bail!("bad character file name '{}'", avatar);
    }
    Ok(root()?.join(stem))
}

fn chat_path(avatar: &str, file: &str) -> Result<PathBuf> {
    let name = file.strip_suffix(".jsonl").unwrap_or(file);
    if !safe_name(name) {
        bail!("bad chat name '{}'", file);
    }
    Ok(char_dir(avatar)?.join(format!("{}.jsonl", name)))
}

fn parse_lines(text: &str) -> Vec<Value> {
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

fn integrity(first: Option<&Value>) -> Option<String> {
    first?
        .get("chat_metadata")?
        .get("integrity")?
        .as_str()
        .map(|s| s.to_string())
}

/// The chat files of a character, in the shape tarpi always used: each entry
/// has `file_name` (`x.jsonl`), `file_id`, `last_mes` (file mtime, ms),
/// `chat_items` (messages) and `file_size`. No folder yet means no chats.
pub fn list(avatar: &str) -> Result<Vec<Value>> {
    let dir = char_dir(avatar)?;
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(id) = p.file_stem().and_then(|x| x.to_str()).map(str::to_string) else {
            continue;
        };
        let Ok(meta) = e.metadata() else {
            continue;
        };
        let ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let items = std::fs::read_to_string(&p)
            .map(|t| t.lines().filter(|l| !l.trim().is_empty()).count().saturating_sub(1))
            .unwrap_or(0);
        out.push(json!({
            "file_name": format!("{}.jsonl", id),
            "file_id": id,
            "last_mes": ms,
            "chat_items": items,
            "file_size": format!("{:.2}kb", meta.len() as f64 / 1024.0),
        }));
    }
    Ok(out)
}

/// The lines of a chat: the header first, then the messages. A chat that does
/// not exist is empty.
pub fn load(avatar: &str, file: &str) -> Result<Vec<Value>> {
    read_path(&chat_path(avatar, file)?)
}

fn read_path(path: &Path) -> Result<Vec<Value>> {
    match std::fs::read_to_string(path) {
        Ok(t) => {
            remember(path, &t);
            Ok(parse_lines(&t))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            forget(path);
            Ok(Vec::new())
        }
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn write(path: &Path, lines: &[Value]) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    let text = lines
        .iter()
        .map(|l| serde_json::to_string(l))
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("writing the chat")?
        .join("\n");
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    remember(path, &text);
    Ok(())
}

/// Save a chat (`file` without `.jsonl`).
pub fn save(avatar: &str, file: &str, lines: &[Value]) -> Result<()> {
    save_to(&chat_path(avatar, file)?, lines)
}

fn save_to(path: &Path, lines: &[Value]) -> Result<()> {
    if let Ok(old) = std::fs::read_to_string(path) {
        let old_first = old
            .lines()
            .find(|l| !l.trim().is_empty())
            .and_then(|l| serde_json::from_str::<Value>(l).ok());
        if let (Some(a), Some(b)) = (integrity(old_first.as_ref()), integrity(lines.first())) {
            if a != b {
                bail!(
                    "save refused: the chat file changed elsewhere (another terminal?). \
                     Reopen tarpi to reload it."
                );
            }
        }
        // Same chat, but not the file this process last read or wrote: another
        // device (or terminal) added to it since, and what is open here does not
        // have those messages. Saving would erase them, here and, through the
        // sync, on every device.
        if let Some(known) = seen(path) {
            if known != crate::mirror::hash_bytes(old.as_bytes()) {
                bail!(
                    "save refused: the chat file changed elsewhere (another device or terminal?). \
                     Reopen tarpi to reload it."
                );
            }
        }
    }
    write(path, lines)
}

/// Delete one chat file (`file` without `.jsonl`). A missing file is fine.
pub fn delete(avatar: &str, file: &str) -> Result<()> {
    let path = chat_path(avatar, file)?;
    forget(&path);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("deleting {}", path.display())),
    }
}

/// Delete every chat of a character (its folder).
pub fn delete_all(avatar: &str) -> Result<()> {
    let dir = char_dir(avatar)?;
    forget_under(&dir);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("deleting {}", dir.display())),
    }
}

/// Put a chat made elsewhere (copied from SillyTavern) into the folder.
/// Returns false when a chat of that name is already here and `overwrite` is off.
pub fn import(avatar: &str, file: &str, lines: &[Value], overwrite: bool) -> Result<bool> {
    let path = chat_path(avatar, file)?;
    if path.exists() && !overwrite {
        return Ok(false);
    }
    write(&path, lines)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tarpi-test-{}-{}", tag, uuid::Uuid::new_v4().simple()));
        d.join("c.jsonl")
    }

    /// A chat with the given messages (same header, so the same `integrity`).
    fn chat(messages: &[&str]) -> Vec<Value> {
        let mut v = vec![json!({ "chat_metadata": { "integrity": "abc" } })];
        for m in messages {
            v.push(json!({ "mes": m }));
        }
        v
    }

    fn as_file(lines: &[Value]) -> String {
        lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn a_chat_another_device_added_to_is_not_overwritten() {
        let p = temp_path("seen");
        save_to(&p, &chat(&["hi"])).unwrap();
        assert_eq!(read_path(&p).unwrap().len(), 2);
        // Saving what was loaded, and more, keeps working.
        save_to(&p, &chat(&["hi", "there"])).unwrap();

        // A sync replaces the file with another device's longer version of the
        // same chat (the same `integrity` id: only messages were added).
        std::fs::write(&p, as_file(&chat(&["hi", "there", "from the phone"]))).unwrap();
        let err = save_to(&p, &chat(&["hi", "there", "from here"])).unwrap_err();
        assert!(format!("{:#}", err).contains("save refused"));
        // Nothing was written over it.
        assert_eq!(read_path(&p).unwrap().len(), 4);

        // After reopening (loading it again) saving works again.
        save_to(&p, &chat(&["hi", "there", "from the phone", "and here"])).unwrap();
        assert_eq!(read_path(&p).unwrap().len(), 5);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn a_chat_never_opened_here_is_still_saved_by_its_integrity_id() {
        let p = temp_path("unknown");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, as_file(&chat(&["old"]))).unwrap();
        // Not loaded by this process: only the `integrity` check applies.
        save_to(&p, &chat(&["old", "new"])).unwrap();
        let mut other = chat(&["x"]);
        other[0] = json!({ "chat_metadata": { "integrity": "different" } });
        assert!(save_to(&p, &other).is_err());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn a_deleted_chat_can_be_made_again() {
        let p = temp_path("deleted");
        save_to(&p, &chat(&["a"])).unwrap();
        std::fs::remove_file(&p).unwrap();
        forget(&p);
        // Another device's chat of the same name arrives, this one starts afresh.
        std::fs::write(&p, as_file(&chat(&["theirs"]))).unwrap();
        save_to(&p, &chat(&["mine"])).unwrap();
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
