//! Personas: who "you" are in the chat.
//!
//! Personas belong to st-cli, not to SillyTavern. Each one is a small YAML file
//! in `~/.config/st-cli/persona/` (`<id>.yaml`: name, title, description,
//! position, depth, role, lorebook), and the file `persona/active` holds the id
//! of the one in use. `/persona` in a chat edits that folder; `st-cli import-st`
//! copies the personas out of a SillyTavern server into it. Nothing here talks
//! to the server. SillyTavern's `settings.json` is only read by the import and
//! written to in memory by [`normalize`], so the prompt builders keep reading
//! the `power_user.persona_description*` fields they always did.

use crate::config;
use crate::jv::*;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Persona {
    /// File name without `.yaml`, the persona's id. Not stored in the file.
    #[serde(skip)]
    pub id: String,
    pub name: String,
    pub title: String,
    pub description: String,
    pub position: i64,
    pub depth: i64,
    pub role: i64,
    pub lorebook: String,
}

impl Default for Persona {
    fn default() -> Self {
        Persona {
            id: String::new(),
            name: String::new(),
            title: String::new(),
            description: String::new(),
            position: 0,
            depth: 2,
            role: 0,
            lorebook: String::new(),
        }
    }
}

impl Persona {
    /// `Name (Title)`, or just `Name` when there is no title.
    pub fn label(&self) -> String {
        if self.title.trim().is_empty() {
            self.name.clone()
        } else {
            format!("{} ({})", self.name, self.title.trim())
        }
    }
}

// ---- the persona folder ----------------------------------------------------

fn dir() -> Result<PathBuf> {
    config::persona_dir().ok_or_else(|| anyhow!("no config directory on this system"))
}

/// Ids become file names, so only plain characters are allowed in them.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A file-name friendly id made from a persona's name.
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
        "persona".to_string()
    } else {
        out
    }
}

/// `base`, or `base-2`, `base-3`, ... when that file already exists.
fn unique_id(dir: &std::path::Path, base: &str) -> String {
    if !dir.join(format!("{}.yaml", base)).exists() {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{}-{}", base, n))
        .find(|id| !dir.join(format!("{}.yaml", id)).exists())
        .unwrap()
}

/// What you are before any persona has been saved: shown in the list so it is
/// never empty, written to disk only when it is edited.
fn fallback() -> Persona {
    Persona {
        id: "user".to_string(),
        name: "User".to_string(),
        ..Persona::default()
    }
}

/// Every persona file, unsorted. Files that cannot be read are skipped.
fn load_all() -> Vec<Persona> {
    let Ok(dir) = dir() else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("yaml") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|x| x.to_str()).map(str::to_string) else {
            continue;
        };
        if !valid_id(&id) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut p) = serde_yaml::from_str::<Persona>(&text) else {
            continue;
        };
        p.id = id;
        if p.name.trim().is_empty() {
            p.name = p.id.clone();
        }
        out.push(p);
    }
    out
}

/// All personas, sorted by name. Never empty: with none saved yet, a plain
/// `User` is returned alone.
pub fn list() -> Vec<Persona> {
    let mut out = load_all();
    out.sort_by_key(|p| p.name.to_lowercase());
    if out.is_empty() {
        out.push(fallback());
    }
    out
}

/// Id of the persona in use, from `persona/active` (None when never chosen).
pub fn active_id() -> Option<String> {
    let text = std::fs::read_to_string(dir().ok()?.join("active")).ok()?;
    let id = text.trim().to_string();
    if valid_id(&id) {
        Some(id)
    } else {
        None
    }
}

/// The persona that is active right now: the chosen one, else the first one.
pub fn current() -> Persona {
    let all = list();
    let id = active_id();
    all.iter()
        .find(|p| Some(&p.id) == id.as_ref())
        .or_else(|| all.first())
        .cloned()
        .unwrap_or_else(fallback)
}

fn write_atomic(path: &std::path::Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Write one persona file (the folder is made when needed).
fn save(p: &Persona) -> Result<()> {
    if !valid_id(&p.id) {
        bail!("bad persona id '{}'", p.id);
    }
    let dir = dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let text = serde_yaml::to_string(p).context("writing the persona")?;
    write_atomic(&dir.join(format!("{}.yaml", p.id)), &text)
}

/// Make `id` the active persona.
pub fn activate(id: &str) -> Result<Persona> {
    let p = list()
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| anyhow!("no persona '{}'", id))?;
    set_active(&p.id)?;
    Ok(p)
}

fn set_active(id: &str) -> Result<()> {
    let dir = dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    write_atomic(&dir.join("active"), id)
}

/// Create a persona (`id` None), or change the name, title and description of
/// an existing one (position, depth, role and lorebook stay as they are).
pub fn put(id: Option<&str>, name: &str, title: &str, description: &str) -> Result<Persona> {
    let mut p = match id {
        Some(id) => list()
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| anyhow!("no persona '{}'", id))?,
        None => Persona {
            id: unique_id(&dir()?, &slug(name)),
            ..Persona::default()
        },
    };
    p.name = name.to_string();
    p.title = title.to_string();
    p.description = description.to_string();
    save(&p)?;
    Ok(p)
}

/// Delete the persona `id` (nothing happens when it has no file).
pub fn remove(id: &str) -> Result<()> {
    if !valid_id(id) {
        bail!("bad persona id '{}'", id);
    }
    let path = dir()?.join(format!("{}.yaml", id));
    if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("deleting {}", path.display()))?;
    }
    Ok(())
}

/// What [`import`] did with one persona.
pub enum Imported {
    Added(String),
    Updated(String),
    /// An identical persona is already here.
    Skipped(String),
}

impl Imported {
    pub fn id(&self) -> &str {
        match self {
            Imported::Added(id) | Imported::Updated(id) | Imported::Skipped(id) => id,
        }
    }
}

fn same_text(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Add a persona made elsewhere. Several personas may share a name (each gets
/// its own file, `alex.yaml`, `alex-2.yaml`, ...). A persona that is the same
/// in name, title and description as a saved one is skipped, so running an
/// import twice makes no copies. With `overwrite`, a saved persona with the
/// same name and title is replaced instead of getting a second one.
pub fn import(mut p: Persona, overwrite: bool) -> Result<Imported> {
    let all = load_all();
    if let Some(old) = all.iter().find(|o| {
        same_text(&o.name, &p.name)
            && same_text(&o.title, &p.title)
            && o.description.trim() == p.description.trim()
    }) {
        return Ok(Imported::Skipped(old.id.clone()));
    }
    if overwrite {
        if let Some(old) = all
            .iter()
            .find(|o| same_text(&o.name, &p.name) && same_text(&o.title, &p.title))
        {
            p.id = old.id.clone();
            save(&p)?;
            return Ok(Imported::Updated(p.id));
        }
    }
    p.id = unique_id(&dir()?, &slug(&p.name));
    save(&p)?;
    Ok(Imported::Added(p.id))
}

/// Make `id` active unless a persona was chosen before.
pub fn activate_if_unset(id: &str) -> Result<bool> {
    if active_id().is_some() {
        return Ok(false);
    }
    set_active(id)?;
    Ok(true)
}

// ---- reading SillyTavern's settings (for `import-st`) -----------------------

fn from_descriptor(avatar: &str, name: &str, d: &Value) -> Persona {
    Persona {
        id: avatar.to_string(),
        name: name.to_string(),
        title: jstr(d, "title"),
        description: jstr(d, "description"),
        position: ji64(d, "position", 0),
        depth: ji64(d, "depth", 2),
        role: ji64(d, "role", 0),
        lorebook: jstr(d, "lorebook"),
    }
}

/// The persona SillyTavern has active (its `user_avatar`).
fn st_current(settings: &Value) -> Persona {
    let null = Value::Null;
    let pu = settings.get("power_user").unwrap_or(&null);
    let avatar = jstr(settings, "user_avatar");
    let mapped = pu
        .get("personas")
        .map(|p| jstr(p, &avatar))
        .unwrap_or_default();
    let name = [mapped, jstr(settings, "username")]
        .into_iter()
        .find(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "User".to_string());
    match pu.get("persona_descriptions").and_then(|d| d.get(&avatar)) {
        Some(d) => from_descriptor(&avatar, &name, d),
        // Older settings: only the single active description exists.
        None => Persona {
            id: avatar,
            name,
            title: String::new(),
            description: jstr(pu, "persona_description"),
            position: ji64(pu, "persona_description_position", 0),
            depth: ji64(pu, "persona_description_depth", 2),
            role: ji64(pu, "persona_description_role", 0),
            lorebook: jstr(pu, "persona_description_lorebook"),
        },
    }
}

/// Every persona SillyTavern has (`power_user.personas`), with the avatar file
/// as the id, sorted by name. Empty when it has none, except that an old
/// single-persona settings.json with a description yields that one.
pub fn st_list(settings: &Value) -> Vec<Persona> {
    let null = Value::Null;
    let pu = settings.get("power_user").unwrap_or(&null);
    let descs = pu.get("persona_descriptions").unwrap_or(&null);
    let mut out: Vec<Persona> = pu
        .get("personas")
        .and_then(|p| p.as_object())
        .map(|m| {
            m.iter()
                .map(|(avatar, name)| {
                    let name = name.as_str().unwrap_or("").to_string();
                    let d = descs.get(avatar).unwrap_or(&null);
                    from_descriptor(avatar, &name, d)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|p| p.name.to_lowercase());
    if out.is_empty() {
        let p = st_current(settings);
        if !p.description.trim().is_empty() {
            out.push(p);
        }
    }
    out
}

/// The avatar id of the persona SillyTavern has active.
pub fn st_active_id(settings: &Value) -> String {
    st_current(settings).id
}

/// Write a persona's fields into `power_user` (what the prompt builders read).
fn write_fields(settings: &mut Value, p: &Persona) {
    settings["username"] = json!(p.name);
    if !settings.get("power_user").map(|v| v.is_object()).unwrap_or(false) {
        settings["power_user"] = json!({});
    }
    let pu = &mut settings["power_user"];
    pu["persona_description"] = json!(p.description);
    pu["persona_description_position"] = json!(p.position);
    pu["persona_description_depth"] = json!(p.depth);
    pu["persona_description_role"] = json!(p.role);
    pu["persona_description_lorebook"] = json!(p.lorebook);
}

/// Make `power_user.persona_description*` and `username` agree with the active
/// persona, so the rest of st-cli can read them. In memory only: the settings
/// are SillyTavern's and are never written back.
pub fn normalize(settings: &mut Value) {
    let p = current();
    write_fields(settings, &p);
}

/// One-line, shortened view of a persona's description.
pub fn brief(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max).collect();
        format!("{}\u{2026}", cut.trim_end())
    }
}
