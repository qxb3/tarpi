//! Personas: who "you" are in the chat.
//!
//! SillyTavern keeps them in settings.json: `power_user.personas` maps an
//! avatar file to a name, `power_user.persona_descriptions` maps it to the
//! description, position and title, and `user_avatar` says which one is active.
//! `st-cli persona` switches the active one by writing those fields back, the
//! same way the web UI does when you click a persona.

use crate::api::Api;
use crate::config;
use crate::jv::*;
use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use serde_json::{json, Value};
use std::io::{self, IsTerminal};

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

// ------------------------------------------------------------------- menu

/// One-line, shortened view of a persona's description.
fn brief(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max).collect();
        format!("{}\u{2026}", cut.trim_end())
    }
}

fn print_menu(shown: &[&Persona], active: &str) {
    println!();
    let w = format!("[{}]", shown.len()).len();
    for (i, p) in shown.iter().enumerate() {
        println!(
            "{:<w$} {}{}",
            format!("[{}]", i + 1),
            p.label(),
            if p.avatar == active { " (current)" } else { "" },
            w = w
        );
        // Fit one terminal row (indent included), at most 110 characters.
        let b = brief(&p.description, crate::ui::term_cols().saturating_sub(w + 2).clamp(10, 110));
        let pad = " ".repeat(w + 1);
        if b.is_empty() {
            println!("{}(no description)", pad);
        } else {
            println!("{}{}", pad, b);
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "st-cli persona",
    about = "Pick the persona (who you are) used in chats",
    long_about = "Show the personas saved in SillyTavern and pick one. The choice is \
                  written to SillyTavern's settings, so st-cli and the web UI both use \
                  it from then on.",
    after_help = concat!(
        "EXAMPLES:\n",
        "  st-cli persona          pick from a menu\n",
        "  st-cli persona alice    switch to the persona matching 'alice'\n",
        "  st-cli persona -l       list personas and exit"
    )
)]
struct PersonaCli {
    /// Persona name (a partial match works). Leave it out to pick from a menu.
    persona: Option<String>,

    /// List personas and exit
    #[arg(short, long)]
    list: bool,
}

fn find<'a>(all: &'a [Persona], q: &str) -> Result<&'a Persona> {
    let ql = q.trim().to_lowercase();
    let exact: Vec<&Persona> = all
        .iter()
        .filter(|p| p.name.to_lowercase() == ql || p.avatar.to_lowercase() == ql)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0]);
    }
    let hits: Vec<&Persona> = all
        .iter()
        .filter(|p| p.name.to_lowercase().contains(&ql) || p.title.to_lowercase().contains(&ql))
        .collect();
    match hits.len() {
        0 => bail!("no persona matches '{}'. Run `st-cli persona -l`.", q),
        1 => Ok(hits[0]),
        _ => {
            let names: Vec<String> = hits.iter().take(10).map(|p| p.label()).collect();
            bail!("'{}' matches several personas: {}", q, names.join(", "))
        }
    }
}

/// Numbered menu; typing part of a name narrows it. `None` when the user quits.
fn pick<'a>(all: &'a [Persona], active: &str) -> Option<&'a Persona> {
    let mut shown: Vec<&Persona> = all.iter().collect();
    let mut reprint = true;
    loop {
        if reprint {
            print_menu(&shown, active);
            println!("[q] quit");
            reprint = false;
        }
        let ans = crate::ask("\nSelect > ")?;
        if crate::is_quit(&ans) {
            return None;
        }
        if ans.is_empty() {
            shown = all.iter().collect();
            reprint = true;
            continue;
        }
        if let Ok(n) = ans.parse::<usize>() {
            if n >= 1 && n <= shown.len() {
                return Some(shown[n - 1]);
            }
            println!("there is no number {}", n);
            continue;
        }
        let q = ans.to_lowercase();
        let hits: Vec<&Persona> = all
            .iter()
            .filter(|p| p.name.to_lowercase().contains(&q) || p.title.to_lowercase().contains(&q))
            .collect();
        if let Some(e) = hits.iter().find(|p| p.name.to_lowercase() == q) {
            return Some(*e);
        }
        match hits.len() {
            0 => println!("nothing matches '{}'", ans),
            1 => return Some(hits[0]),
            _ => {
                shown = hits;
                reprint = true;
            }
        }
    }
}

pub async fn run() -> Result<()> {
    // argv is `st-cli persona ...`; drop the program name so "persona" plays
    // the part of the binary name in clap's messages.
    let cli = PersonaCli::parse_from(std::env::args().skip(1));
    let cfg = config::load()?;
    let server = cfg
        .url
        .clone()
        .unwrap_or_else(|| "http://127.0.0.1:8000".to_string());
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();

    let api = Api::connect(&server).await.with_context(|| {
        format!(
            "Is SillyTavern running at {}? Start it, or set `url:` in \
             ~/.config/st-cli/config.yaml",
            server
        )
    })?;
    let mut settings = api.settings().await?;
    let all = list(&settings);
    let active = current(&settings).avatar;

    if cli.list {
        let shown: Vec<&Persona> = all.iter().collect();
        print_menu(&shown, &active);
        return Ok(());
    }

    let chosen = match cli.persona.as_deref() {
        Some(q) => find(&all, q)?,
        None if interactive => match pick(&all, &active) {
            Some(p) => p,
            None => return Ok(()),
        },
        None => bail!("say which persona to use: st-cli persona <name> (or -l to list them)"),
    };

    if chosen.avatar == active {
        println!("Already using {}.", chosen.label());
        return Ok(());
    }
    let avatar = chosen.avatar.clone();
    let p = activate(&mut settings, &avatar)?;
    api.save_settings(&settings)
        .await
        .context("saving the persona to SillyTavern")?;
    println!("Now using {}.", p.label());
    Ok(())
}
