//! `/formatting`: a full-screen editor for the context template, the instruct
//! template and the reply-cleanup switches (see `formatting`), opened from
//! inside a chat like `/api`. Which settings exist comes from
//! `formatting::FIELDS`, so a new one needs no changes here.

use crate::formatting::{self, Field, Formatting, Kind, Sec, FIELDS};
use crate::input::{Input, Mode};
use crate::ui;
use anyhow::Result;
use crossterm::event::{
    Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use futures_util::StreamExt;
use ratatui::{
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use serde_json::{json, Value};
use unicode_width::UnicodeWidthChar;

const DIM: Color = Color::Rgb(128, 128, 128);

// ------------------------------------------------------------------- state

#[derive(Clone, Copy, PartialEq)]
enum Row {
    Header(&'static str),
    /// Index into `FIELDS`.
    Field(usize),
}

#[derive(Clone, Copy, PartialEq)]
enum Confirm {
    Quit,
    ResetAll,
}

enum Modal {
    None,
    /// Typing a text setting, in the chat box's editor (vim modes when they
    /// are on). The story string is lines of its own; every other text is one
    /// line, with a line break typed as `\n`.
    Multi(Box<MultiEdit>),
    Confirm(Confirm),
    /// Choosing a preset of this template (the one the cursor is in).
    Presets(Sec, usize),
}

/// A text setting in a window of its own: the chat box's editor, so every vim
/// motion works (when `vim.enabled`). The story string has real line breaks;
/// the other texts are one line (`single`) and the window is only as big as
/// the text needs.
struct MultiEdit {
    input: Input,
    /// A one-line setting: Enter accepts, a line break is typed as `\n`.
    single: bool,
    /// The text when the window opened (Esc asks before dropping a change).
    orig: String,
    /// First visible row (`draw_multi` keeps the cursor in view).
    scroll: usize,
    /// Text width and visible rows, set when drawing.
    w: usize,
    h: usize,
    /// Esc was pressed with unaccepted changes; Esc again drops them.
    armed: bool,
    note: Option<(String, bool)>,
}

struct Tui {
    /// The chat box: only its vim settings are used (`vim.enabled`, leader,
    /// the keys that leave INSERT mode).
    cfg: Input,
    fmt: Formatting,
    /// What the file holds (or the defaults when there is none): "unsaved
    /// changes" means `fmt` differs from this.
    saved: Formatting,
    /// Earlier states of `fmt`, newest last (`u` goes back one).
    undo: Vec<Formatting>,
    /// `formatting.yaml` could not be read: keep a copy before it is replaced.
    backup_first: bool,
    rows: Vec<Row>,
    /// Selected row (index into `rows`).
    row: usize,
    scroll: usize,
    /// Scroll to keep the selected row visible (off while using the wheel).
    follow: bool,
    status: Option<(String, bool)>,
    modal: Modal,
    quit: bool,
    rows_rect: Rect,
}

impl Tui {
    fn new(fmt: Formatting) -> Tui {
        let mut rows = Vec::new();
        let mut sec = None;
        for (i, fd) in FIELDS.iter().enumerate() {
            if sec != Some(fd.sec) {
                sec = Some(fd.sec);
                rows.push(Row::Header(fd.sec.title()));
            }
            rows.push(Row::Field(i));
        }
        Tui {
            cfg: Input::default(),
            saved: fmt.clone(),
            undo: Vec::new(),
            backup_first: false,
            fmt,
            rows,
            row: 1,
            scroll: 0,
            follow: true,
            status: None,
            modal: Modal::None,
            quit: false,
            rows_rect: Rect::default(),
        }
    }

    fn say(&mut self, msg: impl Into<String>, is_error: bool) {
        self.status = Some((msg.into(), is_error));
    }

    /// Whether the settings differ from what is saved.
    fn dirty(&self) -> bool {
        self.fmt.normalized() != self.saved.normalized()
    }

    /// Remember the settings as they are, before a change.
    fn snapshot(&mut self) {
        if self.undo.len() >= 100 {
            self.undo.remove(0);
        }
        self.undo.push(self.fmt.clone());
    }

    /// `u`: take back the last change.
    fn undo_last(&mut self) {
        match self.undo.pop() {
            Some(prev) => {
                self.fmt = prev;
                self.say("undone", false);
            }
            None => self.say("nothing to undo", true),
        }
    }

    /// Tab / Shift+Tab: the first setting of the next / previous section.
    fn jump_section(&mut self, dir: i32) {
        let heads: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, Row::Header(_)))
            .map(|(i, _)| i)
            .collect();
        if heads.is_empty() {
            return;
        }
        let cur = heads.iter().rposition(|h| *h <= self.row).unwrap_or(0) as i32;
        let next = (cur + dir).rem_euclid(heads.len() as i32) as usize;
        self.row = (heads[next] + 1).min(self.rows.len().saturating_sub(1));
        self.follow = true;
    }

    fn field(&self) -> Option<&'static Field> {
        match self.rows.get(self.row) {
            Some(Row::Field(i)) => FIELDS.get(*i),
            _ => None,
        }
    }

    fn move_row(&mut self, dir: i32) {
        let n = self.rows.len() as i32;
        let mut r = self.row as i32;
        loop {
            r += dir;
            if r < 0 || r >= n {
                return;
            }
            if !matches!(self.rows[r as usize], Row::Header(_)) {
                self.row = r as usize;
                self.follow = true;
                return;
            }
        }
    }

    fn set(&mut self, fd: &Field, v: Value) {
        if self.fmt.get(fd) != v {
            self.snapshot();
            self.fmt.set(fd, v);
        }
    }

    /// Left / Right: flip a switch or step through a list of choices.
    fn adjust(&mut self, dir: i32) {
        let Some(fd) = self.field() else {
            return;
        };
        match fd.kind {
            Kind::Toggle => {
                let on = self.fmt.get(fd).as_bool().unwrap_or(false);
                self.set(fd, Value::Bool(!on));
            }
            Kind::Choice(opts) => {
                let cur = self.fmt.get(fd);
                let i = opts
                    .iter()
                    .position(|o| Some(*o) == cur.as_str())
                    .unwrap_or(0) as i32;
                let j = (i + dir).rem_euclid(opts.len() as i32) as usize;
                self.set(fd, json!(opts[j]));
            }
            Kind::Text => {}
        }
    }

    /// Enter: type a new text, or flip / step the setting.
    fn begin_edit(&mut self) {
        let Some(fd) = self.field() else {
            return;
        };
        if fd.kind != Kind::Text {
            self.adjust(1);
            return;
        }
        let v = self.fmt.get(fd);
        let single = fd.key != "story_string";
        let text = if single {
            // One line: a line break shows as `\n`.
            formatting::esc(v.as_str().unwrap_or(""))
        } else {
            // Several lines: real line breaks.
            v.as_str().unwrap_or("").replace("\r\n", "\n").replace('\r', "\n")
        };
        let mut input = Input::default();
        input.copy_config(&self.cfg);
        if single {
            // Quotes and brackets are typed as they are in these texts.
            input.set_autopair(false);
        }
        // The cursor starts at the very beginning; only a one-line text with
        // the vim modes off starts at its end (ready to append).
        input.cursor = if single && !input.vim_enabled() {
            text.chars().count()
        } else {
            0
        };
        input.text = text.clone();
        // The window opens in NORMAL mode (INSERT when the vim modes are off).
        input.enter_insert();
        if input.vim_enabled() {
            input.enter_normal();
        }
        self.modal = Modal::Multi(Box::new(MultiEdit {
            input,
            single,
            orig: text,
            scroll: 0,
            w: 60,
            h: 10,
            armed: false,
            note: None,
        }));
    }

    /// Store a typed text. `Err` (the editor stays open) when the setting
    /// cannot hold it; things that are accepted but look wrong are said in the
    /// status line.
    fn commit_edit(&mut self, text: &str) -> Result<(), String> {
        self.commit_value(formatting::unesc(text))
    }

    /// Like `commit_edit`, for a text that already has its real line breaks.
    fn commit_value(&mut self, value: String) -> Result<(), String> {
        let Some(fd) = self.field() else {
            return Ok(());
        };
        formatting::validate(fd, &value)?;
        let warn = formatting::warnings(fd, &value);
        self.set(fd, Value::String(value));
        if !warn.is_empty() {
            self.say(format!("saved as typed, but: {}", warn.join("; ")), true);
        }
        Ok(())
    }

    fn reset_row(&mut self) {
        let Some(fd) = self.field() else {
            return;
        };
        self.set(fd, fd.default_value());
        self.fmt.reset(fd);
        self.say(format!("{} reset to its default", fd.label), false);
    }

    fn reset_all(&mut self) {
        self.snapshot();
        self.fmt = Formatting::default();
        self.say("every setting is back to its default (u undoes)", false);
    }

    /// `p`: the presets of the template the selected setting belongs to.
    fn open_presets(&mut self) {
        match self.field().map(|fd| fd.sec) {
            Some(sec) if !formatting::presets(sec).is_empty() => {
                // The cursor starts on the preset in use, if there is one.
                let at = formatting::presets(sec)
                    .iter()
                    .position(|p| self.fmt.preset_is_current(p))
                    .unwrap_or(0);
                self.modal = Modal::Presets(sec, at);
            }
            _ => self.say("this part has no presets", true),
        }
    }

    fn apply_preset(&mut self, sec: Sec, i: usize) {
        let Some(p) = formatting::presets(sec).get(i).copied() else {
            return;
        };
        self.snapshot();
        self.fmt.apply_preset(p);
        self.say(
            format!(
                "{} preset '{}' applied (u undoes, s saves)",
                sec.title().to_lowercase(),
                p.name
            ),
            false,
        );
    }

    fn save(&mut self) {
        // A file that could not be read is about to be replaced: keep it.
        if self.backup_first {
            if let Some(path) = crate::config::formatting_path() {
                if std::fs::copy(&path, path.with_extension("yaml.bad")).is_ok() {
                    self.backup_first = false;
                }
            }
        }
        match self.fmt.save() {
            Ok(()) => {
                self.saved = self.fmt.clone();
                self.say("saved: chats pick the change up on the next reply", false);
            }
            Err(e) => self.say(format!("could not save: {:#}", e), true),
        }
    }

    fn try_quit(&mut self) {
        if self.dirty() {
            self.modal = Modal::Confirm(Confirm::Quit);
        } else {
            self.quit = true;
        }
    }

    /// The selected text setting as it is typed (`\n` for a line break),
    /// wrapped to `width` columns, at most `max` rows; nothing for a switch, a
    /// choice or an empty text.
    fn detail_lines(&self, width: usize, max: usize) -> Vec<String> {
        let Some(fd) = self.field() else {
            return Vec::new();
        };
        if fd.kind != Kind::Text {
            return Vec::new();
        }
        let v = self.fmt.get(fd);
        let chars: Vec<char> = formatting::esc(v.as_str().unwrap_or("")).chars().collect();
        if chars.is_empty() {
            return Vec::new();
        }
        let rows = wrap_rows(&chars, width.max(1));
        // One row is what the setting's own line already shows.
        if rows.len() <= 1 && dwidth(&chars.iter().collect::<String>()) + 30 <= width {
            return Vec::new();
        }
        let mut out: Vec<String> = rows
            .iter()
            .take(max)
            .map(|(a, b)| chars[*a..*b].iter().collect())
            .collect();
        if rows.len() > max {
            if let Some(last) = out.last_mut() {
                *last = clip(&format!("{}\u{2026}", last), width);
            }
        }
        out
    }

    // ------------------------------------------------------------- events

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(k.code, KeyCode::Char('c')) {
            self.try_quit();
            return;
        }
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::Multi(m) => {
                if let Some(m) = self.multi_key(*m, k) {
                    self.modal = Modal::Multi(Box::new(m));
                }
            }
            Modal::Confirm(what) => {
                let yes = matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y'));
                match what {
                    Confirm::Quit => match k.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('s') => {
                            self.save();
                            self.quit = !self.dirty();
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') => self.quit = true,
                        _ => {}
                    },
                    Confirm::ResetAll => {
                        if yes {
                            self.reset_all();
                        }
                    }
                }
            }
            Modal::Presets(sec, mut i) => {
                let last = formatting::presets(sec).len().saturating_sub(1);
                let mut keep = true;
                match k.code {
                    KeyCode::Esc | KeyCode::Char('q') => keep = false,
                    KeyCode::Up | KeyCode::Char('k') => i = i.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => i = (i + 1).min(last),
                    KeyCode::PageUp => i = i.saturating_sub(8),
                    KeyCode::PageDown => i = (i + 8).min(last),
                    KeyCode::Home => i = 0,
                    KeyCode::End => i = last,
                    KeyCode::Enter => {
                        self.apply_preset(sec, i);
                        keep = false;
                    }
                    _ => {}
                }
                if keep {
                    self.modal = Modal::Presets(sec, i);
                }
            }
            Modal::None => self.on_key_plain(k),
        }
    }

    fn on_key_plain(&mut self, k: KeyEvent) {
        self.status = None;
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.try_quit(),
            KeyCode::Char('s') => self.save(),
            KeyCode::Char('p') => self.open_presets(),
            KeyCode::Char('u') => self.undo_last(),
            KeyCode::Tab => self.jump_section(1),
            KeyCode::BackTab => self.jump_section(-1),
            KeyCode::Char('R') => self.modal = Modal::Confirm(Confirm::ResetAll),
            KeyCode::Char('r') | KeyCode::Backspace | KeyCode::Delete => self.reset_row(),
            KeyCode::Up | KeyCode::Char('k') => self.move_row(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_row(1),
            KeyCode::PageUp => (0..8).for_each(|_| self.move_row(-1)),
            KeyCode::PageDown => (0..8).for_each(|_| self.move_row(1)),
            KeyCode::Left | KeyCode::Char('h') => self.adjust(-1),
            KeyCode::Right | KeyCode::Char('l') => self.adjust(1),
            KeyCode::Home => {
                self.row = 1;
                self.follow = true;
            }
            KeyCode::End => {
                self.row = self.rows.len().saturating_sub(1);
                self.follow = true;
            }
            KeyCode::Enter => self.begin_edit(),
            _ => {}
        }
    }

    /// A key in the story string window. `None`: the window is closed
    /// (accepted or cancelled).
    fn multi_key(&mut self, mut m: MultiEdit, k: KeyEvent) -> Option<MultiEdit> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let w = m.w.max(1);
        let page = m.h.saturating_sub(1).max(1);
        let vim = m.input.vim_enabled();
        let mode = m.input.mode;
        m.note = None;
        let was_armed = std::mem::replace(&mut m.armed, false);
        let (mut accept, mut cancel) = (false, false);
        self.status = None;

        if ctrl && k.code == KeyCode::Char('s') {
            accept = true;
        } else if mode == Mode::Command {
            // `:` command line (`:%s/find/new/g`, `:w`, `:wq`).
            match k.code {
                KeyCode::Char('u') if ctrl => m.input.command_clear(),
                KeyCode::Char('w') if ctrl => m.input.command_delete_word(),
                KeyCode::Char(ch) if !ctrl && !alt => m.input.command_push(ch),
                KeyCode::Backspace => m.input.command_backspace(),
                KeyCode::Esc => m.input.cancel_command(),
                KeyCode::Enter => {
                    match m.input.run_command() {
                        Ok(msg) if !msg.is_empty() => m.note = Some((msg, false)),
                        Ok(_) => {}
                        Err(e) => m.note = Some((e, true)),
                    }
                    // `:w` and `:wq` both accept the text and close.
                    if m.input.take_write_req() | m.input.take_quit_req() {
                        accept = true;
                    }
                }
                _ => {}
            }
        } else if vim
            && ctrl
            && !alt
            && matches!(k.code, KeyCode::Char('u') | KeyCode::Char('d'))
            && matches!(mode, Mode::Normal | Mode::Visual)
        {
            let dir = if k.code == KeyCode::Char('u') { -1 } else { 1 };
            m.input.move_rows(dir, (m.h / 2).max(1), w);
        } else if matches!(mode, Mode::Normal | Mode::Visual)
            && !ctrl
            && !alt
            && matches!(k.code, KeyCode::Char(_))
        {
            // NORMAL / VISUAL mode: letters are vim commands.
            if let KeyCode::Char(ch) = k.code {
                m.input.normal_key(ch, w);
                if let Some(text) = m.input.take_clipboard() {
                    m.note = Some(crate::app::copy_to_clipboard(&text));
                }
            }
        } else {
            match k.code {
                KeyCode::Esc => match mode {
                    Mode::Visual => m.input.exit_visual(),
                    // Like vim: Esc leaves INSERT mode.
                    Mode::Insert if vim => m.input.enter_normal(),
                    _ => {
                        m.input.cancel_pending();
                        if m.input.text != m.orig && !was_armed {
                            m.armed = true;
                            m.note = Some((
                                "changed: Esc again drops it, Ctrl+S accepts".to_string(),
                                true,
                            ));
                        } else {
                            cancel = true;
                        }
                    }
                },
                KeyCode::Enter if m.single => accept = true,
                KeyCode::Enter => match mode {
                    Mode::Insert => m.input.insert_char('\n'),
                    Mode::Normal => multi_vertical(&mut m, 1),
                    _ => {}
                },
                KeyCode::Backspace => m.input.backspace(),
                KeyCode::Delete => m.input.delete(),
                KeyCode::Left if alt && !ctrl => m.input.word_left(w),
                KeyCode::Right if alt && !ctrl => m.input.word_right(w),
                KeyCode::Left => m.input.left(),
                KeyCode::Right => m.input.right(),
                KeyCode::Up => multi_vertical(&mut m, -1),
                KeyCode::Down => multi_vertical(&mut m, 1),
                KeyCode::Home => m.input.home(),
                KeyCode::End => m.input.end(),
                KeyCode::PageUp => (0..page).for_each(|_| multi_vertical(&mut m, -1)),
                KeyCode::PageDown => (0..page).for_each(|_| multi_vertical(&mut m, 1)),
                // The Ctrl shortcuts exist only with the vim modes on.
                KeyCode::Char('a') if ctrl && vim => m.input.home(),
                KeyCode::Char('e') if ctrl && vim => m.input.end(),
                KeyCode::Char('w') if ctrl && vim => m.input.delete_word_back(),
                // INSERT mode; typing the NORMAL-mode keys (`qq`) quickly
                // switches to NORMAL mode.
                KeyCode::Char(ch) if (!ctrl && !alt) || (ctrl && alt) => m.input.type_char(ch),
                _ => {}
            }
        }
        m.input.clamp_normal();

        if accept {
            let text = m.input.text.replace("\r\n", "\n").replace('\r', "\n");
            let done = if m.single {
                self.commit_edit(&text)
            } else {
                self.commit_value(text)
            };
            match done {
                Ok(()) => return None,
                Err(e) => m.note = Some((e, true)),
            }
        }
        if cancel {
            return None;
        }
        Some(m)
    }

    fn on_paste(&mut self, s: &str) {
        if let Modal::Multi(m) = &mut self.modal {
            if m.input.mode == Mode::Command {
                return;
            }
            let text = s.replace("\r\n", "\n").replace('\r', "\n");
            if m.single {
                // One line: a pasted line break becomes `\n`, other control
                // characters are dropped.
                let one: String = text
                    .chars()
                    .filter(|c| *c == '\n' || !c.is_control())
                    .map(|c| if c == '\n' { "\\n".to_string() } else { c.to_string() })
                    .collect();
                m.input.paste_text(&one);
            } else {
                m.input.paste_text(&text);
            }
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        if !matches!(self.modal, Modal::None) {
            return;
        }
        match m.kind {
            MouseEventKind::ScrollUp => {
                self.follow = false;
                self.scroll = self.scroll.saturating_sub(3);
            }
            MouseEventKind::ScrollDown => {
                self.follow = false;
                self.scroll += 3;
            }
            MouseEventKind::Down(MouseButton::Left)
                if self.rows_rect.contains(Position::new(m.column, m.row)) =>
            {
                let r = self.scroll + (m.row - self.rows_rect.y) as usize;
                if matches!(self.rows.get(r), Some(Row::Field(_))) {
                    // A click selects; a click on the selected row acts on it.
                    let was = self.row == r;
                    self.row = r;
                    self.follow = true;
                    self.status = None;
                    if was {
                        self.begin_edit();
                    }
                }
            }
            _ => {}
        }
    }
}

/// Up (`dir < 0`) / Down one display row in the story string window.
fn multi_vertical(m: &mut MultiEdit, dir: i32) {
    let cur = m.input.cursor.min(m.input.len_chars());
    m.input.cursor = ui::vertical_target(&m.input.text, cur, m.w.max(1), dir);
}

// ----------------------------------------------------------------- drawing

/// Columns a character takes on the screen (CJK and full-width forms take 2).
fn cw(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

fn dwidth(s: &str) -> usize {
    s.chars().map(cw).sum()
}

/// `s` cut to `w` columns, with an ellipsis when something is left out.
fn clip(s: &str, w: usize) -> String {
    if dwidth(s) <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let k = cw(c);
        if used + k > w - 1 {
            break;
        }
        out.push(c);
        used += k;
    }
    out.push('\u{2026}');
    out
}

/// `buf` cut into screen rows of at most `w` columns, as `(start, end)`
/// character ranges. There is always at least one row, and a last one for the
/// cursor when the text fills its row exactly.
fn wrap_rows(buf: &[char], w: usize) -> Vec<(usize, usize)> {
    let w = w.max(1);
    let mut rows = Vec::new();
    let mut start = 0;
    let mut col = 0;
    for (i, c) in buf.iter().enumerate() {
        let k = cw(*c);
        if col + k > w && i > start {
            rows.push((start, i));
            start = i;
            col = 0;
        }
        col += k;
    }
    rows.push((start, buf.len()));
    if col >= w && !buf.is_empty() {
        rows.push((buf.len(), buf.len()));
    }
    rows
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn row_line(t: &Tui, idx: usize, width: usize, label_w: usize) -> Line<'static> {
    match t.rows[idx] {
        Row::Header(h) => Line::from(Span::styled(
            format!(" {}", h),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
        Row::Field(i) => {
            let fd = &FIELDS[i];
            let v = t.fmt.get(fd);
            let (text, empty) = match fd.kind {
                Kind::Toggle => {
                    let s = if v.as_bool().unwrap_or(false) { "[x] on" } else { "[ ] off" };
                    (s.to_string(), false)
                }
                _ => {
                    let s = formatting::esc(v.as_str().unwrap_or(""));
                    if s.is_empty() {
                        ("(empty)".to_string(), true)
                    } else {
                        (s, false)
                    }
                }
            };
            let label = format!("  {:<w$} ", clip(fd.label, label_w), w = label_w);
            let value = clip(&text, width.saturating_sub(label_w + 3));
            if idx == t.row {
                let line = format!("{}{}", label, value);
                let pad = " ".repeat(width.saturating_sub(dwidth(&line)));
                Line::from(Span::styled(
                    format!("{}{}", line, pad),
                    Style::default().fg(Color::Black).bg(Color::Cyan),
                ))
            } else {
                let vs = if empty { Style::default().fg(DIM) } else { Style::default() };
                Line::from(vec![Span::raw(label), Span::styled(value, vs)])
            }
        }
    }
}

/// The mode as the chat box shows it: black on a colour, on the bottom edge.
fn mode_tag(input: &Input) -> Span<'static> {
    let (tag, color) = match input.mode {
        Mode::Insert => (" INSERT ", Color::Green),
        Mode::Normal => (" NORMAL ", Color::Magenta),
        Mode::Command => (" COMMAND ", Color::LightBlue),
        Mode::Visual => {
            if input.visual_linewise() {
                (" V-LINE ", Color::Yellow)
            } else {
                (" VISUAL ", Color::Yellow)
            }
        }
    };
    Span::styled(
        tag,
        Style::default()
            .fg(Color::Black)
            .bg(color)
            .add_modifier(Modifier::BOLD),
    )
}

/// The window of a text setting: the text as lines, the cursor, a VISUAL
/// selection, the mode on the bottom edge and, in a row below the text, the
/// `:` command, a message or the keys. A one-line setting gets a small window;
/// both grow with the text, up to the screen.
fn draw_multi(f: &mut Frame, area: Rect, title: &str, m: &mut MultiEdit) {
    let w = if m.single {
        area.width.saturating_sub(4).clamp(20, 64)
    } else {
        area.width.saturating_sub(4).clamp(20, 110)
    };
    let iw = (w as usize).saturating_sub(2).max(1);
    let (lines, (cr, cc), starts) = ui::layout_input(&m.input.text, m.input.cursor, iw);
    // Text rows: what the text needs, at least one (a few more for the story
    // string, which is for long texts), at most what the screen has room for
    // next to the borders (2) and the row below the text (1).
    let min_rows = if m.single { 1 } else { 12 };
    let max_rows = (area.height as usize).saturating_sub(5).max(1);
    let rows = lines.len().max(min_rows).min(max_rows);
    let rect = centered(area, w, rows as u16 + 3);
    f.render_widget(Clear, rect);
    m.w = iw;
    m.h = rows;
    if cr < m.scroll {
        m.scroll = cr;
    } else if cr >= m.scroll + rows {
        m.scroll = cr + 1 - rows;
    }
    m.scroll = m.scroll.min(lines.len().saturating_sub(1));
    let sel = m.input.selection();
    let hi = Style::default().fg(Color::Black).bg(Color::Cyan);
    let mut shown: Vec<Line> = Vec::new();
    for r in m.scroll..(m.scroll + rows).min(lines.len()) {
        match sel {
            Some((a, b)) => {
                let spans: Vec<Span> = lines[r]
                    .chars()
                    .enumerate()
                    .map(|(j, c)| {
                        let i = starts[r] + j;
                        let st = if i >= a && i < b { hi } else { Style::default() };
                        Span::styled(c.to_string(), st)
                    })
                    .collect();
                shown.push(Line::from(spans));
            }
            None => shown.push(Line::from(lines[r].clone())),
        }
    }
    while shown.len() < rows {
        shown.push(Line::from(""));
    }
    let vim = m.input.vim_enabled();
    let command = m.input.mode == Mode::Command;
    let bottom = if command {
        format!(":{}", m.input.command_text())
    } else if let Some((msg, _)) = &m.note {
        format!(" {}", msg)
    } else {
        match (vim, m.single) {
            (true, true) => " Enter accept \u{b7} Esc cancel \u{b7} \\n = line break".to_string(),
            (true, false) => " Ctrl+S accept  :w accept  Esc leaves INSERT, then cancels".to_string(),
            (false, true) => " Enter accept  Esc cancel  \\n = line break".to_string(),
            (false, false) => " Enter new line  Ctrl+S accept  Esc cancel".to_string(),
        }
    };
    let bstyle = match &m.note {
        Some((_, true)) if !command => Style::default().fg(Color::Red),
        Some((_, false)) if !command => Style::default().fg(Color::Green),
        _ => Style::default().fg(DIM),
    };
    shown.push(Line::from(Span::styled(clip(&bottom, iw), bstyle)));
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(format!(" {} ", title));
    // The mode tag only exists with the vim modes on (config `vim.enabled`).
    if vim {
        block = block.title_bottom(Line::from(mode_tag(&m.input)));
    }
    f.render_widget(Paragraph::new(shown).block(block), rect);
    if command {
        let col = 1 + dwidth(&m.input.command_text().to_string());
        f.set_cursor_position((rect.x + 1 + col.min(iw) as u16, rect.y + 1 + rows as u16));
    } else {
        f.set_cursor_position((
            rect.x + 1 + cc.min(iw) as u16,
            rect.y + 1 + (cr - m.scroll.min(cr)) as u16,
        ));
    }
}

fn draw(f: &mut Frame, t: &mut Tui) {
    let area = f.area();
    if area.width < 30 || area.height < 8 {
        f.render_widget(Paragraph::new("too small"), area);
        return;
    }
    // The whole value of the selected text, when it does not fit its row (a
    // story string is several lines long).
    // Its height does not change from one setting to the next, so the list
    // does not jump while moving through it.
    let detail_h: usize = if area.height >= 24 { 3 } else { 0 };
    let detail = if detail_h > 0 {
        t.detail_lines(area.width.saturating_sub(2) as usize, detail_h)
    } else {
        Vec::new()
    };
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(detail_h as u16),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);

    // ---- title
    let mut title = vec![Span::styled(
        " Formatting ",
        Style::default().fg(Color::Black).bg(Color::White),
    )];
    if t.dirty() {
        title.push(Span::styled("  unsaved changes", Style::default().fg(Color::Yellow)));
    }
    f.render_widget(Paragraph::new(Line::from(title)), chunks[0]);

    // ---- settings
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Settings ");
    let inner = block.inner(chunks[1]);
    f.render_widget(block, chunks[1]);
    t.rows_rect = inner;

    let h = inner.height as usize;
    let w = inner.width as usize;
    if t.follow {
        let mut first = t.row;
        // Keep the heading of the group in view too.
        if first > 0 && matches!(t.rows.get(first - 1), Some(Row::Header(_))) {
            first -= 1;
        }
        if first < t.scroll {
            t.scroll = first;
        } else if t.row >= t.scroll + h {
            t.scroll = t.row + 1 - h;
        }
    }
    t.scroll = t.scroll.min(t.rows.len().saturating_sub(h));
    let label_w = if w >= 56 { 24 } else { (w / 2).saturating_sub(2).max(8) };
    let end = (t.scroll + h).min(t.rows.len());
    let lines: Vec<Line> = (t.scroll..end).map(|i| row_line(t, i, w, label_w)).collect();
    f.render_widget(Paragraph::new(lines), inner);

    // ---- full value of the selected text
    if !detail.is_empty() {
        let lines: Vec<Line> = detail
            .into_iter()
            .map(|l| Line::from(Span::styled(format!(" {}", l), Style::default().fg(Color::Gray))))
            .collect();
        f.render_widget(Paragraph::new(lines), chunks[2]);
    }

    // ---- help line
    let help = t.field().map(|fd| fd.help).unwrap_or("");
    f.render_widget(
        Paragraph::new(Span::styled(
            format!(" {}", clip(help, area.width.saturating_sub(2) as usize)),
            Style::default().fg(DIM),
        )),
        chunks[3],
    );

    // ---- footer: question, message or key hints
    let footer = chunks[4];
    if let Modal::Confirm(what) = &t.modal {
        let q = match what {
            Confirm::Quit => " Save changes? [y] save and quit  [n] discard  [any] cancel",
            Confirm::ResetAll => " Reset every setting to its default? [y/N]",
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                clip(q, area.width as usize),
                Style::default().fg(Color::Yellow),
            )),
            footer,
        );
    } else if let Some((msg, err)) = &t.status {
        let color = if *err { Color::Red } else { Color::Green };
        f.render_widget(
            Paragraph::new(Span::styled(
                format!(" {}", clip(msg, area.width.saturating_sub(2) as usize)),
                Style::default().fg(color),
            )),
            footer,
        );
    } else {
        let hints = " \u{2191}\u{2193} select  \u{2190}\u{2192} toggle  Enter edit  Tab section  r reset  u undo  p presets  s save  q quit";
        f.render_widget(
            Paragraph::new(Span::styled(
                clip(hints, area.width as usize),
                Style::default().fg(DIM),
            )),
            footer,
        );
    }

    // ---- text editor
    if matches!(t.modal, Modal::Multi(_)) {
        let title = t.field().map(|fd| fd.label).unwrap_or("");
        if let Modal::Multi(m) = &mut t.modal {
            draw_multi(f, area, title, m);
        }
    }

    // ---- preset picker
    if let Modal::Presets(sec, sel) = &t.modal {
        let list = formatting::presets(*sec);
        let rect = centered(area, 70, list.len() as u16 + 4);
        f.render_widget(Clear, rect);
        let inner_w = rect.width.saturating_sub(2) as usize;
        // The list scrolls when the window is shorter than it.
        let vis = (rect.height as usize).saturating_sub(3).max(1);
        let start = if *sel >= vis { *sel + 1 - vis } else { 0 };
        let mut lines: Vec<Line> = list
            .iter()
            .enumerate()
            .skip(start)
            .take(vis)
            .map(|(i, p)| {
                let style = if i == *sel {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default()
                };
                // `*` marks the preset that is in use.
                let mark = if t.fmt.preset_is_current(p) { "*" } else { " " };
                Line::from(Span::styled(
                    clip(&format!(" {}{:<14} {}", mark, p.name, p.help), inner_w),
                    style,
                ))
            })
            .collect();
        let note = match sec {
            Sec::Context => " Enter apply  Esc cancel  * = in use  (only the story string, separator and chat start change)",
            _ => " Enter apply  Esc cancel  * = in use  (the story string is kept)",
        };
        lines.push(Line::from(Span::styled(clip(note, inner_w), Style::default().fg(DIM))));
        f.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(format!(" {} presets ", sec.title())),
            ),
            rect,
        );
    }
}

// ------------------------------------------------------------------- entry

/// Open the editor in the chat's own terminal and run it until it is quit.
/// With no `formatting.yaml` yet it starts from the built-in defaults.
/// Whatever was saved is in `formatting.yaml` when this returns.
pub async fn manage(
    term: &mut crate::Term,
    events: &mut EventStream,
    cfg: &Input,
) -> Result<()> {
    // A file that cannot be read still opens the editor (on the defaults), so
    // it can be fixed from here; saving keeps a copy of the old file.
    let (fmt, fresh, broken) = match Formatting::load() {
        Ok(Some(f)) => (f, false, None),
        Ok(None) => (Formatting::default(), true, None),
        Err(e) => (Formatting::default(), false, Some(format!("{:#}", e))),
    };
    let mut t = Tui::new(fmt);
    // The same vim settings as the chat box.
    t.cfg.copy_config(cfg);
    if let Some(e) = broken {
        t.backup_first = true;
        t.say(
            format!("{} \u{2014} showing the defaults; saving keeps the old file as formatting.yaml.bad", e),
            true,
        );
    } else if fresh {
        t.say("no formatting.yaml yet: these are the defaults, press s to save them", false);
    }
    loop {
        term.draw(|f| draw(f, &mut t))?;
        match events.next().await {
            Some(Ok(Event::Key(k))) if k.kind != KeyEventKind::Release => t.on_key(k),
            Some(Ok(Event::Mouse(m))) => t.on_mouse(m),
            Some(Ok(Event::Paste(s))) => t.on_paste(&s),
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(e.into()),
            None => break,
        }
        if t.quit {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn rows_break_at_the_width() {
        assert_eq!(wrap_rows(&chars(""), 5), vec![(0, 0)]);
        assert_eq!(wrap_rows(&chars("abc"), 5), vec![(0, 3)]);
        assert_eq!(wrap_rows(&chars("abcdefg"), 5), vec![(0, 5), (5, 7)]);
        // a full row leaves a row for the cursor
        assert_eq!(wrap_rows(&chars("abcde"), 5), vec![(0, 5), (5, 5)]);
    }

    #[test]
    fn wide_characters_count_twice() {
        // each of these takes two columns: three fit in six, not four
        let t = chars("\u{ff5c}\u{ff5c}\u{ff5c}\u{ff5c}");
        assert_eq!(wrap_rows(&t, 6), vec![(0, 3), (3, 4)]);
        assert_eq!(dwidth("a\u{ff5c}"), 3);
        assert_eq!(clip("ab\u{ff5c}cd", 4), "ab\u{2026}");
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("anything", 0), "");
    }

    #[test]
    fn a_change_can_be_undone_and_unsaved_follows_the_content() {
        let mut t = Tui::new(Formatting::default());
        let fd = FIELDS.iter().find(|f| f.key == "wrap").unwrap();
        assert!(!t.dirty());
        t.set(fd, json!(false));
        assert!(t.dirty());
        t.set(fd, json!(true)); // back to what is saved
        assert!(!t.dirty());
        t.undo_last();
        assert!(t.dirty());
        t.undo_last();
        assert!(!t.dirty());
        t.undo_last(); // nothing left
        assert!(!t.dirty());
    }

    #[test]
    fn an_invalid_text_is_refused_and_a_warning_is_kept() {
        let mut t = Tui::new(Formatting::default());
        let i = t.rows.iter().position(|r| matches!(r, Row::Field(i) if FIELDS[*i].key == "story_string_depth")).unwrap();
        t.row = i;
        assert!(t.commit_edit("abc").is_err());
        assert!(!t.dirty());
        assert!(t.commit_edit("4").is_ok());
        assert!(t.dirty());
        let j = t.rows.iter().position(|r| matches!(r, Row::Field(i) if FIELDS[*i].key == "story_string")).unwrap();
        t.row = j;
        assert!(t.commit_edit("{{#if description}}x").is_ok());
        assert!(t.status.as_ref().is_some_and(|(m, err)| *err && m.contains("never closed")));
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn select(t: &mut Tui, name: &str) {
        t.row = t
            .rows
            .iter()
            .position(|r| matches!(r, Row::Field(i) if FIELDS[*i].key == name))
            .unwrap();
    }

    #[test]
    fn the_story_string_is_edited_as_lines_and_ctrl_s_keeps_it() {
        let mut t = Tui::new(Formatting::default());
        select(&mut t, "story_string");
        t.begin_edit();
        assert!(matches!(t.modal, Modal::Multi(_)));
        // It opens in NORMAL mode; `i` goes to INSERT mode, where Enter is a
        // line break and letters are typed.
        t.on_key(key(KeyCode::Char('i'), KeyModifiers::NONE));
        t.on_key(key(KeyCode::Enter, KeyModifiers::NONE));
        t.on_key(key(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(matches!(t.modal, Modal::Multi(_)));
        t.on_key(key(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(matches!(t.modal, Modal::None));
        let fd = FIELDS.iter().find(|f| f.key == "story_string").unwrap();
        assert!(t.fmt.get(fd).as_str().unwrap().starts_with("\nx"));
        assert!(t.dirty());
    }

    #[test]
    fn esc_in_the_story_string_asks_before_dropping_a_change() {
        let mut t = Tui::new(Formatting::default());
        t.cfg.configure(false, vec![]);
        select(&mut t, "story_string");
        t.begin_edit();
        t.on_key(key(KeyCode::Char('x'), KeyModifiers::NONE));
        t.on_key(key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(t.modal, Modal::Multi(_)));
        t.on_key(key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(t.modal, Modal::None));
        assert!(!t.dirty());
    }

    fn mode_of(t: &Tui) -> Mode {
        match &t.modal {
            Modal::Multi(m) => m.input.mode,
            _ => panic!("no editor"),
        }
    }

    #[test]
    fn every_text_opens_in_normal_mode_with_vim_and_in_insert_without() {
        for name in ["story_string", "input_sequence", "story_string_depth", "custom_stopping_strings"] {
            let mut t = Tui::new(Formatting::default());
            select(&mut t, name);
            t.begin_edit();
            assert_eq!(mode_of(&t), Mode::Normal, "{}", name);
            let mut t = Tui::new(Formatting::default());
            t.cfg.configure(false, vec![]);
            select(&mut t, name);
            t.begin_edit();
            assert_eq!(mode_of(&t), Mode::Insert, "{}", name);
        }
    }

    #[test]
    fn a_one_line_text_is_typed_with_vim_and_enter_accepts_it() {
        let mut t = Tui::new(Formatting::default());
        select(&mut t, "input_sequence");
        t.begin_edit();
        assert!(matches!(&t.modal, Modal::Multi(m) if m.single));
        t.on_key(key(KeyCode::Char('i'), KeyModifiers::NONE));
        t.on_key(key(KeyCode::Char('a'), KeyModifiers::NONE));
        t.on_key(key(KeyCode::Char('b'), KeyModifiers::NONE));
        t.on_key(key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(t.modal, Modal::None));
        let fd = FIELDS.iter().find(|f| f.key == "input_sequence").unwrap();
        assert_eq!(t.fmt.get(fd).as_str(), Some("ab"));
        // The same with Enter pressed in NORMAL mode.
        t.begin_edit();
        t.on_key(key(KeyCode::Char('x'), KeyModifiers::NONE)); // the cursor is on the `a`
        t.on_key(key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(t.modal, Modal::None));
        assert_eq!(t.fmt.get(fd).as_str(), Some("b"));
    }

    #[test]
    fn a_one_line_text_opens_with_the_cursor_at_the_start_or_at_the_end_without_vim() {
        let cursor = |t: &Tui| match &t.modal {
            Modal::Multi(m) => m.input.cursor,
            _ => panic!("no editor"),
        };
        let fd = FIELDS.iter().find(|f| f.key == "output_suffix").unwrap();
        // Vim modes on: the very beginning.
        let mut t = Tui::new(Formatting::default());
        t.set(fd, json!("hello"));
        select(&mut t, "output_suffix");
        t.begin_edit();
        assert_eq!(cursor(&t), 0);
        // Vim modes off: the end.
        let mut t = Tui::new(Formatting::default());
        t.cfg.configure(false, vec![]);
        t.set(fd, json!("hello"));
        select(&mut t, "output_suffix");
        t.begin_edit();
        assert_eq!(cursor(&t), 5);
    }

    #[test]
    fn a_one_line_text_shows_line_breaks_as_backslash_n_and_keeps_them() {
        let mut t = Tui::new(Formatting::default());
        t.cfg.configure(false, vec![]);
        let fd = FIELDS.iter().find(|f| f.key == "output_suffix").unwrap();
        t.set(fd, json!("x\ny"));
        select(&mut t, "output_suffix");
        t.begin_edit();
        match &t.modal {
            Modal::Multi(m) => assert_eq!(m.input.text, "x\\ny"),
            _ => panic!("no editor"),
        }
        t.on_key(key(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(t.fmt.get(fd).as_str(), Some("x\ny"));
    }

    #[test]
    fn a_pasted_line_break_in_a_one_line_text_becomes_backslash_n() {
        let mut t = Tui::new(Formatting::default());
        t.cfg.configure(false, vec![]);
        select(&mut t, "input_sequence");
        t.begin_edit();
        t.on_paste("a\r\nb\u{7}");
        match &t.modal {
            Modal::Multi(m) => assert_eq!(m.input.text, "a\\nb"),
            _ => panic!("no editor"),
        }
    }

    #[test]
    fn a_one_line_text_is_checked_before_it_is_accepted() {
        let mut t = Tui::new(Formatting::default());
        select(&mut t, "story_string_depth");
        t.begin_edit();
        t.on_key(key(KeyCode::Char('A'), KeyModifiers::NONE));
        t.on_key(key(KeyCode::Char('z'), KeyModifiers::NONE));
        t.on_key(key(KeyCode::Enter, KeyModifiers::NONE));
        // Not a number: the window stays open and says why.
        assert!(matches!(&t.modal, Modal::Multi(m) if matches!(m.note, Some((_, true)))));
        assert!(!t.dirty());
    }

    #[test]
    fn the_mode_tag_has_a_background_colour() {
        let mut i = Input::default();
        for (mode, bg) in [
            (Mode::Insert, Color::Green),
            (Mode::Normal, Color::Magenta),
            (Mode::Command, Color::LightBlue),
        ] {
            i.mode = mode;
            let tag = mode_tag(&i);
            assert_eq!(tag.style.bg, Some(bg));
            assert_eq!(tag.style.fg, Some(Color::Black));
        }
    }

    #[test]
    fn tab_walks_the_sections() {
        let mut t = Tui::new(Formatting::default());
        let first = t.row;
        t.jump_section(1);
        assert!(t.row > first);
        assert!(matches!(t.rows[t.row - 1], Row::Header(_)));
        t.jump_section(-1);
        assert_eq!(t.row, first);
        t.jump_section(-1); // wraps to the last section
        assert!(t.row > first);
    }
}
