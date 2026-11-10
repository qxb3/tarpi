use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

/// Settings read from `~/.config/tarpi/config.yaml`.
///
/// Every key is optional: whatever is missing keeps its default. An older
/// `config.toml` (same keys) is still read when there is no `config.yaml`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FileConfig {
    /// Base URL of a SillyTavern server, e.g. "http://127.0.0.1:8000". Only
    /// `tarpi import-st` uses it (as its default); chatting never contacts it.
    pub url: Option<String>,
    /// Rough characters-per-token ratio used when trimming old history to fit
    /// the context window. Lower = more conservative. Default 3.2.
    pub chars_per_token: Option<f32>,
    /// Auto-close `"`, `'`, `*` and brackets while typing in the input box.
    /// Default true.
    pub autopair: Option<bool>,
    /// The menu of `/commands` that opens above the input box while a command
    /// is typed (Tab / arrows / Enter pick from it). Default true.
    pub command_menu: Option<bool>,
    /// Offline spell checking in the chat input box: misspelled words get a
    /// red underline. Default false (off).
    pub spellcheck: Option<bool>,
    mouse: Option<MouseConfig>,
    vim: Option<VimConfig>,
}

/// `mouse:` section.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MouseConfig {
    /// Phones (Termux): tapping the input box opens the on-screen keyboard.
    /// Mouse reporting is switched off for a moment so the terminal sees the
    /// tap itself. Has no effect on desktop terminals.
    pub open_keyboard_on_tap: bool,
    /// Clicking / tapping inside the input box moves the text cursor there.
    pub tap_to_move_cursor: bool,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            open_keyboard_on_tap: true,
            tap_to_move_cursor: true,
        }
    }
}

/// `vim:` section.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct VimConfig {
    /// `true`: vim-style INSERT / NORMAL / VISUAL modes in the input box.
    /// `false`: plain text editing, the box is always in "typing" mode.
    pub enabled: bool,
    /// Keys that, typed quickly one after the other in INSERT mode, switch to
    /// NORMAL mode. Two or more characters, e.g. `qq` or `jk`.
    pub normal_mode_keys: String,
    /// The `<leader>` key of the NORMAL / VISUAL mappings (`<leader>y` copies
    /// to the system clipboard). One character, or `space`. Default space.
    pub leader: String,
}

impl Default for VimConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            normal_mode_keys: "qq".to_string(),
            leader: "space".to_string(),
        }
    }
}

impl VimConfig {
    /// The NORMAL-mode key sequence as characters (already validated by `load`).
    pub fn keys(&self) -> Vec<char> {
        self.normal_mode_keys.trim().chars().collect()
    }
}

impl VimConfig {
    /// The leader key: a single character, or `space` / `<space>`.
    pub fn leader_char(&self) -> Option<char> {
        let l = self.leader.as_str();
        if l.eq_ignore_ascii_case("space") || l.eq_ignore_ascii_case("<space>") || l == " " {
            return Some(' ');
        }
        let mut it = l.chars();
        match (it.next(), it.next()) {
            (Some(c), None) if !c.is_alphanumeric() && !c.is_control() => Some(c),
            _ => None,
        }
    }
}

impl FileConfig {
    pub fn mouse(&self) -> MouseConfig {
        self.mouse.clone().unwrap_or_default()
    }

    pub fn vim(&self) -> VimConfig {
        self.vim.clone().unwrap_or_default()
    }
}

/// A commented starting point, written the first time tarpi runs without
/// any config file.
const TEMPLATE: &str = "\
# tarpi configuration. Every option is optional; the values below are the defaults.

# SillyTavern server, used only by `tarpi import-st` when no address is given.
# Chatting never contacts it.
# url: http://127.0.0.1:8000

# Characters per token, used to trim old chat history to fit the context window.
# Lower is more conservative.
# chars_per_token: 3.2

# Auto-close quotes, *asterisks* and brackets while typing: \" gives \"|\",
# ' gives '|', * gives *|*, ** gives **|**, ( gives (|). Typing the closer steps
# over it, and Backspace between a pair removes both. Stays off next to words
# (2*3, 5\", don't).
# autopair: true

# The menu of /commands that opens above the input box when you type a
# slash: the top entry is highlighted, Tab / Down and Shift+Tab / Up move
# through it, Enter runs the highlighted one. false: no menu, and Enter runs
# exactly what you typed.
# command_menu: true

# Spell checking in the input box: misspelled words get a red underline (the
# red of your terminal's theme). Works offline with a built-in English word
# list; your own words can go in ~/.config/tarpi/dictionary.txt (one per
# line), and the character's and your persona's names are always accepted.
# Off by default.
# spellcheck: false

mouse:
  # Phones (Termux): tapping the input box opens the on-screen keyboard.
  # (tarpi turns mouse reporting off for a moment so the tap reaches the
  # terminal.) Does nothing on desktop terminals.
  open_keyboard_on_tap: true
  # Clicking / tapping in the input box moves the text cursor to that spot.
  tap_to_move_cursor: true

vim:
  # true: vim-style INSERT / NORMAL / VISUAL modes in the input box.
  # false: normal text editing (arrow keys, Home/End, ...), no modes.
  enabled: true
  # Typed quickly in INSERT mode, these keys switch to NORMAL mode.
  # Two or more characters, for example qq or jk. Ignored when vim is disabled.
  normal_mode_keys: qq
  # The <leader> key for NORMAL / VISUAL mode mappings: one character
  # (for example a comma) or the word space. <leader>y copies to the
  # system clipboard (<leader>yy a line, <leader>y{motion}, or on a VISUAL
  # selection); plain y only fills vim's own register.
  leader: space
";

fn dir() -> Option<PathBuf> {
    let base = dirs::config_dir()?;
    let dir = base.join("tarpi");
    // The folder of the project's earlier name: moved over once, so nothing
    // (characters, chats, settings) is lost by the rename.
    let old = base.join("st-cli");
    if !dir.exists() && old.is_dir() && std::fs::rename(&old, &dir).is_err() {
        return Some(old);
    }
    Some(dir)
}

pub fn config_path() -> Option<PathBuf> {
    dir().map(|d| d.join("config.yaml"))
}

/// `~/.config/tarpi/dictionary.txt`: extra words the spell checker accepts.
pub fn dictionary_path() -> Option<PathBuf> {
    dir().map(|d| d.join("dictionary.txt"))
}

/// `~/.config/tarpi/command_history`: the `/commands` typed in chats (see `history`).
pub fn history_path() -> Option<PathBuf> {
    dir().map(|d| d.join("command_history"))
}

/// `~/.config/tarpi/api.yaml`: the standalone API connections (see `conn`).
pub fn api_path() -> Option<PathBuf> {
    dir().map(|d| d.join("api.yaml"))
}

/// `~/.config/tarpi/formatting.yaml`: the context and instruct templates (see `formatting`).
pub fn formatting_path() -> Option<PathBuf> {
    dir().map(|d| d.join("formatting.yaml"))
}

/// `~/.config/tarpi/persona/`: one `<id>.yaml` per persona, plus `active` (see `persona`).
pub fn persona_dir() -> Option<PathBuf> {
    dir().map(|d| d.join("persona"))
}

/// `~/.config/tarpi/system-prompt.jsonl`: the system prompts, one JSON object per line (see `sysprompt`).
pub fn sysprompt_path() -> Option<PathBuf> {
    dir().map(|d| d.join("system-prompt.jsonl"))
}

/// `~/.config/tarpi/characters/`: the character cards, `.png` (card embedded) or `.json` (see `charstore`).
pub fn characters_dir() -> Option<PathBuf> {
    dir().map(|d| d.join("characters"))
}

/// `~/.config/tarpi/chats/`: one folder per character, one `.jsonl` per chat (see `chatstore`).
pub fn chats_dir() -> Option<PathBuf> {
    dir().map(|d| d.join("chats"))
}

fn legacy_path() -> Option<PathBuf> {
    dir().map(|d| d.join("config.toml"))
}

pub fn load() -> Result<FileConfig> {
    let Some(yaml) = config_path() else {
        return Ok(FileConfig::default());
    };
    let cfg = if yaml.exists() {
        let text = std::fs::read_to_string(&yaml)
            .with_context(|| format!("reading {}", yaml.display()))?;
        // An empty (or all-comment) file parses as "nothing set".
        if text.trim().is_empty() || text.lines().all(|l| l.trim().is_empty() || l.trim_start().starts_with('#')) {
            FileConfig::default()
        } else {
            serde_yaml::from_str(&text).with_context(|| format!("parsing {}", yaml.display()))?
        }
    } else if let Some(toml_path) = legacy_path().filter(|p| p.exists()) {
        let text = std::fs::read_to_string(&toml_path)
            .with_context(|| format!("reading {}", toml_path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", toml_path.display()))?
    } else {
        // First run: leave a documented config behind. Failing to write it
        // (read-only home, ...) is not a problem.
        if let Some(parent) = yaml.parent() {
            let _ = std::fs::create_dir_all(parent);
            let _ = std::fs::write(&yaml, TEMPLATE);
        }
        FileConfig::default()
    };

    let keys = cfg.vim().keys();
    if cfg.vim().enabled && (keys.len() < 2 || keys.iter().any(|c| c.is_whitespace())) {
        bail!(
            "{}: vim.normal_mode_keys must be two or more characters without spaces \
             (for example qq or jk), got '{}'",
            yaml.display(),
            cfg.vim().normal_mode_keys
        );
    }
    if cfg.vim().enabled && cfg.vim().leader_char().is_none() {
        bail!(
            "{}: vim.leader must be a single character that is not a letter or digit \
             (for example , or \\), or the word space, got '{}'",
            yaml.display(),
            cfg.vim().leader
        );
    }
    Ok(cfg)
}

/// The settings the prompt builders start from, before tarpi's own persona,
/// system prompt, formatting and API connection are written over them. They
/// live in memory only; nothing is read from or sent to any server.
pub fn base_settings() -> serde_json::Value {
    serde_json::json!({
        "main_api": "textgenerationwebui",
        "max_context": 8192,
        "amount_gen": 350,
        "power_user": {},
    })
}
