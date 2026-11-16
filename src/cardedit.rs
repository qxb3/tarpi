//! The `/character` window: every field of the character card in one form,
//! with Cancel and Save. State and text editing live here; drawing is in
//! `ui::draw_card_modal`, key / mouse routing and saving in `app/card_modal.rs`.

use crate::character::Character;
use crate::input::{Input, Mode};
use crate::config::VectorMemoryConfig;
use crate::note::{self, Note};
use crate::ui;
use ratatui::layout::Rect;
use serde_json::Value;

/// One section of the form.
pub struct CardField {
    /// Card field written on save (`edit-attribute`); empty when not editable.
    pub key: &'static str,
    pub label: String,
    pub text: String,
    /// The text as it is on the card, to tell what changed.
    pub orig: String,
    pub editable: bool,
    /// One of the card's alternate greetings (they are saved as one list).
    pub alt: bool,
}

/// What a screen row of the form shows (set by `ui::draw_card_modal`).
pub struct Hit {
    pub y: u16,
    pub field: usize,
    /// Display row inside the field's text, `None` for the field's title row.
    pub row: Option<usize>,
}

/// A row of the form that is changed with the arrow keys instead of typed:
/// one of a few words, or a whole number up to the limit. The field's text
/// holds the value (the word's key, or the digits), so saving, `dirty` and
/// `value` work as they do for typed fields.
#[derive(Clone, Copy)]
pub enum Pick {
    /// `(key, shown)` pairs; the key is what is stored.
    Words(&'static [(&'static str, &'static str)]),
    /// `0..=max`.
    Number(usize),
}

/// Where the Author's Note goes (`note::position_word` keys).
pub const POSITIONS: &[(&str, &str)] = &[
    ("chat", "In the chat"),
    ("after", "After the card"),
    ("before", "Before the card"),
];
/// Who the Author's Note says it is from (`note::role_word` keys).
pub const ROLES: &[(&str, &str)] = &[
    ("system", "system"),
    ("user", "user"),
    ("assistant", "assistant"),
];

/// The on / off row of the `/memory` form.
pub const ON_OFF: &[(&str, &str)] = &[("off", "Off"), ("on", "On")];
/// Largest `results` and `recent` the `/memory` form steps to.
pub const VECTOR_MAX_RESULTS: usize = 20;
pub const VECTOR_MAX_RECENT: usize = 999;

pub struct CardModal {
    /// Window title (`character card`, `persona`).
    pub title: String,
    pub fields: Vec<CardField>,
    /// Field being edited (always an editable one).
    pub focus: usize,
    /// The editor of the focused field: text, cursor and vim mode live here
    /// (the same box as the chat input, so every vim motion works). Its text
    /// is copied back into `fields[focus]` by `sync` after every key.
    pub input: Input,
    /// Undo history of every field (the editor holds the one of the focused
    /// field; moving to another field stores it here and takes that field's).
    undo: Vec<Vec<(String, usize)>>,
    /// One-line message in the window (errors, `:s` results, discard warning).
    pub note: String,
    pub note_err: bool,
    /// Esc was pressed with unsaved changes; Esc again discards them.
    pub discard_armed: bool,
    /// First visible line of the form (`ui::draw_card_modal` keeps it in range).
    pub scroll: usize,
    /// Keep the cursor in view when drawing (off while the wheel scrolls).
    pub follow: bool,
    /// Text width and visible rows, set by `ui::draw_card_modal`.
    pub text_w: usize,
    pub view_h: usize,
    pub text_x: u16,
    /// Where the text cursor was drawn (screen column, row), for the spelling menu.
    pub cursor_xy: Option<(u16, u16)>,
    pub cancel_btn: Rect,
    pub save_btn: Rect,
    pub hits: Vec<Hit>,
    /// Rows changed with the arrow keys (field index, kind); empty in the
    /// forms that are all text.
    picks: Vec<(usize, Pick)>,
    /// The next digit typed replaces a number row's value instead of adding.
    fresh: bool,
    /// Show `//` and `/* */` notes in gray (the `/char` window). They stay in
    /// the card but are cut from the prompt (`prompt::strip_card_comments`).
    pub comments: bool,
}

fn norm(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

impl CardModal {
    pub fn new(ch: &Character, cfg: &Input) -> CardModal {
        let mut fields: Vec<CardField> = Vec::new();
        let mut info = |label: &str, text: String| {
            fields.push(CardField {
                key: "",
                label: label.to_string(),
                orig: text.clone(),
                text,
                editable: false,
                alt: false,
            });
        };
        // The card file (the picture) comes first, then the name.
        if !ch.avatar.trim().is_empty() {
            info("Card file", ch.avatar.clone());
        }

        // The name can be renamed (written to the card as `name`).
        let name = norm(&ch.name);
        fields.push(CardField {
            key: "name",
            label: "Name".to_string(),
            orig: name.clone(),
            text: name,
            editable: true,
            alt: false,
        });

        let plain: [(&'static str, &str, &String); 7] = [
            ("description", "Description", &ch.description),
            ("personality", "Personality", &ch.personality),
            ("scenario", "Scenario", &ch.scenario),
            ("first_mes", "First message", &ch.first_mes),
            ("mes_example", "Example dialogue", &ch.mes_example),
            ("system_prompt", "System prompt", &ch.system_prompt),
            (
                "post_history_instructions",
                "Post-history instructions",
                &ch.post_history_instructions,
            ),
        ];
        for (key, label, value) in plain {
            let t = norm(value);
            fields.push(CardField {
                key,
                label: label.to_string(),
                orig: t.clone(),
                text: t,
                editable: true,
                alt: false,
            });
        }
        for (n, g) in ch.alternate_greetings.iter().enumerate() {
            let t = norm(g);
            fields.push(CardField {
                key: "alternate_greetings",
                label: format!("Alternate greeting {}", n + 1),
                orig: t.clone(),
                text: t,
                editable: true,
                alt: true,
            });
        }
        // The Character's Note (SillyTavern's depth prompt): its text, then
        // rows for the depth and role (always shown).
        let (cn_text, cn_depth, cn_role) = match &ch.depth_prompt {
            Some(d) => (norm(&d.prompt), d.depth, d.role.clone()),
            None => (String::new(), 4, "system".to_string()),
        };
        let cn = fields.len();
        for (key, label, text) in [
            ("depth_prompt", "Character's note", cn_text),
            ("cn_depth", "Depth", cn_depth.min(note::MAX_DEPTH).to_string()),
            ("cn_role", "Role", cn_role),
        ] {
            fields.push(CardField {
                key,
                label: label.to_string(),
                orig: text.clone(),
                text,
                editable: true,
                alt: false,
            });
        }

        let mut m = CardModal::from_fields(fields, cfg);
        m.picks = vec![
            (cn + 1, Pick::Number(note::MAX_DEPTH)),
            (cn + 2, Pick::Words(ROLES)),
        ];
        m.title = "character card".to_string();
        m.comments = true;
        // `/char` opens for reading and moving around: NORMAL mode (does
        // nothing when vim modes are off).
        m.input.enter_normal();
        m
    }

    /// The form of a persona: name, title and description, all editable.
    pub fn persona(name: &str, title: &str, description: &str, cfg: &Input) -> CardModal {
        let field = |key: &'static str, label: &str, text: &str| {
            let t = norm(text);
            CardField {
                key,
                label: label.to_string(),
                orig: t.clone(),
                text: t,
                editable: true,
                alt: false,
            }
        };
        let fields = vec![
            field("name", "Name", name),
            field("title", "Title", title),
            field("description", "Description", description),
        ];
        let mut m = CardModal::from_fields(fields, cfg);
        m.title = "persona".to_string();
        m
    }

    /// The form of a system prompt: name, the prompt and the post-history
    /// instructions, all editable.
    pub fn sysprompt(name: &str, content: &str, post_history: &str, cfg: &Input) -> CardModal {
        let field = |key: &'static str, label: &str, text: &str| {
            let t = norm(text);
            CardField {
                key,
                label: label.to_string(),
                orig: t.clone(),
                text: t,
                editable: true,
                alt: false,
            }
        };
        let fields = vec![
            field("name", "Name", name),
            field("content", "System prompt", content),
            field("post_history", "Post-history instructions", post_history),
        ];
        let mut m = CardModal::from_fields(fields, cfg);
        m.title = "system prompt".to_string();
        m
    }

    /// The form of the chat's Author's Note: the text to type, then rows that
    /// say where and how often it is sent, changed with the arrow keys (depth
    /// and role are always shown; they only matter for the chat position).
    pub fn note(n: &Note, cfg: &Input) -> CardModal {
        let field = |key: &'static str, label: &str, text: &str| {
            let t = norm(text);
            CardField {
                key,
                label: label.to_string(),
                orig: t.clone(),
                text: t,
                editable: true,
                alt: false,
            }
        };
        let fields = vec![
            field("note", "Author's note", &n.text),
            field("position", "Position", note::position_word(n.position)),
            field("depth", "Depth", &n.depth.to_string()),
            field("role", "Role", note::role_word(n.role)),
            field("interval", "Interval", &n.interval.to_string()),
        ];
        let mut m = CardModal::from_fields(fields, cfg);
        m.picks = vec![
            (1, Pick::Words(POSITIONS)),
            (2, Pick::Number(note::MAX_DEPTH)),
            (3, Pick::Words(ROLES)),
            (4, Pick::Number(note::MAX_INTERVAL)),
        ];
        m.title = "author's note".to_string();
        // Like `/char` and `/config`: opens in NORMAL mode (does nothing when
        // vim modes are off).
        m.input.enter_normal();
        m
    }

    /// The `/memory` form: rows of arrow-key choices (see `Pick`), one per
    /// vector memory setting, then the text to put into the prompt, with
    /// `{{result}}` where the recalled messages go.
    pub fn vector(v: &VectorMemoryConfig, cfg: &Input) -> CardModal {
        let field = |key: &'static str, label: &str, text: &str| CardField {
            key,
            label: label.to_string(),
            orig: text.to_string(),
            text: text.to_string(),
            editable: true,
            alt: false,
        };
        let role = note::role_word(note::parse_role(&v.role).unwrap_or(0));
        let fields = vec![
            field("enabled", "Memory", if v.enabled { "on" } else { "off" }),
            field("results", "Results", &v.results.min(VECTOR_MAX_RESULTS).to_string()),
            field("recent", "Skip", &v.recent.min(VECTOR_MAX_RECENT).to_string()),
            field("min_score", "Match", &v.min_score.min(100).to_string()),
            field("depth", "Depth", &v.depth.min(note::MAX_DEPTH).to_string()),
            field("role", "Role", role),
            field("template", "Template", &norm(&v.template)),
        ];
        let mut m = CardModal::from_fields(fields, cfg);
        m.picks = vec![
            (0, Pick::Words(ON_OFF)),
            (1, Pick::Number(VECTOR_MAX_RESULTS)),
            (2, Pick::Number(VECTOR_MAX_RECENT)),
            (3, Pick::Number(100)),
            (4, Pick::Number(note::MAX_DEPTH)),
            (5, Pick::Words(ROLES)),
        ];
        m.title = "vector memory".to_string();
        m.input.enter_normal();
        m
    }

    /// The `/config` window: `config.yaml` as one editable field. It opens in
    /// NORMAL mode when vim modes are on (see the `vim:` config), so the file
    /// can be read and moved around in with the usual motions.
    pub fn config(text: &str, cfg: &Input) -> CardModal {
        let t = norm(text);
        let fields = vec![CardField {
            key: "config",
            label: "config.yaml".to_string(),
            orig: t.clone(),
            text: t,
            editable: true,
            alt: false,
        }];
        let mut m = CardModal::from_fields(fields, cfg);
        m.title = "config".to_string();
        // It is YAML: no auto-closing of quotes and brackets.
        m.input.set_autopair(false);
        m.input.enter_normal();
        m
    }

    /// How the field at `i` is changed, when it is a row of arrow-key choices.
    pub fn pick_of(&self, i: usize) -> Option<Pick> {
        self.picks.iter().find(|(f, _)| *f == i).map(|(_, p)| *p)
    }

    /// The value of a pick row as the window shows it.
    pub fn shown(&self, i: usize) -> String {
        let text = self.fields[i].text.trim();
        match self.pick_of(i) {
            Some(Pick::Words(w)) => w
                .iter()
                .find(|(k, _)| *k == text)
                .map(|(_, s)| s.to_string())
                .unwrap_or_else(|| w[0].1.to_string()),
            Some(Pick::Number(_)) => {
                let n: usize = text.parse().unwrap_or(0);
                match (self.fields[i].key, n) {
                    (k, 0) if k.ends_with("depth") => "after the last message".to_string(),
                    (k, 1) if k.ends_with("depth") => "before the last message".to_string(),
                    (k, n) if k.ends_with("depth") => format!("before the last {} messages", n),
                    ("results", 0) => "none".to_string(),
                    ("recent", n) => format!("not the latest {} messages", n),
                    ("min_score", n) => format!("{}% alike or more", n),
                    ("interval", 0) => "never".to_string(),
                    ("interval", 1) => "every message of yours".to_string(),
                    ("interval", n) => format!("every {} messages of yours", n),
                    (_, n) => n.to_string(),
                }
            }
            None => text.to_string(),
        }
    }

    fn set_pick_text(&mut self, i: usize, t: String) {
        self.fields[i].text = t.clone();
        if i == self.focus {
            self.input.text = t;
            self.input.cursor = 0;
        }
    }

    /// Change the focused pick row by `d` steps (words wrap around, numbers
    /// stop at their ends).
    pub fn step_pick(&mut self, d: i64) {
        let i = self.focus;
        match self.pick_of(i) {
            Some(Pick::Words(w)) => {
                let now = w
                    .iter()
                    .position(|(k, _)| *k == self.fields[i].text.trim())
                    .unwrap_or(0) as i64;
                let next = (now + d.signum()).rem_euclid(w.len() as i64) as usize;
                self.set_pick_text(i, w[next].0.to_string());
            }
            Some(Pick::Number(max)) => {
                let now: i64 = self.fields[i].text.trim().parse().unwrap_or(0);
                let n = (now + d).clamp(0, max as i64);
                self.set_pick_text(i, n.to_string());
                self.fresh = true;
            }
            None => {}
        }
    }

    /// A digit typed on a number row: the first one replaces the value.
    pub fn pick_digit(&mut self, c: char) {
        let i = self.focus;
        let (Some(Pick::Number(max)), Some(n)) = (self.pick_of(i), c.to_digit(10)) else {
            return;
        };
        let now: usize = self.fields[i].text.trim().parse().unwrap_or(0);
        let n = n as usize;
        let next = if self.fresh { n } else { now * 10 + n };
        self.fresh = false;
        self.set_pick_text(i, (if next > max { n } else { next }).to_string());
    }

    /// Backspace on a number row drops its last digit.
    pub fn pick_backspace(&mut self) {
        let i = self.focus;
        if let Some(Pick::Number(_)) = self.pick_of(i) {
            let now: usize = self.fields[i].text.trim().parse().unwrap_or(0);
            self.set_pick_text(i, (now / 10).to_string());
            self.fresh = false;
        }
    }

    /// Move the cursor to the next / previous row of the form.
    pub fn move_row(&mut self, dir: i32) {
        if let Some(p) = self.step_field(dir) {
            // Going up into text, land at its end. The mode stays: leaving a
            // row of choices does not start typing.
            self.focus_field(p, if dir < 0 { usize::MAX } else { 0 }, false);
        }
    }

    /// The text of the field with this key, as typed.
    fn value_raw(&self, key: &str) -> String {
        self.fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.text.clone())
            .unwrap_or_default()
    }

    /// The text of the field with this key, trimmed.
    pub fn value(&self, key: &str) -> String {
        self.fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.text.trim().to_string())
            .unwrap_or_default()
    }

    fn from_fields(fields: Vec<CardField>, cfg: &Input) -> CardModal {
        let focus = fields.iter().position(|f| f.editable).unwrap_or(0);
        let mut input = Input::default();
        input.copy_config(cfg);
        input.text = fields[focus].text.clone();
        // Open with the cursor at the very top of the card, not the end.
        input.cursor = 0;
        input.enter_insert();
        CardModal {
            title: String::new(),
            undo: vec![Vec::new(); fields.len()],
            fields,
            focus,
            input,
            note: String::new(),
            note_err: false,
            discard_armed: false,
            scroll: 0,
            follow: true,
            text_w: 60,
            view_h: 10,
            text_x: 0,
            cursor_xy: None,
            cancel_btn: Rect::default(),
            save_btn: Rect::default(),
            hits: Vec::new(),
            picks: Vec::new(),
            fresh: true,
            comments: false,
        }
    }

    /// Is anything different from the card?
    pub fn dirty(&self) -> bool {
        self.fields.iter().any(|f| f.editable && f.text != f.orig)
    }

    /// The card fields to write: `(field, value)`. The alternate greetings go
    /// out as one list (empty ones left out).
    pub fn changes(&self) -> Vec<(&'static str, Value)> {
        let mut out: Vec<(&'static str, Value)> = Vec::new();
        let mut rename: Option<String> = None;
        for f in &self.fields {
            if f.editable && !f.alt && f.text != f.orig {
                // The Character's Note is written as one object (below).
                if f.key == "depth_prompt" || f.key.starts_with("cn_") {
                    continue;
                }
                if f.key == "name" {
                    // One line, never empty; written last (see below).
                    let n = f.text.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !n.is_empty() && n != f.orig {
                        rename = Some(n);
                    }
                    continue;
                }
                out.push((f.key, Value::String(f.text.clone())));
            }
        }
        if self.fields.iter().any(|f| f.alt && f.text != f.orig) {
            let arr: Vec<Value> = self
                .fields
                .iter()
                .filter(|f| f.alt && !f.text.trim().is_empty())
                .map(|f| Value::String(f.text.clone()))
                .collect();
            out.push(("alternate_greetings", Value::Array(arr)));
        }
        let cn_changed = self
            .fields
            .iter()
            .any(|f| (f.key == "depth_prompt" || f.key.starts_with("cn_")) && f.text != f.orig);
        if cn_changed {
            let text = self.value_raw("depth_prompt");
            let depth: usize = self.value("cn_depth").parse().unwrap_or(4);
            let role = self.value("cn_role");
            out.push((
                "depth_prompt",
                serde_json::json!({ "prompt": text, "depth": depth, "role": role }),
            ));
        }
        // The name goes last: the other fields are written under the old one.
        if let Some(n) = rename {
            out.push(("name", Value::String(n)));
        }
        out
    }

    /// A field was written to the card: it is the new baseline.
    pub fn mark_saved(&mut self, key: &str) {
        for f in self.fields.iter_mut() {
            let same = f.key == key || (key == "depth_prompt" && f.key.starts_with("cn_"));
            if f.editable && same {
                f.orig = f.text.clone();
            }
        }
    }

    /// Copy the editor's text into the focused field.
    pub fn sync(&mut self) {
        let i = self.focus;
        if self.fields[i].text != self.input.text {
            self.fields[i].text = self.input.text.clone();
        }
    }

    /// Make field `i` the one being edited, with the cursor at `cursor`.
    /// `insert`: start in INSERT mode (otherwise the mode stays).
    fn focus_field(&mut self, i: usize, cursor: usize, insert: bool) {
        self.sync();
        // Keep the undo history of the field that is left, and give the new
        // field its own, so `u` never pulls in another field's text.
        let left = self.input.take_undo();
        if let Some(slot) = self.undo.get_mut(self.focus) {
            *slot = left;
        }
        self.focus = i;
        self.fresh = true;
        self.input.text = self.fields[i].text.clone();
        self.input.cursor = cursor.min(self.input.len_chars());
        self.input.reset_vim();
        self.input.set_undo(self.undo.get(i).cloned().unwrap_or_default());
        if insert {
            self.input.enter_insert();
        } else {
            if self.input.mode == Mode::Visual {
                self.input.exit_visual();
            }
            self.input.cancel_pending();
            self.input.clamp_normal();
        }
    }

    fn step_field(&self, dir: i32) -> Option<usize> {
        let mut i = self.focus as i32 + dir;
        while i >= 0 && (i as usize) < self.fields.len() {
            if self.fields[i as usize].editable {
                return Some(i as usize);
            }
            i += dir;
        }
        None
    }

    /// Pasted text goes in at the cursor.
    pub fn paste(&mut self, s: &str) {
        if self.pick_of(self.focus).is_some() {
            return;
        }
        self.input.paste_text(&norm(s));
        self.sync();
    }

    /// Up one display row; from the first row on to the previous field.
    pub fn up(&mut self) {
        let w = self.text_w.max(1);
        let cur = self.input.cursor.min(self.input.len_chars());
        let (_, (row, col), _) = ui::layout_input(&self.input.text, cur, w);
        if row > 0 {
            self.input.cursor = ui::vertical_target(&self.input.text, cur, w, -1);
        } else if let Some(p) = self.step_field(-1) {
            let t = self.fields[p].text.clone();
            let (lines, _, _) = ui::layout_input(&t, 0, w);
            let c = ui::index_at(&t, w, lines.len().saturating_sub(1), col);
            let keep_mode = self.input.mode == Mode::Insert;
            self.focus_field(p, c, keep_mode);
        } else {
            self.input.cursor = 0;
        }
    }

    /// Down one display row; from the last row on to the next field.
    pub fn down(&mut self) {
        let w = self.text_w.max(1);
        let cur = self.input.cursor.min(self.input.len_chars());
        let (lines, (row, col), _) = ui::layout_input(&self.input.text, cur, w);
        if row + 1 < lines.len() {
            self.input.cursor = ui::vertical_target(&self.input.text, cur, w, 1);
        } else if let Some(n) = self.step_field(1) {
            let t = self.fields[n].text.clone();
            let c = ui::index_at(&t, w, 0, col);
            let keep_mode = self.input.mode == Mode::Insert;
            self.focus_field(n, c, keep_mode);
        } else {
            self.input.cursor = self.input.len_chars();
        }
    }

    /// Ctrl+U (`dir < 0`) / Ctrl+D: move half a window of rows. When the
    /// cursor reaches the first / last row of the field before that, it just
    /// moves on into the previous / next field and stops there.
    pub fn half_page(&mut self, dir: i32) {
        let w = self.text_w.max(1);
        let half = (self.view_h / 2).max(1);
        let moved = self.input.move_rows(dir, half, w);
        if moved >= half {
            return;
        }
        // Stopped at the edge of the field.
        let has_next = self.step_field(dir).is_some();
        if has_next {
            if dir < 0 {
                self.up();
            } else {
                self.down();
            }
        }
    }

    /// A key in NORMAL / VISUAL mode. `j` / `k` on the last / first row of a
    /// field carry on to the next / previous field, everything else is the
    /// chat box's own vim code.
    pub fn normal_key(&mut self, c: char) {
        let w = self.text_w.max(1);
        if self.input.mode == Mode::Normal && !self.input.leader_waiting() {
            // `G` / `gg` go to the very end / start of the whole form, not
            // just of the field being edited.
            if c == 'G' && !self.input.has_pending() {
                self.jump_end(false);
                return;
            }
            if c == 'g' && self.input.pending_g() {
                self.jump_end(true);
                return;
            }
        }
        if self.input.mode == Mode::Normal && !self.input.leader_waiting() && (c == 'j' || c == 'k') {
            let (lines, (row, _), _) =
                ui::layout_input(&self.input.text, self.input.cursor, w);
            if c == 'j' && row + 1 >= lines.len() {
                self.down();
                return;
            }
            if c == 'k' && row == 0 {
                self.up();
                return;
            }
        }
        self.input.normal_key(c, w);
    }

    /// `gg` (`top`) / `G`: the first / last line of the first / last editable
    /// field of the form.
    fn jump_end(&mut self, top: bool) {
        let w = self.text_w.max(1);
        let target = if top {
            self.fields.iter().position(|f| f.editable)
        } else {
            self.fields.iter().rposition(|f| f.editable)
        };
        let Some(t) = target else {
            return;
        };
        self.input.cancel_pending();
        if t != self.focus {
            self.focus_field(t, 0, false);
        }
        // Plain `gg` / `G` inside the field now reach its first / last line.
        if top {
            self.input.normal_key('g', w);
            self.input.normal_key('g', w);
            self.scroll = 0;
        } else {
            self.input.normal_key('G', w);
        }
    }

    /// A click on screen row `y`, column `x`: focus that field and put the
    /// cursor there.
    pub fn click(&mut self, x: u16, y: u16) {
        // The rows on screen may be a `:s` preview, not the real text.
        if self.input.mode == Mode::Command {
            return;
        }
        let Some(hit) = self.hits.iter().find(|h| h.y == y) else {
            return;
        };
        let (field, row) = (hit.field, hit.row);
        if !self.fields[field].editable {
            return;
        }
        if self.pick_of(field).is_some() {
            // A row of choices: the left half steps back, the right half on.
            if field != self.focus {
                self.focus_field(field, 0, false);
            }
            let half = self.text_x + (self.text_w / 2) as u16;
            self.step_pick(if x < half { -1 } else { 1 });
            self.follow = true;
            return;
        }
        let idx = match row {
            Some(r) => {
                let col = x.saturating_sub(self.text_x) as usize;
                ui::index_at(&self.fields[field].text, self.text_w.max(1), r, col)
            }
            None => 0,
        };
        if field == self.focus {
            self.input.cursor = idx.min(self.input.len_chars());
            self.input.cancel_pending();
            self.input.clamp_normal();
        } else {
            // The mode stays: a click in NORMAL mode does not start typing.
            self.focus_field(field, idx, false);
        }
        self.follow = true;
    }
}

/// Ranges of `text` that are `//` or `/* */` comments, as `(start, end)` char
/// indices, the delimiters included. The line break after a `//` comment is
/// not included. The card window draws these gray; the prompt builder cuts
/// them out (`prompt::strip_comments`).
///
/// `//` only counts at the start of a line or after whitespace, so a URL such
/// as `https://example.com` in a card is left alone. `/* */` may span lines,
/// does not nest, and an unclosed `/*` runs to the end of the field.
pub fn comment_ranges(text: &str) -> Vec<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '/' && i + 1 < chars.len() {
            match chars[i + 1] {
                '/' if at_line_start(&chars, i) => {
                    let start = i;
                    while i < chars.len() && chars[i] != '\n' {
                        i += 1;
                    }
                    // A CRLF line break is not part of the comment.
                    let mut end = i;
                    if end > start && chars[end - 1] == '\r' {
                        end -= 1;
                    }
                    out.push((start, end));
                    continue;
                }
                '*' => {
                    let start = i;
                    i += 2;
                    while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                        i += 1;
                    }
                    if i + 1 < chars.len() {
                        i += 2;
                    } else {
                        i = chars.len();
                    }
                    out.push((start, i));
                    continue;
                }
                _ => {}
            }
        }
        i += 1;
    }
    out
}

/// Whether the `/` at `i` stands at the start of a line or after whitespace
/// (so `https://` is not taken for a comment).
fn at_line_start(chars: &[char], i: usize) -> bool {
    i == 0 || chars[i - 1].is_whitespace()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_ranges_are_char_indices() {
        assert_eq!(comment_ranges("é // ü"), vec![(2, 6)]);
    }

    #[test]
    fn a_url_is_not_a_comment() {
        assert!(comment_ranges("see https://example.com/x").is_empty());
        assert_eq!(comment_ranges("a // note"), vec![(2, 9)]);
    }

    #[test]
    fn block_comments_do_not_nest() {
        assert_eq!(comment_ranges("/* a /* b */ c */"), vec![(0, 12)]);
    }

    #[test]
    fn undo_history_is_kept_per_field() {
        let mut m = CardModal::persona("Al", "Dr", "text", &Input::default());
        assert_eq!(m.focus, 0);
        m.input.cursor = 2;
        m.input.type_char('x');
        m.sync();
        assert_eq!(m.fields[0].text, "Alx");
        // The next field starts with a history of its own: `u` does not
        // bring the first field's text in.
        m.focus_field(1, 0, true);
        assert_eq!(m.input.text, "Dr");
        m.input.enter_normal();
        m.input.normal_key('u', 40);
        assert_eq!(m.input.text, "Dr");
        // Back in the first field its history is still there.
        m.focus_field(0, 0, true);
        assert_eq!(m.input.text, "Alx");
        m.input.enter_normal();
        m.input.normal_key('u', 40);
        assert_eq!(m.input.text, "Al");
    }

    fn note_form() -> CardModal {
        CardModal::note(&Note::default(), &Input::default())
    }

    #[test]
    fn the_note_opens_in_normal_mode_with_vim_and_insert_without() {
        assert_eq!(note_form().input.mode, Mode::Normal);
        let mut cfg = Input::default();
        cfg.configure(false, vec![]);
        let m = CardModal::note(&Note::default(), &cfg);
        assert_eq!(m.input.mode, Mode::Insert);
    }

    #[test]
    fn note_rows_step_through_their_choices() {
        let mut m = note_form();
        m.focus_field(1, 0, true);
        m.step_pick(1);
        assert_eq!(m.value("position"), "after");
        m.step_pick(1);
        m.step_pick(1);
        assert_eq!(m.value("position"), "chat");
        m.step_pick(-1);
        assert_eq!(m.value("position"), "before");
        assert_eq!(m.shown(1), "Before the card");
    }

    #[test]
    fn numbers_stop_at_their_ends_and_take_digits() {
        let mut m = note_form();
        m.focus_field(2, 0, true);
        assert_eq!(m.value("depth"), "4");
        m.step_pick(-10);
        assert_eq!(m.value("depth"), "0");
        assert_eq!(m.shown(2), "after the last message");
        m.pick_digit('1');
        m.pick_digit('2');
        assert_eq!(m.value("depth"), "12");
        m.pick_backspace();
        assert_eq!(m.value("depth"), "1");
        m.focus_field(4, 0, true);
        m.step_pick(-1);
        assert_eq!(m.value("interval"), "0");
        assert_eq!(m.shown(4), "never");
        m.step_pick(-1);
        assert_eq!(m.value("interval"), "0");
    }

    #[test]
    fn a_stepped_row_makes_the_form_dirty() {
        let mut m = note_form();
        assert!(!m.dirty());
        m.focus_field(3, 0, true);
        m.step_pick(1);
        assert!(m.dirty());
        assert_eq!(m.value("role"), "user");
    }

    fn card_form(dp: Option<crate::character::DepthPrompt>) -> CardModal {
        let ch = Character {
            name: "Al".into(),
            depth_prompt: dp,
            ..Character::default()
        };
        CardModal::new(&ch, &Input::default())
    }

    fn at(m: &CardModal, key: &str) -> usize {
        m.fields.iter().position(|f| f.key == key).unwrap()
    }

    #[test]
    fn the_characters_note_rows_start_from_the_card_or_the_defaults() {
        let m = card_form(None);
        assert_eq!(m.value("cn_depth"), "4");
        assert_eq!(m.value("cn_role"), "system");
        assert!(m.changes().is_empty());
        let dp = crate::character::DepthPrompt {
            prompt: "Stay in character.".into(),
            depth: 6,
            role: "user".into(),
        };
        let m = card_form(Some(dp));
        assert_eq!(m.value("cn_depth"), "6");
        assert_eq!(m.shown(at(&m, "cn_depth")), "before the last 6 messages");
        assert_eq!(m.shown(at(&m, "cn_role")), "user");
        assert!(m.changes().is_empty());
    }

    #[test]
    fn the_characters_note_is_saved_as_one_object() {
        let mut m = card_form(None);
        let t = at(&m, "depth_prompt");
        m.fields[t].text = "Keep it tense.".into();
        m.focus_field(at(&m, "cn_role"), 0, true);
        m.step_pick(1);
        let ch = m.changes();
        assert_eq!(ch.len(), 1);
        assert_eq!(ch[0].0, "depth_prompt");
        assert_eq!(
            ch[0].1,
            serde_json::json!({"prompt": "Keep it tense.", "depth": 4, "role": "user"})
        );
        m.mark_saved("depth_prompt");
        assert!(!m.dirty());
    }
}
