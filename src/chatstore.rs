//! Chats: st-cli's own folder, `~/.config/st-cli/chats/`.
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

use crate::config;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

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

/// The chat files of a character, in the shape st-cli always used: each entry
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
    let path = chat_path(avatar, file)?;
    match std::fs::read_to_string(&path) {
        Ok(t) => Ok(parse_lines(&t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
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
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Save a chat (`file` without `.jsonl`).
pub fn save(avatar: &str, file: &str, lines: &[Value]) -> Result<()> {
    let path = chat_path(avatar, file)?;
    if let Ok(old) = std::fs::read_to_string(&path) {
        let old_first = old
            .lines()
            .find(|l| !l.trim().is_empty())
            .and_then(|l| serde_json::from_str::<Value>(l).ok());
        if let (Some(a), Some(b)) = (integrity(old_first.as_ref()), integrity(lines.first())) {
            if a != b {
                bail!(
                    "save refused: the chat file changed elsewhere (another terminal?). \
                     Reopen st-cli to reload it."
                );
            }
        }
    }
    write(&path, lines)
}

/// Delete one chat file (`file` without `.jsonl`). A missing file is fine.
pub fn delete(avatar: &str, file: &str) -> Result<()> {
    let path = chat_path(avatar, file)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("deleting {}", path.display())),
    }
}

/// Delete every chat of a character (its folder).
pub fn delete_all(avatar: &str) -> Result<()> {
    let dir = char_dir(avatar)?;
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
