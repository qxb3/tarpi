//! `/api`: a full-screen manager for the standalone API connections
//! (see `conn`), opened from inside a chat. Lists the saved connections,
//! edits the server address and API key, and shows every model setting of the
//! selected connection as a slider, switch or text field. Which settings exist
//! comes from the connection kind (`conn::Provider::params`), so a new kind of
//! connection needs no changes here.

use crate::conn::{self, Connection, Info, ModelEntry, ParamDef, ParamKind, Store};
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

/// A fetched model list: the address it was fetched from, then the list (or
/// a message to show).
type ModelsMsg = (String, std::result::Result<Vec<ModelEntry>, String>);

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
    /// Choosing the model from the backend's own list (`Provider::model_source`):
    /// the typed filter and the highlighted entry of the matches.
    PickModel { filter: Vec<char>, sel: usize },
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
    /// The last model list fetched, with the address it came from.
    models: Option<(String, Vec<ModelEntry>)>,
    models_loading: bool,
    models_err: Option<String>,
    mtx: UnboundedSender<ModelsMsg>,
    /// Where the model picker drew its list, and the first entry shown.
    pick_rect: Rect,
    pick_top: usize,
}

impl Tui {
    fn new(
        store: Store,
        tx: UnboundedSender<(String, Probe)>,
        mtx: UnboundedSender<ModelsMsg>,
    ) -> Tui {
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
            models: None,
            models_loading: false,
            models_err: None,
            mtx,
            pick_rect: Rect::default(),
            pick_top: 0,
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
        self.models_loading = false;
        self.models_err = None;
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
        }
        .or_else(|| self.model_ctx());
        let Some(n) = n else {
            self.say(
                "no context size known yet: test the connection (t) or pick a model",
                true,
            );
            return;
        };
        match self.defs.iter().position(|d| d.key == "max_context") {
            Some(i) => {
                let d = self.defs[i];
                self.set_param(i, d.snap(n as f64));
                self.say(format!("context size set to {}", n), false);
            }
            None => self.say("this connection has no context size setting", true),
        }
    }

    // ------------------------------------------------------- model list

    fn model_source(&self) -> Option<conn::ModelSource> {
        self.conn()?.provider()?.model_source()
    }

    /// Address the selected connection's model list comes from.
    fn models_key(&self) -> Option<String> {
        self.conn().map(|c| c.base())
    }

    /// The fetched list, if it is the selected connection's.
    fn cached_models(&self) -> Option<&Vec<ModelEntry>> {
        let key = self.models_key()?;
        match &self.models {
            Some((k, list)) if *k == key => Some(list),
            _ => None,
        }
    }

    /// The model the connection is set to (empty = none).
    fn current_model(&self) -> String {
        let Some(src) = self.model_source() else {
            return String::new();
        };
        let Some(c) = self.conn() else {
            return String::new();
        };
        match self.defs.iter().find(|d| d.key == src.param) {
            Some(d) => c.get(d).as_str().unwrap_or("").trim().to_string(),
            None => String::new(),
        }
    }

    /// Context length of the current model, when the list knows it.
    fn model_ctx(&self) -> Option<i64> {
        let cur = self.current_model();
        if cur.is_empty() {
            return None;
        }
        self.cached_models()?
            .iter()
            .find(|m| m.id == cur)
            .and_then(|m| m.context)
    }

    /// Ids of the models matching `filter`, in list order.
    fn match_ids(&self, filter: &[char]) -> Vec<String> {
        let text: String = filter.iter().collect();
        match self.cached_models() {
            Some(list) => filter_models(list, &text)
                .into_iter()
                .map(|m| m.id.clone())
                .collect(),
            None => Vec::new(),
        }
    }

    /// Where the current model sits among the matches (0 if it is not there).
    fn model_pos(&self, filter: &[char]) -> usize {
        let cur = self.current_model();
        self.match_ids(filter)
            .iter()
            .position(|id| *id == cur)
            .unwrap_or(0)
    }

    fn load_models(&mut self) {
        let Some(c) = self.conn().cloned() else {
            return;
        };
        self.models_loading = true;
        self.models_err = None;
        let tx = self.mtx.clone();
        tokio::spawn(async move {
            let res = conn::list_models(&c).await.map_err(|e| format!("{:#}", e));
            let _ = tx.send((c.base(), res));
        });
    }

    fn on_models(&mut self, key: String, res: std::result::Result<Vec<ModelEntry>, String>) {
        self.models_loading = false;
        let current = self.models_key().as_deref() == Some(key.as_str());
        match res {
            Ok(list) => {
                self.models = Some((key, list));
                if current {
                    self.models_err = None;
                    // Put the highlight on the model in use, unless the person
                    // already started typing or moving.
                    let pos = if let Modal::PickModel { filter, sel } = &self.modal {
                        if filter.is_empty() && *sel == 0 {
                            Some(self.model_pos(filter))
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    if let (Some(p), Modal::PickModel { sel, .. }) = (pos, &mut self.modal) {
                        *sel = p;
                    }
                }
            }
            Err(e) => {
                if current {
                    self.models_err = Some(e);
                }
            }
        }
    }

    fn open_model_picker(&mut self) {
        if self.cached_models().is_none() {
            self.load_models();
        }
        let filter: Vec<char> = Vec::new();
        let sel = self.model_pos(&filter);
        self.modal = Modal::PickModel { filter, sel };
    }

    /// Store `id` as the model (from the list, or typed when nothing matched).
    fn pick_model(&mut self, id: &str) {
        let Some(src) = self.model_source() else {
            return;
        };
        let Some(i) = self.defs.iter().position(|d| d.key == src.param) else {
            return;
        };
        self.set_param(i, Value::String(id.to_string()));
        let ctx = self
            .cached_models()
            .and_then(|l| l.iter().find(|m| m.id == id))
            .and_then(|m| m.context);
        let msg = match ctx {
            Some(n) => format!(
                "model set to {} (context {}: press c to use it as Context size; s saves)",
                id, n
            ),
            None => format!("model set to {} (press s to save)", id),
        };
        self.say(msg, false);
    }

    fn on_mouse_picker(&mut self, m: MouseEvent) {
        let Modal::PickModel { filter, sel } = std::mem::replace(&mut self.modal, Modal::None)
        else {
            return;
        };
        let ids = self.match_ids(&filter);
        let last = ids.len().saturating_sub(1);
        let mut sel = sel.min(last);
        let mut keep = true;
        match m.kind {
            MouseEventKind::ScrollUp => sel = sel.saturating_sub(3),
            MouseEventKind::ScrollDown => sel = (sel + 3).min(last),
            MouseEventKind::Down(MouseButton::Left) => {
                if self.pick_rect.contains(Position::new(m.column, m.row)) {
                    let idx = self.pick_top + (m.row - self.pick_rect.y) as usize;
                    if let Some(id) = ids.get(idx) {
                        keep = false;
                        self.pick_model(id);
                    }
                }
            }
            _ => {}
        }
        if keep {
            self.modal = Modal::PickModel { filter, sel };
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
                if self.model_source().is_some_and(|src| src.param == d.key) {
                    self.open_model_picker();
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
            Modal::PickModel { mut filter, mut sel } => {
                let ids = self.match_ids(&filter);
                let last = ids.len().saturating_sub(1);
                let mut keep = true;
                match k.code {
                    KeyCode::Esc => keep = false,
                    KeyCode::Enter => {
                        let typed: String = filter.iter().collect();
                        let typed = typed.trim();
                        // The highlighted match, or (when nothing matches) what
                        // was typed, so a model missing from the list still works.
                        let chosen = match ids.get(sel) {
                            Some(id) => Some(id.clone()),
                            None if !typed.is_empty() => Some(typed.to_string()),
                            None => None,
                        };
                        if let Some(id) = chosen {
                            keep = false;
                            self.pick_model(&id);
                        }
                    }
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Down => sel = (sel + 1).min(last),
                    KeyCode::PageUp => sel = sel.saturating_sub(10),
                    KeyCode::PageDown => sel = (sel + 10).min(last),
                    KeyCode::Home => sel = 0,
                    KeyCode::End => sel = last,
                    KeyCode::Char('u') if ctrl => {
                        filter.clear();
                        sel = 0;
                    }
                    KeyCode::Char('r') if ctrl => self.load_models(),
                    KeyCode::Char(c) if !ctrl => {
                        filter.push(c);
                        sel = 0;
                    }
                    KeyCode::Backspace => {
                        filter.pop();
                        sel = 0;
                    }
                    _ => {}
                }
                if keep {
                    self.modal = Modal::PickModel { filter, sel };
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
        match &mut self.modal {
            Modal::Edit { buf, cur, .. } => {
                for c in s.chars().filter(|c| !c.is_control()) {
                    buf.insert(*cur, c);
                    *cur += 1;
                }
            }
            Modal::PickModel { filter, sel } => {
                filter.extend(s.chars().filter(|c| !c.is_control()));
                *sel = 0;
            }
            _ => {}
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        if matches!(self.modal, Modal::PickModel { .. }) {
            self.on_mouse_picker(m);
            return;
        }
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

/// The models whose id or name has every word of `filter` (any case).
fn filter_models<'a>(list: &'a [ModelEntry], filter: &str) -> Vec<&'a ModelEntry> {
    let words: Vec<String> = filter.split_whitespace().map(|w| w.to_lowercase()).collect();
    list.iter()
        .filter(|m| {
            let hay = format!("{} {}", m.id, m.name).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}

/// `131072` -> `131k`, `1048576` -> `1M`.
fn short_ctx(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{}M", (n + 500_000) / 1_000_000)
    } else if n >= 1000 {
        format!("{}k", (n + 500) / 1000)
    } else {
        n.to_string()
    }
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
            "Bearer token: needed by hosted services (OpenRouter) and servers run with --api-key. Empty = none.".to_string()
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

    // ---- model picker
    let picking = if let Modal::PickModel { filter, sel } = &t.modal {
        Some((filter.iter().collect::<String>(), *sel))
    } else {
        None
    };
    if let Some((filter_s, sel)) = picking {
        let rect = centered(area, 100, area.height.saturating_sub(2));
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Pick a model ");
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let parts = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

        let head = " Filter: ";
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(head, Style::default().fg(Color::Cyan)),
                Span::raw(filter_s.clone()),
            ])),
            parts[0],
        );
        let cx = parts[0].x + head.chars().count() as u16 + filter_s.chars().count() as u16;
        f.set_cursor_position((
            cx.min(parts[0].x + parts[0].width.saturating_sub(1)),
            parts[0].y,
        ));

        let current = t.current_model();
        let h = parts[2].height as usize;
        let w = parts[2].width as usize;
        let (lines, total, all, top) = {
            let hits: Vec<&ModelEntry> = match t.cached_models() {
                Some(l) => filter_models(l, &filter_s),
                None => Vec::new(),
            };
            let all = t.cached_models().map(|l| l.len()).unwrap_or(0);
            let total = hits.len();
            let sel = sel.min(total.saturating_sub(1));
            let mut top = t.pick_top;
            if sel < top {
                top = sel;
            } else if h > 0 && sel >= top + h {
                top = sel + 1 - h;
            }
            top = top.min(total.saturating_sub(h));
            let mut lines: Vec<Line> = Vec::new();
            for (i, m) in hits.iter().enumerate().skip(top).take(h) {
                let mut right = String::new();
                if let Some(n) = m.context {
                    right.push_str(&format!("{} ctx", short_ctx(n)));
                }
                if !m.price.is_empty() {
                    if !right.is_empty() {
                        right.push_str("  ");
                    }
                    right.push_str(&m.price);
                }
                let right_w = right.chars().count();
                let show_right = right_w > 0 && w > right_w + 14;
                let left_w = if show_right { w - right_w - 2 } else { w };
                let mark = if m.id == current { "*" } else { " " };
                let left = clip(&format!(" {}{}", mark, m.id), left_w);
                let left = format!("{:<lw$}", left, lw = left_w);
                let base = if i == sel {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default()
                };
                let dim = if i == sel { base } else { Style::default().fg(DIM) };
                let mut spans = vec![Span::styled(left, base)];
                if show_right {
                    spans.push(Span::styled("  ", base));
                    spans.push(Span::styled(right, dim));
                }
                lines.push(Line::from(spans));
            }
            (lines, total, all, top)
        };
        t.pick_top = top;
        t.pick_rect = parts[2];
        f.render_widget(Paragraph::new(lines), parts[2]);

        let (msg, color) = if t.models_loading && all == 0 {
            (" loading the model list\u{2026}".to_string(), Color::Yellow)
        } else if let (Some(e), 0) = (&t.models_err, all) {
            (
                format!(" could not load the list: {} (Ctrl+R retries)", e),
                Color::Red,
            )
        } else if total == 0 && !filter_s.trim().is_empty() {
            (
                format!(" no match: Enter uses '{}' as typed", filter_s.trim()),
                Color::Yellow,
            )
        } else {
            (
                format!(
                    " {} of {} models{}",
                    total,
                    all,
                    if t.models_loading { " (refreshing\u{2026})" } else { "" }
                ),
                DIM,
            )
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                clip(&msg, parts[1].width as usize),
                Style::default().fg(color),
            )),
            parts[1],
        );
        f.render_widget(
            Paragraph::new(Span::styled(
                clip(
                    " \u{2191}\u{2193} select  Enter pick  Esc cancel  Ctrl+U clear  Ctrl+R reload  * = in use",
                    parts[3].width as usize,
                ),
                Style::default().fg(DIM),
            )),
            parts[3],
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

    // First time: start with one connection of the first kind (llama.cpp) to fill in.
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
    let (mtx, mut mrx) = unbounded_channel();
    let mut tui = Tui::new(store, tx, mtx);
    tui.dirty = fresh;
    if fresh {
        tui.say("first connection created: set its address, then press s to save", false);
    }
    event_loop(term, &mut tui, &mut rx, &mut mrx, events).await
}

async fn event_loop(
    term: &mut crate::Term,
    t: &mut Tui,
    rx: &mut UnboundedReceiver<(String, Probe)>,
    mrx: &mut UnboundedReceiver<ModelsMsg>,
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
            m = mrx.recv() => {
                if let Some((key, res)) = m {
                    t.on_models(key, res);
                }
            }
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

    fn entry(id: &str, name: &str) -> ModelEntry {
        ModelEntry {
            id: id.to_string(),
            name: name.to_string(),
            ..ModelEntry::default()
        }
    }

    fn ids<'a>(list: &'a [ModelEntry], f: &str) -> Vec<&'a str> {
        filter_models(list, f).iter().map(|m| m.id.as_str()).collect()
    }

    #[test]
    fn filter_matches_every_word_in_id_or_name() {
        let list = vec![
            entry("meta-llama/llama-3.3-70b-instruct", "Meta: Llama 3.3 70B Instruct"),
            entry("anthropic/claude-3.5-sonnet", "Anthropic: Claude 3.5 Sonnet"),
            entry("mistralai/mistral-nemo:free", "Mistral: Nemo (free)"),
        ];
        assert_eq!(ids(&list, "").len(), 3);
        assert_eq!(ids(&list, "LLAMA 70b"), ["meta-llama/llama-3.3-70b-instruct"]);
        assert_eq!(ids(&list, "sonnet claude"), ["anthropic/claude-3.5-sonnet"]);
        assert_eq!(ids(&list, "free"), ["mistralai/mistral-nemo:free"]);
        assert!(ids(&list, "gpt").is_empty());
    }

    #[test]
    fn context_is_short() {
        assert_eq!(short_ctx(8192), "8k");
        assert_eq!(short_ctx(131072), "131k");
        assert_eq!(short_ctx(1048576), "1M");
        assert_eq!(short_ctx(512), "512");
    }
}
