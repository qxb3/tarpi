//! `/api`: a full-screen manager for the standalone API connections
//! (see `conn`), opened from inside a chat. Lists the saved connections,
//! edits the server address and API key, and shows every model setting of the
//! selected connection as a slider, switch or text field. Which settings exist
//! comes from the connection kind (`conn::Provider::params`), so a new kind of
//! connection needs no changes here.

use crate::conn::{self, Connection, Info, ParamDef, ParamKind, Store};
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
use serde_json::Value;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// What a connection test came back with (`Err` = a message to show).
type Probe = std::result::Result<Info, String>;

// ------------------------------------------------------------------- state

#[derive(Clone, Copy, PartialEq)]
enum Row {
    Header(&'static str),
    Name,
    Url,
    Key,
    Param(usize),
}

#[derive(Clone, Copy, PartialEq)]
enum Confirm {
    Quit,
    Delete,
    ResetAll,
}

enum Modal {
    None,
    /// Typing a value for the selected row.
    Edit {
        title: String,
        buf: Vec<char>,
        cur: usize,
    },
    Confirm(Confirm),
    /// Choosing the kind of a new connection.
    PickKind(usize),
    /// Choosing a preset (Normal, Creative, ...) for the selected connection.
    PickPreset(usize),
}

struct Tui {
    store: Store,
    /// Selected connection.
    cur: usize,
    /// Settings of the selected connection's kind.
    defs: Vec<ParamDef>,
    rows: Vec<Row>,
    /// Selected row (index into `rows`).
    row: usize,
    scroll: usize,
    /// Scroll to keep the selected row visible (off while using the wheel).
    follow: bool,
    focus_list: bool,
    dirty: bool,
    status: Option<(String, bool)>,
    /// Result of the last test of the selected connection.
    info: Option<Probe>,
    testing: bool,
    modal: Modal,
    quit: bool,
    drag: bool,
    // Where things were drawn, for the mouse.
    list_rect: Rect,
    rows_rect: Rect,
    track_x: u16,
    track_w: u16,
    tx: UnboundedSender<(String, Probe)>,
}

impl Tui {
    fn new(store: Store, tx: UnboundedSender<(String, Probe)>) -> Tui {
        let cur = store
            .connections
            .iter()
            .position(|c| c.name == store.active)
            .unwrap_or(0);
        let mut t = Tui {
            store,
            cur,
            defs: Vec::new(),
            rows: Vec::new(),
            row: 1,
            scroll: 0,
            follow: true,
            focus_list: false,
            dirty: false,
            status: None,
            info: None,
            testing: false,
            modal: Modal::None,
            quit: false,
            drag: false,
            list_rect: Rect::default(),
            rows_rect: Rect::default(),
            track_x: 0,
            track_w: 0,
            tx,
        };
        t.rebuild();
        t
    }

    fn conn(&self) -> Option<&Connection> {
        self.store.connections.get(self.cur)
    }

    fn conn_mut(&mut self) -> Option<&mut Connection> {
        self.store.connections.get_mut(self.cur)
    }

    fn say(&mut self, msg: impl Into<String>, is_error: bool) {
        self.status = Some((msg.into(), is_error));
    }

    /// Recompute the settings list for the selected connection.
    fn rebuild(&mut self) {
        self.defs = self
            .conn()
            .and_then(|c| c.provider())
            .map(|p| p.params())
            .unwrap_or_default();
        self.rows.clear();
        if self.conn().is_some() {
            self.rows.push(Row::Header("Connection"));
            self.rows.push(Row::Name);
            self.rows.push(Row::Url);
            self.rows.push(Row::Key);
            let mut group = "";
            for (i, d) in self.defs.iter().enumerate() {
                if d.group != group {
                    group = d.group;
                    self.rows.push(Row::Header(d.group));
                }
                self.rows.push(Row::Param(i));
            }
        }
        if self.row >= self.rows.len() || matches!(self.rows.get(self.row), Some(Row::Header(_))) {
            self.row = if self.rows.len() > 1 { 1 } else { 0 };
        }
    }

    fn select_conn(&mut self, i: usize) {
        if i >= self.store.connections.len() {
            return;
        }
        self.cur = i;
        self.info = None;
        self.testing = false;
        self.row = 1;
        self.scroll = 0;
        self.follow = true;
        self.rebuild();
        self.test();
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

    fn selected_def(&self) -> Option<(usize, ParamDef)> {
        match self.rows.get(self.row) {
            Some(Row::Param(i)) => self.defs.get(*i).map(|d| (*i, *d)),
            _ => None,
        }
    }

    fn value(&self, d: &ParamDef) -> Value {
        self.conn().map(|c| c.get(d)).unwrap_or(Value::Null)
    }

    /// Store a setting; a value equal to the default is not written at all.
    fn set_param(&mut self, i: usize, v: Value) {
        let Some(d) = self.defs.get(i).copied() else {
            return;
        };
        let is_default = v == d.default_value();
        if let Some(c) = self.conn_mut() {
            if is_default {
                c.params.remove(d.key);
            } else {
                c.set(d.key, v);
            }
            self.dirty = true;
        }
    }

    /// Left / Right on the selected setting (`steps` of its step size).
    fn adjust(&mut self, steps: f64) {
        let Some((i, d)) = self.selected_def() else {
            return;
        };
        let cur = self.value(&d);
        match d.kind {
            ParamKind::Slider { step, .. } => {
                let x = cur.as_f64().unwrap_or(0.0) + steps * step;
                self.set_param(i, d.snap(x));
            }
            ParamKind::Number { .. } => {
                let x = cur.as_f64().unwrap_or(0.0) + steps;
                self.set_param(i, d.snap(x));
            }
            ParamKind::Toggle { .. } => {
                let on = cur.as_bool().unwrap_or(false);
                self.set_param(i, Value::Bool(!on));
            }
            ParamKind::Text { .. } => {}
        }
    }

    /// Home / End on a slider.
    fn to_end(&mut self, max: bool) {
        let Some((i, d)) = self.selected_def() else {
            return;
        };
        if let ParamKind::Slider { min, max: hi, .. } = d.kind {
            self.set_param(i, d.snap(if max { hi } else { min }));
        }
    }

    fn set_from_x(&mut self, x: u16) {
        let Some((i, d)) = self.selected_def() else {
            return;
        };
        if let ParamKind::Slider { min, max, .. } = d.kind {
            if self.track_w < 2 {
                return;
            }
            let dx = x.saturating_sub(self.track_x).min(self.track_w - 1) as f64;
            let frac = dx / (self.track_w - 1) as f64;
            self.set_param(i, d.snap(min + frac * (max - min)));
        }
    }

    fn reset_row(&mut self) {
        let Some((i, d)) = self.selected_def() else {
            return;
        };
        self.set_param(i, d.default_value());
        self.say(format!("{} reset to its default", d.label), false);
    }

    fn reset_all(&mut self) {
        if let Some(c) = self.conn_mut() {
            c.params.clear();
            self.dirty = true;
        }
        self.say("every setting is back to its default", false);
    }

    /// The presets of the selected connection's kind.
    fn presets(&self) -> Vec<conn::Preset> {
        self.conn()
            .and_then(|c| c.provider())
            .map(|p| p.presets())
            .unwrap_or_default()
    }

    /// Whether every setting of `p` already has the preset's value.
    fn preset_is_current(&self, p: &conn::Preset) -> bool {
        p.values.iter().all(|(k, v)| {
            let Some(d) = self.defs.iter().find(|d| d.key == *k) else {
                return true;
            };
            let cur = self.value(d);
            match (cur.as_f64(), v.as_f64()) {
                (Some(a), Some(b)) => (a - b).abs() < 1e-9,
                _ => cur == *v,
            }
        })
    }

    /// Open the preset list (`p`), with the cursor on the preset in use.
    fn open_presets(&mut self) {
        let presets = self.presets();
        if presets.is_empty() {
            self.say("this connection has no presets", true);
            return;
        }
        let at = presets.iter().position(|p| self.preset_is_current(p)).unwrap_or(0);
        self.modal = Modal::PickPreset(at);
    }

    /// Write every value of preset number `i` into the connection.
    fn apply_preset(&mut self, i: usize) {
        let Some(preset) = self.presets().into_iter().nth(i) else {
            return;
        };
        for (key, v) in preset.values {
            if let Some(idx) = self.defs.iter().position(|d| d.key == key) {
                self.set_param(idx, v);
            }
        }
        self.say(
            format!("preset '{}' applied (press s to save)", preset.name),
            false,
        );
    }

    fn use_server_ctx(&mut self) {
        let n = match &self.info {
            Some(Ok(i)) => i.n_ctx,
            _ => None,
        };
        let Some(n) = n else {
            self.say("no context size known yet: test the connection first (t)", true);
            return;
        };
        match self.defs.iter().position(|d| d.key == "max_context") {
            Some(i) => {
                let d = self.defs[i];
                self.set_param(i, d.snap(n as f64));
                self.say(format!("context size set to the server's {}", n), false);
            }
            None => self.say("this connection has no context size setting", true),
        }
    }

    // ------------------------------------------------------------ editing

    fn begin_edit(&mut self) {
        let Some(row) = self.rows.get(self.row).copied() else {
            return;
        };
        let (title, text) = match row {
            Row::Header(_) => return,
            Row::Name => (
                "Name".to_string(),
                self.conn().map(|c| c.name.clone()).unwrap_or_default(),
            ),
            Row::Url => (
                "Server address".to_string(),
                self.conn().map(|c| c.url.clone()).unwrap_or_default(),
            ),
            Row::Key => (
                "API key".to_string(),
                self.conn().map(|c| c.api_key.clone()).unwrap_or_default(),
            ),
            Row::Param(i) => {
                let Some(d) = self.defs.get(i).copied() else {
                    return;
                };
                if matches!(d.kind, ParamKind::Toggle { .. }) {
                    self.adjust(1.0);
                    return;
                }
                let v = self.value(&d);
                (d.label.to_string(), d.format(&v))
            }
        };
        let buf: Vec<char> = text.chars().collect();
        let cur = buf.len();
        self.modal = Modal::Edit { title, buf, cur };
    }

    /// Apply typed text to the selected row. `Err` keeps the editor open.
    fn commit_edit(&mut self, text: &str) -> std::result::Result<(), String> {
        let Some(row) = self.rows.get(self.row).copied() else {
            return Ok(());
        };
        match row {
            Row::Header(_) => {}
            Row::Name => {
                let n = text.trim().to_string();
                if n.is_empty() {
                    return Err("the name cannot be empty".to_string());
                }
                let cur = self.cur;
                if self
                    .store
                    .connections
                    .iter()
                    .enumerate()
                    .any(|(i, c)| i != cur && c.name == n)
                {
                    return Err(format!("there is already a connection named '{}'", n));
                }
                let old = self.conn().map(|c| c.name.clone()).unwrap_or_default();
                if self.store.active == old {
                    self.store.active = n.clone();
                }
                if let Some(c) = self.conn_mut() {
                    c.name = n;
                }
                self.dirty = true;
            }
            Row::Url => {
                if let Some(c) = self.conn_mut() {
                    c.url = text.trim().to_string();
                }
                self.dirty = true;
                self.info = None;
            }
            Row::Key => {
                if let Some(c) = self.conn_mut() {
                    c.api_key = text.trim().to_string();
                }
                self.dirty = true;
                self.info = None;
            }
            Row::Param(i) => {
                let Some(d) = self.defs.get(i).copied() else {
                    return Ok(());
                };
                match d.kind {
                    ParamKind::Slider { .. } | ParamKind::Number { .. } => {
                        let x: f64 = text
                            .trim()
                            .replace(',', ".")
                            .parse()
                            .map_err(|_| "type a number".to_string())?;
                        self.set_param(i, d.snap(x));
                    }
                    ParamKind::Text { .. } => {
                        if let Some(p) = self.conn().and_then(|c| c.provider()) {
                            p.validate_text(d.key, text)?;
                        }
                        self.set_param(i, Value::String(text.to_string()));
                    }
                    ParamKind::Toggle { .. } => {}
                }
            }
        }
        Ok(())
    }

    // --------------------------------------------------------- connections

    fn test(&mut self) {
        let Some(c) = self.conn().cloned() else {
            return;
        };
        self.testing = true;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let res = conn::probe(&c).await.map_err(|e| format!("{:#}", e));
            let _ = tx.send((c.name.clone(), res));
        });
    }

    fn on_probe(&mut self, name: String, res: Probe) {
        if self.conn().map(|c| c.name == name).unwrap_or(false) {
            self.info = Some(res);
            self.testing = false;
        }
    }

    fn activate(&mut self) {
        let Some(name) = self.conn().map(|c| c.name.clone()) else {
            return;
        };
        self.store.active = name.clone();
        self.dirty = true;
        self.say(format!("'{}' will be used for chats (press s to save)", name), false);
    }

    fn new_connection(&mut self) {
        let kinds = conn::providers();
        match kinds.len() {
            0 => {}
            1 => self.add_connection(kinds[0].kind()),
            _ => self.modal = Modal::PickKind(0),
        }
    }

    fn add_connection(&mut self, kind: &str) {
        let name = self.store.unique_name(kind);
        let Some(c) = Connection::new(kind, &name) else {
            return;
        };
        self.store.connections.push(c);
        if self.store.active.is_empty() {
            self.store.active = name;
        }
        self.dirty = true;
        let last = self.store.connections.len() - 1;
        self.select_conn(last);
        self.say("new connection: set its address, then press s to save", false);
    }

    fn delete_connection(&mut self) {
        if self.cur >= self.store.connections.len() {
            return;
        }
        let gone = self.store.connections.remove(self.cur);
        if self.store.active == gone.name {
            self.store.active = self
                .store
                .connections
                .first()
                .map(|c| c.name.clone())
                .unwrap_or_default();
        }
        self.dirty = true;
        self.cur = self.cur.min(self.store.connections.len().saturating_sub(1));
        self.info = None;
        self.testing = false;
        self.row = 1;
        self.rebuild();
        self.say(format!("deleted '{}' (press s to save)", gone.name), false);
        self.test();
    }

    fn save(&mut self) {
        if !self.store.connections.iter().any(|c| c.name == self.store.active) {
            self.store.active = self
                .store
                .connections
                .first()
                .map(|c| c.name.clone())
                .unwrap_or_default();
        }
        match self.store.save() {
            Ok(()) => {
                self.dirty = false;
                self.say("saved: chats pick the change up on the next reply", false);
            }
            Err(e) => self.say(format!("could not save: {:#}", e), true),
        }
    }

    fn try_quit(&mut self) {
        if self.dirty {
            self.modal = Modal::Confirm(Confirm::Quit);
        } else {
            self.quit = true;
        }
    }

    // ------------------------------------------------------------- events

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(k.code, KeyCode::Char('c')) {
            self.try_quit();
            return;
        }
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::Edit { title, mut buf, mut cur } => {
                let mut keep = true;
                match k.code {
                    KeyCode::Esc => keep = false,
                    KeyCode::Enter => {
                        let text: String = buf.iter().collect();
                        match self.commit_edit(&text) {
                            Ok(()) => {
                                self.status = None;
                                keep = false;
                            }
                            Err(m) => self.say(m, true),
                        }
                    }
                    KeyCode::Char('u') if ctrl => {
                        buf.clear();
                        cur = 0;
                    }
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
                    self.modal = Modal::Edit { title, buf, cur };
                }
            }
            Modal::Confirm(what) => {
                let yes = matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y'));
                match what {
                    Confirm::Quit => match k.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('s') => {
                            self.save();
                            self.quit = !self.dirty;
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') => self.quit = true,
                        KeyCode::Char('q') if ctrl => self.quit = true,
                        _ => {}
                    },
                    Confirm::Delete => {
                        if yes {
                            self.delete_connection();
                        }
                    }
                    Confirm::ResetAll => {
                        if yes {
                            self.reset_all();
                        }
                    }
                }
            }
            Modal::PickKind(mut i) => {
                let kinds = conn::providers();
                let mut keep = true;
                match k.code {
                    KeyCode::Esc => keep = false,
                    KeyCode::Up | KeyCode::Char('k') => i = i.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => {
                        i = (i + 1).min(kinds.len().saturating_sub(1))
                    }
                    KeyCode::Enter => {
                        keep = false;
                        if let Some(p) = kinds.get(i) {
                            self.add_connection(p.kind());
                        }
                    }
                    _ => {}
                }
                if keep {
                    self.modal = Modal::PickKind(i);
                }
            }
            Modal::PickPreset(mut i) => {
                let count = self.presets().len();
                let mut keep = true;
                match k.code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('p') => keep = false,
                    KeyCode::Up | KeyCode::Char('k') => i = i.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => {
                        i = (i + 1).min(count.saturating_sub(1))
                    }
                    KeyCode::Char(c @ '1'..='9') => {
                        let n = c as usize - '1' as usize;
                        if n < count {
                            i = n;
                        }
                    }
                    KeyCode::Enter => {
                        keep = false;
                        self.apply_preset(i);
                    }
                    _ => {}
                }
                if keep {
                    self.modal = Modal::PickPreset(i);
                }
            }
            Modal::None => self.on_key_plain(k),
        }
    }

    fn on_key_plain(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let big = if shift { 10.0 } else { 1.0 };
        self.status = None;
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.try_quit(),
            KeyCode::Char('s') => self.save(),
            KeyCode::Tab | KeyCode::BackTab => {
                if self.list_rect.width > 0 {
                    self.focus_list = !self.focus_list;
                }
            }
            KeyCode::Char('[') => {
                if self.cur > 0 {
                    self.select_conn(self.cur - 1);
                }
            }
            KeyCode::Char(']') => {
                if self.cur + 1 < self.store.connections.len() {
                    self.select_conn(self.cur + 1);
                }
            }
            KeyCode::Char('n') => self.new_connection(),
            KeyCode::Char('d') if self.conn().is_some() => {
                self.modal = Modal::Confirm(Confirm::Delete)
            }
            KeyCode::Char('a') => self.activate(),
            KeyCode::Char('p') if self.conn().is_some() => self.open_presets(),
            KeyCode::Char('t') => {
                self.info = None;
                self.test();
            }
            KeyCode::Char('c') if !ctrl => self.use_server_ctx(),
            KeyCode::Char('R') if self.conn().is_some() => {
                self.modal = Modal::Confirm(Confirm::ResetAll)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.focus_list {
                    if self.cur > 0 {
                        self.select_conn(self.cur - 1);
                    }
                } else {
                    self.move_row(-1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.focus_list {
                    if self.cur + 1 < self.store.connections.len() {
                        self.select_conn(self.cur + 1);
                    }
                } else {
                    self.move_row(1);
                }
            }
            KeyCode::PageUp => {
                for _ in 0..8 {
                    self.move_row(-1);
                }
            }
            KeyCode::PageDown => {
                for _ in 0..8 {
                    self.move_row(1);
                }
            }
            KeyCode::Left | KeyCode::Char('h') if !self.focus_list => self.adjust(-big),
            KeyCode::Right | KeyCode::Char('l') if !self.focus_list => self.adjust(big),
            KeyCode::Char('H') if !self.focus_list => self.adjust(-10.0),
            KeyCode::Char('L') if !self.focus_list => self.adjust(10.0),
            KeyCode::Home => match self.selected_def() {
                Some(_) => self.to_end(false),
                None => {
                    self.row = if self.rows.len() > 1 { 1 } else { 0 };
                    self.follow = true;
                }
            },
            KeyCode::End => match self.selected_def() {
                Some(_) => self.to_end(true),
                None => {
                    self.row = self.rows.len().saturating_sub(1);
                    self.follow = true;
                }
            },
            KeyCode::Enter => {
                if self.focus_list {
                    self.focus_list = false;
                } else {
                    self.begin_edit();
                }
            }
            KeyCode::Char('r') | KeyCode::Backspace | KeyCode::Delete => self.reset_row(),
            _ => {}
        }
    }

    fn on_paste(&mut self, s: &str) {
        if let Modal::Edit { buf, cur, .. } = &mut self.modal {
            for c in s.chars().filter(|c| !c.is_control()) {
                buf.insert(*cur, c);
                *cur += 1;
            }
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        if !matches!(self.modal, Modal::None) {
            return;
        }
        let pos = Position::new(m.column, m.row);
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                self.follow = false;
                if m.kind == MouseEventKind::ScrollUp {
                    self.scroll = self.scroll.saturating_sub(3);
                } else {
                    self.scroll += 3;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self.list_rect.contains(pos) {
                    self.focus_list = true;
                    let idx = (m.row - self.list_rect.y).saturating_sub(1) as usize;
                    if idx < self.store.connections.len() {
                        self.select_conn(idx);
                    } else if idx == self.store.connections.len() {
                        self.new_connection();
                    }
                } else if self.rows_rect.contains(pos) {
                    self.focus_list = false;
                    let r = self.scroll + (m.row - self.rows_rect.y) as usize;
                    let hit = self.rows.get(r).copied();
                    match hit {
                        Some(Row::Header(_)) | None => {}
                        Some(_) => {
                            self.row = r;
                            self.follow = true;
                            if let Some((_, d)) = self.selected_def() {
                                match d.kind {
                                    ParamKind::Slider { .. } => {
                                        if self.track_w > 0
                                            && m.column >= self.track_x
                                            && m.column < self.track_x + self.track_w
                                        {
                                            self.drag = true;
                                            self.set_from_x(m.column);
                                        }
                                    }
                                    ParamKind::Toggle { .. } => self.adjust(1.0),
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.drag {
                    self.set_from_x(m.column);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.drag = false,
            _ => {}
        }
    }
}

// ----------------------------------------------------------------- drawing

fn clip(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(w - 1).collect();
    out.push('\u{2026}');
    out
}

fn host_of(url: &str) -> String {
    let u = url.trim();
    let u = u.split_once("://").map(|(_, r)| r).unwrap_or(u);
    u.split('/').next().unwrap_or("").to_string()
}

const DIM: Color = Color::Rgb(128, 128, 128);

/// Column layout of the settings pane.
struct Cols {
    label: usize,
    value: usize,
    track: usize,
}

fn row_line(t: &Tui, idx: usize, width: usize, cols: &Cols) -> Line<'static> {
    let Some(row) = t.rows.get(idx).copied() else {
        return Line::from("");
    };
    if let Row::Header(g) = row {
        return Line::from(Span::styled(
            format!(" {}", g),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }
    let selected = idx == t.row && !t.focus_list;
    let marker = if selected { "\u{25b8} " } else { "  " };
    let label_style = if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let c = t.conn();
    let rest = width.saturating_sub(2 + cols.label + 1);

    let (label, value, changed, track): (String, String, bool, Option<f64>) = match row {
        Row::Name => (
            "Name".to_string(),
            c.map(|c| c.name.clone()).unwrap_or_default(),
            false,
            None,
        ),
        Row::Url => (
            "Server address".to_string(),
            c.map(|c| c.url.clone()).unwrap_or_default(),
            false,
            None,
        ),
        Row::Key => {
            let k = c.map(|c| c.api_key.clone()).unwrap_or_default();
            let shown = if k.is_empty() {
                "(none)".to_string()
            } else {
                "*".repeat(k.chars().count().min(16))
            };
            ("API key".to_string(), shown, false, None)
        }
        Row::Param(i) => {
            let Some(d) = t.defs.get(i).copied() else {
                return Line::from("");
            };
            let v = t.value(&d);
            let changed = v != d.default_value();
            let (shown, frac) = match d.kind {
                ParamKind::Slider { min, max, .. } => {
                    let x = v.as_f64().unwrap_or(min);
                    let f = if max > min { (x - min) / (max - min) } else { 0.0 };
                    (d.format(&v), Some(f.max(0.0).min(1.0)))
                }
                ParamKind::Toggle { .. } => {
                    let on = v.as_bool().unwrap_or(false);
                    (
                        if on { "[x] on".to_string() } else { "[ ] off".to_string() },
                        None,
                    )
                }
                ParamKind::Number { .. } => (d.format(&v), None),
                ParamKind::Text { .. } => {
                    let s = d.format(&v);
                    (if s.is_empty() { "(empty)".to_string() } else { s }, None)
                }
            };
            (d.label.to_string(), shown, changed, frac)
        }
        Row::Header(_) => unreachable!(),
    };

    let value_style = if changed {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let mut spans = vec![
        Span::styled(marker.to_string(), label_style),
        Span::styled(
            format!("{:<w$}", clip(&label, cols.label), w = cols.label),
            label_style,
        ),
        Span::raw(" "),
    ];
    match track {
        Some(frac) if cols.track > 0 => {
            let n = cols.track;
            let knob = ((frac * (n - 1) as f64).round() as usize).min(n - 1);
            spans.push(Span::styled(
                "\u{2501}".repeat(knob),
                Style::default().fg(Color::Cyan),
            ));
            spans.push(Span::styled(
                "\u{25cf}",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                "\u{2500}".repeat(n - 1 - knob),
                Style::default().fg(DIM),
            ));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("{:>w$}", clip(&value, cols.value), w = cols.value),
                value_style,
            ));
        }
        _ => {
            let dim = value == "(empty)" || value == "(none)";
            let style = if dim { Style::default().fg(DIM) } else { value_style };
            spans.push(Span::styled(clip(&value, rest), style));
        }
    }
    Line::from(spans)
}

fn help_for(t: &Tui) -> String {
    match t.rows.get(t.row) {
        Some(Row::Name) => "Name of this connection (used in api.yaml).".to_string(),
        Some(Row::Url) => "Address of the server, e.g. http://127.0.0.1:8080".to_string(),
        Some(Row::Key) => {
            "Bearer token, only if the server runs with --api-key. Empty = none.".to_string()
        }
        Some(Row::Param(i)) => t.defs.get(*i).map(|d| d.help.to_string()).unwrap_or_default(),
        _ => String::new(),
    }
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

fn draw(f: &mut Frame, t: &mut Tui) {
    let area = f.area();
    if area.width < 30 || area.height < 8 {
        f.render_widget(Paragraph::new("too small"), area);
        return;
    }
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);

    // ---- title
    let mut title = vec![Span::styled(
        " API settings ",
        Style::default().fg(Color::Black).bg(Color::White),
    )];
    if t.dirty {
        title.push(Span::styled(
            "  unsaved changes",
            Style::default().fg(Color::Yellow),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(title)), chunks[0]);

    // ---- connection status line
    let mut info: Vec<Span> = Vec::new();
    match t.conn() {
        None => info.push(Span::styled(
            " no connections yet: press n to add one",
            Style::default().fg(DIM),
        )),
        Some(c) => {
            let label = c.provider().map(|p| p.label()).unwrap_or("?");
            info.push(Span::raw(format!(" {} @ {}  ", label, host_of(&c.url))));
            if t.testing {
                info.push(Span::styled(
                    "testing\u{2026}",
                    Style::default().fg(Color::Yellow),
                ));
            } else {
                match &t.info {
                    None => info.push(Span::styled("not tested (t)", Style::default().fg(DIM))),
                    Some(Ok(i)) => {
                        let mut s = "\u{25cf} connected".to_string();
                        if !i.model.is_empty() {
                            s.push_str(&format!(" \u{b7} {}", i.model));
                        }
                        if let Some(n) = i.n_ctx {
                            s.push_str(&format!(" \u{b7} ctx {}", n));
                        }
                        info.push(Span::styled(s, Style::default().fg(Color::Green)));
                    }
                    Some(Err(e)) => info.push(Span::styled(
                        format!("\u{25cf} {}", clip(e, area.width.saturating_sub(24) as usize)),
                        Style::default().fg(Color::Red),
                    )),
                }
            }
            if t.store.active == c.name {
                info.push(Span::styled(
                    "  \u{2605} used for chats",
                    Style::default().fg(Color::Yellow),
                ));
            }
        }
    }
    f.render_widget(Paragraph::new(Line::from(info)), chunks[1]);

    // ---- body: connection list (when wide enough) + settings
    let body = chunks[2];
    let (list_area, set_area) = if body.width >= 64 {
        let parts = Layout::horizontal([Constraint::Length(24), Constraint::Min(30)]).split(body);
        (Some(parts[0]), parts[1])
    } else {
        (None, body)
    };

    t.list_rect = list_area.unwrap_or_default();
    if let Some(la) = list_area {
        let mut lines: Vec<Line> = Vec::new();
        for (i, c) in t.store.connections.iter().enumerate() {
            let star = if c.name == t.store.active { "\u{2605} " } else { "  " };
            let text = format!("{}{}", star, clip(&c.name, la.width.saturating_sub(6) as usize));
            let style = if i == t.cur {
                if t.focus_list {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                } else {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                }
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(text, style)));
        }
        lines.push(Line::from(Span::styled(
            "  + new (n)",
            Style::default().fg(DIM),
        )));
        let border = if t.focus_list { Color::Cyan } else { DIM };
        f.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border))
                    .title(" Connections "),
            ),
            la,
        );
    }

    let heading = match t.conn() {
        Some(c) => format!(
            " {} ",
            clip(&c.name, set_area.width.saturating_sub(6) as usize)
        ),
        None => " Settings ".to_string(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if t.focus_list { DIM } else { Color::Cyan }))
        .title(heading);
    let inner = block.inner(set_area);
    f.render_widget(block, set_area);
    t.rows_rect = inner;

    let h = inner.height as usize;
    let w = inner.width as usize;
    let max_scroll = t.rows.len().saturating_sub(h);
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
    t.scroll = t.scroll.min(max_scroll);

    let label_w = if w >= 56 { 22 } else { (w / 2).saturating_sub(2).max(8) };
    let value_w = 8;
    let track_w = w.saturating_sub(2 + label_w + 1 + 1 + value_w);
    let track_w = if track_w >= 6 { track_w } else { 0 };
    let cols = Cols {
        label: label_w,
        value: value_w,
        track: track_w,
    };
    t.track_x = inner.x + (2 + label_w + 1) as u16;
    t.track_w = track_w as u16;

    let end = (t.scroll + h).min(t.rows.len());
    let lines: Vec<Line> = (t.scroll..end).map(|i| row_line(t, i, w, &cols)).collect();
    f.render_widget(Paragraph::new(lines), inner);

    // ---- help line
    let help = help_for(t);
    f.render_widget(
        Paragraph::new(Span::styled(
            format!(" {}", clip(&help, area.width.saturating_sub(2) as usize)),
            Style::default().fg(DIM),
        )),
        chunks[3],
    );

    // ---- footer: editor, question, message or key hints
    let footer = chunks[4];
    match &t.modal {
        Modal::Edit { title, buf, cur } => {
            let text: String = buf.iter().collect();
            let head = format!(" {}: ", title);
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(head.clone(), Style::default().fg(Color::Cyan)),
                    Span::raw(text),
                ])),
                footer,
            );
            let x = footer.x + head.chars().count() as u16 + *cur as u16;
            f.set_cursor_position((x.min(footer.x + footer.width.saturating_sub(1)), footer.y));
        }
        Modal::Confirm(what) => {
            let name = t.conn().map(|c| c.name.clone()).unwrap_or_default();
            let q = match what {
                Confirm::Quit => " Save changes? [y] save and quit  [n] discard  [any] cancel".to_string(),
                Confirm::Delete => format!(" Delete '{}'? [y/N]", name),
                Confirm::ResetAll => format!(" Reset every setting of '{}' to its default? [y/N]", name),
            };
            f.render_widget(
                Paragraph::new(Span::styled(
                    clip(&q, area.width as usize),
                    Style::default().fg(Color::Yellow),
                )),
                footer,
            );
        }
        _ => {
            if let Some((msg, err)) = &t.status {
                let color = if *err { Color::Red } else { Color::Green };
                f.render_widget(
                    Paragraph::new(Span::styled(
                        format!(" {}", clip(msg, area.width.saturating_sub(2) as usize)),
                        Style::default().fg(color),
                    )),
                    footer,
                );
            } else {
                let hints = " \u{2191}\u{2193} select  \u{2190}\u{2192} adjust (Shift \u{d7}10)  Enter type value  r reset  p presets  t test  a use  n new  d delete  s save  q quit";
                f.render_widget(
                    Paragraph::new(Span::styled(
                        clip(hints, area.width as usize),
                        Style::default().fg(DIM),
                    )),
                    footer,
                );
            }
        }
    }

    // ---- preset picker
    if let Modal::PickPreset(sel) = &t.modal {
        let presets = t.presets();
        let rect = centered(area, 62, presets.len() as u16 + 4);
        f.render_widget(Clear, rect);
        let inner_w = rect.width.saturating_sub(2) as usize;
        let lines: Vec<Line> = presets
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mark = if t.preset_is_current(p) { "*" } else { " " };
                let text = format!(" {}{:<9} {}", mark, p.name, p.help);
                let style = if i == *sel {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default()
                };
                Line::from(Span::styled(clip(&text, inner_w), style))
            })
            .collect();
        let mut lines = lines;
        lines.push(Line::from(Span::styled(
            clip(" Enter apply  Esc cancel  * = in use", inner_w),
            Style::default().fg(DIM),
        )));
        f.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(" Presets (sampler settings) "),
            ),
            rect,
        );
    }

    // ---- kind picker
    if let Modal::PickKind(sel) = &t.modal {
        let kinds = conn::providers();
        let rect = centered(area, 34, kinds.len() as u16 + 4);
        f.render_widget(Clear, rect);
        let lines: Vec<Line> = kinds
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let style = if i == *sel {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default()
                };
                Line::from(Span::styled(format!(" {} ", p.label()), style))
            })
            .collect();
        f.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(" New connection: kind "),
            ),
            rect,
        );
    }
}

// ------------------------------------------------------------------- entry

/// Open the manager in the chat's own terminal and run it until it is quit.
/// The chat keeps the screen (no leaving and re-entering the alternate
/// screen), and the event stream is lent so no keypress is lost between the
/// two views. Whatever was saved is in `api.yaml` when this returns.
pub async fn manage(term: &mut crate::Term, events: &mut EventStream) -> Result<()> {
    let mut store = Store::load()?;

    // First time: start with one llama.cpp connection to fill in.
    let mut fresh = false;
    if store.connections.is_empty() {
        if let Some(p) = conn::providers().first() {
            let name = "local".to_string();
            if let Some(c) = Connection::new(p.kind(), &name) {
                store.connections.push(c);
                store.active = name;
                fresh = true;
            }
        }
    }

    let (tx, mut rx) = unbounded_channel();
    let mut tui = Tui::new(store, tx);
    tui.dirty = fresh;
    if fresh {
        tui.say("first connection created: set its address, then press s to save", false);
    }
    event_loop(term, &mut tui, &mut rx, events).await
}

async fn event_loop(
    term: &mut crate::Term,
    t: &mut Tui,
    rx: &mut UnboundedReceiver<(String, Probe)>,
    events: &mut EventStream,
) -> Result<()> {
    t.test();
    loop {
        term.draw(|f| draw(f, t))?;
        tokio::select! {
            ev = events.next() => {
                match ev {
                    Some(Ok(Event::Key(k))) if k.kind != KeyEventKind::Release => t.on_key(k),
                    Some(Ok(Event::Mouse(m))) => t.on_mouse(m),
                    Some(Ok(Event::Paste(s))) => t.on_paste(&s),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.into()),
                    None => break,
                }
            }
            r = rx.recv() => {
                if let Some((name, res)) = r {
                    t.on_probe(name, res);
                }
            }
        }
        if t.quit {
            break;
        }
    }
    Ok(())
}
