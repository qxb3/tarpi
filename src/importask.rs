//! The question `/import` asks when the card's name is already taken: import
//! it anyway, under the next free numbered name (`Sofia-1`), or leave it out.
//! State lives in `App::import_ask`, drawing in `ui::draw_import_ask`.
//!
//! Two buttons, *Yes* and *No*. Enter / `y` answer yes, Esc / `n` / `q` answer
//! no, the arrows (or Tab, `h` / `l`) move between the buttons and Enter acts
//! on the one that is lit. A click on a button answers; a click elsewhere does
//! nothing, so a stray tap cannot decide for the user. When the card carries a
//! lorebook, the window names it and *Yes* copies it too.

use crate::import::Ready;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    None,
    Yes,
    No,
}

pub struct ImportAsk {
    /// The name of the card, which another character already has.
    pub character: String,
    /// The numbered name the duplicate gets (`Sofia-1`).
    pub new_name: String,
    /// The lorebook that comes with it: the name it will get, and its entries.
    pub lorebook: Option<(String, usize)>,
    /// The card, checked and waiting to be written.
    pub ready: Ready,
    /// The button that is lit: `true` is *Yes*.
    pub yes: bool,
    /// Why the last *Yes* failed, for the bottom edge (the window stays open).
    pub note: Option<String>,
    /// The window and its two buttons at the last draw, for the mouse.
    pub window: Rect,
    pub yes_area: Rect,
    pub no_area: Rect,
}

impl ImportAsk {
    pub fn new(
        character: String,
        new_name: String,
        lorebook: Option<(String, usize)>,
        ready: Ready,
    ) -> ImportAsk {
        ImportAsk {
            character,
            new_name,
            lorebook,
            ready,
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

    fn ready() -> Ready {
        Ready {
            bytes: b"{}".to_vec(),
            file_name: "Sofia.png".into(),
            name: "Sofia".into(),
        }
    }

    fn ask() -> ImportAsk {
        ImportAsk::new(
            "Sofia".into(),
            "Sofia-1".into(),
            Some(("Sofia lorebook-1".into(), 12)),
            ready(),
        )
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
