//! The question that follows `/import` when the card that was just added
//! carries a lorebook: copy it into the user's own lorebooks (`/lore`) and
//! switch it on for that character, or leave it alone. State lives in
//! `App::lore_ask`, drawing in `ui::draw_lore_ask`.
//!
//! Two buttons, *Yes* and *No*. Enter / `y` answer yes, Esc / `n` / `q` answer
//! no, the arrows (or Tab, `h` / `l`) move between the buttons and Enter acts
//! on the one that is lit. A click on a button answers; a click elsewhere does
//! nothing, so a stray tap cannot decide for the user.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    None,
    Yes,
    No,
}

pub struct LoreAsk {
    /// The card file name of the character that was just imported.
    pub avatar: String,
    /// What to call the character in the window.
    pub character: String,
    /// The name the copied book will get.
    pub book: String,
    /// How many entries the card's book has.
    pub entries: usize,
    /// The button that is lit: `true` is *Yes*.
    pub yes: bool,
    /// Why the last *Yes* failed, for the bottom edge (the window stays open).
    pub note: Option<String>,
    /// The window and its two buttons at the last draw, for the mouse.
    pub window: Rect,
    pub yes_area: Rect,
    pub no_area: Rect,
}

impl LoreAsk {
    pub fn new(avatar: String, character: String, book: String, entries: usize) -> LoreAsk {
        LoreAsk {
            avatar,
            character,
            book,
            entries,
            yes: true,
            note: None,
            window: Rect::default(),
            yes_area: Rect::default(),
            no_area: Rect::default(),
        }
    }

    pub fn handle_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Esc => Action::No,
            KeyCode::Char('c') if ctrl => Action::No,
            KeyCode::Char('y') | KeyCode::Char('Y') if !ctrl && !alt => Action::Yes,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('q') if !ctrl && !alt => {
                Action::No
            }
            KeyCode::Enter => {
                if self.yes {
                    Action::Yes
                } else {
                    Action::No
                }
            }
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Char('h')
            | KeyCode::Char('l') => {
                self.yes = !self.yes;
                self.note = None;
                Action::None
            }
            _ => Action::None,
        }
    }

    pub fn on_mouse(&mut self, m: MouseEvent) -> Action {
        if let MouseEventKind::Down(MouseButton::Left) = m.kind {
            let pos = Position::new(m.column, m.row);
            if self.yes_area.contains(pos) {
                return Action::Yes;
            }
            if self.no_area.contains(pos) {
                return Action::No;
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

    fn ask() -> LoreAsk {
        LoreAsk::new("Sofia.png".into(), "Sofia".into(), "Sofia lorebook".into(), 12)
    }

    #[test]
    fn enter_answers_yes_until_the_other_button_is_lit() {
        let mut a = ask();
        assert_eq!(a.handle_key(key(KeyCode::Enter)), Action::Yes);
        assert_eq!(a.handle_key(key(KeyCode::Right)), Action::None);
        assert_eq!(a.handle_key(key(KeyCode::Enter)), Action::No);
        assert_eq!(a.handle_key(key(KeyCode::Tab)), Action::None);
        assert_eq!(a.handle_key(key(KeyCode::Enter)), Action::Yes);
    }

    #[test]
    fn letters_and_escape_answer_at_once() {
        assert_eq!(ask().handle_key(key(KeyCode::Char('y'))), Action::Yes);
        assert_eq!(ask().handle_key(key(KeyCode::Char('n'))), Action::No);
        assert_eq!(ask().handle_key(key(KeyCode::Esc)), Action::No);
        assert_eq!(
            ask().handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::No
        );
        // Other keys do nothing.
        assert_eq!(ask().handle_key(key(KeyCode::Char('x'))), Action::None);
    }

    #[test]
    fn a_click_answers_only_on_a_button() {
        let mut a = ask();
        a.window = Rect::new(0, 0, 40, 8);
        a.yes_area = Rect::new(5, 6, 9, 1);
        a.no_area = Rect::new(16, 6, 8, 1);
        let click = |x, y| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(a.on_mouse(click(6, 6)), Action::Yes);
        assert_eq!(a.on_mouse(click(17, 6)), Action::No);
        assert_eq!(a.on_mouse(click(30, 2)), Action::None);
        assert_eq!(a.on_mouse(click(60, 20)), Action::None);
    }
}
