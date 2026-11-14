//! Application state and input handling.
//!
//! `App` is the state of the chat screen; everything it can do is spread over
//! the modules below, each one an `impl App` block for one topic (the struct
//! and its constructor are here, the small data types in `types`).
//!
//! | module | what it handles |
//! | --- | --- |
//! | `status` | labels and token statistics for the title and status rows |
//! | `lifecycle` | tick, resize, mouse reporting, leaving, quitting, saving |
//! | `events`, `mouse` | keyboard, and mouse / touch input |
//! | `render` | the chat as lines for the UI |
//! | `cmdline`, `commands` | the input box: completion and history, then sending |
//! | `editing`, `greetings` | editing messages; the card's greetings |
//! | `chat_ops` | one-shot, undo, new / delete chat, regenerate, branches, swipes |
//! | `pickers`, `persona_ui`, `sysprompt_ui`, `prompt_view` | the choose-one windows |
//! | `card_modal`, `note_config` | the `/char`, `/char-img`, `/note`, `/memory` and `/config` windows |
//! | `import_flow` | `/import` and the lorebook question after it |
//! | `spellcheck` | spell checking of the input box |
//! | `connection`, `generation` | the API connection, and writing replies |
//! | `summarize` | `/summarize` |
//!
//! The child modules see everything private in this file (`use super::*`),
//! so the fields of `App` stay private to the `app` module. Methods that
//! another module here calls are `pub(super)`.

mod card_modal;
mod chat_ops;
mod cmdline;
mod commands;
mod connection;
mod editing;
mod events;
mod generation;
mod greetings;
mod import_flow;
mod lifecycle;
mod mouse;
mod note_config;
mod persona_ui;
mod pickers;
mod prompt_view;
mod render;
mod spellcheck;
mod status;
mod summarize;
mod sysprompt_ui;
mod types;

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
use crate::loreask::{Action as LoreAskAction, LoreAsk};
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
use futures_util::FutureExt;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::text::Line;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};
use tokio::{sync::mpsc::UnboundedSender, task::JoinHandle};

// What the rest of the program uses from `app`.
pub use self::events::copy_to_clipboard;
pub use self::greetings::{make_greeting, sync_greetings};
pub use self::types::{ChatStats, Exit, PickKind, Pending, PromptView, SpellMenu};
pub(crate) use self::types::SPELL_DELAY;
// For the child modules, which pick these up with `use super::*`.
use self::connection::backend_info;
use self::types::*;

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
    /// Tokens the backend reported for the reply that just finished (prompt
    /// plus reply), until `update_stats` pins them to the chat as it is then.
    reported_pending: Option<usize>,
    /// `(prompt_fingerprint, tokens)`: the backend's own count, valid while
    /// the chat is exactly as it was when the reply finished.
    reported: Option<(u64, usize)>,
    /// A running `/summarize` (its passes are generated one after the other).
    summary_job: Option<summary::Job>,
    /// A running `/import` (a download can take a while) and when it started.
    import_job: Option<(JoinHandle<anyhow::Result<charstore::Saved>>, Instant)>,
    /// Said on the terminal once the chat screen has closed (see `main`).
    pub exit_note: Option<String>,
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
    /// Start-up without a character on the command line: the chat screen is
    /// only the setting for the `/chars` window. Nothing of it is saved, no
    /// character counts as the current one, choosing always leaves the screen
    /// (to chat with that character) and closing the window quits tarpi.
    pub startup_pick: bool,
    /// The position window of the `/persona` window's `p` key, if open.
    pub pos_edit: Option<PosEdit>,
    /// The question asked after `/import` when the new card has a lorebook:
    /// copy it into `/lore` and link it to the character?
    pub lore_ask: Option<LoreAsk>,
    /// Questions that came up while another window was open; asked one at a
    /// time, as soon as nothing else is in the way.
    lore_pending: Vec<LoreAsk>,
    /// Set while `card_modal` is the persona form, not the character card.
    persona_edit: Option<PersonaEdit>,
    /// Set while `card_modal` is the system prompt form.
    sys_edit: Option<SysEdit>,
    /// Set while `card_modal` is the Author's Note form (`/note`).
    note_edit: bool,
    /// Set while `card_modal` is the vector memory form (`/memory`).
    vector_edit: bool,
    /// Set while `card_modal` is the `/config` window (config.yaml).
    pub config_edit: bool,
    /// `/one-shot` was typed once and waits for the second time.
    one_shot_armed: bool,
    /// Set by `/api`: the main loop opens the API manager in this terminal and
    /// then continues with this same screen.
    pub open_api: bool,
    /// Set by `/formatting`: the main loop then opens the formatting editor.
    pub open_formatting: bool,
    /// Set by `/lore`: the main loop then opens the lorebook manager.
    pub open_lore: bool,
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
    /// The same for the `/import` path menu: the screen row of each entry
    /// and its index in the list `path_completion` gives.
    pub path_rows: Vec<(u16, usize)>,
    /// The last folder listing of the path menu (see `pathcomp::Cache`).
    path_cache: crate::pathcomp::Cache,
    /// The path menu is open: Tab opened it after `/import ` (see
    /// `path_key`). It closes when the box stops being an `/import` path, on
    /// Esc / Ctrl+C and when a file is picked.
    path_open: bool,
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
    /// The character of the line between messages (config `msg_seperator`);
    /// `None`: an empty row.
    pub msg_sep: Option<char>,
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
            import_job: None,
            exit_note: None,
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
            open_lore: false,
            show_help: false,
            prompt_view: None,
            card_modal: None,
            image_modal: None,
            picker: None,
            startup_pick: false,
            pos_edit: None,
            lore_ask: None,
            lore_pending: Vec::new(),
            persona_edit: None,
            sys_edit: None,
            note_edit: false,
            vector_edit: false,
            config_edit: false,
            one_shot_armed: false,
            help_scroll: 0,
            help_page: 10,
            editing: None,
            tokens: tokens::Counter::default(),
            reported_pending: None,
            reported: None,
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
            spell: None,
            spell_menu: None,
            spell_area: Rect::default(),
            spell_rows: Vec::new(),
            spell_key: 0,
            spell_since: Instant::now(),
            comp_area: Rect::default(),
            comp_rows: Vec::new(),
            path_rows: Vec::new(),
            path_cache: crate::pathcomp::Cache::default(),
            path_open: false,
            input_first_row: 0,
            input_scroll: None,
            press: None,
            vim: true,
            tap_move_cursor: true,
            tap_open_keyboard: true,
            msg_sep: Some('\u{254C}'),
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
}
