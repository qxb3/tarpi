//! The `/note` (Author's Note) and `/config` forms.

use super::*;

impl App {
    /// `/note`: the Author's Note of this chat as a form.
    pub(super) async fn open_note(&mut self) {
        let n = Note::from_chat(&self.chat);
        self.card_modal = Some(CardModal::note(&n, &self.input));
        self.note_edit = true;
    }

    /// `/memory`: the vector memory settings as a form of arrow-key rows.
    pub(super) async fn open_vector(&mut self) {
        let v = config::load_vector_memory();
        self.card_modal = Some(CardModal::vector(&v, &self.input));
        self.vector_edit = true;
    }

    /// Ctrl+S in the vector memory form: write the settings to
    /// `vector-memory.yaml`. The next reply's prompt uses them.
    pub(super) async fn save_vector_form(&mut self, close: bool) {
        let v = match self.card_modal.as_mut() {
            Some(m) => {
                m.sync();
                config::VectorMemoryConfig {
                    enabled: m.value("enabled") == "on",
                    results: note::parse_number(&m.value("results"), crate::cardedit::VECTOR_MAX_RESULTS).unwrap_or(3),
                    recent: note::parse_number(&m.value("recent"), crate::cardedit::VECTOR_MAX_RECENT).unwrap_or(12),
                    min_score: note::parse_number(&m.value("min_score"), 100).unwrap_or(15),
                    depth: note::parse_number(&m.value("depth"), note::MAX_DEPTH).unwrap_or(4),
                    role: m.value("role"),
                    template: m.value("template"),
                }
            }
            None => return,
        };
        if let Err(e) = config::save_vector_memory(&v) {
            self.card_note(format!("not saved: {:#}", e), true);
            return;
        }
        // The prompt (and its token count) now holds the memories, or not.
        self.settings_ver += 1;
        let msg = if v.enabled {
            "vector memory saved (on)"
        } else {
            "vector memory saved (off)"
        };
        if close {
            self.card_modal = None;
            self.vector_edit = false;
        } else {
            if let Some(m) = self.card_modal.as_mut() {
                for k in ["enabled", "results", "recent", "min_score", "depth", "role", "template"] {
                    m.mark_saved(k);
                }
            }
            self.card_note(msg, false);
        }
        self.set_status(msg, false);
    }

    /// `/config`: `config.yaml` in a window, edited with the same vim modes
    /// as the chat box (when `vim.enabled`). Ctrl+S / `:w` / Save checks it,
    /// writes it and applies it to this running screen (see `apply_config`).
    pub(super) async fn open_config(&mut self) {
        let text = config::read_text();
        self.card_modal = Some(CardModal::config(&text, &self.input));
        self.config_edit = true;
    }

    /// Ctrl+S in the `/config` window: check the YAML, write it to
    /// `config.yaml` and use it right away. On a mistake nothing is written
    /// and the window stays open, saying what is wrong.
    pub(super) async fn save_config_form(&mut self, close: bool) {
        let text = match self.card_modal.as_mut() {
            Some(m) => {
                m.sync();
                m.fields.first().map(|f| f.text.clone()).unwrap_or_default()
            }
            None => return,
        };
        let cfg = match config::parse_text(&text) {
            Ok(c) => c,
            Err(e) => {
                self.card_note(format!("not saved: {:#}", e), true);
                return;
            }
        };
        if let Err(e) = config::write_text(&text) {
            self.card_note(format!("{:#}", e), true);
            return;
        }
        self.apply_config(&cfg);
        if close {
            self.card_modal = None;
            self.config_edit = false;
        } else if let Some(m) = self.card_modal.as_mut() {
            m.mark_saved("config");
            // The window itself follows the new vim settings at once.
            m.input.configure(cfg.vim().enabled, cfg.vim().keys());
            if let Some(l) = cfg.vim().leader_char() {
                m.input.set_leader(l);
            }
            m.note = "config.yaml saved and applied".to_string();
            m.note_err = false;
        }
        self.set_status("config.yaml saved and applied", false);
    }

    /// Use these settings in the running screen (start-up and `/config` both
    /// come here). `serve_url` / `serve_trust_all` are only read when
    /// `tarpi serve` / `tarpi connect` start, so they need a restart.
    pub fn apply_config(&mut self, cfg: &config::FileConfig) {
        self.vim = cfg.vim().enabled;
        self.input.configure(cfg.vim().enabled, cfg.vim().keys());
        self.input.set_autopair(cfg.autopair.unwrap_or(true));
        self.command_menu = cfg.command_menu.unwrap_or(true);
        self.comp = Comp::default();
        // The word list is only loaded when spell checking is switched on.
        if cfg.spellcheck.unwrap_or(false) {
            if self.spell.is_none() {
                self.spell = Some(crate::spell::Checker::new());
            }
        } else {
            self.spell = None;
            self.spell_menu = None;
        }
        if let Some(l) = cfg.vim().leader_char() {
            self.input.set_leader(l);
        }
        self.tap_move_cursor = cfg.mouse().tap_to_move_cursor;
        self.tap_open_keyboard = cfg.mouse().open_keyboard_on_tap;
        let sep = cfg.msg_separator();
        self.msg_sep = if sep.enabled { sep.glyph() } else { None };
        self.chars_per_token = cfg.chars_per_token.unwrap_or(3.2);
        // The token estimate (and so the status row) depends on it.
        self.settings_ver += 1;
    }

    /// Ctrl+S in the Author's Note form: keep the note in the chat file and
    /// use it from the next reply on.
    pub(super) async fn save_note_form(&mut self, close: bool) {
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
        // The rows only hold values they can step to, so these always parse;
        // the defaults are for a field that somehow does not.
        let d = Note::default();
        let position = note::parse_position(&position).unwrap_or(d.position);
        let depth = note::parse_number(&depth, note::MAX_DEPTH).unwrap_or(d.depth);
        let role = note::parse_role(&role).unwrap_or(d.role);
        let interval = note::parse_number(&interval, note::MAX_INTERVAL).unwrap_or(d.interval);
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
        let msg = if n.text.trim().is_empty() {
            "author's note saved (no text: nothing is sent)"
        } else if n.interval == 0 {
            "author's note saved (interval: never)"
        } else {
            "author's note saved"
        };
        if close {
            self.card_modal = None;
            self.note_edit = false;
        } else {
            if let Some(m) = self.card_modal.as_mut() {
                for k in ["note", "position", "depth", "role", "interval"] {
                    m.mark_saved(k);
                }
            }
            self.card_note(msg, false);
        }
        self.set_status(msg, false);
    }
}
