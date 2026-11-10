//! `tarpi import-st <url>`: copy the personas, the system prompts and the
//! characters of a SillyTavern server into tarpi's own folders and files
//! (`~/.config/tarpi/persona/`, `~/.config/tarpi/system-prompt.jsonl` and
//! `~/.config/tarpi/characters/`, see `persona`, `sysprompt` and `charstore`).
//!
//! The system prompts are the server's system prompt presets, plus the one it
//! has switched on when that is not one of them. An identical one (name, prompt
//! and post-history instructions) is skipped; with `--overwrite` one with the
//! same name is replaced. If you have not chosen a system prompt here yet, the
//! one the server had on becomes yours too.
//!
//! Only reads from the server; nothing there is changed. A persona's picture is
//! not copied. A character is copied as its card PNG with the file name it has
//! on the server. Its chats are copied too (`~/.config/tarpi/chats/`); a chat
//! that already exists here is kept unless `--overwrite` is given.
//! Personas may share a name: an identical one (name, title and description) is
//! skipped, so the command can be run again safely; with `--overwrite` one with
//! the same name and title is replaced. Characters are skipped when a card of
//! that file name is already here, or replaced with `--overwrite`.

use crate::api::Api;
use crate::charstore;
use crate::chatstore;
use crate::config;
use crate::jv::*;
use crate::persona::{self, Imported};
use crate::sysprompt;
use anyhow::{Context, Result};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "tarpi import-st",
    about = "Import the personas, system prompts and characters of a SillyTavern server into tarpi",
    long_about = "Copy every persona (name, title, description, position, depth, role and \
                  lorebook), every system prompt (name, prompt and post-history \
                  instructions) and every character card of a running SillyTavern server \
                  into tarpi's own folders and files, ~/.config/tarpi/persona/, \
                  ~/.config/tarpi/system-prompt.jsonl and \
                  ~/.config/tarpi/characters/. Nothing is changed on the server. Every character's chats \
                  are copied to ~/.config/tarpi/chats/ too (chats that already exist here \
                  are kept). A character that is already here is skipped. \
                  Personas with the same name are kept side by side; an identical one \
                  is skipped, and --overwrite replaces one with the same name and title.",
    after_help = concat!(
        "EXAMPLES:\n",
        "  tarpi import-st http://127.0.0.1:8000\n",
        "  tarpi import-st 192.168.1.20:8000\n",
        "  tarpi import-st http://127.0.0.1:8000 --overwrite"
    )
)]
struct ImportStArgs {
    /// Address of the SillyTavern server, e.g. http://127.0.0.1:8000
    url: String,

    /// Replace a persona with the same name and title, a system prompt with the same
    /// name, and a character with the same file name, instead of skipping it
    #[arg(short, long)]
    overwrite: bool,
}

/// `host:port` becomes `http://host:port`.
fn with_scheme(url: &str) -> String {
    let t = url.trim();
    let lower = t.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        t.to_string()
    } else {
        format!("http://{}", t)
    }
}

pub async fn run() -> Result<()> {
    // argv is `tarpi import-st ...`; drop the program name so "import-st"
    // plays the part of the binary name in clap's messages.
    let cli = ImportStArgs::parse_from(std::env::args().skip(1));
    let server = with_scheme(&cli.url);

    let api = Api::connect(&server).await.with_context(|| {
        format!(
            "Is SillyTavern running at {}? Start it, or check the address",
            server
        )
    })?;
    let settings = api.settings().await?;
    let found = persona::st_list(&settings);
    let st_active = persona::st_active_id(&settings);
    if found.is_empty() {
        println!("No personas found on {}.", server);
    }

    let found_any = !found.is_empty();
    let (mut added, mut updated, mut skipped) = (0, 0, 0);
    let mut active_here: Option<String> = None;
    for p in found {
        let label = p.label();
        let is_active = p.id == st_active;
        let outcome = persona::import(p, cli.overwrite)?;
        match &outcome {
            Imported::Added(_) => {
                added += 1;
                println!("  added    {}", label);
            }
            Imported::Updated(_) => {
                updated += 1;
                println!("  updated  {}", label);
            }
            Imported::Skipped(_) => {
                skipped += 1;
                println!("  skipped  {} (identical one already here)", label);
            }
        }
        if is_active {
            active_here = Some(outcome.id().to_string());
        }
    }

    // First import on a fresh install: keep using the persona the server had.
    let mut note = String::new();
    if let Some(id) = active_here {
        if persona::activate_if_unset(&id)? {
            note = " The server's active persona is now yours too.".to_string();
        }
    }
    if found_any {
        println!(
            "Personas: {} added, {} updated, {} skipped ({}).{}",
            added,
            updated,
            skipped,
            config::persona_dir()
                .map(|d| d.display().to_string())
                .unwrap_or_else(|| "the persona folder".to_string()),
            note
        );
    }

    // ---- system prompts
    // The server's presets (empty when it is too old to send them), and the
    // one it has switched on.
    let presets = api.sysprompts().await.unwrap_or_default();
    let st_prompt = sysprompt::st_active(&settings);
    let prompts = sysprompt::st_list(&presets, &settings);
    if prompts.is_empty() {
        println!("No system prompts found on {}.", server);
    }
    let prompts_any = !prompts.is_empty();
    let (mut s_added, mut s_updated, mut s_skipped) = (0, 0, 0);
    let mut s_active_here: Option<String> = None;
    for p in prompts {
        let label = p.name.clone();
        let is_active = st_prompt
            .as_ref()
            .map(|a| sysprompt::same_prompt(a, &p))
            .unwrap_or(false);
        let outcome = sysprompt::import(p, cli.overwrite)?;
        match &outcome {
            Imported::Added(_) => {
                s_added += 1;
                println!("  added    {}", label);
            }
            Imported::Updated(_) => {
                s_updated += 1;
                println!("  updated  {}", label);
            }
            Imported::Skipped(_) => {
                s_skipped += 1;
                println!("  skipped  {} (identical one already here)", label);
            }
        }
        if is_active {
            s_active_here = Some(outcome.id().to_string());
        }
    }
    // First import on a fresh install: keep using the prompt the server had on.
    let mut s_note = String::new();
    if let Some(id) = s_active_here {
        if sysprompt::activate_if_unset(&id)? {
            s_note = " The server's system prompt is now yours too.".to_string();
        }
    }
    if prompts_any {
        println!(
            "System prompts: {} added, {} updated, {} skipped ({}).{}",
            s_added,
            s_updated,
            s_skipped,
            config::sysprompt_path()
                .map(|d| d.display().to_string())
                .unwrap_or_else(|| "the system prompt file".to_string()),
            s_note
        );
    }

    // ---- characters
    let list = api.characters().await?;
    if list.is_empty() {
        println!("No characters found on {}.", server);
    }
    let (mut c_added, mut c_updated, mut c_skipped, mut c_failed) = (0, 0, 0, 0);
    let (mut chats_added, mut chats_skipped, mut chats_failed) = (0, 0, 0);
    for c in &list {
        let avatar = jstr(c, "avatar");
        let name = {
            let n = jstr(c, "name");
            if n.is_empty() {
                avatar.clone()
            } else {
                n
            }
        };
        let stem = avatar.strip_suffix(".png").unwrap_or(&avatar).to_string();
        let result = async {
            if avatar.is_empty() {
                anyhow::bail!("no file name");
            }
            let existed = charstore::get(&avatar).is_ok();
            if existed && !cli.overwrite {
                return Ok::<_, anyhow::Error>("skipped");
            }
            let bytes = api.character_image(&avatar).await?;
            charstore::save(&bytes, &avatar, Some(&stem), cli.overwrite)?;
            Ok(if existed { "updated" } else { "added" })
        }
        .await;
        // The chats of every character that is here now (also one that was
        // already here), unless a chat of that name exists locally.
        if !matches!(result, Err(_)) && !avatar.is_empty() {
            match api.chats(&avatar).await {
                Ok(chats) => {
                    for ch in &chats {
                        let file = crate::chat::file_id(ch);
                        let one = async {
                            let lines = api.load_chat(&avatar, &name, &file).await?;
                            if lines.is_empty() {
                                anyhow::bail!("empty chat");
                            }
                            chatstore::import(&avatar, &file, &lines, cli.overwrite)
                        }
                        .await;
                        match one {
                            Ok(true) => chats_added += 1,
                            Ok(false) => chats_skipped += 1,
                            Err(e) => {
                                chats_failed += 1;
                                println!("  chat failed  {} / {}: {:#}", name, file, e);
                            }
                        }
                    }
                }
                Err(_) => {}
            }
        }
        match result {
            Ok("added") => {
                c_added += 1;
                println!("  added    {}", name);
            }
            Ok("updated") => {
                c_updated += 1;
                println!("  updated  {}", name);
            }
            Ok(_) => {
                c_skipped += 1;
                println!("  skipped  {} (already here, use --overwrite to replace)", name);
            }
            Err(e) => {
                c_failed += 1;
                println!("  failed   {}: {:#}", name, e);
            }
        }
    }
    if !list.is_empty() {
        println!(
            "Characters: {} added, {} updated, {} skipped, {} failed ({}).",
            c_added,
            c_updated,
            c_skipped,
            c_failed,
            config::characters_dir()
                .map(|d| d.display().to_string())
                .unwrap_or_else(|| "the characters folder".to_string())
        );
    }
    if chats_added + chats_skipped + chats_failed > 0 {
        println!(
            "Chats: {} added, {} skipped, {} failed ({}).",
            chats_added,
            chats_skipped,
            chats_failed,
            config::chats_dir()
                .map(|d| d.display().to_string())
                .unwrap_or_else(|| "the chats folder".to_string())
        );
    }
    Ok(())
}
