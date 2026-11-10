mod api;
mod apitui;
mod app;
mod cardedit;
mod character;
mod charstore;
mod chatstore;
mod charimg;
mod chat;
mod clipboard;
mod cmds;
mod config;
mod conn;
mod connect;
mod fmttui;
mod formatting;
mod gen;
mod health;
mod history;
mod import;
mod import_st;
mod input;
mod jv;
mod mirror;
mod note;
mod persona;
mod picker;
mod posedit;
mod prompt;
mod prose;
mod serve;
mod spell;
mod summary;
mod sysprompt;
mod termcolors;
mod textgen;
mod tokens;
mod ui;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, EventStream, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use futures_util::StreamExt;
use ratatui::{backend::CrosstermBackend, Terminal};
use serde_json::Value;
use std::io::{self, IsTerminal, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;

use crate::app::App;
use crate::character::Character;
use crate::chat::{file_id, ChatState};
use crate::jv::*;

/// How the chat screen of a round was reached (told to the `tarpi serve` log
/// when connected, see `connect::report`).
#[derive(Clone, Copy)]
enum Arrival {
    Start,
    Chat,
    Character,
}

#[derive(Parser, Debug)]
#[command(
    name = "tarpi",
    version,
    about = "Standalone terminal chat client for character cards",
    long_about = "Chat with a character from the terminal. Everything is kept by \
                  tarpi itself in ~/.config/tarpi/ (characters, personas, system prompts, \
                  formatting, chats). Add cards with `/import` (in a chat or in the character \
                  menu); `tarpi import-st <url>` can copy yours over from a \
                  SillyTavern server. Nothing needs a server \
                  except the backend: replies come from the API connection managed with \
                  `/api` inside a chat.",
    args_conflicts_with_subcommands = true,
    disable_help_subcommand = true,
    after_help = concat!(
        "INSIDE A CHAT:\n",
        "  /chats          choose, create or delete chats (a window)\n",
        "  /chars          choose another character; d deletes one (a window)\n",
        "  /import <card>  add a character: card .png / .json file, chub.ai or pygmalion.chat link\n",
        "  /persona        choose who you are in the chat (a window)\n",
        "  /note           edit this chat's Author's Note (/an)\n",
        "  /one-shot       lock this chat: no editing, undo, regen, swipes, branches\n",
        "  /api            API settings (address, model and sampler); back to the chat on quit\n",
        "  /formatting     context and instruct templates (story string, prefixes, suffixes)\n",
        "  /char           view and edit this character's card\n",
        "  /char-img       show the card's picture in a square window\n",
        "  /new            new chat with this character (keeps this one)\n",
        "  /newchat        delete this chat and start a new one (/nc)\n",
        "  /summarize      add an in-depth summary of the chat as a new summary branch (/summ)\n",
        "  /deletebranch   delete the branch that is showing (/db)\n",
        "  /regen [nudge]  regenerate the last reply, optionally steered\n",
        "  /continue       continue the last reply (/c)\n",
        "  /undo [n]       delete the last message, or the last n (/u)\n",
        "  /help           all keys and commands\n",
        "\nSHARING BETWEEN DEVICES:\n",
        "  tarpi serve             share your characters, chats and settings on the network\n",
        "  tarpi connect [token]   use them from another device (serve_url in config.yaml)\n",
        "\nTyping / opens a command menu above the input box; the top entry is picked\n(Tab, Shift+Tab, Up, Down to change it, Enter to run).\nWith only a / in the box, Up brings back the commands you ran before."
    )
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Character name or card file name (a partial match works).
    /// Leave it out to pick from a list.
    character: Option<String>,

    /// Chat to open: its number (3) or name (Sofia03). Default: the latest.
    #[arg(requires = "character", conflicts_with = "new")]
    chat: Option<String>,

    /// Start a new chat instead of continuing the latest one
    #[arg(short, long)]
    new: bool,

    /// List characters, or the chats of the given character, and exit
    #[arg(short, long)]
    list: bool,
}

/// The commands that are not a chat. Each has its own options and `--help`,
/// handled by the module that runs it; clap only passes everything after the
/// command name on (`Rest`).
#[derive(Subcommand, Debug)]
enum Commands {
    /// Copy the personas and characters of a SillyTavern server into tarpi's own folders
    #[command(disable_help_flag = true)]
    ImportSt(Rest),
    /// Share characters, chats, personas, system prompts and API connections with other devices (no chat screen)
    #[command(disable_help_flag = true)]
    Serve(Rest),
    /// Chat using the data of a machine that runs `tarpi serve`, keeping a copy in step
    #[command(disable_help_flag = true)]
    Connect(Rest),
}

/// Everything after a command's name, passed on unchanged (`--help` included).
#[derive(Args, Debug)]
#[allow(dead_code)]
struct Rest {
    #[arg(num_args = 0.., trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

type Term = Terminal<CrosstermBackend<Stdout>>;

/// Set when the terminal was asked to report modified keys (Shift+Enter).
static KEYS_ENHANCED: AtomicBool = AtomicBool::new(false);

fn char_display_name(c: &Value) -> String {
    let n = jstr(c, "name");
    if n.is_empty() {
        jstr(c, "avatar")
    } else {
        n
    }
}

fn avatar_stem(c: &Value) -> String {
    let a = jstr(c, "avatar").to_lowercase();
    a.strip_suffix(".png").map(|s| s.to_string()).unwrap_or(a)
}

/// Resolve the user's query to exactly one character, or explain why not.
fn find_character<'a>(list: &'a [Value], query: &str) -> Result<&'a Value> {
    let q = query.trim().to_lowercase();
    let q_stem = q.strip_suffix(".png").unwrap_or(&q).to_string();

    let exact: Vec<&Value> = list
        .iter()
        .filter(|c| jstr(c, "name").to_lowercase() == q || avatar_stem(c) == q_stem)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0]);
    }

    let candidates: Vec<&Value> = if exact.len() > 1 {
        exact
    } else {
        list.iter()
            .filter(|c| {
                jstr(c, "name").to_lowercase().contains(&q) || avatar_stem(c).contains(&q_stem)
            })
            .collect()
    };

    match candidates.len() {
        0 => bail!(
            "no character matches '{}'. Run `tarpi -l` to see what is available.",
            query
        ),
        1 => Ok(candidates[0]),
        _ => {
            let mut msg = format!("'{}' matches several characters:\n", query);
            for c in candidates.iter().take(15) {
                msg.push_str(&format!(
                    "  {}  ({})\n",
                    char_display_name(c),
                    jstr(c, "avatar")
                ));
            }
            msg.push_str("Be more specific, or pass the card file name.");
            Err(anyhow!(msg))
        }
    }
}

fn is_fav(c: &Value) -> bool {
    c.get("fav")
        .map(|f| matches!(f, Value::Bool(true)) || f.as_str() == Some("true"))
        .unwrap_or(false)
}

fn print_characters(list: &[Value]) {
    let mut items: Vec<&Value> = list.iter().collect();
    items.sort_by_key(|c| char_display_name(c).to_lowercase());
    for c in items {
        println!(
            "{}{}  ({})",
            if is_fav(c) { "\u{2605} " } else { "  " },
            char_display_name(c),
            jstr(c, "avatar")
        );
    }
}

fn sort_chats_newest_first(chats: &mut [Value]) {
    chats.sort_by(|a, b| {
        last_ms(b)
            .partial_cmp(&last_ms(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// When a chat was last used, in ms since the epoch (0.0 if unknown).
/// SillyTavern has sent `last_mes` as a millisecond number (file mtime) and,
/// in newer versions, as a date string (ISO 8601 or its own formats).
fn last_ms(c: &Value) -> f64 {
    // Some servers send seconds; anything below 1e11 cannot be ms in this century.
    let norm = |x: f64| if x > 0.0 && x < 1e11 { x * 1000.0 } else { x };
    match c.get("last_mes") {
        Some(Value::Number(n)) => norm(n.as_f64().unwrap_or(0.0)),
        Some(Value::String(s)) => {
            let t = s.trim();
            match t.parse::<f64>() {
                Ok(x) => norm(x),
                Err(_) => parse_date_ms(t),
            }
        }
        _ => 0.0,
    }
}

fn parse_date_ms(s: &str) -> f64 {
    use chrono::TimeZone;
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(s) {
        return d.timestamp_millis() as f64;
    }
    // SillyTavern's own formats carry no zone: they are local time.
    const FORMATS: [&str; 9] = [
        "%Y-%m-%d@%Hh%Mm%Ss%3fms",
        "%Y-%m-%d @%Hh %Mm %Ss %3fms",
        "%Y-%m-%d@%Hh%Mm%Ss",
        "%Y-%m-%d @%Hh %Mm %Ss",
        "%B %d, %Y %I:%M%P",
        "%B %d, %Y %I:%M%p",
        "%b %d, %Y %I:%M%P",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
    ];
    for f in FORMATS {
        if let Ok(n) = chrono::NaiveDateTime::parse_from_str(s, f) {
            if let Some(d) = chrono::Local.from_local_datetime(&n).single() {
                return d.timestamp_millis() as f64;
            }
        }
    }
    0.0
}

/// "5m ago" for a chat; if the server's date is in a format we cannot read,
/// show it as it came instead of hiding it.
fn when(c: &Value) -> String {
    let ms = last_ms(c);
    if ms > 0.0 {
        return ago(ms);
    }
    let raw = jstr(c, "last_mes");
    if raw.trim().is_empty() {
        "unknown".to_string()
    } else {
        raw
    }
}

/// "5m ago", "3h ago", "2d ago" from a millisecond timestamp.
fn ago(ms: f64) -> String {
    if ms <= 0.0 {
        return "unknown".to_string();
    }
    let secs = ((chrono::Utc::now().timestamp_millis() as f64 - ms) / 1000.0).max(0.0) as u64;
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

fn print_chat_list(chats: &[Value], indent: &str) {
    let width = chats
        .iter()
        .map(|c| file_id(c).chars().count())
        .max()
        .unwrap_or(0);
    // Narrow terminals: drop the size column and shorten the wording.
    let narrow = ui::term_cols() < 60;
    for c in chats {
        if narrow {
            println!(
                "{}{:<w$}  {:>4} msgs  {}",
                indent,
                file_id(c),
                ji64(c, "chat_items", 0),
                when(c),
                w = width
            );
        } else {
            println!(
                "{}{:<w$}  {:>4} messages  {:>8}  {}",
                indent,
                file_id(c),
                ji64(c, "chat_items", 0),
                jstr(c, "file_size"),
                when(c),
                w = width
            );
        }
    }
}

// ------------------------------------------------------ chat name lookup

/// Find a chat of one character by exact name, chat number (`3` -> `Sofia03`)
/// or a unique partial name.
fn match_chat(char_name: &str, chats: &[Value], query: &str) -> Result<String> {
    let q = query.trim().trim_end_matches(".jsonl").to_string();
    let ql = q.to_lowercase();
    let ids: Vec<String> = chats.iter().map(file_id).collect();

    if let Some(id) = ids.iter().find(|i| i.to_lowercase() == ql) {
        return Ok(id.clone());
    }
    if let Ok(n) = q.parse::<u32>() {
        let safe = chat::safe_name(char_name).to_lowercase();
        for cand in [format!("{}{:02}", safe, n), format!("{}{}", safe, n)] {
            if let Some(id) = ids.iter().find(|i| i.to_lowercase() == cand) {
                return Ok(id.clone());
            }
        }
        bail!(
            "{} has no chat number {}. See `tarpi {} -l`.",
            char_name,
            n,
            char_name
        );
    }
    let prefix: Vec<&String> = ids
        .iter()
        .filter(|i| i.to_lowercase().starts_with(&ql))
        .collect();
    let contains: Vec<&String> = ids
        .iter()
        .filter(|i| i.to_lowercase().contains(&ql))
        .collect();
    for set in [prefix, contains] {
        if set.len() == 1 {
            return Ok(set[0].clone());
        }
        if set.len() > 1 {
            let names: Vec<&str> = set.iter().take(10).map(|s| s.as_str()).collect();
            bail!("'{}' matches several chats: {}", query, names.join(", "));
        }
    }
    bail!(
        "{} has no chat '{}'. See `tarpi {} -l`.",
        char_name,
        query,
        char_name
    )
}

// ------------------------------------------------------- interactive menus

fn ask(prompt: &str) -> Option<String> {
    print!("{}", prompt);
    let _ = io::stdout().flush();
    let mut s = String::new();
    match io::stdin().read_line(&mut s) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(s.trim().to_string()),
    }
}

fn is_quit(s: &str) -> bool {
    matches!(s.to_lowercase().as_str(), "q" | "quit" | "exit")
}

const MENU_ROWS: usize = 40;

/// The cards in the characters folder, sorted by name.
fn sorted_characters() -> Vec<Value> {
    let mut v = charstore::list();
    v.sort_by_key(|c| char_display_name(c).to_lowercase());
    v
}

/// `/import <card>` typed in the character menu: what comes after it.
fn import_arg(ans: &str) -> Option<&str> {
    let head = ans.get(..7)?;
    if !head.eq_ignore_ascii_case("/import") {
        return None;
    }
    let rest = &ans[7..];
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest.trim())
    } else {
        None
    }
}

/// Numbered character menu. Typing part of a name narrows the list;
/// `/import <card>` adds a character first (the only way in when there are
/// none yet), as inside a chat.
async fn pick_character() -> Result<Value> {
    let mut sorted = sorted_characters();
    let mut shown: Vec<Value> = sorted.clone();
    let mut reprint = true;
    loop {
        if reprint {
            println!();
            if sorted.is_empty() {
                println!("No characters yet.");
            }
            let w = format!("[{}]", shown.len().min(MENU_ROWS)).len();
            for (i, c) in shown.iter().enumerate().take(MENU_ROWS) {
                println!(
                    "{:<w$} {}{}",
                    format!("[{}]", i + 1),
                    if is_fav(c) { "\u{2605} " } else { "" },
                    char_display_name(c),
                    w = w
                );
            }
            if shown.len() > MENU_ROWS {
                println!(
                    "\u{2026} and {} more: type part of a name to narrow it down",
                    shown.len() - MENU_ROWS
                );
            }
            println!("[/import <card file or chub.ai / pygmalion.chat link>] add a character");
            println!("[q] quit");
            reprint = false;
        }
        let Some(ans) = ask("\nSelect > ") else {
            std::process::exit(0);
        };
        if let Some(arg) = import_arg(&ans) {
            if arg.is_empty() {
                println!("{}", import::USAGE);
                continue;
            }
            println!("Importing...");
            match import::import(arg).await {
                Ok(saved) => {
                    let name = if saved.name.is_empty() {
                        saved.avatar.clone()
                    } else {
                        saved.name.clone()
                    };
                    println!("Imported \"{}\" ({})", name, saved.avatar);
                    connect::report(format!("imported character {}", name));
                    sorted = sorted_characters();
                    shown = sorted.clone();
                    reprint = true;
                }
                Err(e) => println!("{:#}", e),
            }
            continue;
        }
        if ans.is_empty() {
            shown = sorted.clone();
            reprint = true;
            continue;
        }
        if is_quit(&ans) {
            std::process::exit(0);
        }
        if let Ok(n) = ans.parse::<usize>() {
            if n >= 1 && n <= shown.len() {
                return Ok(shown[n - 1].clone());
            }
            println!("there is no number {}", n);
            continue;
        }
        let q = ans.to_lowercase();
        let hits: Vec<Value> = sorted
            .iter()
            .filter(|c| jstr(c, "name").to_lowercase().contains(&q) || avatar_stem(c).contains(&q))
            .cloned()
            .collect();
        if let Some(e) = hits.iter().find(|c| jstr(c, "name").to_lowercase() == q) {
            return Ok(e.clone());
        }
        match hits.len() {
            0 => println!("nothing matches '{}'", ans),
            1 => return Ok(hits[0].clone()),
            _ => {
                shown = hits;
                reprint = true;
            }
        }
    }
}

enum Want {
    Latest,
    New,
    File(String),
}

/// Termux or any other Android terminal.
fn on_android() -> bool {
    std::env::var_os("TERMUX_VERSION").is_some()
        || std::env::var_os("ANDROID_ROOT").is_some()
        || std::env::var("PREFIX")
            .map(|p| p.contains("com.termux"))
            .unwrap_or(false)
}

fn setup_terminal(mouse: bool) -> Result<Term> {
    enable_raw_mode().context("enabling raw mode")?;
    // Read the terminal's theme colours for the prose styling. Must run now:
    // raw mode is on and nothing else is reading the keyboard yet.
    termcolors::detect();
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableBracketedPaste)?;
    // With mouse reporting on, taps go to tarpi as clicks. With it off the
    // terminal handles touch itself, so a tap opens the phone's keyboard.
    if mouse {
        execute!(out, EnableMouseCapture)?;
    }
    // Terminals that speak the kitty keyboard protocol (kitty, WezTerm,
    // foot, Ghostty, Alacritty, recent iTerm2, ...) can then tell Shift+Enter
    // from Enter. Others keep working; Alt+Enter / Ctrl+J are the fallback.
    if matches!(supports_keyboard_enhancement(), Ok(true))
        && execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok()
    {
        KEYS_ENHANCED.store(true, Ordering::SeqCst);
    }
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

fn restore_terminal() {
    if KEYS_ENHANCED.swap(false, Ordering::SeqCst) {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
}

/// `/api`: show the API manager in this same terminal, then carry on with the
/// chat exactly where it was (draft, scroll and swipes are untouched).
async fn open_api_manager(term: &mut Term, events: &mut EventStream, app: &mut App) {
    // The manager is clicked and dragged, so mouse reporting must be on even
    // when a phone's keyboard had switched it off for the chat.
    let mouse_was_on = app.mouse;
    if !mouse_was_on {
        let _ = execute!(io::stdout(), EnableMouseCapture);
    }
    let res = apitui::manage(term, events).await;
    if !mouse_was_on {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
    // Redraw every cell: the manager left its own picture on the screen.
    let _ = term.clear();
    let rows = crossterm::terminal::size().map(|(_, h)| h).unwrap_or(0);
    app.api_closed(res, rows);
}

/// `/formatting`: the same as `/api`, for the context and instruct templates.
async fn open_formatting_manager(term: &mut Term, events: &mut EventStream, app: &mut App) {
    let mouse_was_on = app.mouse;
    if !mouse_was_on {
        let _ = execute!(io::stdout(), EnableMouseCapture);
    }
    let res = fmttui::manage(term, events).await;
    if !mouse_was_on {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
    let _ = term.clear();
    let rows = crossterm::terminal::size().map(|(_, h)| h).unwrap_or(0);
    app.formatting_closed(res, rows);
}

async fn run_tui(
    term: &mut Term,
    app: &mut App,
    gen_rx: &mut tokio::sync::mpsc::UnboundedReceiver<gen::GenMsg>,
) -> Result<()> {
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let img_proto = charimg::detect();
    let mut img_shown = false;
    loop {
        // The picture window closed: take a terminal-drawn picture off the screen.
        if img_shown && app.image_modal.is_none() {
            if img_proto == charimg::Proto::Kitty {
                let w = term.backend_mut();
                w.write_all(charimg::KITTY_DELETE)?;
                w.flush()?;
            }
            term.clear()?;
            img_shown = false;
        }
        term.draw(|f| ui::draw(f, app))?;
        // Terminal graphics are sent after the frame, over the blank cells.
        if let Some(m) = app.image_modal.as_mut() {
            if let Some(r) = m.want.take() {
                if m.placed != Some(r) {
                    let mut bytes = Vec::new();
                    if m.proto == charimg::Proto::Kitty {
                        bytes.extend_from_slice(charimg::KITTY_DELETE);
                    }
                    bytes.extend(m.escape(r));
                    let w = term.backend_mut();
                    w.write_all(&bytes)?;
                    w.flush()?;
                    m.placed = Some(r);
                    img_shown = true;
                }
            }
        }
        tokio::select! {
            ev = events.next() => {
                match ev {
                    Some(Ok(Event::Resize(_, rows))) => {
                        if let Some(m) = app.image_modal.as_mut() {
                            m.placed = None; // the screen was cleared: send it again
                        }
                        app.on_resize(rows)
                    }
                    Some(Ok(e)) => app.on_event(e).await,
                    Some(Err(e)) => return Err(e.into()),
                    None => break,
                }
            }
            msg = gen_rx.recv() => {
                if let Some(m) = msg {
                    app.on_gen_event(m).await;
                }
            }
            _ = tick.tick() => {
                // `tarpi connect`: the server went away, so there is nothing to chat on.
                if connect::server_lost() {
                    app.should_quit = true;
                }
                app.on_tick()
            }
        }
        if app.open_api {
            app.open_api = false;
            open_api_manager(term, &mut events, app).await;
            // Do not fire the ticks that piled up while the manager was open.
            tick.reset();
        }
        if app.open_formatting {
            app.open_formatting = false;
            open_formatting_manager(term, &mut events, app).await;
            tick.reset();
        }
        if app.should_quit {
            break;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut cli = Cli::parse();
    // The commands have their own arguments and `--help`: each module reads
    // them again from the command line. Everything else is the chat CLI.
    // `connect` is the chat CLI too, on a copy of another machine's data: it
    // syncs first (see `connect`), then you pick a character as usual.
    let mut sync_session: Option<connect::Session> = None;
    match cli.command.take() {
        Some(Commands::ImportSt(_)) => return import_st::run().await,
        Some(Commands::Serve(_)) => return serve::run().await,
        Some(Commands::Connect(_)) => sync_session = Some(connect::run().await?),
        None => {}
    }
    let cfg = config::load()?;

    let chars_per_token = cfg.chars_per_token.unwrap_or(3.2);
    // Mouse reporting starts on, so messages can be clicked or tapped. On a
    // phone, tapping the input box turns it off to bring up the keyboard
    // (see `App::touch`).
    let touch = on_android();
    let mut mouse_on = true;

    let characters = charstore::list();
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();

    // ---- -l: list characters, or one character's chats, and exit
    if cli.list {
        match cli.character.as_deref() {
            Some(q) => {
                let entry = find_character(&characters, q)?;
                let mut chats = chatstore::list(&jstr(entry, "avatar"))?;
                sort_chats_newest_first(&mut chats);
                if chats.is_empty() {
                    println!("(no chats yet for {})", char_display_name(entry));
                } else {
                    print_chat_list(&chats, "");
                }
            }
            None => print_characters(&characters),
        }
        return Ok(());
    }

    // ---- who to chat with, and which chat
    let mut entry: Value = match cli.character.as_deref() {
        Some(q) => find_character(&characters, q)?.clone(),
        None if interactive => pick_character().await?,
        None => {
            eprintln!("Usage: tarpi [character] [chat] [-n]   (tarpi --help for more)\n");
            eprintln!("Characters:");
            print_characters(&characters);
            std::process::exit(2);
        }
    };
    let mut want = match cli.chat.clone() {
        Some(c) => Want::File(c),
        None if cli.new => Want::New,
        None => Want::Latest,
    };

    // Always give the terminal back, even on panic.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    // One round per chat screen. Choosing something in the `/chats` or
    // `/characters` window leaves the screen and comes back here for the next round.
    let mut arrival = Arrival::Start;
    loop {
        let avatar = jstr(&entry, "avatar");
        // The list may be "shallow" (lazyLoadCharacters), so always fetch the full card.
        let full = charstore::get(&avatar)?;
        let character = Character::from_value(&full);

        let mut chats = chatstore::list(&avatar)?;
        sort_chats_newest_first(&mut chats);
        let ids: Vec<String> = chats.iter().map(file_id).collect();
        let next_name = chat::next_chat_name(&character.name, &ids);

        let mut settings = config::base_settings();
        persona::normalize(&mut settings);
        let user_name = persona::current().name;
        let persona_text = jstr(settings.get("power_user").unwrap_or(&Value::Null), "persona_description");

        let explicit = matches!(want, Want::File(_));
        let (file, created_new) = match std::mem::replace(&mut want, Want::Latest) {
            Want::New => (next_name, true),
            Want::Latest => match chats.iter().find(|c| !chat::is_summary_backup(&file_id(c))) {
                Some(c) => (file_id(c), false),
                None => (next_name, true),
            },
            Want::File(q) => (match_chat(&character.name, &chats, &q)?, false),
        };

        let (chat_state, synced) = if created_new {
            let mut st = ChatState::new_empty(&avatar, &character.name, &file);
            if let Some(g) = app::make_greeting(&character, &user_name, &persona_text) {
                st.messages.push(g);
            }
            (st, false)
        } else {
            let lines = chatstore::load(&avatar, &file)?;
            if lines.is_empty() && explicit {
                bail!(
                    "chat '{}' not found for {}. Run `tarpi {} -l`.",
                    file,
                    character.name,
                    character.name
                );
            }
            let mut st = ChatState::from_lines(&avatar, &character.name, &file, lines);
            // Older chats get the card's alternate greetings too.
            let synced = app::sync_greetings(&mut st.messages, &character, &user_name, &persona_text);
            (st, synced)
        };

        let char_name = character.name.clone();
        connect::report(match (created_new, arrival) {
            (true, _) => format!("started a new chat {} with {}", file, char_name),
            (false, Arrival::Start) => format!("opened chat {} with {}", file, char_name),
            (false, Arrival::Chat) => format!("switched to chat {} with {}", file, char_name),
            (false, Arrival::Character) => {
                format!("switched to {}: opened chat {}", char_name, file)
            }
        });
        let (gen_tx, mut gen_rx) = unbounded_channel();
        let mut app = App::new(
            settings,
            character,
            chat_state,
            gen_tx,
            chars_per_token,
        );
        if created_new || synced {
            app.save().await;
        }

        // The standalone API connection (`/api`), if one is set up.
        app.set_connection(conn::Store::load()?.active_connection());
        app.hint_if_no_connection();
        app.touch = touch;
        app.vim = cfg.vim().enabled;
        app.input.configure(cfg.vim().enabled, cfg.vim().keys());
        app.input.set_autopair(cfg.autopair.unwrap_or(true));
        app.command_menu = cfg.command_menu.unwrap_or(true);
        app.set_author_note(cfg.author_note.unwrap_or(true));
        // Spell checking is off unless `spellcheck: true` is in the config.
        if cfg.spellcheck.unwrap_or(false) {
            app.spell = Some(spell::Checker::new());
        }
        if let Some(l) = cfg.vim().leader_char() {
            app.input.set_leader(l);
        }
        app.tap_move_cursor = cfg.mouse().tap_to_move_cursor;
        app.tap_open_keyboard = cfg.mouse().open_keyboard_on_tap;
        app.max_rows = crossterm::terminal::size().map(|(_, h)| h).unwrap_or(0);
        app.mouse = mouse_on;
        let mut term = setup_terminal(mouse_on)?;
        let result = run_tui(&mut term, &mut app, &mut gen_rx).await;
        restore_terminal();
        let _ = term.show_cursor();
        result?;
        // Said after the screen is gone (a character was deleted: tarpi quit).
        if let Some(note) = app.exit_note.take() {
            println!("{}", note);
        }
        mouse_on = app.mouse || touch;
        if connect::server_lost() {
            break;
        }

        match app.exit_to.take() {
            None => {
                connect::report(format!("left chat {} with {}", file, char_name));
                break;
            }
            Some(app::Exit::NewChat) => want = Want::New,
            Some(app::Exit::Chat(file)) => {
                arrival = Arrival::Chat;
                want = Want::File(file);
            }
            Some(app::Exit::Character(avatar)) => {
                arrival = Arrival::Character;
                // Read the list again: cards may have been added since the start.
                let fresh = charstore::list();
                entry = fresh
                    .iter()
                    .find(|c| jstr(c, "avatar") == avatar)
                    .cloned()
                    .ok_or_else(|| anyhow!("character '{}' not found", avatar))?;
                want = Want::Latest;
            }
        }
    }
    // `tarpi connect`: send what changed during the last seconds.
    if let Some(session) = sync_session {
        session.finish().await;
    }
    Ok(())
}
