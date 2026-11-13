use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use unicode_width::UnicodeWidthChar;

/// Settings read from `~/.config/tarpi/config.yaml`.
///
/// Every key is optional: whatever is missing keeps its default. An older
/// `config.toml` (same keys) is still read when there is no `config.yaml`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FileConfig {
    /// Rough characters-per-token ratio used when trimming old history to fit
    /// the context window. Lower = more conservative. Default 3.2.
    pub chars_per_token: Option<f32>,
    /// Auto-close `"`, `'`, `*` and brackets while typing in the input box.
    /// Default true.
    pub autopair: Option<bool>,
    /// The menu of `/commands` that opens above the input box while a command
    /// is typed (Tab / arrows / Enter pick from it). Default true.
    pub command_menu: Option<bool>,
    /// Offline spell checking in the chat input box and the text windows
    /// (`/char`, `/persona`, `/lore`): misspelled words get a red underline.
    /// Default false (off).
    pub spellcheck: Option<bool>,
    /// Address of the machine that runs `tarpi serve`, e.g. "192.168.1.20:7878"
    /// (port 7878 if left out). `tarpi connect` uses it. Default: none.
    pub serve_url: Option<String>,
    /// `true`: no token is needed between `tarpi serve` and `tarpi connect`.
    /// On the serving machine, `serve` lets every client in; on the other
    /// device, `connect` does not ask for a token. `false` (default): the
    /// token is required.
    pub serve_trust_all: Option<bool>,
    mouse: Option<MouseConfig>,
    vim: Option<VimConfig>,
    /// `msg_seperator:` section (`msg_separator:` is read too).
    #[serde(rename = "msg_seperator", alias = "msg_separator")]
    msg_separator: Option<MsgSeparatorConfig>,
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

/// `msg_seperator:` section: the rule drawn between two messages.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MsgSeparatorConfig {
    /// `true`: a faint line of `char` under every message. `false`: just an
    /// empty row between messages.
    pub enabled: bool,
    /// The character the line is made of (one character).
    #[serde(rename = "char")]
    pub ch: String,
}

impl Default for MsgSeparatorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ch: "\u{254C}".to_string(),
        }
    }
}

impl MsgSeparatorConfig {
    /// The character of the line: exactly one visible character (not a
    /// control or zero-width one).
    pub fn glyph(&self) -> Option<char> {
        let mut it = self.ch.chars();
        match (it.next(), it.next()) {
            (Some(c), None) if !c.is_control() && c.width().unwrap_or(0) > 0 => Some(c),
            _ => None,
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

    pub fn msg_separator(&self) -> MsgSeparatorConfig {
        self.msg_separator.clone().unwrap_or_default()
    }
}

/// A commented starting point, written the first time tarpi runs without
/// any config file.
const TEMPLATE: &str = "\
# tarpi configuration. Every option is optional; the values below are the defaults.

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
# through it, Enter chooses the highlighted one (and adds a space). false: no
# menu, and Enter runs exactly what you typed.
# command_menu: true

# Spell checking in the input box and the /char, /persona and /lore windows:
# misspelled words get a red underline (the
# red of your terminal's theme). Works offline with a built-in English word
# list; your own words can go in ~/.config/tarpi/dictionary.txt (one per
# line), and the character's and your persona's names are always accepted.
# Off by default.
# spellcheck: false

# Sharing between devices (`tarpi serve` / `tarpi connect`).
# serve_url: the machine that runs `tarpi serve`, for `tarpi connect` on this
# device (host:port; the port may be left out for 7878).
# serve_url: 192.168.1.20:7878
# serve_trust_all: true needs no token: `tarpi serve` (on the serving machine)
# lets every client in without one, and `tarpi connect` finds that out from the
# server and does not ask for one (it only matters on the serving machine).
# Anyone who can reach the port can then read and change everything shared,
# API keys included, so only use it on a network you fully trust.
# false: the token `tarpi serve` prints is required.
# serve_trust_all: false

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

msg_seperator:
  # true: a faint line runs under every message, made of the character below.
  # false: no line, only an empty row between messages.
  enabled: true
  # The character the line is made of: exactly one character. Put it in quotes
  # (YAML reads a bare - # * or ' as something else).
  char: \"\u{254C}\"
";

/// `~/.config/tarpi`: where the settings that belong to this machine live
/// (`config.yaml`, `command_history`, `dictionary.txt`) and, normally, the data.
fn dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("tarpi"))
}

/// Set by `tarpi connect`: the folder that holds a copy of the server's data.
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
/// `tarpi connect --local-api`: `api.yaml` stays on this machine.
static API_LOCAL: AtomicBool = AtomicBool::new(false);

/// Where the shared data lives (characters, chats, personas, system prompts,
/// formatting, API connections): `~/.config/tarpi`, unless `tarpi connect` has
/// pointed it at its copy of a server's data (see `connect`).
fn data() -> Option<PathBuf> {
    DATA_DIR.get().cloned().or_else(dir)
}

/// `tarpi connect`: use `path` instead of `~/.config/tarpi` for the shared data.
/// Only the first call counts; call it before anything reads the data.
pub fn set_data_dir(path: PathBuf) {
    let _ = DATA_DIR.set(path);
}

/// `tarpi connect --local-api`: keep `api.yaml` in `~/.config/tarpi` instead of syncing it.
pub fn set_api_local(local: bool) {
    API_LOCAL.store(local, Ordering::Relaxed);
}

/// `~/.config/tarpi` itself, whatever `set_data_dir` says.
pub fn local_dir() -> Option<PathBuf> {
    dir()
}

/// The folder the shared data is read from and written to (what `tarpi serve` serves).
pub fn data_root() -> Option<PathBuf> {
    data()
}

/// `~/.config/tarpi/serve-token`: the secret `tarpi connect` must present to `tarpi serve`.
pub fn serve_token_path() -> Option<PathBuf> {
    dir().map(|d| d.join("serve-token"))
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
    if API_LOCAL.load(Ordering::Relaxed) {
        dir().map(|d| d.join("api.yaml"))
    } else {
        data().map(|d| d.join("api.yaml"))
    }
}

/// `~/.config/tarpi/formatting.yaml`: the context and instruct templates (see `formatting`).
/// Always this device's own: `tarpi serve` / `connect` do not share it.
pub fn formatting_path() -> Option<PathBuf> {
    dir().map(|d| d.join("formatting.yaml"))
}

/// `~/.config/tarpi/persona/`: one `<id>.yaml` per persona, plus `active` (see `persona`).
pub fn persona_dir() -> Option<PathBuf> {
    data().map(|d| d.join("persona"))
}

/// `~/.config/tarpi/lorebooks/`: one `<id>.json` per standalone lorebook (see `lorestore`).
pub fn lorebooks_dir() -> Option<PathBuf> {
    data().map(|d| d.join("lorebooks"))
}

/// `~/.config/tarpi/system-prompt.jsonl`: the system prompts, one JSON object per line (see `sysprompt`).
pub fn sysprompt_path() -> Option<PathBuf> {
    data().map(|d| d.join("system-prompt.jsonl"))
}

/// `~/.config/tarpi/characters/`: the character cards, `.png` (card embedded) or `.json` (see `charstore`).
pub fn characters_dir() -> Option<PathBuf> {
    data().map(|d| d.join("characters"))
}

/// `~/.config/tarpi/chats/`: one folder per character, one `.jsonl` per chat (see `chatstore`).
pub fn chats_dir() -> Option<PathBuf> {
    data().map(|d| d.join("chats"))
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

    validate(&cfg, &yaml)?;
    Ok(cfg)
}

/// The checks `load` makes on a parsed config (`yaml` is only named in the errors).
fn validate(cfg: &FileConfig, yaml: &std::path::Path) -> Result<()> {
    let keys = cfg.vim().keys();
    if cfg.vim().enabled && (keys.len() < 2 || keys.iter().any(|c| c.is_whitespace())) {
        bail!(
            "{}: vim.normal_mode_keys must be two or more characters without spaces \
             (for example qq or jk), got '{}'",
            yaml.display(),
            cfg.vim().normal_mode_keys
        );
    }
    if cfg.msg_separator().enabled && cfg.msg_separator().glyph().is_none() {
        bail!(
            "{}: msg_seperator.char must be exactly one visible character \
             (for example \u{254C} or -), got '{}'",
            yaml.display(),
            cfg.msg_separator().ch
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
    Ok(())
}

/// `/config`: the text of `config.yaml` as it is now (the commented starting
/// point when there is no file yet).
pub fn read_text() -> String {
    config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_else(|| TEMPLATE.to_string())
}

/// `/config`: check `text` the way `load` checks the file, without writing
/// anything. The error says what is wrong (YAML mistake, bad `vim` value).
pub fn parse_text(text: &str) -> Result<FileConfig> {
    let name = std::path::Path::new("config.yaml");
    let cfg = if text.trim().is_empty()
        || text.lines().all(|l| l.trim().is_empty() || l.trim_start().starts_with('#'))
    {
        FileConfig::default()
    } else {
        serde_yaml::from_str(text).with_context(|| format!("parsing {}", name.display()))?
    };
    validate(&cfg, name)?;
    Ok(cfg)
}

/// `/config`: write `text` as `config.yaml`.
pub fn write_text(text: &str) -> Result<()> {
    let path = config_path().context("no config folder")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_is_valid_and_holds_the_defaults() {
        let cfg = parse_text(TEMPLATE).unwrap();
        let sep = cfg.msg_separator();
        assert!(sep.enabled);
        assert_eq!(sep.ch, "\u{254C}");
        assert_eq!(sep.glyph(), Some('\u{254C}'));
        // Same as leaving the section out.
        assert_eq!(MsgSeparatorConfig::default().ch, sep.ch);
    }

    #[test]
    fn the_separator_defaults_when_missing_or_partly_given() {
        let cfg = parse_text("autopair: false\n").unwrap();
        assert!(cfg.msg_separator().enabled);
        assert_eq!(cfg.msg_separator().glyph(), Some('\u{254C}'));
        let cfg = parse_text("msg_seperator:\n  enabled: false\n").unwrap();
        assert!(!cfg.msg_separator().enabled);
        assert_eq!(cfg.msg_separator().glyph(), Some('\u{254C}'));
        let cfg = parse_text("msg_seperator:\n  char: \"-\"\n").unwrap();
        assert!(cfg.msg_separator().enabled);
        assert_eq!(cfg.msg_separator().glyph(), Some('-'));
    }

    #[test]
    fn the_correct_spelling_is_read_too() {
        let cfg = parse_text("msg_separator:\n  char: \"=\"\n").unwrap();
        assert_eq!(cfg.msg_separator().glyph(), Some('='));
    }

    #[test]
    fn the_separator_char_must_be_one_visible_character() {
        for bad in ["", "ab", "\\u0007", "\\u200b"] {
            let text = format!("msg_seperator:\n  char: \"{}\"\n", bad);
            let err = parse_text(&text).unwrap_err().to_string();
            assert!(err.contains("msg_seperator.char"), "{:?}: {}", bad, err);
        }
        // A wide character is one character.
        assert!(parse_text("msg_seperator:\n  char: \"\u{ff5c}\"\n").is_ok());
        // Not checked while the line is off.
        assert!(parse_text("msg_seperator:\n  enabled: false\n  char: \"ab\"\n").is_ok());
    }
}
