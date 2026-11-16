//! The plain data types of the chat screen: what a pending reply, an edit, a
//! window or the completion menu is. `App` itself is in `mod.rs`.

use std::time::{Duration, Instant};

/// Shown when a reply is asked for without an API connection.
pub(super) const NO_CONNECTION: &str =
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
pub(super) enum GreetingSlot {
    /// The card's `first_mes`.
    First,
    /// `alternate_greetings[n]`, counting only the non-empty ones (the chat
    /// is built from that list too, see `make_greeting`).
    Alt(usize),
}

/// A message being edited in the input box.
pub(super) struct Editing {
    /// Index into `chat.messages`.
    pub(super) idx: usize,
    /// What was in the input box before the edit started (text, cursor).
    pub(super) draft: (String, usize),
    /// Which card greeting this message is (decided when the edit starts,
    /// from the text it had then), or `None` when it is not one.
    pub(super) slot: Option<GreetingSlot>,
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
    /// The `choose a character` window, as at start-up (the character's last
    /// chat was deleted, or the character of this chat was).
    Pick,
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
pub(super) const NEW_CHAT: &str = "\u{0}new";

/// Key of the "new persona" row of the `/persona` window.
pub(super) const NEW_PERSONA: &str = "\u{0}new";

/// Key of the "new system prompt" row of the `/system` window.
pub(super) const NEW_SYSPROMPT: &str = "\u{0}new";

/// The persona form (`App::card_modal`) is open for this persona.
pub(super) struct PersonaEdit {
    /// `None`: a new persona is being made.
    pub(super) avatar: Option<String>,
}

/// The `/prompt` window: the prompt of the next reply, as text, on two
/// pages: the pre-prompt (everything before the messages) and the
/// post-prompt (the last `textgen::POST_MESSAGES` messages and the cue).
pub struct PromptView {
    /// The two pages, tidied for the screen (no tabs or control characters).
    pub texts: [String; 2],
    /// The page showing (0 pre-prompt, 1 post-prompt).
    pub tab: usize,
    /// Window title: size of the whole prompt.
    pub title: String,
    /// First visible row (`ui::draw_prompt_modal` keeps it in range).
    pub scroll: usize,
    /// Text rows the window shows, for PgUp / PgDn (set when drawing).
    pub page: usize,
    /// `text` cut into screen rows for the width in `wrapped_w`.
    pub wrapped: Vec<String>,
    pub wrapped_w: usize,
}

impl PromptView {
    /// Show page `tab` (from its top); the same page again changes nothing.
    pub fn set_tab(&mut self, tab: usize) {
        if self.tab != tab {
            self.tab = tab;
            self.scroll = 0;
            self.wrapped.clear();
        }
    }
}

/// The system prompt form (`App::card_modal`) is open for this prompt.
pub(super) struct SysEdit {
    /// `None`: a new system prompt is being made.
    pub(super) id: Option<String>,
}

pub(super) enum Outcome {
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
pub(super) struct Comp {
    /// What was typed before Tab / the arrows started filling commands in
    /// (without the slash). `None`: nothing has been picked from the menu.
    pub(super) base: Option<String>,
    /// Which entry of the menu for `base` is filled in.
    pub(super) sel: usize,
    /// A command was just chosen with the mouse: the menu stays shut until
    /// the next key.
    pub(super) done: bool,
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
pub(crate) const SPELL_DELAY: Duration = Duration::from_millis(1000);

/// A run of mouse-wheel events from one finger drag (see `App::wheel_gesture`).
pub(super) struct WheelBurst {
    pub(super) last: Instant,
    pub(super) x0: u16,
    pub(super) y0: u16,
    /// Chat scroll position before the drag started.
    pub(super) scroll0: usize,
    pub(super) fired: bool,
}

/// What to do with one wheel event.
pub(super) enum WheelAction {
    Scroll,
    Ignore,
    Swipe(i32),
}
