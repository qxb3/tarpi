//! The window of the `p` key in the `/persona` window: where the description
//! of a persona is injected into the prompt. State lives in
//! `App::pos_edit`, drawing in `ui::draw_pos_edit`.
//!
//! Three rows: *Position* (in the prompt, at the top or bottom of the Author's
//! Note, in the chat at a depth, or not sent), and, only for the chat position,
//! *Depth* (messages from the end, 0 is after the last one) and *Role* (system,
//! user or assistant). A persona put into the Author's Note goes where the
//! note itself goes (`/note`). The one position st-cli cannot use (SillyTavern's
//! old *After the character*) is shown as it is when a persona has it, but
//! cannot be chosen.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

/// Into the story string of the context template, with the character card.
pub const IN_PROMPT: i64 = 0;
/// Above the text of the chat's Author's Note (see `note`).
pub const TOP_NOTE: i64 = 2;
/// Below the text of the chat's Author's Note.
pub const BOTTOM_NOTE: i64 = 3;
/// Into the chat history, `depth` messages from the end.
pub const AT_DEPTH: i64 = 4;
/// Not sent at all.
pub const NOT_SENT: i64 = 9;

/// The positions that can be chosen, in the order the arrows go through them.
const CHOICES: [i64; 5] = [IN_PROMPT, TOP_NOTE, BOTTOM_NOTE, AT_DEPTH, NOT_SENT];
const MAX_DEPTH: i64 = 999;

/// Name of a position for the window.
pub fn position_name(p: i64) -> String {
    match p {
        IN_PROMPT => "In the prompt".to_string(),
        TOP_NOTE => "Top of the Author's Note".to_string(),
        BOTTOM_NOTE => "Bottom of the Author's Note".to_string(),
        AT_DEPTH => "In the chat, at a depth".to_string(),
        NOT_SENT => "Not sent".to_string(),
        1 => "After the character (not supported, not sent)".to_string(),
        n => format!("Unknown ({}) (not sent)", n),
    }
}

/// Name of a role number.
pub fn role_name(r: i64) -> &'static str {
    match r {
        1 => "user",
        2 => "assistant",
        _ => "system",
    }
}

/// One line about what the chosen position does.
pub fn position_note(p: i64, depth: i64, role: i64) -> String {
    match p {
        IN_PROMPT => "With the character card, in the story string.".to_string(),
        TOP_NOTE => "Above the Author's Note text (/note), wherever it goes.".to_string(),
        BOTTOM_NOTE => "Below the Author's Note text (/note), wherever it goes.".to_string(),
        AT_DEPTH => {
            if depth == 0 {
                format!("After the last message, as a {} message.", role_name(role))
            } else {
                format!(
                    "{} message{} from the end, as a {} message.",
                    depth,
                    if depth == 1 { "" } else { "s" },
                    role_name(role)
                )
            }
        }
        _ => "The description of the persona is left out.".to_string(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Row {
    Position,
    Depth,
    Role,
}

impl Row {
    pub fn label(self) -> &'static str {
        match self {
            Row::Position => "Position",
            Row::Depth => "Depth",
            Row::Role => "Role",
        }
    }
}

pub enum Action {
    None,
    Cancel,
    Save,
}

pub struct PosEdit {
    /// Id of the persona (its file name).
    pub id: String,
    /// `Name (Title)`, for the title.
    pub label: String,
    pub position: i64,
    pub depth: i64,
    pub role: i64,
    /// The row with the cursor.
    pub sel: Row,
    /// The next digit typed replaces the depth instead of adding to it.
    fresh: bool,
    /// Message for the bottom edge, shown after a failed save.
    pub note: Option<String>,
    /// Window and the area of its rows at the last draw, for the mouse.
    pub window: Rect,
    pub rows_area: Rect,
}

impl PosEdit {
    pub fn new(id: String, label: String, position: i64, depth: i64, role: i64) -> PosEdit {
        PosEdit {
            id,
            label,
            position,
            depth: depth.clamp(0, MAX_DEPTH),
            role: role.clamp(0, 2),
            sel: Row::Position,
            fresh: true,
            note: None,
            window: Rect::default(),
            rows_area: Rect::default(),
        }
    }

    /// The rows shown: depth and role only matter in the chat position.
    pub fn rows(&self) -> Vec<Row> {
        if self.position == AT_DEPTH {
            vec![Row::Position, Row::Depth, Row::Role]
        } else {
            vec![Row::Position]
        }
    }

    fn select(&mut self, r: Row) {
        self.sel = r;
        self.fresh = true;
    }

    fn move_by(&mut self, d: isize) {
        let rows = self.rows();
        let at = rows.iter().position(|r| *r == self.sel).unwrap_or(0) as isize;
        let n = rows.len() as isize;
        self.select(rows[(at + d).rem_euclid(n) as usize]);
    }

    /// Change the value of the selected row by `d` steps.
    fn change(&mut self, d: i64) {
        self.note = None;
        match self.sel {
            Row::Position => {
                let step = d.signum();
                let now = CHOICES.iter().position(|c| *c == self.position);
                let n = CHOICES.len() as i64;
                let next = match now {
                    Some(i) => (i as i64 + step).rem_euclid(n),
                    // A position that cannot be chosen: go to the first / last.
                    None if step >= 0 => 0,
                    None => n - 1,
                };
                // Only this row can be selected when the position changes, so
                // the rows that appear or go away never hold the cursor.
                self.position = CHOICES[next as usize];
            }
            Row::Depth => {
                self.depth = (self.depth + d).clamp(0, MAX_DEPTH);
                self.fresh = true;
            }
            Row::Role => {
                self.role = (self.role + d.signum()).rem_euclid(3);
            }
        }
    }

    fn digit(&mut self, c: char) {
        let Some(n) = c.to_digit(10) else {
            return;
        };
        self.note = None;
        let n = n as i64;
        let next = if self.fresh {
            n
        } else {
            self.depth * 10 + n
        };
        self.depth = if next > MAX_DEPTH { n } else { next };
        self.fresh = false;
    }

    pub fn handle_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let big = if shift { 10 } else { 1 };
        match k.code {
            KeyCode::Esc => return Action::Cancel,
            KeyCode::Char('c') if ctrl => return Action::Cancel,
            KeyCode::Char('q') if !ctrl && !alt => return Action::Cancel,
            KeyCode::Enter => return Action::Save,
            KeyCode::Char('s') if ctrl => return Action::Save,
            KeyCode::Up | KeyCode::BackTab => self.move_by(-1),
            KeyCode::Down | KeyCode::Tab => self.move_by(1),
            KeyCode::Char('k') if !ctrl && !alt => self.move_by(-1),
            KeyCode::Char('j') if !ctrl && !alt => self.move_by(1),
            KeyCode::Left => self.change(-big),
            KeyCode::Right => self.change(big),
            KeyCode::Char('h') if !ctrl && !alt => self.change(-1),
            KeyCode::Char('l') if !ctrl && !alt => self.change(1),
            KeyCode::Char('H') if !ctrl && !alt => self.change(-10),
            KeyCode::Char('L') if !ctrl && !alt => self.change(10),
            KeyCode::PageUp => self.change(-10),
            KeyCode::PageDown => self.change(10),
            KeyCode::Home if self.sel == Row::Depth => {
                self.depth = 0;
                self.fresh = true;
            }
            KeyCode::Char(c) if c.is_ascii_digit() && !ctrl && !alt => {
                if self.sel == Row::Depth {
                    self.digit(c);
                }
            }
            KeyCode::Backspace if self.sel == Row::Depth => {
                self.depth /= 10;
                self.fresh = false;
            }
            _ => {}
        }
        Action::None
    }

    /// A click outside closes the window; a click on a row selects it and
    /// changes it, the left half of the row one step back, the right half one
    /// step on.
    pub fn on_mouse(&mut self, m: MouseEvent) -> Action {
        if let MouseEventKind::Down(MouseButton::Left) = m.kind {
            let pos = Position::new(m.column, m.row);
            if self.rows_area.contains(pos) {
                let rows = self.rows();
                let idx = (m.row - self.rows_area.y) as usize;
                if let Some(r) = rows.get(idx) {
                    self.select(*r);
                    let half = self.rows_area.x + self.rows_area.width / 2;
                    self.change(if m.column < half { -1 } else { 1 });
                }
            } else if !self.window.contains(pos) {
                return Action::Cancel;
            }
        }
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn edit(position: i64) -> PosEdit {
        PosEdit::new("me".into(), "Me".into(), position, 2, 0)
    }

    #[test]
    fn only_the_chat_position_has_depth_and_role() {
        assert_eq!(edit(IN_PROMPT).rows(), vec![Row::Position]);
        assert_eq!(edit(TOP_NOTE).rows(), vec![Row::Position]);
        assert_eq!(edit(BOTTOM_NOTE).rows(), vec![Row::Position]);
        assert_eq!(edit(NOT_SENT).rows(), vec![Row::Position]);
        assert_eq!(
            edit(AT_DEPTH).rows(),
            vec![Row::Position, Row::Depth, Row::Role]
        );
    }

    #[test]
    fn arrows_go_through_the_positions_and_wrap() {
        let mut e = edit(IN_PROMPT);
        e.handle_key(key(KeyCode::Right));
        assert_eq!(e.position, TOP_NOTE);
        e.handle_key(key(KeyCode::Right));
        assert_eq!(e.position, BOTTOM_NOTE);
        e.handle_key(key(KeyCode::Right));
        assert_eq!(e.position, AT_DEPTH);
        e.handle_key(key(KeyCode::Right));
        assert_eq!(e.position, NOT_SENT);
        e.handle_key(key(KeyCode::Right));
        assert_eq!(e.position, IN_PROMPT);
        e.handle_key(key(KeyCode::Left));
        assert_eq!(e.position, NOT_SENT);
    }

    #[test]
    fn a_position_that_cannot_be_chosen_leaves_on_either_side() {
        let mut e = edit(1);
        e.handle_key(key(KeyCode::Right));
        assert_eq!(e.position, IN_PROMPT);
        let mut e = edit(1);
        e.handle_key(key(KeyCode::Left));
        assert_eq!(e.position, NOT_SENT);
    }

    #[test]
    fn depth_and_role_change_in_the_chat_position() {
        let mut e = edit(AT_DEPTH);
        e.handle_key(key(KeyCode::Down));
        assert_eq!(e.sel, Row::Depth);
        e.handle_key(key(KeyCode::Right));
        assert_eq!(e.depth, 3);
        e.handle_key(key(KeyCode::PageDown));
        assert_eq!(e.depth, 13);
        e.handle_key(key(KeyCode::Down));
        assert_eq!(e.sel, Row::Role);
        e.handle_key(key(KeyCode::Right));
        assert_eq!(e.role, 1);
        e.handle_key(key(KeyCode::Left));
        e.handle_key(key(KeyCode::Left));
        assert_eq!(e.role, 2);
    }

    #[test]
    fn depth_stays_between_0_and_999() {
        let mut e = edit(AT_DEPTH);
        e.handle_key(key(KeyCode::Down));
        for _ in 0..5 {
            e.handle_key(key(KeyCode::Left));
        }
        assert_eq!(e.depth, 0);
        e.depth = 995;
        e.handle_key(key(KeyCode::PageDown));
        assert_eq!(e.depth, 999);
    }

    #[test]
    fn typed_digits_replace_the_depth_first() {
        let mut e = edit(AT_DEPTH);
        e.handle_key(key(KeyCode::Down));
        e.handle_key(key(KeyCode::Char('1')));
        assert_eq!(e.depth, 1);
        e.handle_key(key(KeyCode::Char('2')));
        assert_eq!(e.depth, 12);
        e.handle_key(key(KeyCode::Backspace));
        assert_eq!(e.depth, 1);
        // Digits on another row do nothing.
        e.handle_key(key(KeyCode::Up));
        e.handle_key(key(KeyCode::Char('7')));
        assert_eq!(e.depth, 1);
    }

    #[test]
    fn leaving_the_chat_position_puts_the_cursor_back_on_it() {
        let mut e = edit(AT_DEPTH);
        e.handle_key(key(KeyCode::Down));
        e.handle_key(key(KeyCode::Right));
        assert_eq!(e.sel, Row::Depth);
        let mut e = edit(AT_DEPTH);
        e.sel = Row::Position;
        e.handle_key(key(KeyCode::Right));
        assert_eq!(e.position, NOT_SENT);
        assert_eq!(e.sel, Row::Position);
    }

    #[test]
    fn enter_saves_and_esc_cancels() {
        let mut e = edit(IN_PROMPT);
        assert!(matches!(e.handle_key(key(KeyCode::Enter)), Action::Save));
        assert!(matches!(e.handle_key(key(KeyCode::Esc)), Action::Cancel));
        assert!(matches!(e.handle_key(key(KeyCode::Char('q'))), Action::Cancel));
    }
}
