//! A centred list window (modal) to choose one entry from: `/characters`,
//! `/chats` and `/persona` inside the chat screen (state lives in
//! `App::picker`, drawing in `ui::draw_picker`). The arrow keys, the wheel
//! and clicks / taps move and choose.

use crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

pub struct Item {
    /// What the caller gets back when this entry is chosen.
    pub key: String,
    pub label: String,
    /// Dimmed text after the label (may be cut to fit).
    pub detail: String,
    /// The entry in use right now: tagged, and selected when the window opens.
    pub current: bool,
    /// The `d` key offers to delete it.
    pub deletable: bool,
    /// The `e` key edits it.
    pub editable: bool,
}

pub enum Action {
    None,
    Cancel,
    Choose(String),
    Delete(String),
    Edit(String),
    /// The `p` key (only where `Picker::position_key` is set).
    Position(String),
}

pub struct Picker {
    pub title: String,
    pub items: Vec<Item>,
    /// Selected row, counted in `visible()`.
    pub sel: usize,
    /// First row shown, kept in range by `ui::draw_picker`.
    pub first: usize,
    /// Rows the list shows (for PgUp / PgDn); set by `ui::draw_picker`.
    pub page: usize,
    /// Whole window and list area of the last draw, for the mouse.
    pub window: Rect,
    pub rows: Rect,
    /// Key of the entry waiting for a "delete it?" answer.
    pub confirm: Option<String>,
    /// One-line message for the bottom edge: (text, is_error).
    pub note: Option<(String, bool)>,
    /// The `p` key asks for the position settings of an editable entry (the
    /// `/persona` window).
    pub position_key: bool,
}

impl Picker {
    pub fn new(title: impl Into<String>, items: Vec<Item>) -> Picker {
        let mut p = Picker {
            title: title.into(),
            items,
            sel: 0,
            first: 0,
            page: 10,
            window: Rect::default(),
            rows: Rect::default(),
            confirm: None,
            note: None,
            position_key: false,
        };
        p.sel = p.items.iter().position(|i| i.current).unwrap_or(0);
        p
    }

    /// Indexes into `items` of the entries shown (all of them).
    pub fn visible(&self) -> Vec<usize> {
        (0..self.items.len()).collect()
    }

    fn move_by(&mut self, d: isize) {
        let n = self.visible().len();
        if n == 0 {
            self.sel = 0;
            return;
        }
        self.sel = (self.sel as isize + d).clamp(0, n as isize - 1) as usize;
    }

    fn chosen(&self) -> Action {
        match self.visible().get(self.sel) {
            Some(&idx) => Action::Choose(self.items[idx].key.clone()),
            None => Action::None,
        }
    }

    fn ask_edit(&self) -> Action {
        match self.visible().get(self.sel) {
            Some(&idx) if self.items[idx].editable => Action::Edit(self.items[idx].key.clone()),
            _ => Action::None,
        }
    }

    fn ask_position(&self) -> Action {
        match self.visible().get(self.sel) {
            Some(&idx) if self.position_key && self.items[idx].editable => {
                Action::Position(self.items[idx].key.clone())
            }
            _ => Action::None,
        }
    }

    fn ask_delete(&mut self) {
        if let Some(&idx) = self.visible().get(self.sel) {
            if self.items[idx].deletable {
                self.confirm = Some(self.items[idx].key.clone());
            }
        }
    }

    /// Drop an entry (after it was deleted for good).
    pub fn remove(&mut self, key: &str) {
        self.items.retain(|i| i.key != key);
        let n = self.visible().len();
        self.sel = self.sel.min(n.saturating_sub(1));
    }

    pub fn handle_key(&mut self, k: KeyEvent) -> Action {
        if let Some(key) = self.confirm.take() {
            return match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => Action::Delete(key),
                _ => Action::None,
            };
        }
        self.note = None;
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let page = self.page.max(1) as isize;
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') if !ctrl && !alt => return Action::Cancel,
            KeyCode::Char('c') if ctrl => return Action::Cancel,
            KeyCode::Enter => return self.chosen(),
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            KeyCode::Char('p') if ctrl => self.move_by(-1),
            KeyCode::Char('n') if ctrl => self.move_by(1),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = self.visible().len().saturating_sub(1),
            KeyCode::Char('d') if !ctrl && !alt => self.ask_delete(),
            KeyCode::Char('e') if !ctrl && !alt => return self.ask_edit(),
            KeyCode::Char('p') if !ctrl && !alt => return self.ask_position(),
            KeyCode::Char('j') if !ctrl && !alt => self.move_by(1),
            KeyCode::Char('k') if !ctrl && !alt => self.move_by(-1),
            _ => {}
        }
        Action::None
    }

    pub fn on_mouse(&mut self, m: MouseEvent) -> Action {
        match m.kind {
            MouseEventKind::ScrollUp => self.move_by(-3),
            MouseEventKind::ScrollDown => self.move_by(3),
            MouseEventKind::Down(MouseButton::Left) => {
                let pos = Position::new(m.column, m.row);
                if self.rows.contains(pos) {
                    let idx = self.first + (m.row - self.rows.y) as usize;
                    if idx < self.visible().len() {
                        self.sel = idx;
                        self.confirm = None;
                        return self.chosen();
                    }
                } else if !self.window.contains(pos) {
                    return Action::Cancel;
                }
            }
            _ => {}
        }
        Action::None
    }
}
