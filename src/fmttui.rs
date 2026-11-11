//! `/formatting`: a full-screen editor for the context template, the instruct
//! template and the reply-cleanup switches (see `formatting`), opened from
//! inside a chat like `/api`. Which settings exist comes from
//! `formatting::FIELDS`, so a new one needs no changes here.

use crate::formatting::{self, Field, Formatting, Kind, Sec, FIELDS};
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
    /// Typing a value for the selected row (a line break is typed as `\n`).
    Edit { buf: Vec<char>, cur: usize },
    Confirm(Confirm),
    /// Choosing a preset of this template (the one the cursor is in).
    Presets(Sec, usize),
}

struct Tui {
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
        let buf: Vec<char> = formatting::esc(v.as_str().unwrap_or("")).chars().collect();
        let cur = buf.len();
        self.modal = Modal::Edit { buf, cur };
    }

    /// Store a typed text. `Err` (the editor stays open) when the setting
    /// cannot hold it; things that are accepted but look wrong are said in the
    /// status line.
    fn commit_edit(&mut self, text: &str) -> Result<(), String> {
        let Some(fd) = self.field() else {
            return Ok(());
        };
        let value = formatting::unesc(text);
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
            Some(sec) if !formatting::presets(sec).is_empty() => self.modal = Modal::Presets(sec, 0),
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
            Modal::Edit { mut buf, mut cur } => {
                self.status = None;
                let mut keep = true;
                match k.code {
                    KeyCode::Esc => keep = false,
                    KeyCode::Enter => {
                        let text: String = buf.iter().collect();
                        self.status = None;
                        match self.commit_edit(&text) {
                            Ok(()) => keep = false,
                            Err(msg) => self.say(msg, true),
                        }
                    }
                    KeyCode::Char('u') if ctrl => {
                        buf.clear();
                        cur = 0;
                    }
                    KeyCode::Char('a') if ctrl => cur = 0,
                    KeyCode::Char('e') if ctrl => cur = buf.len(),
                    KeyCode::Char('k') if ctrl => buf.truncate(cur),
                    KeyCode::Char('w') if ctrl => {
                        let to = word_left(&buf, cur);
                        buf.drain(to..cur);
                        cur = to;
                    }
                    KeyCode::Left if ctrl => cur = word_left(&buf, cur),
                    KeyCode::Right if ctrl => cur = word_right(&buf, cur),
                    KeyCode::Char(c) if !ctrl => {
                        buf.insert(cur, c);
                        cur += 1;
                    }
                    KeyCode::Backspace => {
                        if cur > 0 {
                            buf.remove(cur - 1);
                            cur -= 1;
                        }
                    }
                    KeyCode::Delete => {
                        if cur < buf.len() {
                            buf.remove(cur);
                        }
                    }
                    KeyCode::Left => cur = cur.saturating_sub(1),
                    KeyCode::Right => cur = (cur + 1).min(buf.len()),
                    KeyCode::Home => cur = 0,
                    KeyCode::End => cur = buf.len(),
                    _ => {}
                }
                if keep {
                    self.modal = Modal::Edit { buf, cur };
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

    fn on_paste(&mut self, s: &str) {
        if let Modal::Edit { buf, cur } = &mut self.modal {
            for c in s.chars() {
                match c {
                    '\n' => {
                        buf.insert(*cur, '\\');
                        buf.insert(*cur + 1, 'n');
                        *cur += 2;
                    }
                    c if c.is_control() => {}
                    c => {
                        buf.insert(*cur, c);
                        *cur += 1;
                    }
                }
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

/// Where the word before `cur` starts.
fn word_left(buf: &[char], cur: usize) -> usize {
    let mut i = cur.min(buf.len());
    while i > 0 && buf[i - 1].is_whitespace() {
        i -= 1;
    }
    while i > 0 && !buf[i - 1].is_whitespace() {
        i -= 1;
    }
    i
}

/// Where the word after `cur` ends.
fn word_right(buf: &[char], cur: usize) -> usize {
    let mut i = cur.min(buf.len());
    while i < buf.len() && buf[i].is_whitespace() {
        i += 1;
    }
    while i < buf.len() && !buf[i].is_whitespace() {
        i += 1;
    }
    i
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

fn draw_edit(f: &mut Frame, area: Rect, title: &str, buf: &[char], cur: usize) {
    let w = area.width.saturating_sub(4).clamp(20, 100);
    let iw = (w - 2) as usize;
    let rows = wrap_rows(buf, iw);
    let h = rows.len().min(area.height.saturating_sub(5) as usize).max(1);
    let rect = centered(area, w, h as u16 + 3);
    f.render_widget(Clear, rect);
    // The row the cursor is on (a boundary between two rows belongs to the later one).
    let cr = rows.iter().rposition(|(a, _)| *a <= cur).unwrap_or(0);
    let ccol: usize = buf[rows[cr].0..cur.min(buf.len())].iter().map(|c| cw(*c)).sum();
    let start = if cr >= h { cr + 1 - h } else { 0 };
    let mut lines: Vec<Line> = rows
        .iter()
        .skip(start)
        .take(h)
        .map(|(a, b)| Line::from(buf[*a..*b].iter().collect::<String>()))
        .collect();
    lines.push(Line::from(Span::styled(
        clip(" Enter accept  Esc cancel  \\n = line break  Ctrl+A/E/W/K/U edit", iw),
        Style::default().fg(DIM),
    )));
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(format!(" {} ", title)),
        ),
        rect,
    );
    f.set_cursor_position((
        rect.x + 1 + (ccol.min(iw)) as u16,
        rect.y + 1 + (cr - start) as u16,
    ));
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
    if let Modal::Edit { buf, cur } = &t.modal {
        let title = t.field().map(|fd| fd.label).unwrap_or("");
        draw_edit(f, area, title, buf, *cur);
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
                Line::from(Span::styled(
                    clip(&format!(" {:<14} {}", p.name, p.help), inner_w),
                    style,
                ))
            })
            .collect();
        let note = match sec {
            Sec::Context => " Enter apply  Esc cancel  (only the story string, separator and chat start change)",
            _ => " Enter apply  Esc cancel  (the story string is kept)",
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
pub async fn manage(term: &mut crate::Term, events: &mut EventStream) -> Result<()> {
    // A file that cannot be read still opens the editor (on the defaults), so
    // it can be fixed from here; saving keeps a copy of the old file.
    let (fmt, fresh, broken) = match Formatting::load() {
        Ok(Some(f)) => (f, false, None),
        Ok(None) => (Formatting::default(), true, None),
        Err(e) => (Formatting::default(), false, Some(format!("{:#}", e))),
    };
    let mut t = Tui::new(fmt);
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
    fn words_are_skipped_whole() {
        let t = chars("one two  three");
        assert_eq!(word_left(&t, 14), 9);
        assert_eq!(word_left(&t, 9), 4);
        assert_eq!(word_left(&t, 0), 0);
        assert_eq!(word_right(&t, 0), 3);
        assert_eq!(word_right(&t, 3), 7);
        assert_eq!(word_right(&t, 14), 14);
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
