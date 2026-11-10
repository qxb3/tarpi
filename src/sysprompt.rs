//! System prompts: the instructions that open every prompt.
//!
//! System prompts belong to st-cli, not to SillyTavern. They are kept in one
//! file, `~/.config/st-cli/system-prompt.jsonl`, one JSON object per line:
//! `id`, `name`, `content` (the system prompt), `post_history` (the
//! post-history instructions) and, on the one line in use, `"active": true`
//! (hand edits are fine). `/system` in a chat edits that file;
//! `st-cli import-st` copies the system prompts out of a SillyTavern server
//! into it. Nothing here talks to the server while chatting: SillyTavern's
//! `power_user.sysprompt` is overwritten in memory by [`normalize`] with the
//! active prompt of this file, so the prompt builders keep reading the place
//! they always did, but the text comes from here.
//!
//! With no system prompt saved (or none chosen) the system prompt is empty.

use crate::config;
use crate::jv::*;
use crate::persona::Imported;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SysPrompt {
    /// Short file-name-like id, stored in the file.
    pub id: String,
    pub name: String,
    /// The system prompt itself.
    pub content: String,
    /// Post-history instructions (sent after the chat history).
    pub post_history: String,
    /// The prompt in use (at most one line of the file has it).
    #[serde(skip_serializing_if = "is_false")]
    pub active: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

// ---- the file ----------------------------------------------------------------

fn path() -> Result<PathBuf> {
    config::sysprompt_path().ok_or_else(|| anyhow!("no config directory on this system"))
}

/// Ids are short and plain.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// An id made from a prompt's name.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.trim_matches('-').chars().take(40).collect();
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "system-prompt".to_string()
    } else {
        out
    }
}

/// `base`, or `base-2`, `base-3`, ... when that id is taken.
fn unique_id(list: &[SysPrompt], base: &str) -> String {
    if !list.iter().any(|p| p.id == base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{}-{}", base, n))
        .find(|id| !list.iter().any(|p| p.id == *id))
        .unwrap()
}

/// Every prompt of the file, in file order. A missing or unreadable file, and
/// lines that are not a JSON object, give nothing.
fn load_all() -> Vec<SysPrompt> {
    let Ok(path) = path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut out: Vec<SysPrompt> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(mut p) = serde_json::from_str::<SysPrompt>(line) else {
            continue;
        };
        if !valid_id(&p.id) || out.iter().any(|o| o.id == p.id) {
            p.id = unique_id(&out, &slug(&p.name));
        }
        if p.name.trim().is_empty() {
            p.name = p.id.clone();
        }
        out.push(p);
    }
    // Only one prompt is in use.
    let mut seen = false;
    for p in out.iter_mut() {
        if p.active {
            if seen {
                p.active = false;
            }
            seen = true;
        }
    }
    out
}

/// Write the whole file (the folder is made when needed).
fn write_all(list: &[SysPrompt]) -> Result<()> {
    let path = path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut text = String::new();
    for p in list {
        text.push_str(&serde_json::to_string(p).context("writing the system prompt")?);
        text.push('\n');
    }
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, &text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// All system prompts, sorted by name. Empty when none are saved.
pub fn list() -> Vec<SysPrompt> {
    let mut out = load_all();
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

/// The system prompt in use, if one was chosen.
pub fn current() -> Option<SysPrompt> {
    load_all().into_iter().find(|p| p.active)
}

/// Make `id` the system prompt in use.
pub fn activate(id: &str) -> Result<SysPrompt> {
    let mut all = load_all();
    if !all.iter().any(|p| p.id == id) {
        bail!("no system prompt '{}'", id);
    }
    for p in all.iter_mut() {
        p.active = p.id == id;
    }
    write_all(&all)?;
    all.into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| anyhow!("no system prompt '{}'", id))
}

/// Make `id` the one in use unless a system prompt is in use already.
pub fn activate_if_unset(id: &str) -> Result<bool> {
    if current().is_some() {
        return Ok(false);
    }
    activate(id)?;
    Ok(true)
}

/// Create a system prompt (`id` None), or change the name, prompt and
/// post-history instructions of an existing one.
pub fn put(id: Option<&str>, name: &str, content: &str, post_history: &str) -> Result<SysPrompt> {
    let mut all = load_all();
    let saved = match id {
        Some(id) => {
            let p = all
                .iter_mut()
                .find(|p| p.id == id)
                .ok_or_else(|| anyhow!("no system prompt '{}'", id))?;
            p.name = name.to_string();
            p.content = content.to_string();
            p.post_history = post_history.to_string();
            p.clone()
        }
        None => {
            let p = SysPrompt {
                id: unique_id(&all, &slug(name)),
                name: name.to_string(),
                content: content.to_string(),
                post_history: post_history.to_string(),
                active: false,
            };
            all.push(p.clone());
            p
        }
    };
    write_all(&all)?;
    Ok(saved)
}

/// Delete the system prompt `id` (nothing happens when there is none).
pub fn remove(id: &str) -> Result<()> {
    let mut all = load_all();
    let before = all.len();
    all.retain(|p| p.id != id);
    if all.len() != before {
        write_all(&all)?;
    }
    Ok(())
}

fn same_text(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Same name and same prompt text (what `import-st` goes by).
pub fn same_prompt(a: &SysPrompt, b: &SysPrompt) -> bool {
    same_text(&a.name, &b.name) && a.content.trim() == b.content.trim()
}

/// Add a system prompt made elsewhere. One that is the same in name, prompt
/// and post-history instructions as a saved one is skipped, so running an
/// import twice makes no copies. With `overwrite`, a saved prompt with the
/// same name is replaced instead of getting a second one (`name-2`, ...).
pub fn import(mut p: SysPrompt, overwrite: bool) -> Result<Imported> {
    let mut all = load_all();
    let same = all.iter().position(|o| {
        same_text(&o.name, &p.name)
            && o.content.trim() == p.content.trim()
            && o.post_history.trim() == p.post_history.trim()
    });
    if let Some(i) = same {
        return Ok(Imported::Skipped(all[i].id.clone()));
    }
    if overwrite {
        if let Some(i) = all.iter().position(|o| same_text(&o.name, &p.name)) {
            all[i].name = p.name.clone();
            all[i].content = p.content.clone();
            all[i].post_history = p.post_history.clone();
            let id = all[i].id.clone();
            write_all(&all)?;
            return Ok(Imported::Updated(id));
        }
    }
    p.id = unique_id(&all, &slug(&p.name));
    p.active = false;
    let id = p.id.clone();
    all.push(p);
    write_all(&all)?;
    Ok(Imported::Added(id))
}

// ---- reading SillyTavern (for `import-st`) -------------------------------------

/// A SillyTavern system prompt preset (`name`, `content`, `post_history`).
fn from_st(v: &Value) -> Option<SysPrompt> {
    let name = jstr(v, "name");
    let content = jstr(v, "content");
    if name.trim().is_empty() && content.trim().is_empty() {
        return None;
    }
    let name = if name.trim().is_empty() {
        "SillyTavern".to_string()
    } else {
        name
    };
    Some(SysPrompt {
        id: String::new(),
        name,
        content,
        post_history: jstr(v, "post_history"),
        active: false,
    })
}

/// The system prompt SillyTavern has switched on (`power_user.sysprompt`),
/// when it is enabled and has a text.
pub fn st_active(settings: &Value) -> Option<SysPrompt> {
    let sp = settings.get("power_user")?.get("sysprompt")?;
    if !jbool(sp, "enabled", false) {
        return None;
    }
    from_st(sp).filter(|p| !p.content.trim().is_empty())
}

/// Every system prompt of a SillyTavern server: its presets, plus the one it
/// has switched on when that is not one of them (it may carry edits that were
/// never saved as a preset). Sorted by name.
pub fn st_list(presets: &[Value], settings: &Value) -> Vec<SysPrompt> {
    let mut out: Vec<SysPrompt> = presets.iter().filter_map(from_st).collect();
    if let Some(a) = st_active(settings) {
        if !out.iter().any(|p| same_prompt(p, &a)) {
            out.push(a);
        }
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

// ---- feeding the prompt builders -------------------------------------------------

/// Put the active prompt into `power_user.sysprompt`, where the prompt
/// builders read it. In memory only: the settings are SillyTavern's and are
/// never written back. With no prompt in use the text is empty (the system
/// prompt is still "on", so a character card's own system prompt and
/// post-history instructions keep working).
pub fn normalize(settings: &mut Value) {
    let (name, content, post_history) = match current() {
        Some(p) => (p.name, p.content, p.post_history),
        None => (String::new(), String::new(), String::new()),
    };
    if !settings
        .get("power_user")
        .map(|v| v.is_object())
        .unwrap_or(false)
    {
        settings["power_user"] = json!({});
    }
    settings["power_user"]["sysprompt"] = json!({
        "enabled": true,
        "name": name,
        "content": content,
        "post_history": post_history,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str, content: &str) -> SysPrompt {
        SysPrompt {
            name: name.to_string(),
            content: content.to_string(),
            ..SysPrompt::default()
        }
    }

    #[test]
    fn ids_are_plain() {
        assert_eq!(slug("Roleplay - Simple"), "roleplay-simple");
        assert_eq!(slug("  !! "), "system-prompt");
        assert!(valid_id("roleplay-simple"));
        assert!(!valid_id("a/b"));
        assert!(!valid_id(""));
    }

    #[test]
    fn taken_ids_get_a_number() {
        let mut a = p("A", "x");
        a.id = "a".to_string();
        let mut a2 = p("A", "y");
        a2.id = "a-2".to_string();
        let list = vec![a, a2];
        assert_eq!(unique_id(&list, "b"), "b");
        assert_eq!(unique_id(&list, "a"), "a-3");
    }

    #[test]
    fn the_active_flag_is_only_written_when_set() {
        let mut a = p("A", "x");
        a.id = "a".to_string();
        assert!(!serde_json::to_string(&a).unwrap().contains("active"));
        a.active = true;
        assert!(serde_json::to_string(&a).unwrap().contains("\"active\":true"));
    }

    #[test]
    fn lines_without_all_fields_still_load() {
        let a: SysPrompt = serde_json::from_str(r#"{"name":"N","content":"C"}"#).unwrap();
        assert_eq!(a.name, "N");
        assert_eq!(a.content, "C");
        assert!(a.post_history.is_empty());
        assert!(!a.active);
    }

    #[test]
    fn the_enabled_server_prompt_is_added_once() {
        let presets = vec![
            json!({"name": "B", "content": "two", "post_history": ""}),
            json!({"name": "A", "content": "one", "post_history": "after"}),
        ];
        // Switched on and the same as a preset: not listed twice.
        let same = json!({"power_user": {"sysprompt": {"enabled": true, "name": "A", "content": "one"}}});
        let l = st_list(&presets, &same);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].name, "A");
        assert_eq!(l[0].post_history, "after");
        // Switched on with edits that are not a preset: listed as well.
        let edited = json!({"power_user": {"sysprompt": {"enabled": true, "name": "A", "content": "one!"}}});
        assert_eq!(st_list(&presets, &edited).len(), 3);
        // Switched off: only the presets.
        let off = json!({"power_user": {"sysprompt": {"enabled": false, "name": "Z", "content": "z"}}});
        assert_eq!(st_list(&presets, &off).len(), 2);
        assert!(st_active(&off).is_none());
    }
}
