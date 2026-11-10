//! Application state and input handling.

use crate::cardedit::CardModal;
use crate::charimg::ImageModal;
use crate::character::Character;
use crate::chat::{self, ChatState};
use crate::cmds;
use crate::config;
use crate::conn;
use crate::formatting;
use crate::gen::{self, GenEvent, GenMsg};
use crate::health::Health;
use crate::history::History;
use crate::input::{Input, Mode};
use crate::picker::{Action as PickAction, Item as PickItem, Picker};
use crate::posedit::{Action as PosAction, PosEdit};
use crate::jv::*;
use crate::chatstore;
use crate::charstore;
use crate::note::{self, Note};
use crate::persona;
use crate::prompt;
use crate::prose;
use crate::summary;
use crate::sysprompt;
use crate::textgen;
use crate::tokens;
use crate::ui;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::text::Line;
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};
use tokio::{sync::mpsc::UnboundedSender, task::JoinHandle};

/// Shown when a reply is asked for without an API connection.
const NO_CONNECTION: &str =
    "no API connection: type /api, add one and press `a` to use it";

/// A reply that is currently streaming in.
pub struct Pending {
    pub text: String,
    /// True when this generation takes the place of the last assistant message
    /// (as a new swipe, or, with `reset`, instead of all its swipes).
    pub replace_last: bool,
    /// Set by /regen and Ctrl+R: the new reply replaces the message and every
    /// swipe it had. Only an actual swipe (arrow key, tap, drag) adds one.
    pub reset: bool,
    pub started: String,
    /// Set when continuing the last message: the text it had when we started.
    /// The streamed text is appended to it.
    pub continue_from: Option<String>,
    /// A pass of `/summarize`: the text is a summary, not a reply, and is
    /// handled by `finish_summary_pass`.
    pub summary: bool,
}

/// Which greeting of the character card a chat's first message shows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum GreetingSlot {
    /// The card's `first_mes`.
    First,
    /// `alternate_greetings[n]`, counting only the non-empty ones (the chat
    /// is built from that list too, see `make_greeting`).
    Alt(usize),
}

/// A message being edited in the input box.
struct Editing {
    /// Index into `chat.messages`.
    idx: usize,
    /// What was in the input box before the edit started (text, cursor).
    draft: (String, usize),
    /// Which card greeting this message is (decided when the edit starts,
    /// from the text it had then), or `None` when it is not one.
    slot: Option<GreetingSlot>,
}

/// Where to go after the chat screen closes (`None`: leave the program).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Exit {
    /// Open this chat (file name) of the same character.
    Chat(String),
    /// Start a new chat with the same character.
    NewChat,
    /// Chat with another character (its card file name).
    Character(String),
}

/// What the choose-one window (`App::picker`) is choosing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickKind {
    Characters,
    Chats,
    Personas,
    SysPrompts,
}

/// Key of the "new chat" row of the `/chats` window.
const NEW_CHAT: &str = "\u{0}new";
/// Key of the "new persona" row of the `/persona` window.
const NEW_PERSONA: &str = "\u{0}new";

/// Key of the "new system prompt" row of the `/system` window.
const NEW_SYSPROMPT: &str = "\u{0}new";

/// The persona form (`App::card_modal`) is open for this persona.
struct PersonaEdit {
    /// `None`: a new persona is being made.
    avatar: Option<String>,
}

/// The `/prompt` window: the whole prompt of the next reply, as text.
pub struct PromptView {
    /// The prompt, tidied for the screen (no tabs or control characters).
    pub text: String,
    /// Window title: size of the prompt.
    pub title: String,
    /// First visible row (`ui::draw_prompt_modal` keeps it in range).
    pub scroll: usize,
    /// Text rows the window shows, for PgUp / PgDn (set when drawing).
    pub page: usize,
    /// `text` cut into screen rows for the width in `wrapped_w`.
    pub wrapped: Vec<String>,
    pub wrapped_w: usize,
}

/// The system prompt form (`App::card_modal`) is open for this prompt.
struct SysEdit {
    /// `None`: a new system prompt is being made.
    id: Option<String>,
}

enum Outcome {
    Done,
    Stopped,
    Failed(String),
}

/// Totals of the chat for the status row.
#[derive(Default, Clone, Copy)]
pub struct ChatStats {
    pub messages: usize,
    pub tokens: usize,
    /// Counted by the server; `false` while (some of) it is an estimate.
    pub exact: bool,
}

/// The `/command` completion menu (see `App::completion`).
#[derive(Default)]
struct Comp {
    /// What was typed before Tab / the arrows started filling commands in
    /// (without the slash). `None`: nothing has been picked from the menu.
    base: Option<String>,
    /// Which entry of the menu for `base` is filled in.
    sel: usize,
    /// A command was just chosen with the mouse: the menu stays shut until
    /// the next key.
    done: bool,
}

/// The spelling menu (opened with Tab on a misspelled word).
pub struct SpellMenu {
    /// The misspelled word the cursor is in (char range in the box).
    pub range: (usize, usize),
    pub word: String,
    pub sugg: Vec<String>,
    pub sel: usize,
}

/// How long the cursor rests in a misspelled word before it is underlined.
const SPELL_DELAY: Duration = Duration::from_millis(1000);

pub struct App {
    /// The user's full settings.json.
    settings: Value,
    pub character: Character,
    user_name: String,
    /// Title of the active persona (may be empty).
    persona_title: String,
    persona: String,
    persona_position: i64,
    pub chat: ChatState,
    pub input: Input,
    /// Lines scrolled up from the bottom (0 = following the newest message).
    pub scroll: usize,
    /// Number of chat lines at the last draw, to keep the view in place
    /// while text is added below it.
    pub last_total: usize,
    pub viewport_h: usize,
    status: Option<(String, bool, Instant)>,
    pending: Option<Pending>,
    gen_id: u64,
    /// `/regen <nudge>`: direction for the next generation only (taken by
    /// `start_generation_mode`).
    next_nudge: Option<String>,
    gen_handle: Option<JoinHandle<()>>,
    /// A running `/summarize` (its passes are generated one after the other).
    summary_job: Option<summary::Job>,
    gen_tx: UnboundedSender<GenMsg>,
    pub should_quit: bool,
    /// Whether the terminal reports mouse events to us (off briefly on a
    /// phone while the keyboard opens, see `touch`).
    pub mouse: bool,
    /// Running on a phone: tapping the input box briefly turns mouse
    /// reporting off so that the terminal opens the keyboard.
    pub touch: bool,
    /// Tallest the terminal has been (rows). The keyboard shrinks it.
    pub max_rows: u16,
    /// The keyboard looks open (the terminal got a lot shorter).
    kbd_up: bool,
    /// When to turn mouse reporting back on if no resize says the keyboard opened.
    mouse_back_at: Option<Instant>,
    /// Set together with `should_quit` when a choice in the `/chats` or
    /// `/characters` window sends us to another chat.
    pub exit_to: Option<Exit>,
    /// The `/characters` or `/chats` window, if open.
    pub picker: Option<(PickKind, Picker)>,
    /// The position window of the `/persona` window's `p` key, if open.
    pub pos_edit: Option<PosEdit>,
    /// Set while `card_modal` is the persona form, not the character card.
    persona_edit: Option<PersonaEdit>,
    /// Set while `card_modal` is the system prompt form.
    sys_edit: Option<SysEdit>,
    /// Set while `card_modal` is the Author's Note form (`/note`).
    note_edit: bool,
    /// `/one-shot` was typed once and waits for the second time.
    one_shot_armed: bool,
    /// Set by `/api`: the main loop opens the API manager in this terminal and
    /// then continues with this same screen.
    pub open_api: bool,
    /// Set by `/formatting`: the main loop then opens the formatting editor.
    pub open_formatting: bool,
    pub show_help: bool,
    /// The `/prompt` window (the whole prompt of the next reply), if open.
    pub prompt_view: Option<PromptView>,
    /// The `/character` window (the whole card as an editable form), if open.
    pub card_modal: Option<CardModal>,
    /// The `/char-img` window (the card picture), if open.
    pub image_modal: Option<ImageModal>,
    /// First visible line of the help window (`ui::draw` keeps it in range).
    pub help_scroll: usize,
    /// Text rows the help window shows, for PgUp / PgDn (set by `ui::draw`).
    pub help_page: usize,
    editing: Option<Editing>,
    /// Background token counter for the status row.
    tokens: tokens::Counter,
    /// The whole prompt as last built for counting, and what it was built from.
    prompt_text: String,
    prompt_fp: Option<u64>,
    /// Bumped whenever the settings are re-read (they shape the prompt).
    settings_ver: u64,
    /// Message and token totals shown right of the status line (see `update_stats`).
    pub stats: ChatStats,
    stats_at: Option<Instant>,
    /// `(message index, closer)`: markers the TUI itself added to the end of
    /// a finished reply, so that Ctrl+K can take them off again.
    auto_closed: Option<(usize, String)>,
    /// Message to scroll into view on the next draw.
    pub reveal: Option<usize>,
    /// `(message index, first line, end line)` of every drawn message.
    pub line_map: Vec<(usize, usize, usize)>,
    /// Layout facts recorded by `ui::draw`, used to hit-test mouse clicks.
    pub msg_area: Rect,
    pub send_btn: Rect,
    /// The extra "Card save" button, shown while editing a card greeting.
    pub card_btn: Rect,
    /// The input box (with its border) and the first text row it shows.
    pub input_area: Rect,
    pub input_first_row: usize,
    comp: Comp,
    /// The `/commands` run so far (Up on an empty `/` walks through them).
    cmd_hist: History,
    /// Which history entry the box is showing (`None`: not browsing).
    hist_pos: Option<usize>,
    /// What the box held when browsing started (`/`), put back by Down
    /// past the newest entry.
    hist_draft: String,
    /// Config `command_menu`: the `/command` menu above the input box.
    pub command_menu: bool,
    /// Config `author_note`: the Author's Note is on (default) or off.
    author_note: bool,
    /// Config `spellcheck`: the spell checker for the input box (`None` = off).
    pub spell: Option<crate::spell::Checker>,
    pub spell_menu: Option<SpellMenu>,
    /// Where the spelling menu was drawn and the screen row of each entry.
    pub spell_area: Rect,
    pub spell_rows: Vec<(u16, usize)>,
    /// Hash of (text, cursor, mode) and since when it has been the same.
    spell_key: u64,
    spell_since: Instant,

    /// Where the completion menu was drawn (with its border) and the screen
    /// row of each command in it, to hit-test clicks. Empty when it is shut.
    pub comp_area: Rect,
    pub comp_rows: Vec<(u16, &'static str)>,
    /// Manual scroll position of the input box (first text row shown), set
    /// by the mouse wheel over the box. `None`: the box follows the cursor.
    /// Cleared by any key press or paste.
    pub input_scroll: Option<usize>,
    /// Where the left button went down in the message area `(column, row)`.
    /// A click or swipe is decided when it comes up.
    press: Option<(u16, u16)>,
    /// Config `vim.enabled`: vim modes in the input box (false = plain editing).
    pub vim: bool,
    /// Config `mouse.tap_to_move_cursor`: clicking in the input box moves the cursor.
    pub tap_move_cursor: bool,
    /// Config `mouse.open_keyboard_on_tap` (phones): tapping the input box opens the keyboard.
    pub tap_open_keyboard: bool,
    wheel_burst: Option<WheelBurst>,
    /// Total text rows and visible text rows of the input box at the last draw.
    pub input_rows: (usize, usize),
    pub view_start: usize,
    pub input_w: usize,
    chars_per_token: f32,
    model: String,
    source: String,
    /// The API type the title bar shows, e.g. "llamacpp".
    label: String,
    /// `host:port` of the API connection.
    pub server: String,
    /// The standalone API connection (`/api`) replies are generated
    /// with. `None`: nothing is generated until one is set up.
    conn: Option<conn::Connection>,
    /// Whether that connection answers (the title bar shows it red when not).
    health: Health,
}

/// First message for a fresh chat: `first_mes` plus alternate greetings as swipes.
pub fn make_greeting(ch: &Character, user_name: &str, persona: &str) -> Option<Value> {
    let mut all: Vec<String> = Vec::new();
    if !ch.first_mes.trim().is_empty() {
        all.push(ch.first_mes.clone());
    }
    all.extend(ch.alternate_greetings.iter().cloned());
    let all: Vec<String> = all
        .iter()
        .map(|g| chat::tidy(&prompt::subst_card(g, user_name, persona, ch)))
        .collect();
    if all.is_empty() {
        None
    } else {
        Some(chat::greeting_message(&ch.name, &all))
    }
}

/// A run of mouse-wheel events from one finger drag (see `App::wheel_gesture`).
struct WheelBurst {
    last: Instant,
    x0: u16,
    y0: u16,
    /// Chat scroll position before the drag started.
    scroll0: usize,
    fired: bool,
}

/// What to do with one wheel event.
enum WheelAction {
    Scroll,
    Ignore,
    Swipe(i32),
}

/// Chats started before the card had alternate greetings (or by another
/// client) carry only one greeting. Add the card's other greetings as swipes of
/// the first message, so Left/Right can browse them. Only a first message
/// that really is one of the card's greetings is touched. Returns true when
/// something was added.
pub fn sync_greetings(
    messages: &mut [Value],
    ch: &Character,
    user_name: &str,
    persona: &str,
) -> bool {
    let mut cards: Vec<String> = Vec::new();
    if !ch.first_mes.trim().is_empty() {
        cards.push(ch.first_mes.clone());
    }
    cards.extend(ch.alternate_greetings.iter().cloned());
    let cards: Vec<String> = cards
        .iter()
        .map(|g| chat::tidy(&prompt::subst_card(g, user_name, persona, ch)))
        .collect();
    if cards.len() < 2 {
        return false;
    }
    let Some(first) = messages.first_mut() else {
        return false;
    };
    if chat::is_user(first) || chat::is_system(first) {
        return false;
    }
    // A branched chat (original conversation + summary) keeps its first messages as they are.
    if chat::branch_pos(first).is_some() {
        return false;
    }
    let have: Vec<String> = match first.get("swipes").and_then(|s| s.as_array()) {
        Some(a) if !a.is_empty() => a
            .iter()
            .filter_map(|t| t.as_str())
            .map(chat::tidy)
            .collect(),
        _ => vec![chat::tidy(chat::mes(first))],
    };
    if !have.iter().any(|t| cards.contains(t)) {
        return false;
    }
    let mut missing: Vec<String> = Vec::new();
    for c in cards {
        if !have.contains(&c) && !missing.contains(&c) {
            missing.push(c);
        }
    }
    if missing.is_empty() {
        return false;
    }
    chat::add_greeting_swipes(first, &missing);
    true
}

/// `host:port` of a URL, without the scheme or any path.
pub fn host_of(url: &str) -> String {
    let u = url.trim();
    let u = u.split_once("://").map(|(_, rest)| rest).unwrap_or(u);
    u.split('/').next().unwrap_or("").to_string()
}

/// `(source, model, api type for the title bar)` for the active API.
fn backend_info(settings: &Value) -> (String, String, String) {
    match jstr(settings, "main_api").as_str() {
        "openai" => {
            let oai = settings.get("oai_settings").cloned().unwrap_or(Value::Null);
            let source = {
                let s = jstr(&oai, "chat_completion_source");
                if s.is_empty() {
                    "openai".to_string()
                } else {
                    s
                }
            };
            let model = gen::model_for(&oai, &source);
            let label = source.clone();
            (source, model, label)
        }
        "textgenerationwebui" => {
            let ty = textgen::api_type(settings);
            let label = ty.clone();
            (ty, String::new(), label)
        }
        other => (
            other.to_string(),
            String::new(),
            format!("{} (unsupported)", other),
        ),
    }
}

impl App {
    pub fn new(
        mut settings: Value,
        character: Character,
        chat: ChatState,
        gen_tx: UnboundedSender<GenMsg>,
        chars_per_token: f32,
    ) -> App {
        persona::normalize(&mut settings);
        sysprompt::normalize(&mut settings);
        formatting::normalize(&mut settings);
        let (user_name, persona_title) = {
            let p = persona::current();
            (p.name, p.title)
        };
        let (persona, persona_position) = match settings.get("power_user") {
            Some(pu) => (
                jstr(pu, "persona_description"),
                ji64(pu, "persona_description_position", 0),
            ),
            None => (String::new(), 0),
        };
        let (source, model, label) = backend_info(&settings);
        let mut app = App {
            settings,
            character,
            user_name,
            persona_title,
            persona,
            persona_position,
            chat,
            input: Input::default(),
            scroll: 0,
            last_total: 0,
            viewport_h: 20,
            status: None,
            pending: None,
            gen_id: 0,
            next_nudge: None,
            gen_handle: None,
            summary_job: None,
            gen_tx,
            should_quit: false,
            mouse: true,
            touch: false,
            max_rows: 0,
            kbd_up: false,
            mouse_back_at: None,
            exit_to: None,
            open_api: false,
            open_formatting: false,
            show_help: false,
            prompt_view: None,
            card_modal: None,
            image_modal: None,
            picker: None,
            pos_edit: None,
            persona_edit: None,
            sys_edit: None,
            note_edit: false,
            one_shot_armed: false,
            help_scroll: 0,
            help_page: 10,
            editing: None,
            tokens: tokens::Counter::default(),
            prompt_text: String::new(),
            prompt_fp: None,
            settings_ver: 0,
            conn: None,
            stats: ChatStats::default(),
            stats_at: None,
            auto_closed: None,
            reveal: None,
            line_map: Vec::new(),
            msg_area: Rect::default(),
            send_btn: Rect::default(),
            card_btn: Rect::default(),
            input_area: Rect::default(),
            comp: Comp::default(),
            cmd_hist: History::load(),
            hist_pos: None,
            hist_draft: String::new(),
            command_menu: true,
            author_note: true,
            spell: None,
            spell_menu: None,
            spell_area: Rect::default(),
            spell_rows: Vec::new(),
            spell_key: 0,
            spell_since: Instant::now(),
            comp_area: Rect::default(),
            comp_rows: Vec::new(),
            input_first_row: 0,
            input_scroll: None,
            press: None,
            vim: true,
            tap_move_cursor: true,
            tap_open_keyboard: true,
            wheel_burst: None,
            input_rows: (0, 0),
            view_start: 0,
            input_w: 60,
            chars_per_token,
            model,
            source,
            label,
            server: String::new(),
            health: Health::new(),
        };
        app.update_stats();
        app
    }

    pub fn is_generating(&self) -> bool {
        self.pending.is_some()
    }

    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// No connection is set, or the backend did not answer the last check.
    pub fn api_down(&self) -> bool {
        self.conn.is_none() || self.health.down()
    }

    /// `api@host:port`: the type and address of the API connection.
    pub fn model_label(&self) -> String {
        if self.server.is_empty() {
            self.label.clone()
        } else {
            format!("{}@{}", self.label, self.server)
        }
    }

    /// `Name (Title)` of the active persona, for the title bar.
    pub fn persona_label(&self) -> String {
        if self.persona_title.trim().is_empty() {
            self.user_name.clone()
        } else {
            format!("{} ({})", self.user_name, self.persona_title.trim())
        }
    }

    fn set_status(&mut self, msg: impl Into<String>, is_error: bool) {
        self.status = Some((msg.into(), is_error, Instant::now()));
    }

    /// Text and "is error" flag for the bottom status line.
    /// Where the backend can tokenize: only through the standalone connection.
    fn token_backend(&self) -> Option<tokens::Backend> {
        self.conn.as_ref()?;
        if jstr(&self.settings, "main_api") != "textgenerationwebui" {
            return None;
        }
        let api_type = textgen::api_type(&self.settings);
        let server = textgen::server_url(&self.settings);
        if api_type.is_empty() || server.is_empty() {
            return None;
        }
        let model = textgen::current_model(&self.settings);
        Some(tokens::Backend {
            api_type,
            server,
            model,
            conn: self.conn.clone(),
        })
    }

    /// Hash of everything the prompt is built from (chat and settings).
    fn prompt_fingerprint(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.settings_ver.hash(&mut h);
        for m in &self.chat.messages {
            chat::is_user(m).hash(&mut h);
            chat::mes(m).hash(&mut h);
        }
        h.finish()
    }

    /// The complete prompt for the current chat as text: system prompt,
    /// character card, persona, examples, formatting and all messages. Not
    /// trimmed to the context window, so the count is the full total.
    fn full_prompt_text(&self) -> String {
        // A chars-per-token this large keeps the history trimming from cutting anything.
        const NO_TRIM: f32 = 1.0e6;
        let history = &self.chat.messages[..];
        let note = Note::from_chat(&self.chat);
        match jstr(&self.settings, "main_api").as_str() {
            "textgenerationwebui" => {
                textgen::build_prompt(&textgen::TgArgs {
                    settings: &self.settings,
                    ch: &self.character,
                    user_name: &self.user_name,
                    persona: &self.persona,
                    persona_position: self.persona_position,
                    note: &note,
                    history,
                    chars_per_token: NO_TRIM,
                    continue_text: None,
                    nudge: None,
                })
                .prompt
            }
            "openai" => {
                let null = Value::Null;
                let oai = self.settings.get("oai_settings").unwrap_or(&null);
                let pu = self.settings.get("power_user").unwrap_or(&null);
                prompt::build_chat_completion(&prompt::BuildArgs {
                    oai,
                    power_user: pu,
                    ch: &self.character,
                    user_name: &self.user_name,
                    persona: &self.persona,
                    persona_position: self.persona_position,
                    history,
                    chars_per_token: NO_TRIM,
                    continue_last: false,
                })
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n")
            }
            _ => history
                .iter()
                .map(|m| chat::mes(m))
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    /// Recount the totals. The prompt is rebuilt only when the chat or the
    /// settings changed; its token count is exact once the backend has
    /// counted it, and the characters / `chars_per_token` estimate until then.
    fn update_stats(&mut self) {
        let fp = self.prompt_fingerprint();
        if self.prompt_fp != Some(fp) {
            self.prompt_text = self.full_prompt_text();
            self.prompt_fp = Some(fp);
        } else if self.stats.exact && self.stats_at.is_some() {
            self.stats_at = Some(Instant::now());
            return;
        }
        let backend = self.token_backend();
        let t = self.tokens.total(
            backend.as_ref(),
            &self.prompt_text,
            self.chars_per_token,
        );
        self.stats = ChatStats {
            messages: self.chat.messages.len(),
            tokens: t.tokens,
            exact: t.exact,
        };
        self.stats_at = Some(Instant::now());
        // Say why the exact count is not available (once per failure).
        if let Some(e) = self.tokens.take_error() {
            self.set_status(
                format!("token count failed, using the estimate: {}", e),
                true,
            );
        }
    }

    pub fn status_line(&self) -> (String, bool) {
        if let Some(j) = &self.summary_job {
            let n = j.plan.parts.len();
            let text = if n > 1 {
                format!("summarizing\u{2026} part {}/{} (Esc stops)", j.next + 1, n)
            } else {
                "summarizing\u{2026} (Esc stops)".to_string()
            };
            return (text, false);
        }
        if let Some((text, is_err, when)) = &self.status {
            if when.elapsed() < Duration::from_secs(10) {
                return (text.clone(), *is_err);
            }
        }
        // A fresh chat with alternate greetings: say how to browse them.
        let greetings = match self.chat.messages.as_slice() {
            [m] if !chat::is_user(m) => chat::swipe_pos(m).filter(|&(_, t)| t > 1),
            _ => None,
        };
        if let Some((cur, total)) = greetings {
            let how = if self.touch {
                format!("tap \u{2039}{}/{}\u{203A} to switch greeting", cur + 1, total)
            } else {
                format!("Ctrl+\u{2190}/\u{2192} switch greeting ({}/{})", cur + 1, total)
            };
            return (how, false);
        }
        // A summarized chat: say which branch this is and how to switch.
        if let Some((cur, total)) = self.chat.messages.first().and_then(chat::branch_pos) {
            let how = if self.touch {
                format!("tap summary branch {}/{} to switch", cur + 1, total)
            } else {
                format!("Ctrl+\u{2190}/\u{2192} switch summary branch ({}/{})", cur + 1, total)
            };
            return (how, false);
        }
        // Nothing useful to add when idle: the box title already lists the
        // basic keys, and F1 has the rest.
        (String::new(), false)
    }

    pub fn on_tick(&mut self) {
        self.health.tick(self.conn.as_ref());
        if self.stats_at.map_or(true, |t| t.elapsed() >= Duration::from_millis(400)) {
            self.update_stats();
        }
        // No resize came after the tap (the keyboard was already open): give
        // the messages their taps back anyway.
        if let Some(t) = self.mouse_back_at {
            if Instant::now() >= t {
                self.mouse_back_at = None;
                self.set_mouse(true);
            }
        }
        if let Some((_, _, when)) = &self.status {
            if when.elapsed() >= Duration::from_secs(10) {
                self.status = None;
            }
        }
    }

    // ------------------------------------------------------------ rendering

    pub fn build_lines(&mut self, width: usize) -> Vec<Line<'static>> {
        let mut out: Vec<Line<'static>> = Vec::new();
        let mut map: Vec<(usize, usize, usize)> = Vec::new();
        let hide_last = self
            .pending
            .as_ref()
            .map(|p| p.replace_last)
            .unwrap_or(false);
        // While continuing, the last message is shown with the new text appended.
        let cont: Option<(String, String)> = self
            .pending
            .as_ref()
            .and_then(|p| p.continue_from.clone().map(|o| (o, p.text.clone())));
        let editing_idx = self.editing.as_ref().map(|e| e.idx);
        let count = self.chat.messages.len();
        for (idx, m) in self.chat.messages.iter().enumerate() {
            if hide_last && idx + 1 == count {
                break;
            }
            if chat::is_system(m) {
                continue;
            }
            let is_user = chat::is_user(m);
            let swipe = if is_user { None } else { chat::swipe_pos(m) };
            let continuing = idx + 1 == count && cont.is_some();
            // {{user}} / {{char}} / <USER> are shown as the names (the stored
            // text is left alone, so editing still shows what was written).
            let text = match &cont {
                Some((orig, chunk)) if continuing => prompt::subst_display(
                    &prose::close_for_display(&chat::join_continuation(orig, chunk)),
                    &self.user_name,
                    &self.character.name,
                ),
                _ => prompt::subst_display(chat::mes(m), &self.user_name, &self.character.name),
            };
            let start = out.len();
            let branch = if idx == 0 { chat::branch_pos(m) } else { None };
            out.extend(ui::message_lines(
                &chat::name(m),
                is_user,
                &text,
                width,
                swipe,
                branch,
                continuing,
                editing_idx == Some(idx),
            ));
            map.push((idx, start, out.len()));
        }
        let mut last_in_map = true;
        // A summary is not shown while it is written: it appears whole, as the
        // summary branch, when it is finished (the status row says it is working).
        if let Some(p) = self.pending.as_ref().filter(|p| !p.summary) {
            if p.continue_from.is_none() {
                last_in_map = false;
                let shown = prose::close_for_display(&p.text);
                let text = if p.text.is_empty() {
                    "\u{2026}"
                } else {
                    shown.as_str()
                };
                out.extend(ui::message_lines(
                    self.character.name.as_str(),
                    false,
                    text,
                    width,
                    None,
                    None,
                    true,
                    false,
                ));
            }
        }
        // No separator line under the very last message.
        if out.pop().is_some() && last_in_map {
            if let Some(l) = map.last_mut() {
                l.2 = l.2.saturating_sub(1);
            }
        }
        self.line_map = map;
        out
    }

    // --------------------------------------------------------------- events

    pub async fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k).await,
            Event::Paste(s) => {
                if let Some(c) = self.card_modal.as_mut() {
                    c.paste(&s);
                    c.follow = true;
                    return;
                }
                if self.picker.is_some() || self.pos_edit.is_some() || self.prompt_view.is_some() {
                    return;
                }
                self.input_scroll = None;
                if self.input.mode == Mode::Command {
                    self.input.command_paste(&s);
                } else {
                    self.input.paste_text(&s);
                }
                self.scroll = 0;
            }
            Event::Mouse(m) => self.on_mouse(m).await,
            _ => {}
        }
    }

    async fn on_mouse(&mut self, m: MouseEvent) {
        if self.image_modal.is_some() {
            if matches!(m.kind, MouseEventKind::Down(_)) {
                self.image_modal = None;
            }
            return;
        }
        if self.card_modal.is_some() {
            self.on_card_mouse(m).await;
            return;
        }
        if let Some(e) = self.pos_edit.as_mut() {
            let act = e.on_mouse(m);
            self.pos_action(act).await;
            return;
        }
        if let Some((_, p)) = self.picker.as_mut() {
            let act = p.on_mouse(m);
            self.picker_action(act).await;
            return;
        }
        if self.prompt_view.is_some() {
            // The wheel scrolls the prompt; a click closes it.
            match m.kind {
                MouseEventKind::ScrollUp => {
                    if let Some(v) = self.prompt_view.as_mut() {
                        v.scroll = v.scroll.saturating_sub(3);
                    }
                }
                MouseEventKind::ScrollDown => {
                    if let Some(v) = self.prompt_view.as_mut() {
                        v.scroll = v.scroll.saturating_add(3);
                    }
                }
                MouseEventKind::Down(MouseButton::Left) => self.prompt_view = None,
                _ => {}
            }
            return;
        }
        match m.kind {
            // The wheel scrolls whatever is under the pointer: the text box
            // when it is over the box, the chat everywhere else.
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if self.show_help {
                    self.help_scroll = if up {
                        self.help_scroll.saturating_sub(3)
                    } else {
                        self.help_scroll.saturating_add(3)
                    };
                    return;
                }
                if self.input_area.contains(Position::new(m.column, m.row)) {
                    let (total, visible) = self.input_rows;
                    let max = total.saturating_sub(visible);
                    let base = self.input_scroll.unwrap_or(self.input_first_row);
                    let next = if up {
                        base.saturating_sub(3)
                    } else {
                        (base + 3).min(max)
                    };
                    self.input_scroll = Some(next.min(max));
                } else {
                    match self.wheel_gesture(m.column, m.row) {
                        WheelAction::Swipe(dir) => self.swipe(dir).await,
                        WheelAction::Ignore => {}
                        WheelAction::Scroll if up => {
                            self.scroll = self.scroll.saturating_add(3)
                        }
                        WheelAction::Scroll => self.scroll = self.scroll.saturating_sub(3),
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some((x0, y0)) = self.press.take() else {
                    return;
                };
                let dx = m.column as i32 - x0 as i32;
                let dy = m.row as i32 - y0 as i32;
                if dx.abs() >= 6 && dx.abs() > dy.abs() * 2 {
                    // Finger / pointer moved left: next swipe; right: previous.
                    self.swipe(if dx < 0 { 1 } else { -1 }).await;
                } else if dx.abs() <= 1 && dy.abs() <= 1 {
                    self.tap_message(x0, y0).await;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.press = None;
                if self.show_help {
                    self.show_help = false;
                    self.help_scroll = 0;
                    return;
                }
                let pos = Position::new(m.column, m.row);
                if self.spell_menu.is_some() && self.spell_area.contains(pos) {
                    // A click on a suggestion puts it in.
                    if let Some(&(_, i)) = self.spell_rows.iter().find(|(y, _)| *y == m.row) {
                        self.spell_accept(i);
                    }
                    return;
                }
                if self.comp_area.contains(pos) {
                    // A click on a command of the menu fills it in.
                    if let Some(&(_, name)) = self.comp_rows.iter().find(|(y, _)| *y == m.row) {
                        self.accept_completion(name);
                    }
                    return;
                }
                if self.card_btn.contains(pos) {
                    self.commit_edit_to_card().await;
                } else if self.send_btn.contains(pos) {
                    // Send / Save, or Stop while a reply is streaming.
                    if self.is_generating() {
                        if !self.stop_blocked() {
                            self.cancel_generation().await;
                        }
                    } else {
                        self.submit().await;
                    }
                } else if self.msg_area.contains(pos) {
                    // Not a click yet: it may become a swipe. Decided on release.
                    self.press = Some((m.column, m.row));
                } else if self.input_area.width > 2 && self.input_area.height > 2 {
                    // Click inside the input box (normal or editing): move the cursor there.
                    let a = self.input_area;
                    let inner = Rect::new(a.x + 1, a.y + 1, a.width - 2, a.height - 2);
                    if inner.contains(pos) {
                        let row = self.input_first_row + (m.row - inner.y) as usize;
                        let col = (m.column - inner.x) as usize;
                        if self.tap_move_cursor {
                            self.input.cursor =
                                ui::index_at(&self.input.text, self.input_w, row, col);
                            self.input.cancel_pending();
                            self.input.clamp_normal();
                        }
                        if self.touch && self.tap_open_keyboard && self.mouse && !self.kbd_up {
                            // With mouse reporting off a tap opens the phone
                            // keyboard. It comes back on by itself when the
                            // keyboard is up (see `on_resize`).
                            self.set_mouse(false);
                            self.mouse_back_at = Some(Instant::now() + Duration::from_secs(3));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// `(current, total, next_generates)` of the last reply, or `None` when it
    /// cannot be swiped right now.
    pub fn swipe_bar_info(&self) -> Option<(usize, usize, bool)> {
        if self.is_generating() || self.is_editing() || self.chat.is_one_shot() {
            return None;
        }
        let m = self.chat.messages.last()?;
        if chat::is_user(m) || chat::is_system(m) {
            return None;
        }
        let (cur, total) = chat::swipe_pos(m).unwrap_or((0, 1));
        let len = self.chat.messages.len();
        if total > 1 || len >= 2 {
            Some((cur, total, cur + 1 >= total && len >= 2))
        } else {
            None
        }
    }

    /// Swipes on a phone, the hack. Termux drops the sideways part of a finger
    /// drag but still sends a wheel event per row of vertical movement, and
    /// each one carries the finger's *current column*. A sideways swipe with a
    /// slight slant therefore shows up as wheel events whose column jumps a
    /// long way over a row or two, while a normal scroll stays in one column.
    /// Such a burst is a swipe: moving left is the next swipe, right the
    /// previous one. The burst's first event has already scrolled the chat, so
    /// that is undone, and the rest of the burst (including the fling that
    /// follows) is ignored so that one drag flips exactly one swipe.
    fn wheel_gesture(&mut self, col: u16, row: u16) -> WheelAction {
        if !self.touch {
            return WheelAction::Scroll;
        }
        let now = Instant::now();
        let gap = Duration::from_millis(350);
        let continues = self
            .wheel_burst
            .as_ref()
            .is_some_and(|b| now.duration_since(b.last) <= gap);
        if !continues {
            self.wheel_burst = Some(WheelBurst {
                last: now,
                x0: col,
                y0: row,
                scroll0: self.scroll,
                fired: false,
            });
            return WheelAction::Scroll;
        }
        let swipeable = self.swipe_bar_info().is_some();
        let Some(b) = self.wheel_burst.as_mut() else {
            return WheelAction::Scroll;
        };
        b.last = now;
        if b.fired {
            return WheelAction::Ignore;
        }
        if !swipeable {
            return WheelAction::Scroll;
        }
        let dx = col as i32 - b.x0 as i32;
        let dy = (row as i32 - b.y0 as i32).abs();
        if dx.abs() >= 10 && dx.abs() >= 4 * dy.max(1) {
            b.fired = true;
            self.scroll = b.scroll0;
            return WheelAction::Swipe(if dx < 0 { 1 } else { -1 });
        }
        WheelAction::Scroll
    }

    /// A click or tap at (`col`, `row`) in the message area: the ‹n/m› counter
    /// on the last reply flips swipes (left half: previous, right half: next);
    /// anywhere else on a message opens it for editing.
    async fn tap_message(&mut self, col: u16, row: u16) {
        let line = self.view_start + row.saturating_sub(self.msg_area.y) as usize;
        let Some(&(idx, start, _)) = self
            .line_map
            .iter()
            .find(|(_, s, e)| line >= *s && line < *e)
        else {
            return;
        };
        if line == start && idx == 0 {
            // The `branch n/m` counter of a summarized chat flips the branch.
            let m = &self.chat.messages[0];
            if let Some((cur, total)) = chat::branch_pos(m) {
                let rel = col.saturating_sub(self.msg_area.x) as usize;
                let (from, to) = ui::branch_zone(&chat::name(m), cur, total);
                if rel >= from && rel < to {
                    let mid = (from + to) / 2;
                    self.switch_branch(if rel < mid { -1 } else { 1 }).await;
                    return;
                }
            }
        }
        if line == start && idx + 1 == self.chat.messages.len() {
            let m = &self.chat.messages[idx];
            if !chat::is_user(m) {
                if let Some((cur, total)) = chat::swipe_pos(m) {
                    if total > 1 {
                        let rel = col.saturating_sub(self.msg_area.x) as usize;
                        let branch = if idx == 0 { chat::branch_pos(m) } else { None };
                        let (from, to) = ui::swipe_zone(&chat::name(m), cur, total, branch);
                        if rel >= from && rel < to {
                            let mid = (from + to) / 2;
                            self.swipe(if rel < mid { -1 } else { 1 }).await;
                            return;
                        }
                    }
                }
            }
        }
        self.begin_edit(idx);
    }

    /// Without vim mode, Up / Down move the cursor in a multi-line draft like
    /// in any text box; with one line they scroll the chat.
    fn text_nav(&self) -> bool {
        !self.vim && self.input_rows.0 > 1
    }

    async fn on_key(&mut self, k: KeyEvent) {
        // Typing or moving the cursor brings the box back to the cursor.
        self.input_scroll = None;
        if self.image_modal.is_some() {
            // Any key closes the picture.
            self.image_modal = None;
            return;
        }
        if self.card_modal.is_some() {
            self.on_card_key(k).await;
            return;
        }
        if let Some(e) = self.pos_edit.as_mut() {
            let act = e.handle_key(k);
            self.pos_action(act).await;
            return;
        }
        if let Some((_, p)) = self.picker.as_mut() {
            let act = p.handle_key(k);
            self.picker_action(act).await;
            return;
        }
        if self.prompt_view.is_some() {
            self.prompt_key(k);
            return;
        }
        if self.show_help {
            // The help window scrolls; any other key closes it.
            let page = self.help_page.max(1);
            match k.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = self.help_scroll.saturating_add(1)
                }
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(page),
                KeyCode::PageDown | KeyCode::Char(' ') => {
                    self.help_scroll = self.help_scroll.saturating_add(page)
                }
                KeyCode::Home | KeyCode::Char('g') => self.help_scroll = 0,
                KeyCode::End | KeyCode::Char('G') => self.help_scroll = usize::MAX,
                _ => {
                    self.show_help = false;
                    self.help_scroll = 0;
                }
            }
            return;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let page = self.viewport_h.saturating_sub(2).max(1);

        // COMMAND mode (`:` command line): keys edit the command, Enter runs
        // it, Esc / Backspace on an empty line leaves.
        if self.input.mode == Mode::Command {
            match k.code {
                KeyCode::Char('c') if ctrl => self.quit().await,
                KeyCode::Char('u') if ctrl => self.input.command_clear(),
                KeyCode::Char('w') if ctrl => self.input.command_delete_word(),
                KeyCode::Char(c) if !ctrl && !alt => self.input.command_push(c),
                KeyCode::Backspace => self.input.command_backspace(),
                KeyCode::Esc => self.input.cancel_command(),
                KeyCode::Enter => {
                    match self.input.run_command() {
                        Ok(msg) if msg.is_empty() => {}
                        Ok(msg) => self.set_status(msg, false),
                        Err(msg) => self.set_status(msg, true),
                    }
                    // `:w` = Ctrl+S.
                    if self.input.take_write_req() {
                        self.commit_edit_to_card().await;
                    }
                }
                _ => {}
            }
            return;
        }

        // The spelling menu above the box (Tab opens it on a misspelled word).
        if self.spell_key(&k) {
            return;
        }
        // Up / Down on a bare `/` walk through the commands run before.
        if self.history_key(&k) {
            return;
        }
        // The `/command` menu above the box: Tab / arrows fill commands in.
        if self.completion_key(&k) {
            return;
        }
        self.comp = Comp::default();

        // Ctrl+U / Ctrl+D in NORMAL / VISUAL mode: half a box up / down.
        if ctrl
            && !alt
            && matches!(self.input.mode, Mode::Normal | Mode::Visual)
            && matches!(k.code, KeyCode::Char('u') | KeyCode::Char('d'))
        {
            let dir = if k.code == KeyCode::Char('u') { -1 } else { 1 };
            let half = (self.input_rows.1 / 2).max(1);
            self.input.move_rows(dir, half, self.input_w);
            return;
        }

        // NORMAL / VISUAL mode (vim): letters are commands. Enter sends in
        // NORMAL mode and does nothing in VISUAL mode.
        if matches!(self.input.mode, Mode::Normal | Mode::Visual) && !ctrl && !alt {
            match k.code {
                KeyCode::Char(c) => {
                    self.input.normal_key(c, self.input_w);
                    if let Some(text) = self.input.take_clipboard() {
                        let (msg, err) = copy_to_clipboard(&text);
                        self.set_status(msg, err);
                    }
                    return;
                }
                KeyCode::Enter if self.input.mode == Mode::Normal => {
                    self.submit().await;
                    return;
                }
                KeyCode::Enter => return,
                _ => {}
            }
        }

        match k.code {
            KeyCode::Char('c') if ctrl => self.quit().await,
            KeyCode::Char('d') if ctrl => {
                if self.input.is_empty() {
                    self.quit().await;
                } else {
                    self.input.delete();
                }
            }
            KeyCode::Char('p') if ctrl => self.edit_last(false),
            KeyCode::Char('o') if ctrl => self.edit_last(true),
            KeyCode::Char('s') if ctrl => self.commit_edit_to_card().await,
            KeyCode::Char('r') if ctrl => self.regenerate(None).await,
            KeyCode::Char('k') if ctrl => self.continue_message().await,
            KeyCode::Char('j') if ctrl => {
                self.input.insert_char('\n');
                self.scroll = 0;
            }
            KeyCode::Char('a') if ctrl => self.input.home(),
            KeyCode::Char('e') if ctrl => self.input.end(),
            KeyCode::Char('u') if ctrl => self.input.clear_undoable(),
            KeyCode::Char('w') if ctrl => self.input.delete_word_back(),
            // Plain Left / Right browse swipes while the box has no draft and no
            // message is being edited; otherwise they move the text cursor.
            KeyCode::Left if ctrl && !alt && self.input.is_empty() && self.editing.is_none() => {
                self.switch_branch(-1).await
            }
            KeyCode::Right if ctrl && !alt && self.input.is_empty() && self.editing.is_none() => {
                self.switch_branch(1).await
            }
            KeyCode::Left if !ctrl && !alt && self.input.is_empty() && self.editing.is_none() => {
                self.swipe(-1).await
            }
            KeyCode::Right if !ctrl && !alt && self.input.is_empty() && self.editing.is_none() => {
                self.swipe(1).await
            }
            KeyCode::Esc => {
                if self.input.mode == Mode::Visual {
                    // First Esc only leaves VISUAL mode.
                    self.input.exit_visual();
                } else {
                    self.input.cancel_pending();
                    if self.editing.is_some() {
                        self.cancel_edit();
                    } else if self.is_generating() && !self.stop_blocked() {
                        self.cancel_generation().await;
                    }
                }
            }
            KeyCode::F(1) => self.show_help = true,
            KeyCode::Enter => {
                // Enter sends (or saves an edit). Shift+Enter writes a new
                // line; Alt+Enter and Ctrl+J do too, for terminals that
                // cannot tell Shift+Enter from Enter.
                let shift = k.modifiers.contains(KeyModifiers::SHIFT);
                if (shift || alt) && !ctrl && self.input.mode == Mode::Insert {
                    self.input.insert_char('\n');
                    self.scroll = 0;
                } else {
                    self.submit().await;
                }
            }
            KeyCode::Char(c) if (!ctrl && !alt) || (ctrl && alt) => {
                // The NORMAL-mode keys (default `qq`, typed quickly) leave INSERT mode.
                self.input.type_char(c);
                self.scroll = 0;
            }
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left if alt && !ctrl => self.input.word_left(self.input_w),
            KeyCode::Right if alt && !ctrl => self.input.word_right(self.input_w),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            KeyCode::Up if self.editing.is_some() || self.text_nav() => {
                self.input.cursor =
                    ui::vertical_target(&self.input.text, self.input.cursor, self.input_w, -1);
            }
            KeyCode::Down if self.editing.is_some() || self.text_nav() => {
                self.input.cursor =
                    ui::vertical_target(&self.input.text, self.input.cursor, self.input_w, 1);
            }
            KeyCode::Up => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Down => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_add(page),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(page),
            _ => {}
        }
        self.input.clamp_normal();
    }

    // ------------------------------------------------------------- commands

    // --------------------------------------------------- command completion

    /// The completion menu, when it should show: the box is in INSERT mode
    /// and holds one `/word` and nothing else. The entries are the commands
    /// that word starts; then the highlighted entry (the top one until Tab /
    /// the arrows move it) and whether it has been filled into the box yet.
    /// The commands the menu offers for `word` (`/note` only with the Author's
    /// Note on).
    fn menu_commands(&self, word: &str) -> Vec<&'static cmds::Cmd> {
        let mut list = cmds::matches(word);
        if !self.author_note {
            list.retain(|c| c.name != "note");
        }
        list
    }

    /// Turn the Author's Note on or off (config `author_note`).
    pub fn set_author_note(&mut self, on: bool) {
        self.author_note = on;
        self.apply_settings(config::base_settings());
    }

    pub fn completion(&self) -> Option<(Vec<&'static cmds::Cmd>, usize, bool)> {
        if !self.command_menu
            || self.editing.is_some()
            || self.comp.done
            || self.input.mode != Mode::Insert
        {
            return None;
        }
        // While cycling, the list stays that of the word typed at the start,
        // even though the box now holds the full command name.
        if let Some(base) = &self.comp.base {
            let list = self.menu_commands(base);
            if let Some(c) = list.get(self.comp.sel) {
                if cmds::typed_word(&self.input.text) == Some(c.name) {
                    return Some((list, self.comp.sel, true));
                }
            }
        }
        let list = self.menu_commands(cmds::typed_word(&self.input.text)?);
        if list.is_empty() {
            None
        } else {
            Some((list, 0, false))
        }
    }

    /// Command history. With nothing but a `/` in the box, Up puts the
    /// newest command that was run before in the box; then Up goes further
    /// back and Down forward, and Down past the newest puts the `/` back.
    /// Typing or editing anything ends the walk (the keys then do what they
    /// always did). Tab / Shift+Tab stay the command menu's keys.
    /// Returns false when the key is not for the history.
    fn history_key(&mut self, k: &KeyEvent) -> bool {
        if self.input.mode != Mode::Insert
            || self.editing.is_some()
            || k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        let up = match k.code {
            KeyCode::Up => true,
            KeyCode::Down => false,
            _ => return false,
        };
        // A walk only goes on while the box still shows the entry it put there.
        if let Some(p) = self.hist_pos {
            if self.cmd_hist.get(p) != Some(&self.input.text) {
                self.hist_pos = None;
            }
        }
        // Only Up starts a walk, and only from a bare `/`.
        if self.hist_pos.is_none() && self.input.text != "/" {
            return false;
        }
        let Some(step) = crate::history::walk(self.hist_pos, up, self.cmd_hist.len()) else {
            return false;
        };
        let to = match step {
            crate::history::Walk::To(i) => i,
            crate::history::Walk::Back => {
                // Past the newest: back to what was there before.
                let draft = std::mem::take(&mut self.hist_draft);
                self.hist_pos = None;
                self.comp = Comp::default();
                self.set_input_text(draft);
                return true;
            }
        };
        if self.hist_pos.is_none() {
            self.hist_draft = self.input.text.clone();
        }
        let Some(cmd) = self.cmd_hist.get(to).cloned() else {
            return false;
        };
        self.hist_pos = Some(to);
        self.comp = Comp::default();
        self.set_input_text(cmd);
        true
    }

    /// The top command is highlighted from the start. Tab / Down move the
    /// highlight to the next one and Shift+Tab / Up to the previous one
    /// (wrapping), filling it into the box; the first press already leaves
    /// the top one. Esc puts back what was typed.
    /// Returns false when the key is not for the menu.
    fn completion_key(&mut self, k: &KeyEvent) -> bool {
        if k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            return false;
        }
        let step: i32 = match k.code {
            KeyCode::Tab | KeyCode::Down => 1,
            KeyCode::BackTab | KeyCode::Up => -1,
            KeyCode::Esc => {
                let picked = self.completion().map_or(false, |(_, _, p)| p);
                if picked {
                    if let Some(base) = self.comp.base.take() {
                        self.comp = Comp::default();
                        self.set_input_text(format!("/{}", base));
                        return true;
                    }
                }
                return false;
            }
            _ => return false,
        };
        let Some((list, sel, picked)) = self.completion() else {
            return false;
        };
        let n = list.len() as i32;
        let (base, idx) = if picked {
            (
                self.comp.base.clone().unwrap_or_default(),
                (sel as i32 + step).rem_euclid(n) as usize,
            )
        } else {
            let typed = cmds::typed_word(&self.input.text).unwrap_or("").to_string();
            // Nothing filled in yet: the top entry is only highlighted, so
            // the first Tab (or arrow) already moves on from it.
            (typed, step.rem_euclid(n) as usize)
        };
        let name = list[idx].name;
        self.set_input_text(format!("/{}", name));
        self.comp = Comp {
            base: Some(base),
            sel: idx,
            done: false,
        };
        true
    }

    /// Enter on a half-typed command runs the highlighted one: `/su` runs
    /// `/summarize`. A word that already is a command or short form (`/c`,
    /// `/nc`) is left as typed.
    fn apply_top_completion(&mut self) {
        let Some((list, sel, _)) = self.completion() else {
            return;
        };
        let word = cmds::typed_word(&self.input.text)
            .unwrap_or("")
            .to_lowercase();
        let complete = cmds::COMMANDS
            .iter()
            .any(|c| c.name == word || (!c.alias.is_empty() && c.alias == word));
        if !complete {
            self.set_input_text(format!("/{}", list[sel].name));
        }
    }

    /// A command was clicked in the menu: fill it in and close the menu.
    fn accept_completion(&mut self, name: &str) {
        self.set_input_text(format!("/{}", name));
        self.comp = Comp {
            base: None,
            sel: 0,
            done: true,
        };
    }

    /// Replace the whole box with `text`, cursor at the end.
    fn set_input_text(&mut self, text: String) {
        self.input.clear();
        self.input.insert_str(&text);
        self.input.cursor = self.input.len_chars();
        self.scroll = 0;
    }

    async fn submit(&mut self) {
        self.submit_inner().await;
        if self.input.text.is_empty() && self.editing.is_none() {
            self.input.enter_insert();
            self.input.reset_vim();
        }
    }

    async fn submit_inner(&mut self) {
        if self.editing.is_some() {
            self.commit_edit().await;
            return;
        }
        if self.is_generating() {
            self.set_status(
                if self.chat.is_one_shot() {
                    "still generating \u{2014} wait for the reply to finish"
                } else {
                    "still generating \u{2014} press Esc to stop it first"
                },
                false,
            );
            return;
        }
        self.apply_top_completion();
        let text = self.input.text.trim().to_string();
        self.hist_pos = None;

        if text.is_empty() {
            // Like the web UI: an empty send adds nothing to the chat, it just
            // asks the character to reply. (Chat Completion adds the "Replace
            // empty message" text from the prompt settings, see prompt.rs.)
            // This also retries when the last message is already the user's.
            self.start_generation(false).await;
            return;
        }

        // Anything but a second `/one-shot` withdraws the first one.
        if !matches!(text.to_lowercase().as_str(), "/one-shot" | "/oneshot") {
            self.one_shot_armed = false;
        }

        if let Some(cmd) = text.strip_prefix('/') {
            let head = cmd.split_whitespace().next().unwrap_or("").to_lowercase();
            // Commands that were understood go in the history (see `history_key`).
            let mut known = true;
            match head.as_str() {
                "help" | "h" | "?" => {
                    self.input.clear();
                    self.show_help = true;
                }
                "quit" | "q" | "exit" => {
                    self.input.clear();
                    self.quit().await;
                }
                "chats" | "chat" | "open" => {
                    self.input.clear();
                    self.open_chats_picker().await;
                }
                "character" | "char" => {
                    self.input.clear();
                    self.open_card_modal().await;
                }
                "char-img" | "charimg" => {
                    self.input.clear();
                    self.open_image_modal().await;
                }
                "characters" | "chars" | "switch" => {
                    self.input.clear();
                    self.open_characters_picker().await;
                }
                "persona" | "personas" | "me" => {
                    self.input.clear();
                    self.open_persona_picker().await;
                }
                "prompt" => {
                    self.input.clear();
                    self.show_prompt().await;
                }
                "system" | "sys" | "sysprompt" | "system-prompt" => {
                    self.input.clear();
                    self.open_sysprompt_picker().await;
                }
                "note" | "an" | "authors-note" | "author" => {
                    self.input.clear();
                    if self.author_note {
                        self.open_note().await;
                    } else {
                        self.set_status(
                            "the Author's Note is turned off (author_note: false in config.yaml)"
                                .to_string(),
                            true,
                        );
                    }
                }
                "one-shot" | "oneshot" => {
                    self.input.clear();
                    self.one_shot().await;
                }
                "api" | "connection" | "conn" => {
                    self.input.clear();
                    self.open_api = true;
                }
                "formatting" | "format" => {
                    self.input.clear();
                    self.open_formatting = true;
                }
                "regen" | "regenerate" | "reroll" => {
                    // Anything after the command is a nudge for this reply.
                    let nudge = cmd
                        .trim_start_matches(|c: char| !c.is_whitespace())
                        .trim()
                        .to_string();
                    self.input.clear();
                    self.regenerate(if nudge.is_empty() { None } else { Some(nudge) })
                        .await;
                }
                "continue" | "cont" | "c" => {
                    self.input.clear();
                    self.continue_message().await;
                }
                "undo" | "u" | "delete" | "del" => {
                    // An optional count: `/undo 3` deletes the last three messages.
                    let arg = cmd.trim_start_matches(|c: char| !c.is_whitespace()).trim();
                    let count = if arg.is_empty() {
                        Some(1)
                    } else {
                        arg.parse::<usize>().ok().filter(|n| *n >= 1)
                    };
                    match count {
                        Some(n) => {
                            self.input.clear();
                            self.undo(n).await;
                        }
                        None => {
                            known = false;
                            self.set_status("usage: /undo [number of messages], e.g. /undo 3", true);
                        }
                    }
                }
                "new" => {
                    self.input.clear();
                    self.new_chat().await;
                }
                "newchat" | "nc" => {
                    self.input.clear();
                    self.delete_chat().await;
                }
                "deletebranch" | "deletebrach" | "db" => {
                    self.input.clear();
                    self.delete_branch().await;
                }
                "summarize" | "summarise" | "summary" | "summ" => {
                    self.input.clear();
                    self.summarize().await;
                }
                _ => {
                    known = false;
                    self.set_status(format!("unknown command /{} (try /help)", head), true);
                }
            }
            if known {
                self.cmd_hist.add(&text);
            }
            return;
        }

        let message = text;
        // Macros ({{user}}, {{char}}, {{roll:1d6}}, ...) are filled in when the
        // message is sent, like the web UI. No stray blank space around it.
        let message = chat::tidy(&prompt::subst_card(
            &message,
            &self.user_name,
            &self.persona,
            &self.character,
        ));
        self.input.clear();
        self.chat
            .messages
            .push(chat::user_message(&self.user_name, &message));
        self.scroll = 0;
        self.save().await;
        self.start_generation(false).await;
    }

    // -------------------------------------------------------------- editing

    /// Ctrl+P (`user` false) / Ctrl+O (`user` true): edit the very last
    /// reply of the character / the very last message of your own.
    /// Is this chat `/one-shot`? Then say that `what` is off and return true.
    fn one_shot_blocks(&mut self, what: &str) -> bool {
        if self.chat.is_one_shot() {
            self.set_status(format!("this chat is one-shot: {} is turned off", what), false);
            true
        } else {
            false
        }
    }

    /// In an `/one-shot` chat a reply that is being written can't be stopped
    /// (not with Esc, the Stop button, or by leaving the chat).
    fn stop_blocked(&mut self) -> bool {
        if self.is_generating() && self.chat.is_one_shot() {
            self.set_status(
                "one-shot chat: a reply that is being written can't be stopped \u{2014} wait for it to finish",
                false,
            );
            true
        } else {
            false
        }
    }

    /// `/one-shot`: lock this chat file as it is (no editing, undo, regenerate,
    /// swipes or branches). Asks for the command a second time to confirm.
    async fn one_shot(&mut self) {
        if self.chat.is_one_shot() {
            self.set_status("this chat is already one-shot", false);
            return;
        }
        if self.is_generating() || self.editing.is_some() {
            self.set_status("finish what you are doing first, then /one-shot", false);
            return;
        }
        if !self.one_shot_armed {
            self.one_shot_armed = true;
            self.set_status(
                "this chat will lose editing, undo, regen, swipes and branches for good \u{2014} type /one-shot again to confirm",
                true,
            );
            return;
        }
        self.one_shot_armed = false;
        self.chat.set_one_shot();
        self.save().await;
        self.set_status("this chat is now one-shot", false);
    }

    fn edit_last(&mut self, user: bool) {
        if self.one_shot_blocks("editing") {
            return;
        }
        let found = self
            .chat
            .messages
            .iter()
            .rposition(|m| !chat::is_system(m) && chat::is_user(m) == user);
        match found {
            Some(idx) => self.begin_edit(idx),
            None => self.set_status(
                if user {
                    "no message of yours to edit yet"
                } else {
                    "no reply from the character to edit yet"
                },
                false,
            ),
        }
    }

    /// Load message `idx` into the input box. The unsent draft is stashed and
    /// comes back when the edit is saved or cancelled.
    fn begin_edit(&mut self, idx: usize) {
        if self.one_shot_blocks("editing") {
            return;
        }
        if self.is_generating() {
            self.set_status("stop the generation first (Esc) to edit a message", false);
            return;
        }
        let Some(m) = self.chat.messages.get(idx) else {
            return;
        };
        let text = chat::mes(m).to_string();
        let draft = match self.editing.take() {
            Some(e) => e.draft,
            None => (self.input.text.clone(), self.input.cursor),
        };
        self.input.clear();
        self.input.reset_vim();
        self.input.insert_str(&text);
        // Edits open in INSERT mode (qq leaves it for NORMAL mode) with the
        // cursor at the very end of the text.
        self.input.enter_insert();
        self.input.cursor = self.input.len_chars();
        let slot = self.greeting_slot_of(idx);
        self.editing = Some(Editing { idx, draft, slot });
        self.reveal = Some(idx);
    }

    /// Which card greeting message `idx` shows: only the very first chat
    /// message can be one. The text it shows is matched against the card's
    /// `first_mes` and alternate greetings (with names filled in, as they
    /// were when the chat was made), so it is right even when the swipes are
    /// not in card order. If nothing matches (the greeting was edited in this
    /// chat only), the swipe number is used, counting `first_mes` first.
    fn greeting_slot_of(&self, idx: usize) -> Option<GreetingSlot> {
        if idx != 0 {
            return None;
        }
        let m = self.chat.messages.first()?;
        if chat::is_user(m) || chat::is_system(m) {
            return None;
        }
        let ch = &self.character;
        let mut cards: Vec<(GreetingSlot, String)> = Vec::new();
        if !ch.first_mes.trim().is_empty() {
            cards.push((GreetingSlot::First, ch.first_mes.clone()));
        }
        for (n, g) in ch.alternate_greetings.iter().enumerate() {
            cards.push((GreetingSlot::Alt(n), g.clone()));
        }
        let shown = chat::tidy(chat::mes(m));
        let by_text = cards.iter().find(|(_, raw)| {
            chat::tidy(&prompt::subst_card(raw, &self.user_name, &self.persona, ch)) == shown
        });
        if let Some((slot, _)) = by_text {
            return Some(*slot);
        }
        // The summary is not a greeting: never write it into the card.
        if chat::is_summary_branch(m) {
            return None;
        }
        let (cur, _) = chat::swipe_pos(m).unwrap_or((0, 1));
        cards.get(cur).map(|(slot, _)| *slot)
    }

    /// The card greeting the message being edited stands for.
    fn editing_slot(&self) -> Option<GreetingSlot> {
        self.editing.as_ref()?.slot
    }

    /// Human-readable name of a greeting slot, for status messages.
    fn slot_name(&self, slot: GreetingSlot) -> String {
        match slot {
            GreetingSlot::First => "first message".to_string(),
            GreetingSlot::Alt(n) => format!("alternate greeting {}", n + 1),
        }
    }

    /// Whether the extra "Card save" button applies right now.
    pub fn card_edit_available(&self) -> bool {
        self.editing_slot().is_some()
    }

    /// "Card save" / Ctrl+S: write the edited greeting into the character
    /// card for good (first message or the matching alternate greeting), then
    /// save it into this chat too, like a normal edit.
    async fn commit_edit_to_card(&mut self) {
        let Some(slot) = self.editing_slot() else {
            self.set_status(
                "Ctrl+S saves to the card only while editing the card's greeting (first message)",
                false,
            );
            return;
        };
        let new_text = chat::tidy(&self.input.text);
        if new_text.trim().is_empty() && slot == GreetingSlot::First {
            self.set_status("the first message would be empty \u{2014} not saving it to the card", true);
            return;
        }
        match self.write_greeting_to_card(slot, &new_text).await {
            Ok(()) => {
                self.commit_edit().await;
                let which = self.slot_name(slot);
                self.set_status(format!("saved to the character card ({})", which), false);
            }
            // Stay in the edit so nothing typed is lost.
            Err(e) => self.set_status(format!("{:#}", e), true),
        }
    }

    async fn write_greeting_to_card(&mut self, slot: GreetingSlot, text: &str) -> anyhow::Result<()> {
        let avatar = self.character.avatar.clone();
        // Read the card fresh: it may have been edited since the chat opened.
        let full = charstore::get(&avatar)?;
        let data = full.get("data").cloned().unwrap_or(Value::Null);
        // The chat shows {{char}} / {{user}} already replaced. Put them back
        // if the card text used them, so the card keeps working with any persona.
        let unsubst = |raw: &str| -> String {
            let lower = raw.to_lowercase();
            let mut out = text.to_string();
            let cname = self.character.name.as_str();
            let uname = self.user_name.as_str();
            // Put back the placeholder style the card used.
            if !cname.is_empty() {
                if lower.contains("{{char}}") {
                    out = out.replace(cname, "{{char}}");
                } else if lower.contains("<bot>") {
                    out = out.replace(cname, "<BOT>");
                } else if lower.contains("<char>") {
                    out = out.replace(cname, "<CHAR>");
                }
            }
            if !uname.is_empty() {
                if lower.contains("{{user}}") {
                    out = out.replace(uname, "{{user}}");
                } else if lower.contains("<user>") {
                    out = out.replace(uname, "<USER>");
                }
            }
            out
        };
        match slot {
            GreetingSlot::First => {
                let raw = {
                    let d = jstr(&data, "first_mes");
                    if d.trim().is_empty() { jstr(&full, "first_mes") } else { d }
                };
                let value = unsubst(&raw);
                charstore::set_field(&avatar, "first_mes", &Value::String(value.clone()))?;
                self.character.first_mes = value;
            }
            GreetingSlot::Alt(n) => {
                let mut arr: Vec<Value> = data
                    .get("alternate_greetings")
                    .or_else(|| full.get("alternate_greetings"))
                    .and_then(|a| a.as_array())
                    .cloned()
                    .unwrap_or_default();
                // `n` counts non-empty greetings only; find it in the raw list.
                let pos = arr
                    .iter()
                    .enumerate()
                    .filter(|(_, g)| g.as_str().map(|t| !t.trim().is_empty()).unwrap_or(false))
                    .nth(n)
                    .map(|(i, _)| i)
                    .ok_or_else(|| anyhow::anyhow!("that alternate greeting is no longer on the card"))?;
                let raw = arr[pos].as_str().unwrap_or("").to_string();
                let value = unsubst(&raw);
                arr[pos] = Value::String(value.clone());
                charstore::set_field(&avatar, "alternate_greetings", &Value::Array(arr))?;
                if let Some(slot) = self.character.alternate_greetings.get_mut(n) {
                    *slot = value;
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------- /characters and /chats windows

    /// `/characters`: choose another character to chat with.
    async fn open_characters_picker(&mut self) {
        let list = charstore::list();
        let mut sorted: Vec<&Value> = list.iter().collect();
        sorted.sort_by_key(|c| crate::char_display_name(c).to_lowercase());
        let items: Vec<PickItem> = sorted
            .into_iter()
            .map(|c| {
                let avatar = jstr(c, "avatar");
                PickItem {
                    label: format!(
                        "{}{}",
                        if crate::is_fav(c) { "\u{2605} " } else { "" },
                        crate::char_display_name(c)
                    ),
                    detail: String::new(),
                    current: avatar == self.chat.avatar,
                    deletable: false,
                    editable: false,
                    key: avatar,
                }
            })
            .collect();
        self.picker = Some((PickKind::Characters, Picker::new("characters", items)));
    }

    /// `/chats`: open, create or delete a chat of this character.
    async fn open_chats_picker(&mut self) {
        let mut chats = match chatstore::list(&self.chat.avatar) {
            Ok(c) => c,
            Err(e) => {
                self.set_status(format!("{:#}", e), true);
                return;
            }
        };
        crate::sort_chats_newest_first(&mut chats);
        let narrow = ui::term_cols() < 60;
        let mut items = vec![PickItem {
            key: NEW_CHAT.to_string(),
            label: "+ new chat".to_string(),
            detail: String::new(),
            current: false,
            deletable: false,
            editable: false,
        }];
        for c in &chats {
            let id = chat::file_id(c);
            // The `_before-summary` copies are kept out of sight.
            if chat::is_summary_backup(&id) {
                continue;
            }
            items.push(PickItem {
                detail: format!(
                    "{} {} \u{b7} {}",
                    ji64(c, "chat_items", 0),
                    if narrow { "msgs" } else { "messages" },
                    crate::when(c)
                ),
                current: id == self.chat.file,
                deletable: true,
                editable: false,
                label: id.clone(),
                key: id,
            });
        }
        let title = format!("chats of {}", self.character.name);
        self.picker = Some((PickKind::Chats, Picker::new(title, items)));
    }

    /// `/persona`: choose who you are in this chat, make a new persona (the
    /// first row), edit one (`e`) or delete one (`d`).
    async fn open_persona_picker(&mut self) {
        let active = persona::current().id;
        let mut items = vec![PickItem {
            key: NEW_PERSONA.to_string(),
            label: "+ new persona".to_string(),
            detail: String::new(),
            current: false,
            deletable: false,
            editable: false,
        }];
        items.extend(persona::list().into_iter().map(|p| PickItem {
            label: p.label(),
            detail: persona::brief(&p.description, 80),
            current: p.id == active,
            deletable: true,
            editable: true,
            key: p.id,
        }));
        let mut picker = Picker::new("persona", items);
        picker.position_key = true;
        self.picker = Some((PickKind::Personas, picker));
    }

    /// The `/persona` window again, with the cursor on persona `id`.
    async fn open_persona_picker_at(&mut self, id: &str) {
        self.open_persona_picker().await;
        if let Some((_, p)) = self.picker.as_mut() {
            if let Some(i) = p.items.iter().position(|it| it.key == id) {
                p.sel = i;
            }
        }
    }

    /// `p` in the `/persona` window: the window for where the description of
    /// persona `id` goes into the prompt.
    async fn open_persona_pos(&mut self, id: String) {
        match persona::list().into_iter().find(|p| p.id == id) {
            Some(p) => {
                self.pos_edit = Some(PosEdit::new(p.id.clone(), p.label(), p.position, p.depth, p.role));
            }
            None => {
                self.set_status("that persona no longer exists", true);
                self.open_persona_picker().await;
            }
        }
    }

    /// Enter / Esc in the position window: save to the persona's file (and use
    /// it from the next reply on), or leave it as it was. Either way the
    /// `/persona` window comes back. A failed save keeps the window open.
    async fn pos_action(&mut self, act: PosAction) {
        match act {
            PosAction::None => {}
            PosAction::Cancel => {
                if let Some(e) = self.pos_edit.take() {
                    self.open_persona_picker_at(&e.id).await;
                }
            }
            PosAction::Save => {
                let Some(e) = self.pos_edit.as_ref() else {
                    return;
                };
                let (id, position, depth, role) = (e.id.clone(), e.position, e.depth, e.role);
                match persona::set_injection(&id, position, depth, role) {
                    Ok(p) => {
                        // The active persona may just have been changed.
                        let settings = self.settings.clone();
                        self.apply_settings(settings);
                        self.pos_edit = None;
                        self.open_persona_picker_at(&id).await;
                        let msg = format!(
                            "{}: {}",
                            p.name,
                            crate::posedit::position_note(p.position, p.depth, p.role)
                        );
                        if let Some((_, pk)) = self.picker.as_mut() {
                            pk.note = Some((msg, false));
                        }
                    }
                    Err(err) => {
                        if let Some(e) = self.pos_edit.as_mut() {
                            e.note = Some(format!("could not save: {:#}", err));
                        }
                    }
                }
            }
        }
    }

    /// Open the persona form for `id`, or an empty one for a new persona.
    async fn open_persona_form(&mut self, id: Option<String>) {
        let (name, title, description) = match &id {
            None => (String::new(), String::new(), String::new()),
            Some(a) => match persona::list().into_iter().find(|p| p.id == *a) {
                Some(p) => (p.name, p.title, p.description),
                None => {
                    self.set_status("that persona no longer exists", true);
                    return;
                }
            },
        };
        self.card_modal = Some(CardModal::persona(&name, &title, &description, &self.input));
        self.persona_edit = Some(PersonaEdit { avatar: id });
    }

    /// Ctrl+S in the persona form: create or update the persona, then show the
    /// persona window again. On a failure the form stays open.
    async fn save_persona_form(&mut self) {
        let (name, title, description) = match self.card_modal.as_mut() {
            Some(m) => {
                m.sync();
                (m.value("name"), m.value("title"), m.value("description"))
            }
            None => return,
        };
        if name.is_empty() {
            self.card_note("give the persona a name", true);
            return;
        }
        let existing = self.persona_edit.as_ref().and_then(|e| e.avatar.clone());
        let msg = match persona::put(existing.as_deref(), &name, &title, &description) {
            Ok(_) if existing.is_some() => format!("saved persona {}", name),
            Ok(_) => format!("created persona {}", name),
            Err(e) => {
                self.card_note(format!("could not save the persona: {:#}", e), true);
                return;
            }
        };
        // The active persona may just have been edited.
        let settings = self.settings.clone();
        self.apply_settings(settings);
        self.card_modal = None;
        self.persona_edit = None;
        self.open_persona_picker().await;
        if let Some((_, p)) = self.picker.as_mut() {
            p.note = Some((msg, false));
        }
    }

    /// Delete a persona chosen in the `/persona` window; the window stays open.
    async fn persona_delete(&mut self, key: &str) {
        let note = if persona::current().id == key {
            (
                "that persona is in use \u{2014} switch to another one first".to_string(),
                true,
            )
        } else {
            match persona::remove(key) {
                Ok(()) => {
                    if let Some((_, p)) = self.picker.as_mut() {
                        p.remove(key);
                    }
                    ("deleted the persona".to_string(), false)
                }
                Err(e) => (format!("could not delete the persona: {:#}", e), true),
            }
        };
        if let Some((_, p)) = self.picker.as_mut() {
            p.note = Some(note);
        }
    }

    /// Make `id` the active persona: remembered in tarpi's persona folder and
    /// used by this chat from the next message on.
    async fn switch_persona(&mut self, id: &str) {
        if persona::current().id == id {
            return;
        }
        let p = match persona::activate(id) {
            Ok(p) => p,
            Err(e) => {
                self.set_status(format!("could not switch the persona: {:#}", e), true);
                return;
            }
        };
        let settings = self.settings.clone();
        self.apply_settings(settings);
        self.set_status(format!("now chatting as {}", p.label()), false);
    }

    /// `/prompt`: show the whole prompt the next reply would be sent with: the
    /// system prompt, character card, persona, examples, instruct formatting
    /// and the messages that fit into the context size, as one text.
    async fn show_prompt(&mut self) {
        // The same fresh settings, persona and system prompt a reply would use.
        self.refresh_settings().await;
        let note = Note::from_chat(&self.chat);
        let tp = textgen::build_prompt(&textgen::TgArgs {
            settings: &self.settings,
            ch: &self.character,
            user_name: &self.user_name,
            persona: &self.persona,
            persona_position: self.persona_position,
            note: &note,
            history: &self.chat.messages[..],
            chars_per_token: self.chars_per_token,
            continue_text: None,
            nudge: None,
        });
        let chars = tp.prompt.chars().count();
        let tokens = prompt::est_tokens(&tp.prompt, self.chars_per_token);
        let mut text = String::new();
        for c in tp.prompt.chars() {
            match c {
                '\n' => text.push('\n'),
                '\t' => text.push_str("    "),
                '\r' => {}
                c if c.is_control() => text.push(' '),
                c => text.push(c),
            }
        }
        self.prompt_view = Some(PromptView {
            text,
            title: format!(
                " prompt \u{b7} {} characters \u{b7} ~{} tokens ",
                chars,
                summary::fmt_tokens(tokens)
            ),
            scroll: 0,
            page: 10,
            wrapped: Vec::new(),
            wrapped_w: 0,
        });
    }

    /// Keys in the `/prompt` window: scroll, Esc / q / Enter close.
    fn prompt_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let mut close = false;
        if let Some(v) = self.prompt_view.as_mut() {
            let page = v.page.max(1);
            let half = (page / 2).max(1);
            match k.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter if !ctrl => close = true,
                KeyCode::Char('c') if ctrl => close = true,
                KeyCode::Up | KeyCode::Char('k') if !ctrl => v.scroll = v.scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') if !ctrl => {
                    v.scroll = v.scroll.saturating_add(1)
                }
                KeyCode::PageUp => v.scroll = v.scroll.saturating_sub(page),
                KeyCode::PageDown | KeyCode::Char(' ') => v.scroll = v.scroll.saturating_add(page),
                KeyCode::Char('u') if ctrl => v.scroll = v.scroll.saturating_sub(half),
                KeyCode::Char('d') if ctrl => v.scroll = v.scroll.saturating_add(half),
                KeyCode::Home | KeyCode::Char('g') => v.scroll = 0,
                KeyCode::End | KeyCode::Char('G') => v.scroll = usize::MAX,
                _ => {}
            }
        }
        if close {
            self.prompt_view = None;
        }
    }

    /// `/system`: choose the system prompt, make a new one (the first row),
    /// edit one (`e`) or delete one (`d`).
    async fn open_sysprompt_picker(&mut self) {
        let active = sysprompt::current().map(|p| p.id);
        let mut items = vec![PickItem {
            key: NEW_SYSPROMPT.to_string(),
            label: "+ new system prompt".to_string(),
            detail: String::new(),
            current: false,
            deletable: false,
            editable: false,
        }];
        let list = sysprompt::list();
        let none_yet = list.is_empty();
        items.extend(list.into_iter().map(|p| PickItem {
            label: p.name.clone(),
            detail: persona::brief(&p.content, 80),
            current: active.as_ref() == Some(&p.id),
            deletable: true,
            editable: true,
            key: p.id,
        }));
        let mut picker = Picker::new("system prompt", items);
        if none_yet {
            picker.note = Some((
                "none yet: make one with + new, or run tarpi import-st".to_string(),
                false,
            ));
        }
        self.picker = Some((PickKind::SysPrompts, picker));
    }

    /// Open the system prompt form for `id`, or an empty one for a new prompt.
    async fn open_sysprompt_form(&mut self, id: Option<String>) {
        let (name, content, post_history) = match &id {
            None => (String::new(), String::new(), String::new()),
            Some(i) => match sysprompt::list().into_iter().find(|p| p.id == *i) {
                Some(p) => (p.name, p.content, p.post_history),
                None => {
                    self.set_status("that system prompt no longer exists", true);
                    return;
                }
            },
        };
        self.card_modal = Some(CardModal::sysprompt(
            &name,
            &content,
            &post_history,
            &self.input,
        ));
        self.sys_edit = Some(SysEdit { id });
    }

    /// Ctrl+S in the system prompt form: create or update the prompt, then
    /// show the `/system` window again. On a failure the form stays open.
    async fn save_sysprompt_form(&mut self) {
        let (name, content, post_history) = match self.card_modal.as_mut() {
            Some(m) => {
                m.sync();
                (m.value("name"), m.value("content"), m.value("post_history"))
            }
            None => return,
        };
        if name.is_empty() {
            self.card_note("give the system prompt a name", true);
            return;
        }
        let existing = self.sys_edit.as_ref().and_then(|e| e.id.clone());
        let msg = match sysprompt::put(existing.as_deref(), &name, &content, &post_history) {
            Ok(_) if existing.is_some() => format!("saved system prompt {}", name),
            Ok(_) => format!("created system prompt {}", name),
            Err(e) => {
                self.card_note(format!("could not save the system prompt: {:#}", e), true);
                return;
            }
        };
        // The prompt in use may just have been edited.
        let settings = self.settings.clone();
        self.apply_settings(settings);
        self.card_modal = None;
        self.sys_edit = None;
        self.open_sysprompt_picker().await;
        if let Some((_, p)) = self.picker.as_mut() {
            p.note = Some((msg, false));
        }
    }

    /// Delete a system prompt chosen in the `/system` window; the window stays
    /// open. The one in use can't be deleted.
    async fn sysprompt_delete(&mut self, key: &str) {
        let in_use = sysprompt::current().map(|p| p.id == key).unwrap_or(false);
        let note = if in_use {
            (
                "that system prompt is in use \u{2014} switch to another one first".to_string(),
                true,
            )
        } else {
            match sysprompt::remove(key) {
                Ok(()) => {
                    if let Some((_, p)) = self.picker.as_mut() {
                        p.remove(key);
                    }
                    ("deleted the system prompt".to_string(), false)
                }
                Err(e) => (format!("could not delete the system prompt: {:#}", e), true),
            }
        };
        if let Some((_, p)) = self.picker.as_mut() {
            p.note = Some(note);
        }
    }

    /// Make `id` the system prompt in use: remembered in tarpi's
    /// `system-prompt.jsonl` and used by this chat from the next message on.
    async fn switch_sysprompt(&mut self, id: &str) {
        if sysprompt::current().map(|p| p.id == id).unwrap_or(false) {
            return;
        }
        let p = match sysprompt::activate(id) {
            Ok(p) => p,
            Err(e) => {
                self.set_status(format!("could not switch the system prompt: {:#}", e), true);
                return;
            }
        };
        let settings = self.settings.clone();
        self.apply_settings(settings);
        self.set_status(format!("system prompt: {}", p.name), false);
    }

    async fn picker_action(&mut self, act: PickAction) {
        let Some(kind) = self.picker.as_ref().map(|(k, _)| *k) else {
            return;
        };
        match act {
            PickAction::None => {}
            PickAction::Cancel => self.picker = None,
            PickAction::Choose(key) => {
                self.picker = None;
                match kind {
                    PickKind::Characters => {
                        if key != self.chat.avatar {
                            self.leave(Exit::Character(key)).await;
                        }
                    }
                    PickKind::Personas => {
                        if key == NEW_PERSONA {
                            self.open_persona_form(None).await;
                        } else {
                            self.switch_persona(&key).await;
                        }
                    }
                    PickKind::SysPrompts => {
                        if key == NEW_SYSPROMPT {
                            self.open_sysprompt_form(None).await;
                        } else {
                            self.switch_sysprompt(&key).await;
                        }
                    }
                    PickKind::Chats => {
                        if key == NEW_CHAT {
                            self.leave(Exit::NewChat).await;
                        } else if key != self.chat.file {
                            self.leave(Exit::Chat(key)).await;
                        }
                    }
                }
            }
            PickAction::Delete(key) => match kind {
                PickKind::Personas => self.persona_delete(&key).await,
                PickKind::SysPrompts => self.sysprompt_delete(&key).await,
                _ => self.picker_delete(&key).await,
            },
            PickAction::Position(key) => {
                if kind == PickKind::Personas {
                    self.picker = None;
                    self.open_persona_pos(key).await;
                }
            }
            PickAction::Edit(key) => {
                if kind == PickKind::Personas {
                    self.picker = None;
                    self.open_persona_form(Some(key)).await;
                } else if kind == PickKind::SysPrompts {
                    self.picker = None;
                    self.open_sysprompt_form(Some(key)).await;
                }
            }
        }
    }

    /// Delete a chat file chosen in the `/chats` window; the window stays open.
    async fn picker_delete(&mut self, key: &str) {
        // The open chat is the only one left: delete it (and its hidden
        // `_before-summary` copies) and leave the program.
        let only_one = self
            .picker
            .as_ref()
            .map(|(_, p)| p.items.iter().filter(|i| i.deletable).count() == 1)
            .unwrap_or(false);
        if key == self.chat.file && only_one {
            // Stop first: finishing a generation would save the chat again.
            self.picker = None;
            self.quit().await;
            let avatar = self.chat.avatar.clone();
            let mut names = vec![key.to_string()];
            if let Ok(chats) = chatstore::list(&avatar) {
                let ids: Vec<String> = chats.iter().map(chat::file_id).collect();
                names.extend(chat::backups_of(key, &ids));
            }
            for name in names {
                let _ = chatstore::delete(&avatar, &name);
            }
            return;
        }
        let note = if key == self.chat.file {
            (
                "this chat is open \u{2014} use /newchat to replace it".to_string(),
                true,
            )
        } else {
            match chatstore::delete(&self.chat.avatar, key) {
                Ok(()) => {
                    if let Some((_, p)) = self.picker.as_mut() {
                        p.remove(key);
                    }
                    (format!("deleted {}", key), false)
                }
                Err(e) => (format!("could not delete {}: {:#}", key, e), true),
            }
        };
        if let Some((_, p)) = self.picker.as_mut() {
            p.note = Some(note);
        }
    }

    // ------------------------------------------------------ /character window

    /// `/char-img`: show the card's picture in a square window.
    async fn open_image_modal(&mut self) {
        let avatar = if self.character.avatar.trim().is_empty() {
            self.chat.avatar.clone()
        } else {
            self.character.avatar.clone()
        };
        let res = match charstore::image(&avatar) {
            Ok(bytes) => ImageModal::new(&self.character.name, &bytes),
            Err(e) => Err(e),
        };
        match res {
            Ok(m) => self.image_modal = Some(m),
            Err(e) => self.set_status(format!("{:#}", e), true),
        }
    }

    /// `/character`: open the card of this character as an editable form.
    async fn open_card_modal(&mut self) {
        // Read the card fresh: it may have been edited since the chat opened.
        let avatar = self.character.avatar.clone();
        let shown = match charstore::get(&avatar) {
            Ok(v) => {
                let mut ch = Character::from_value(&v);
                if ch.avatar.trim().is_empty() {
                    ch.avatar = avatar;
                }
                ch
            }
            Err(_) => self.character.clone(),
        };
        self.card_modal = Some(CardModal::new(&shown, &self.input));
    }

    /// `/note`: the Author's Note of this chat as a form.
    async fn open_note(&mut self) {
        let n = Note::from_chat(&self.chat);
        self.card_modal = Some(CardModal::note(&n, &self.input));
        self.note_edit = true;
    }

    /// Ctrl+S in the Author's Note form: check the settings, keep the note in
    /// the chat file and use it from the next reply on. On a mistake the form
    /// stays open and says what to type.
    async fn save_note_form(&mut self) {
        let (text, position, depth, role, interval) = match self.card_modal.as_mut() {
            Some(m) => {
                m.sync();
                (
                    m.value("note"),
                    m.value("position"),
                    m.value("depth"),
                    m.value("role"),
                    m.value("interval"),
                )
            }
            None => return,
        };
        let Some(position) = note::parse_position(&position) else {
            self.card_note("position: type before, after or chat", true);
            return;
        };
        let Some(depth) = note::parse_number(&depth, note::MAX_DEPTH) else {
            self.card_note(
                format!("depth: type a whole number from 0 to {}", note::MAX_DEPTH),
                true,
            );
            return;
        };
        let Some(role) = note::parse_role(&role) else {
            self.card_note("role: type system, user or assistant", true);
            return;
        };
        let Some(interval) = note::parse_number(&interval, note::MAX_INTERVAL) else {
            self.card_note(
                format!(
                    "interval: type a whole number from 0 to {} (0 = never)",
                    note::MAX_INTERVAL
                ),
                true,
            );
            return;
        };
        let n = Note {
            text,
            position,
            depth,
            role,
            interval,
        };
        n.store(&mut self.chat);
        self.save().await;
        // The prompt (and its token count) is built from the note too.
        self.settings_ver += 1;
        self.card_modal = None;
        self.note_edit = false;
        let msg = if n.text.is_empty() {
            "author's note saved (no text: nothing is sent)"
        } else if n.interval == 0 {
            "author's note saved (interval 0: it is never sent)"
        } else {
            "author's note saved"
        };
        self.set_status(msg, false);
    }

    async fn cancel_card_modal(&mut self) {
        let dirty = self.card_modal.as_ref().map(|m| m.dirty()).unwrap_or(false);
        self.card_modal = None;
        if self.persona_edit.take().is_some() {
            // Back to the persona window.
            self.open_persona_picker().await;
            if dirty {
                self.set_status("persona: changes discarded", false);
            }
            return;
        }
        if self.sys_edit.take().is_some() {
            // Back to the system prompt window.
            self.open_sysprompt_picker().await;
            if dirty {
                self.set_status("system prompt: changes discarded", false);
            }
            return;
        }
        if self.note_edit {
            self.note_edit = false;
            if dirty {
                self.set_status("author's note: changes discarded", false);
            }
            return;
        }
        if dirty {
            self.set_status("character card: changes discarded", false);
        }
    }

    /// Write every changed field of the form to the card, then close it. On a
    /// failure the form stays open (what was already written counts as saved).
    async fn save_card_modal(&mut self) {
        if self.persona_edit.is_some() {
            self.save_persona_form().await;
            return;
        }
        if self.sys_edit.is_some() {
            self.save_sysprompt_form().await;
            return;
        }
        if self.note_edit {
            self.save_note_form().await;
            return;
        }
        let changes = match self.card_modal.as_mut() {
            Some(m) => {
                m.sync();
                m.changes()
            }
            None => return,
        };
        if changes.is_empty() {
            self.card_modal = None;
            self.set_status("character card: no changes to save", false);
            return;
        }
        let avatar = self.character.avatar.clone();
        let total = changes.len();
        for (key, value) in changes {
            if let Err(e) = charstore::set_field(&avatar, key, &value) {
                self.card_note(format!("{:#}", e), true);
                self.set_status(format!("{:#}", e), true);
                return;
            }
            self.apply_card_field(key, &value);
            if let Some(m) = self.card_modal.as_mut() {
                m.mark_saved(key);
            }
        }
        self.card_modal = None;
        self.set_status(
            format!(
                "saved to the character card ({} field{})",
                total,
                if total == 1 { "" } else { "s" }
            ),
            false,
        );
    }

    /// Keep the card held in memory in step with what was just saved.
    fn apply_card_field(&mut self, key: &str, value: &Value) {
        let text = value.as_str().unwrap_or("").to_string();
        let ch = &mut self.character;
        match key {
            "name" => ch.name = text,
            "description" => ch.description = text,
            "personality" => ch.personality = text,
            "scenario" => ch.scenario = text,
            "first_mes" => ch.first_mes = text,
            "mes_example" => ch.mes_example = text,
            "system_prompt" => ch.system_prompt = text,
            "post_history_instructions" => ch.post_history_instructions = text,
            "alternate_greetings" => {
                ch.alternate_greetings = value
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str())
                            .map(|x| x.to_string())
                            .collect::<Vec<String>>()
                    })
                    .unwrap_or_default();
            }
            _ => {}
        }
    }

    /// A one-line message inside the `/character` window.
    fn card_note(&mut self, msg: impl Into<String>, is_error: bool) {
        if let Some(m) = self.card_modal.as_mut() {
            m.note = msg.into();
            m.note_err = is_error;
        }
    }

    /// Keys in the `/character` window. The focused field is edited with the
    /// chat box's own editor, so INSERT / NORMAL / VISUAL / `:` all work.
    async fn on_card_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        if ctrl {
            match k.code {
                KeyCode::Char('c') => {
                    self.quit().await;
                    return;
                }
                KeyCode::Char('s') => {
                    self.save_card_modal().await;
                    return;
                }
                _ => {}
            }
        }
        // Tab opens the spelling menu on a misspelled word.
        if self.spell_key(&k) {
            return;
        }
        let half_dir = match k.code {
            KeyCode::Char('u') if ctrl && !alt => -1,
            KeyCode::Char('d') if ctrl && !alt => 1,
            _ => 0,
        };

        let mut cancel = false;
        let mut cmd_result: Option<Result<String, String>> = None;
        let mut write = false;
        let mut clip: Option<String> = None;
        {
            let Some(c) = self.card_modal.as_mut() else {
                return;
            };
            c.note.clear();
            c.note_err = false;
            let was_armed = std::mem::replace(&mut c.discard_armed, false);
            c.follow = true;
            let width = c.text_w.max(1);
            let page = c.view_h.saturating_sub(1).max(1);

            if c.input.mode == Mode::Command {
                // `:` command line (`:%s/find/new/g` and friends).
                match k.code {
                    KeyCode::Char('u') if ctrl => c.input.command_clear(),
                    KeyCode::Char('w') if ctrl => c.input.command_delete_word(),
                    KeyCode::Char(ch) if !ctrl && !alt => c.input.command_push(ch),
                    KeyCode::Backspace => c.input.command_backspace(),
                    KeyCode::Esc => c.input.cancel_command(),
                    KeyCode::Enter => {
                        cmd_result = Some(c.input.run_command());
                        write = c.input.take_write_req();
                    }
                    _ => {}
                }
            } else if half_dir != 0 && matches!(c.input.mode, Mode::Normal | Mode::Visual) {
                // Ctrl+U / Ctrl+D: half a window up / down; reaching the
                // edge of the field moves on into the neighbouring one.
                c.half_page(half_dir);
            } else if matches!(c.input.mode, Mode::Normal | Mode::Visual)
                && !ctrl
                && !alt
                && matches!(k.code, KeyCode::Char(_))
            {
                // NORMAL / VISUAL mode: letters are vim commands.
                if let KeyCode::Char(ch) = k.code {
                    c.normal_key(ch);
                    clip = c.input.take_clipboard();
                }
            } else {
                let mode = c.input.mode;
                match k.code {
                    KeyCode::Esc => match mode {
                        Mode::Visual => c.input.exit_visual(),
                        // Like vim: Esc leaves INSERT mode.
                        Mode::Insert if c.input.vim_enabled() => c.input.enter_normal(),
                        _ => {
                            c.input.cancel_pending();
                            if c.dirty() && !was_armed {
                                c.discard_armed = true;
                                c.note = "unsaved changes: Esc again discards, Ctrl+S saves"
                                    .to_string();
                                c.note_err = true;
                            } else {
                                cancel = true;
                            }
                        }
                    },
                    KeyCode::Enter => match mode {
                        Mode::Insert => c.input.insert_char('\n'),
                        Mode::Normal => c.down(),
                        _ => {}
                    },
                    KeyCode::Backspace => c.input.backspace(),
                    KeyCode::Delete => c.input.delete(),
                    KeyCode::Left if alt && !ctrl => c.input.word_left(width),
                    KeyCode::Right if alt && !ctrl => c.input.word_right(width),
                    KeyCode::Left => c.input.left(),
                    KeyCode::Right => c.input.right(),
                    KeyCode::Up => c.up(),
                    KeyCode::Down => c.down(),
                    KeyCode::Home => c.input.home(),
                    KeyCode::End => c.input.end(),
                    KeyCode::PageUp => {
                        for _ in 0..page {
                            c.up();
                        }
                    }
                    KeyCode::PageDown => {
                        for _ in 0..page {
                            c.down();
                        }
                    }
                    KeyCode::Char('a') if ctrl => c.input.home(),
                    KeyCode::Char('e') if ctrl => c.input.end(),
                    KeyCode::Char('w') if ctrl => c.input.delete_word_back(),
                    KeyCode::Char('j') if ctrl => c.input.insert_char('\n'),
                    // INSERT mode; typing the NORMAL-mode keys (`qq`) quickly
                    // switches to NORMAL mode.
                    KeyCode::Char(ch) if (!ctrl && !alt) || (ctrl && alt) => {
                        c.input.type_char(ch)
                    }
                    _ => {}
                }
            }
            c.input.clamp_normal();
            c.sync();
        }
        if let Some(text) = clip {
            let (msg, err) = copy_to_clipboard(&text);
            self.card_note(msg.clone(), err);
            self.set_status(msg, err);
        }
        if cancel {
            self.cancel_card_modal().await;
            return;
        }
        if write {
            // `:w` = Ctrl+S / the Save button.
            self.save_card_modal().await;
            return;
        }
        match cmd_result {
            Some(Ok(msg)) if !msg.is_empty() => {
                self.card_note(msg.clone(), false);
                self.set_status(msg, false);
            }
            Some(Err(msg)) => {
                self.card_note(msg.clone(), true);
                self.set_status(msg, true);
            }
            _ => {}
        }
    }

    async fn on_card_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if let Some(c) = self.card_modal.as_mut() {
                    c.follow = false;
                    c.scroll = if up {
                        c.scroll.saturating_sub(3)
                    } else {
                        c.scroll.saturating_add(3)
                    };
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let pos = Position::new(m.column, m.row);
                if self.spell_menu.is_some() {
                    // A click on a suggestion puts it in; anywhere else closes the menu.
                    if self.spell_area.contains(pos) {
                        if let Some(&(_, i)) = self.spell_rows.iter().find(|(y, _)| *y == m.row) {
                            self.spell_accept(i);
                        }
                        return;
                    }
                    self.spell_menu = None;
                }
                let (cancel, save) = match self.card_modal.as_ref() {
                    Some(c) => (c.cancel_btn.contains(pos), c.save_btn.contains(pos)),
                    None => return,
                };
                if cancel {
                    self.cancel_card_modal().await;
                } else if save {
                    self.save_card_modal().await;
                } else if let Some(c) = self.card_modal.as_mut() {
                    c.click(m.column, m.row);
                }
            }
            _ => {}
        }
    }

    fn restore_draft(&mut self, draft: (String, usize)) {
        self.input.clear();
        self.input.insert_str(&draft.0);
        self.input.cursor = draft.1.min(self.input.len_chars());
        self.input.enter_insert();
        self.input.reset_vim();
    }

    fn cancel_edit(&mut self) {
        if let Some(e) = self.editing.take() {
            self.restore_draft(e.draft);
            self.set_status("edit cancelled", false);
        }
    }

    async fn commit_edit(&mut self) {
        let Some(e) = self.editing.take() else {
            return;
        };
        let new_text = chat::tidy(&self.input.text);
        self.auto_closed = None;
        self.restore_draft(e.draft);
        let changed = match self.chat.messages.get_mut(e.idx) {
            Some(m) if chat::mes(m) != new_text => {
                chat::set_text(m, &new_text);
                true
            }
            _ => false,
        };
        if changed {
            self.save().await;
            self.set_status("message edited", false);
        } else {
            self.set_status("no changes", false);
        }
    }

    /// Ctrl+K, `/continue` or `/c`: let the model keep writing the last reply.
    async fn continue_message(&mut self) {
        if self.one_shot_blocks("continue") {
            return;
        }
        if self.is_generating() {
            return;
        }
        let ok = self
            .chat
            .messages
            .last()
            .map(|m| !chat::is_user(m) && !chat::is_system(m))
            .unwrap_or(false);
        if !ok {
            self.set_status(
                "nothing to continue \u{2014} the last message isn't a reply",
                false,
            );
            return;
        }
        // Take off the markers the TUI added when the reply finished, so the
        // continuation can carry on inside them.
        if let Some((idx, closer)) = self.auto_closed.take() {
            let last = self.chat.messages.len() - 1;
            if idx == last {
                if let Some(m) = self.chat.messages.get_mut(last) {
                    let text = chat::mes(m).to_string();
                    if let Some(stripped) = text.strip_suffix(closer.as_str()) {
                        let stripped = stripped.to_string();
                        chat::set_text(m, &stripped);
                    }
                }
            }
        }
        self.start_generation_mode(false, true, false).await;
    }

    /// Turn the terminal's mouse reporting on or off.
    fn set_mouse(&mut self, on: bool) {
        self.mouse = on;
        let _ = if on {
            crossterm::execute!(std::io::stdout(), EnableMouseCapture)
        } else {
            crossterm::execute!(std::io::stdout(), DisableMouseCapture)
        };
    }

    /// The terminal changed size. On a phone that is the keyboard opening or
    /// closing: remember which, and as soon as it is open turn taps on
    /// messages back on.
    /// Misspelled words (char ranges) of `text`, the text of the input box.
    /// Empty when spell checking is off. The word the cursor is in is left
    /// alone while typing (INSERT mode), since it is not finished yet.
    pub fn spell_marks(&mut self, text: &str) -> Vec<(usize, usize)> {
        self.spell_touch();
        let typing = (self.input.mode == crate::input::Mode::Insert && !self.spell_rested())
            .then_some(self.input.cursor);
        self.spell_names();
        match self.spell.as_mut() {
            Some(sp) => sp.marks_typing(text, typing),
            None => Vec::new(),
        }
    }

    /// The text box being typed into: the one of the open `/char` or persona
    /// window, otherwise the chat box.
    fn spell_input(&self) -> &crate::input::Input {
        match self.card_modal.as_ref() {
            Some(c) => &c.input,
            None => &self.input,
        }
    }

    /// Note a change of text, cursor or mode: the menu closes and the rest
    /// timer (the word at the cursor is underlined once it is over) starts
    /// again.
    pub fn spell_touch(&mut self) {
        let mut h = DefaultHasher::new();
        let inp = self.spell_input();
        inp.text.hash(&mut h);
        inp.cursor.hash(&mut h);
        (inp.mode as u8).hash(&mut h);
        self.card_modal.as_ref().map(|c| c.focus).hash(&mut h);
        let key = h.finish();
        if key != self.spell_key {
            self.spell_key = key;
            self.spell_since = Instant::now();
            self.spell_menu = None;
        }
    }

    /// The cursor has not moved and nothing was typed for a second.
    pub fn spell_rested(&self) -> bool {
        self.spell_since.elapsed() >= SPELL_DELAY
    }

    /// The misspelled word the cursor is in (or right behind), if any.
    fn spell_word_at_cursor(&mut self) -> Option<((usize, usize), String)> {
        let inp = self.spell_input();
        if !matches!(inp.mode, Mode::Insert | Mode::Normal) {
            return None;
        }
        // In the windows the Name fields are not checked.
        if let Some(c) = self.card_modal.as_ref() {
            let f = c.fields.get(c.focus)?;
            if !f.editable || f.key == "name" {
                return None;
            }
        }
        let text = inp.text.clone();
        let cur = inp.cursor;
        let (s, e) = self
            .spell
            .as_mut()?
            .marks(&text)
            .into_iter()
            .find(|&(s, e)| cur >= s && cur <= e)?;
        let word: String = text.chars().skip(s).take(e - s).collect();
        Some(((s, e), word))
    }

    /// Open the menu for the word at the cursor.
    fn spell_open(&mut self) -> bool {
        self.spell_names();
        let Some((range, word)) = self.spell_word_at_cursor() else {
            return false;
        };
        let sugg = self
            .spell
            .as_ref()
            .map(|sp| sp.suggest(&word, 6))
            .unwrap_or_default();
        if sugg.is_empty() {
            return false;
        }
        self.spell_menu = Some(SpellMenu {
            range,
            word,
            sugg,
            sel: 0,
        });
        true
    }

    /// Keys for the spelling menu: Tab opens it on a misspelled word (and,
    /// open, moves down); Shift+Tab / Up / Down move; Enter puts the
    /// highlighted entry in; Esc closes it. Returns false when the key is
    /// not for the menu (and then closes it).
    fn spell_key(&mut self, k: &KeyEvent) -> bool {
        if self.spell.is_none()
            || !matches!(self.spell_input().mode, Mode::Insert | Mode::Normal)
            || k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            || (self.card_modal.is_none() && self.completion().is_some())
        {
            self.spell_menu = None;
            return false;
        }
        if self.spell_menu.is_none() {
            return k.code == KeyCode::Tab && self.spell_open();
        }
        let n = self.spell_menu.as_ref().map_or(0, |m| m.sugg.len());
        let m = self.spell_menu.as_mut().unwrap();
        match k.code {
            KeyCode::Tab | KeyCode::Down => m.sel = (m.sel + 1) % n,
            KeyCode::BackTab | KeyCode::Up => m.sel = (m.sel + n - 1) % n,
            KeyCode::Enter => {
                let i = m.sel;
                self.spell_accept(i);
            }
            KeyCode::Esc => self.spell_menu = None,
            _ => {
                self.spell_menu = None;
                return false;
            }
        }
        true
    }

    /// Replace the misspelled word with suggestion `i`.
    pub fn spell_accept(&mut self, i: usize) {
        let Some(m) = self.spell_menu.take() else {
            return;
        };
        let Some(new) = m.sugg.get(i) else {
            return;
        };
        let put = |inp: &mut crate::input::Input| {
            // One undo step of its own: `u` puts the misspelled word back.
            inp.replace_chars(m.range.0, m.range.1, new);
            // In NORMAL mode the cursor sits on a letter, not after the last one.
            inp.clamp_normal();
        };
        match self.card_modal.as_mut() {
            Some(c) => {
                put(&mut c.input);
                c.follow = true;
                c.sync();
            }
            None => {
                put(&mut self.input);
                self.scroll = 0;
            }
        }
    }

    /// Tell the spell checker the names it must accept (character, persona).
    pub fn spell_names(&mut self) {
        if self.spell.is_none() {
            return;
        }
        // The character (its name and card file name) and every persona
        // (names and titles), so nothing the story is about gets marked.
        let avatar = self.character.avatar.trim_end_matches(".png").to_string();
        let mut names: Vec<String> = vec![
            self.character.name.clone(),
            avatar,
            self.user_name.clone(),
            self.persona_title.clone(),
        ];
        for p in crate::persona::list() {
            names.push(p.name);
            names.push(p.title);
        }
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        if let Some(sp) = self.spell.as_mut() {
            sp.set_names(&refs);
        }
    }

    pub fn on_resize(&mut self, rows: u16) {
        self.max_rows = self.max_rows.max(rows);
        self.kbd_up = self.max_rows > 0 && (rows as u32) * 10 < (self.max_rows as u32) * 8;
        if self.touch && !self.mouse && self.mouse_back_at.take().is_some() {
            self.set_mouse(true);
        }
    }

    /// Close the chat screen and go back to a menu (see `main`).
    async fn leave(&mut self, to: Exit) {
        if self.stop_blocked() {
            return;
        }
        self.exit_to = Some(to);
        self.quit().await;
    }

    async fn quit(&mut self) {
        if self.stop_blocked() {
            return;
        }
        if self.is_generating() {
            self.cancel_generation().await;
        }
        self.should_quit = true;
    }

    /// `/undo [n]`: delete the last `n` messages (default 1). A branched chat
    /// always keeps its first message; otherwise `n` is capped at what exists.
    async fn undo(&mut self, count: usize) {
        if self.one_shot_blocks("undo") {
            return;
        }
        if self.editing.is_some() {
            return;
        }
        if self.is_generating() {
            self.set_status("stop the generation first (Esc)", false);
            return;
        }
        let branched =
            !self.chat.messages.is_empty() && chat::branch_pos(&self.chat.messages[0]).is_some();
        let available = self.chat.messages.len().saturating_sub(usize::from(branched));
        if available == 0 {
            if branched {
                self.set_status(
                    "this is the first message of a branched chat \u{2014} it can't be undone (switch branch instead)",
                    false,
                );
            }
            return;
        }
        let n = count.min(available);
        let keep = self.chat.messages.len() - n;
        self.chat.messages.truncate(keep);
        self.auto_closed = None;
        self.scroll = 0;
        self.save().await;
        let msg = if n == 1 {
            if count > 1 {
                "deleted the last message (that was all there was to undo)".to_string()
            } else {
                "deleted the last message".to_string()
            }
        } else if n < count {
            format!("deleted the last {} messages (that was all there was to undo)", n)
        } else {
            format!("deleted the last {} messages", n)
        };
        self.set_status(msg, false);
    }

    async fn new_chat(&mut self) {
        if self.is_generating() {
            self.set_status("stop the generation first (Esc)", false);
            return;
        }
        let ids: Vec<String> = match chatstore::list(&self.chat.avatar) {
            Ok(list) => list.iter().map(chat::file_id).collect(),
            Err(_) => vec![self.chat.file.clone()],
        };
        let file = chat::next_chat_name(&self.character.name, &ids);
        let mut state = ChatState::new_empty(&self.chat.avatar, &self.chat.char_name, &file);
        if let Some(g) = make_greeting(&self.character, &self.user_name, &self.persona) {
            state.messages.push(g);
        }
        self.chat = state;
        self.auto_closed = None;
        self.scroll = 0;
        self.save().await;
        self.set_status(format!("started new chat: {}", self.chat.file), false);
    }

    /// `/newchat` (`/nc`): deletes the chat file right away and
    /// starts a fresh chat.
    async fn delete_chat(&mut self) {
        if self.is_generating() {
            self.set_status("stop the generation first (Esc)", false);
            return;
        }
        let file = self.chat.file.clone();
        match chatstore::delete(&self.chat.avatar, &file) {
            Ok(()) => {
                self.new_chat().await;
                let now = self.chat.file.clone();
                self.set_status(format!("deleted {}; started new chat {}", file, now), false);
            }
            Err(e) => self.set_status(format!("{:#}", e), true),
        }
    }

    async fn regenerate(&mut self, nudge: Option<String>) {
        if self.one_shot_blocks("regenerate") {
            return;
        }
        if self.is_generating() || self.editing.is_some() {
            return;
        }
        let Some(last) = self.chat.messages.last() else {
            return;
        };
        if chat::is_user(last) {
            self.next_nudge = nudge;
            self.start_generation(false).await;
        } else if self.chat.messages.len() < 2 {
            self.set_status(
                "nothing to regenerate yet \u{2014} send a message first (Ctrl+\u{2192} cycles greetings)",
                false,
            );
        } else {
            self.next_nudge = nudge;
            self.start_generation_mode(true, false, true).await;
        }
    }

    /// `/deletebranch` (`/db`): delete the branch that is showing and show
    /// the one before it.
    async fn delete_branch(&mut self) {
        if self.one_shot_blocks("branching") {
            return;
        }
        if self.is_generating() || self.editing.is_some() {
            self.set_status("finish what you are doing first, then /deletebranch", false);
            return;
        }
        let Some((cur, total)) = self.chat.messages.first().and_then(chat::branch_pos) else {
            self.set_status("there is no branch to delete: this chat has no summary branches", false);
            return;
        };
        let was = cur + 1;
        let Some((now, left)) = chat::delete_branch(&mut self.chat.messages) else {
            return;
        };
        self.auto_closed = None;
        self.scroll = 0;
        self.reveal = Some(0);
        let msg = if left >= 2 {
            format!(
                "deleted branch {} of {}; now on summary branch {}/{}",
                was,
                total,
                now + 1,
                left
            )
        } else {
            format!("deleted branch {} of {}; the chat has no more branches", was, total)
        };
        self.save().await;
        // The full-chat copy `/summarize` kept goes with the branch: the newest
        // one, or all that are left when no branch remains.
        let mut gone: Vec<String> = Vec::new();
        if let Ok(chats) = chatstore::list(&self.chat.avatar) {
            let ids: Vec<String> = chats.iter().map(chat::file_id).collect();
            let mut backups = chat::backups_of(&self.chat.file, &ids);
            if left >= 2 {
                backups = backups.into_iter().last().into_iter().collect();
            }
            for name in backups {
                if chatstore::delete(&self.chat.avatar, &name).is_ok() {
                    gone.push(name);
                }
            }
        }
        let msg = if gone.is_empty() {
            msg
        } else {
            format!("{}; deleted {}", msg, gone.join(", "))
        };
        self.set_status(msg, false);
    }

    /// Show the previous (`dir` < 0) or next summary branch of a summarized
    /// chat, wrapping around: branch 1 is the whole conversation, the others
    /// are summaries. Each keeps its own messages.
    async fn switch_branch(&mut self, dir: i32) {
        if self.one_shot_blocks("branching") {
            return;
        }
        if self.is_generating() || self.editing.is_some() {
            return;
        }
        if !chat::switch_branch(&mut self.chat.messages, dir) {
            self.set_status("no summary branches yet: /summarize makes one", false);
            return;
        }
        self.auto_closed = None;
        self.scroll = 0;
        self.reveal = Some(0);
        if let Some((cur, total)) = self.chat.messages.first().and_then(chat::branch_pos) {
            let what = if cur == 0 { "the conversation" } else { "a summary" };
            self.set_status(format!("summary branch {}/{}: {}", cur + 1, total, what), false);
        }
        self.save().await;
    }

    /// Left / Right (empty input box): browse swipes of the last reply (or, while the greeting is the only message, the alternate greetings); going past the
    /// last one generates a new swipe (like the web UI).
    async fn swipe(&mut self, dir: i32) {
        if self.one_shot_blocks("swipes") {
            return;
        }
        if self.is_generating() || self.editing.is_some() {
            return;
        }
        self.auto_closed = None;
        let len = self.chat.messages.len();
        let Some(m) = self.chat.messages.last_mut() else {
            return;
        };
        if chat::is_user(m) {
            return;
        }
        let (cur, total) = chat::swipe_pos(m).unwrap_or((0, 1));
        if len == 1 && total > 1 {
            // Only the greeting so far: browse the alternate greetings,
            // wrapping around at both ends.
            let next = if dir < 0 {
                (cur + total - 1) % total
            } else {
                (cur + 1) % total
            };
            chat::set_swipe(m, next);
            self.set_status(format!("greeting {}/{}", next + 1, total), false);
            self.save().await;
            return;
        }
        if dir < 0 {
            if cur > 0 {
                chat::set_swipe(m, cur - 1);
                self.save().await;
            }
        } else if cur + 1 < total {
            chat::set_swipe(m, cur + 1);
            self.save().await;
        } else if len >= 2 {
            self.start_generation(true).await;
        } else if self.chat.messages.first().is_some_and(chat::is_summary_branch) {
            self.set_status(
                "the summary has no swipes \u{2014} Ctrl+\u{2190}/\u{2192} switches summary branch",
                false,
            );
        } else {
            self.set_status("no more greetings", false);
        }
    }

    // ----------------------------------------------------------- generation

    /// At start-up: say how to get going when there is no API connection yet.
    pub fn hint_if_no_connection(&mut self) {
        if self.conn.is_none() {
            self.set_status(NO_CONNECTION, false);
        }
    }

    /// The API manager (`/api`) was closed: use whatever it saved right away
    /// (title bar, token counts), not only from the next reply on.
    pub fn api_closed(&mut self, res: anyhow::Result<()>, rows: u16) {
        self.on_resize(rows);
        self.reload_connection();
        match res {
            Err(e) => self.set_status(format!("API settings: {:#}", e), true),
            Ok(()) => match &self.conn {
                Some(c) => {
                    let msg = format!("back in the chat, using API connection '{}'", c.name);
                    self.set_status(msg, false);
                }
                None => self.set_status(NO_CONNECTION, true),
            },
        }
    }

    /// The formatting editor (`/formatting`) was closed: use whatever it saved
    /// right away (prompt token counts), not only from the next reply on.
    pub fn formatting_closed(&mut self, res: anyhow::Result<()>, rows: u16) {
        self.on_resize(rows);
        formatting::normalize(&mut self.settings);
        self.settings_ver += 1;
        match res {
            Err(e) => self.set_status(format!("formatting: {:#}", e), true),
            Ok(()) => self.set_status("back in the chat, using your formatting".to_string(), false),
        }
    }

    /// Use `c` from now on (with `None` there is no way to generate).
    /// The title bar then shows the connection's backend and address.
    pub fn set_connection(&mut self, c: Option<conn::Connection>) {
        self.conn = c;
        self.health.reset();
        if let Some(c) = &self.conn {
            conn::overlay(&mut self.settings, c);
            self.server = host_of(&c.url);
        }
        let (source, model, label) = self.backend_labels(&self.settings);
        self.source = source;
        self.model = model;
        self.label = label;
        self.settings_ver += 1;
    }

    /// `(source, model, title-bar label)`: the standalone connection's, or a
    /// reminder that there is none.
    fn backend_labels(&self, settings: &Value) -> (String, String, String) {
        if self.conn.is_some() {
            backend_info(settings)
        } else {
            (String::new(), String::new(), "no API connection".to_string())
        }
    }

    /// Re-read `api.yaml` so changes made in `/api` (even in another
    /// terminal) apply to the next reply. An unreadable file keeps the
    /// connection as it was.
    fn reload_connection(&mut self) {
        let Ok(store) = conn::Store::load() else {
            return;
        };
        let fresh = store.active_connection();
        let changed = match (&self.conn, &fresh) {
            (None, None) => false,
            (Some(a), Some(b)) => {
                a.name != b.name
                    || a.kind != b.kind
                    || a.url != b.url
                    || a.api_key != b.api_key
                    || a.params != b.params
            }
            _ => true,
        };
        if changed {
            self.set_connection(fresh);
        }
    }

    /// Rebuild the settings the next reply is made from: the base settings
    /// with the current persona, system prompt, formatting and API connection
    /// written over them, so changes made in the windows (or by hand in the
    /// files, even from another terminal) apply to the next reply.
    async fn refresh_settings(&mut self) {
        self.reload_connection();
        self.apply_settings(config::base_settings());
    }

    fn apply_settings(&mut self, mut settings: Value) {
        persona::normalize(&mut settings);
        sysprompt::normalize(&mut settings);
        formatting::normalize(&mut settings);
        settings["author_note"] = json!(self.author_note);
        if let Some(c) = &self.conn {
            conn::overlay(&mut settings, c);
        }
        let p = persona::current();
        self.user_name = p.name;
        self.persona_title = p.title;
        if let Some(pu) = settings.get("power_user") {
            self.persona = jstr(pu, "persona_description");
            self.persona_position = ji64(pu, "persona_description_position", 0);
        }
        let (source, model, label) = self.backend_labels(&settings);
        self.source = source;
        self.model = model;
        self.label = label;
        self.settings = settings;
        self.settings_ver += 1;
    }

    /// The same prompt the Text Completion path builds, sent straight to the
    /// standalone connection with its own sampler settings.
    fn build_direct_request(
        &self,
        c: &conn::Connection,
        history_end: usize,
        continue_last: bool,
        nudge: Option<&str>,
    ) -> anyhow::Result<conn::DirectRequest> {
        let history = &self.chat.messages[..history_end];
        let (history, continue_text) = match history.split_last() {
            Some((last, rest)) if continue_last => (rest, Some(chat::mes(last))),
            _ => (history, None),
        };
        let note = Note::from_chat(&self.chat);
        let tp = textgen::build_prompt(&textgen::TgArgs {
            settings: &self.settings,
            ch: &self.character,
            user_name: &self.user_name,
            persona: &self.persona,
            persona_position: self.persona_position,
            note: &note,
            history,
            chars_per_token: self.chars_per_token,
            continue_text,
            nudge,
        });
        conn::build_request(c, &tp.prompt, &tp.stop)
    }

    /// Clean up a finished reply the way the web UI does.
    fn clean_reply(&self, text: &str) -> String {
        let mut out = chat::tidy(text);
        if jstr(&self.settings, "main_api") == "textgenerationwebui" {
            let name_prefix = format!("{}:", self.character.name);
            if let Some(rest) = out.strip_prefix(name_prefix.as_str()) {
                out = rest.trim_start().to_string();
            }
            if self.trims_sentences() {
                out = textgen::trim_to_end_sentence(&out);
            }
        }
        out
    }

    fn trims_sentences(&self) -> bool {
        let null = Value::Null;
        let pu = self.settings.get("power_user").unwrap_or(&null);
        pu.get("context")
            .map(|c| jbool(c, "trim_sentences", false))
            .unwrap_or(false)
            || jbool(pu, "trim_sentences", false)
    }

    /// The old text plus the new chunk, or `None` when nothing was added.
    fn clean_continuation(&self, orig: &str, chunk: &str) -> Option<String> {
        if chunk.trim().is_empty() {
            return None;
        }
        let mut out = chat::join_continuation(orig, chunk);
        if jstr(&self.settings, "main_api") == "textgenerationwebui" && self.trims_sentences() {
            out = textgen::trim_to_end_sentence(&out);
        }
        Some(out)
    }

    async fn start_generation(&mut self, replace_last: bool) {
        self.start_generation_mode(replace_last, false, false).await;
    }

    async fn start_generation_mode(&mut self, replace_last: bool, continue_last: bool, reset: bool) {
        // A nudge belongs to this one call, whether or not it gets to run.
        let nudge = self.next_nudge.take();
        if self.is_generating() {
            return;
        }
        if self.editing.is_some() {
            self.set_status(
                "finish editing first (Enter save \u{b7} Esc cancel)",
                false,
            );
            return;
        }
        self.refresh_settings().await;
        let history_end = if replace_last {
            self.chat.messages.len().saturating_sub(1)
        } else {
            self.chat.messages.len()
        };

        let continue_from = if continue_last {
            self.chat.messages.last().map(|m| chat::mes(m).to_string())
        } else {
            None
        };
        // Replies come only from the standalone connection (`/api`).
        let Some(c) = self.conn.clone() else {
            self.set_status(NO_CONNECTION, true);
            return;
        };
        let direct = match self.build_direct_request(&c, history_end, continue_last, nudge.as_deref()) {
            Ok(r) => r,
            Err(e) => {
                self.set_status(format!("{:#}", e), true);
                return;
            }
        };

        self.gen_id += 1;
        let id = self.gen_id;
        let tx = self.gen_tx.clone();
        self.gen_handle = Some(tokio::spawn(conn::run(direct, id, tx)));
        self.pending = Some(Pending {
            text: String::new(),
            replace_last,
            reset,
            started: chat::now_iso(),
            continue_from,
            summary: false,
        });
        self.scroll = 0;
        self.status = None;
    }

    pub async fn on_gen_event(&mut self, msg: GenMsg) {
        let (id, ev) = msg;
        if id != self.gen_id || self.pending.is_none() {
            return; // stale event from a stopped generation
        }
        match ev {
            GenEvent::Chunk(t) => {
                if let Some(p) = self.pending.as_mut() {
                    p.text.push_str(&t);
                }
            }
            GenEvent::Done => self.finish_generation(Outcome::Done).await,
            GenEvent::Error(e) => self.finish_generation(Outcome::Failed(e)).await,
        }
    }

    async fn cancel_generation(&mut self) {
        if let Some(h) = self.gen_handle.take() {
            h.abort();
        }
        // Any events still in flight belong to the old id and will be ignored.
        self.gen_id += 1;
        self.finish_generation(Outcome::Stopped).await;
    }

    async fn finish_generation(&mut self, outcome: Outcome) {
        let Some(p) = self.pending.take() else {
            return;
        };
        self.gen_handle = None;

        if p.summary {
            self.finish_summary_pass(p, outcome).await;
            return;
        }

        if let Some(orig) = p.continue_from.as_deref() {
            match self.clean_continuation(orig, &p.text) {
                None => match outcome {
                    Outcome::Done => self.set_status("the model had nothing to add", true),
                    Outcome::Stopped => self.set_status("stopped", false),
                    Outcome::Failed(e) => {
                        self.set_status(format!("generation failed: {}", e), true)
                    }
                },
                Some(new_text) => {
                    let finished = chat::now_iso();
                    let (new_text, closer) = prose::close_final(&new_text);
                    if !closer.is_empty() {
                        self.auto_closed = Some((self.chat.messages.len().saturating_sub(1), closer));
                    }
                    if let Some(m) = self.chat.messages.last_mut() {
                        chat::set_text(m, &new_text);
                        chat::touch_finished(m, &finished);
                    }
                    match outcome {
                        Outcome::Done => {}
                        Outcome::Stopped => {
                            self.set_status("stopped (partial continuation kept)", false)
                        }
                        Outcome::Failed(e) => self.set_status(
                            format!("interrupted: {} (partial continuation kept)", e),
                            true,
                        ),
                    }
                    self.save().await;
                }
            }
            return;
        }

        let text = self.clean_reply(&p.text);
        // Close markers the model left open (`*narration`, `"speech`).
        let (text, closer) = prose::close_final(&text);

        if text.is_empty() {
            match outcome {
                Outcome::Done => self.set_status("the model returned an empty reply", true),
                Outcome::Stopped => self.set_status("stopped", false),
                Outcome::Failed(e) => self.set_status(format!("generation failed: {}", e), true),
            }
            return;
        }

        let finished = chat::now_iso();
        let last_is_assistant = self
            .chat
            .messages
            .last()
            .map(|m| !chat::is_user(m))
            .unwrap_or(false);
        if p.replace_last && last_is_assistant {
            if let Some(m) = self.chat.messages.last_mut() {
                if p.reset {
                    chat::reset_swipes(m, &text, &self.model, &p.started, &finished);
                } else {
                    chat::add_swipe(m, &text, &self.model, &p.started, &finished);
                }
            }
        } else {
            self.chat.messages.push(chat::assistant_message(
                &self.character.name,
                &text,
                &self.model,
                &p.started,
                &finished,
            ));
        }

        self.auto_closed = if closer.is_empty() {
            None
        } else {
            Some((self.chat.messages.len().saturating_sub(1), closer))
        };

        match outcome {
            Outcome::Done => {}
            Outcome::Stopped => self.set_status("stopped (partial reply kept)", false),
            Outcome::Failed(e) => {
                self.set_status(format!("interrupted: {} (partial reply kept)", e), true)
            }
        }
        self.save().await;
    }

    // ------------------------------------------------------------ /summarize

    /// `/summarize`: write an in-depth summary of the whole chat. While it is
    /// written nothing is shown but the status row; when it is done it
    /// appears whole as a new summary branch of the chat (the summary is the only
    /// message), and branch 1 stays the conversation as it was. The chat is
    /// also saved whole under another name, and nothing changes if the
    /// summary is stopped or fails.
    async fn summarize(&mut self) {
        if self.one_shot_blocks("branching (summarize)") {
            return;
        }
        if self.is_generating() || self.editing.is_some() {
            self.set_status("finish what you are doing first, then /summarize", false);
            return;
        }
        let blocks =
            summary::transcript(&self.chat.messages, &self.user_name, &self.character.name);
        if blocks.len() < 3 {
            self.set_status("nothing to summarize yet", false);
            return;
        }
        // The context size and the connection may have changed since the last reply.
        self.refresh_settings().await;
        let ctx = ji64(&self.settings, "max_context", 8192).max(512) as usize;
        let plan = match summary::plan(&blocks, ctx, self.chars_per_token) {
            Ok(p) => p,
            Err(e) => {
                self.set_status(e, true);
                return;
            }
        };
        self.summary_job = Some(summary::Job {
            plan,
            count: blocks.len(),
            next: 0,
            so_far: String::new(),
        });
        self.start_summary_pass().await;
    }

    /// Start request `job.next` of the running `/summarize`.
    async fn start_summary_pass(&mut self) {
        let (part, so_far, target, pass, passes) = match self.summary_job.as_ref() {
            Some(j) if j.next < j.plan.parts.len() => (
                j.plan.parts[j.next].clone(),
                j.so_far.clone(),
                j.plan.target,
                j.next,
                j.plan.parts.len(),
            ),
            _ => {
                self.summary_job = None;
                return;
            }
        };
        let Some(c) = self.conn.clone() else {
            self.summary_job = None;
            self.set_status(NO_CONNECTION, true);
            return;
        };
        let pt = summary::pass_target(target, pass, passes);
        let body = summary::body(
            &self.user_name,
            &self.character.name,
            &part,
            &so_far,
            summary::words_for(pt),
            pass,
            passes,
        );
        let tp = textgen::build_summary_prompt(
            &self.settings,
            &self.user_name,
            &self.character.name,
            &body,
        );
        let c = conn::for_summary(&c, summary::reply_cap(pt));
        let direct = match conn::build_request(&c, &tp.prompt, &tp.stop) {
            Ok(r) => r,
            Err(e) => {
                self.summary_job = None;
                self.set_status(format!("{:#}", e), true);
                return;
            }
        };
        self.gen_id += 1;
        let id = self.gen_id;
        let tx = self.gen_tx.clone();
        self.gen_handle = Some(tokio::spawn(conn::run(direct, id, tx)));
        self.pending = Some(Pending {
            text: String::new(),
            replace_last: false,
            reset: false,
            started: chat::now_iso(),
            continue_from: None,
            summary: true,
        });
        self.scroll = 0;
        self.status = None;
    }

    /// A request of `/summarize` ended: go on with the next one, or put the
    /// finished summary into the chat.
    async fn finish_summary_pass(&mut self, p: Pending, outcome: Outcome) {
        let Some(mut job) = self.summary_job.take() else {
            return;
        };
        match outcome {
            Outcome::Done => {}
            Outcome::Stopped => {
                self.set_status("summary stopped \u{2014} the chat is unchanged", false);
                return;
            }
            Outcome::Failed(e) => {
                self.set_status(
                    format!("summary failed: {} \u{2014} the chat is unchanged", e),
                    true,
                );
                return;
            }
        }
        let text = self.clean_summary(&p.text);
        if text.is_empty() {
            self.set_status(
                "the model returned an empty summary \u{2014} the chat is unchanged",
                true,
            );
            return;
        }
        job.so_far = text;
        job.next += 1;
        if job.next < job.plan.parts.len() {
            self.summary_job = Some(job);
            self.start_summary_pass().await;
        } else {
            self.apply_summary(job, p.started).await;
        }
    }

    /// Tidy a finished summary: no name cue in front, and nothing after the
    /// last full sentence (a reply that hit its length limit stops mid-sentence).
    fn clean_summary(&self, text: &str) -> String {
        let mut out = chat::tidy(text);
        let cue = format!("{}:", self.character.name);
        if let Some(rest) = out.strip_prefix(cue.as_str()) {
            out = rest.trim_start().to_string();
        }
        if let Some(rest) = out.strip_prefix("SUMMARY:") {
            out = rest.trim_start().to_string();
        }
        let out = textgen::trim_to_end_sentence(&out);
        let (out, _) = prose::close_final(&out);
        out
    }

    /// Put the finished summary into the chat as its summary branch (see `summarize`).
    async fn apply_summary(&mut self, job: summary::Job, started: String) {
        let summary::Job {
            plan,
            count,
            so_far,
            ..
        } = job;
        let before = plan.before;
        let after = summary::est_tokens(so_far.chars().count(), self.chars_per_token);
        if after >= before {
            self.set_status(
                "the summary is not shorter than the chat \u{2014} the chat is unchanged",
                true,
            );
            return;
        }
        // Keep the whole chat under another name first: a summary can lose
        // details, the story must not be lost with them.
        let backup = match self.backup_chat().await {
            Ok(name) => name,
            Err(e) => {
                self.set_status(
                    format!("could not keep a copy of the chat, so nothing was changed: {:#}", e),
                    true,
                );
                return;
            }
        };
        let finished = chat::now_iso();
        self.chat.messages = chat::with_summary_branch(
            &self.chat.messages,
            &self.character.name,
            &so_far,
            &self.model,
            &started,
            &finished,
        );
        self.auto_closed = None;
        self.scroll = 0;
        self.reveal = Some(0);
        self.save().await;
        let cut = 100usize.saturating_sub(after * 100 / before.max(1));
        let branch_no = self
            .chat
            .messages
            .first()
            .and_then(chat::branch_pos)
            .map(|(a, _)| a + 1)
            .unwrap_or(2);
        self.set_status(
            format!(
                "summarized {} messages: ~{} \u{2192} ~{} tokens (-{}%). Added as summary branch {}/{}; branch 1 is the whole conversation, and a copy is saved as {}",
                count,
                summary::fmt_tokens(before),
                summary::fmt_tokens(after),
                cut,
                branch_no,
                branch_no,
                backup
            ),
            false,
        );
    }

    /// Save the chat as it is now under a new name (`<chat>_before-summary`,
    /// then `...2`, `...3`), and return that name.
    async fn backup_chat(&self) -> anyhow::Result<String> {
        let existing: Vec<String> = chatstore::list(&self.chat.avatar)?
            .iter()
            .map(chat::file_id)
            .collect();
        let base = format!("{}_before-summary", self.chat.file);
        let mut name = base.clone();
        let mut n = 2;
        while existing.iter().any(|e| e.eq_ignore_ascii_case(&name)) {
            name = format!("{}{}", base, n);
            n += 1;
        }
        chatstore::save(&self.chat.avatar, &name, &self.chat.copy_lines())?;
        Ok(name)
    }

    // --------------------------------------------------------------- saving

    pub async fn save(&mut self) {
        let lines = self.chat.lines();
        let result = chatstore::save(&self.chat.avatar, &self.chat.file, &lines);
        if let Err(e) = result {
            self.set_status(format!("{:#}", e), true);
        }
    }
}

/// `<leader>y`: put `text` on the system clipboard; the status-line message
/// and whether it is an error.
fn copy_to_clipboard(text: &str) -> (String, bool) {
    let n = text.chars().count();
    match crate::clipboard::copy(text) {
        Ok(how) => (format!("yanked {} chars to the clipboard ({})", n, how), false),
        Err(e) => (e, true),
    }
}
