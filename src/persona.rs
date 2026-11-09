//! Personas: who "you" are in the chat.
//!
//! SillyTavern keeps them in settings.json: `power_user.personas` maps an
//! avatar file to a name, `power_user.persona_descriptions` maps it to the
//! description, position and title, and `user_avatar` says which one is active.
//! `/persona` in a chat switches the active one by writing those fields back,
//! the same way the web UI does when you click a persona.

use crate::jv::*;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};

#[derive(Debug, Clone, Default)]
pub struct Persona {
    /// Avatar file, the persona's id in settings.json.
    pub avatar: String,
    pub name: String,
    pub title: String,
    pub description: String,
    pub position: i64,
    pub depth: i64,
    pub role: i64,
    pub lorebook: String,
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

fn from_descriptor(avatar: &str, name: &str, d: &Value) -> Persona {
    Persona {
        avatar: avatar.to_string(),
        name: name.to_string(),
        title: jstr(d, "title"),
        description: jstr(d, "description"),
        position: ji64(d, "position", 0),
        depth: ji64(d, "depth", 2),
        role: ji64(d, "role", 0),
        lorebook: jstr(d, "lorebook"),
    }
}

/// The persona that is active right now.
pub fn current(settings: &Value) -> Persona {
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
            avatar,
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

/// All personas on the server, sorted by name. Never empty: when settings.json
/// lists none, the active (legacy) persona is returned alone.
pub fn list(settings: &Value) -> Vec<Persona> {
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
        out.push(current(settings));
    }
    out
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
/// persona, so the rest of st-cli can read them without caring how this
/// SillyTavern version stores personas. In memory only; nothing is saved.
pub fn normalize(settings: &mut Value) {
    let p = current(settings);
    write_fields(settings, &p);
}

/// Make `avatar` the active persona (in `settings`, ready to be saved).
pub fn activate(settings: &mut Value, avatar: &str) -> Result<Persona> {
    let p = list(settings)
        .into_iter()
        .find(|p| p.avatar == avatar)
        .ok_or_else(|| anyhow!("no persona '{}'", avatar))?;
    settings["user_avatar"] = json!(p.avatar);
    write_fields(settings, &p);
    Ok(p)
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

/// The plain 32x32 picture a persona created here gets as its avatar (SillyTavern
/// lists personas by their image; the web UI can replace it later).
pub const AVATAR_PNG: &[u8] = &[137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 32, 0, 0, 0, 32, 8, 2, 0, 0, 0, 252, 24, 237, 163, 0, 0, 0, 42, 73, 68, 65, 84, 120, 218, 99, 136, 202, 235, 161, 41, 98, 24, 181, 96, 212, 130, 81, 11, 70, 45, 24, 181, 96, 212, 130, 81, 11, 70, 45, 24, 181, 96, 212, 130, 81, 11, 134, 138, 5, 0, 39, 74, 80, 76, 121, 203, 72, 248, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130];

fn ensure_obj<'a>(v: &'a mut Value, key: &str) -> &'a mut Value {
    if !v.get(key).map(|x| x.is_object()).unwrap_or(false) {
        v[key] = json!({});
    }
    &mut v[key]
}

fn descriptor(p: &Persona) -> Value {
    json!({
        "description": p.description,
        "position": p.position,
        "depth": p.depth,
        "role": p.role,
        "lorebook": p.lorebook,
        "title": p.title,
    })
}

/// Create the persona `avatar`, or change its name, title and description
/// (position, depth, role and lorebook stay as they are). In `settings`,
/// ready to be saved. The active persona's `power_user` fields follow.
pub fn put(settings: &mut Value, avatar: &str, name: &str, title: &str, description: &str) {
    // Settings that only know the single legacy persona: write it into the
    // maps first, so adding one does not make it vanish from the list.
    let cur = current(settings);
    let known = settings
        .get("power_user")
        .and_then(|pu| pu.get("personas"))
        .map(|m| m.get(&cur.avatar).is_some())
        .unwrap_or(false);
    let keep = list(settings).into_iter().find(|p| p.avatar == avatar);
    let mut base = keep.unwrap_or(Persona {
        avatar: avatar.to_string(),
        depth: 2,
        ..Persona::default()
    });
    base.name = name.to_string();
    base.title = title.to_string();
    base.description = description.to_string();

    if !settings.get("power_user").map(|v| v.is_object()).unwrap_or(false) {
        settings["power_user"] = json!({});
    }
    let pu = &mut settings["power_user"];
    if !known && !cur.avatar.is_empty() && cur.avatar != avatar {
        ensure_obj(pu, "personas")[cur.avatar.as_str()] = json!(cur.name);
        ensure_obj(pu, "persona_descriptions")[cur.avatar.as_str()] = descriptor(&cur);
    }
    ensure_obj(pu, "personas")[avatar] = json!(name);
    ensure_obj(pu, "persona_descriptions")[avatar] = descriptor(&base);
    if jstr(settings, "user_avatar") == avatar {
        let p = current(settings);
        write_fields(settings, &p);
    }
}

/// Forget the persona `avatar` (the image file is deleted separately).
pub fn remove(settings: &mut Value, avatar: &str) {
    for map in ["personas", "persona_descriptions"] {
        if let Some(m) = settings
            .get_mut("power_user")
            .and_then(|pu| pu.get_mut(map))
            .and_then(|m| m.as_object_mut())
        {
            m.remove(avatar);
        }
    }
}
