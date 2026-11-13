//! `/lore`: a full-screen manager for lorebooks, opened from inside a chat like
//! `/api` and `/formatting`. Three screens:
//!
//! - the list of books (new, import, delete, duplicate, export, on / off),
//! - one book: its settings (name, who it is on for, scan depth, token
//!   budget) and its entries,
//! - one entry: every setting the prompt builder understands (keys, secondary
//!   keys, always on, position, depth, role, order, text).
//!
//! What a book is and how it is stored and imported is in `lorestore`; what a
//! book does in a prompt is in `lorebook` and `textgen`. The text windows are
//! the ones of `/formatting` (`fmttui::MultiEdit`), vim modes included.

use crate::fmttui::{centered, clip, draw_multi, dwidth, multi_edit_key, EditEnd, MultiEdit};
use crate::input::{Input, Mode};
use crate::jv::*;
use crate::lorestore::{self, Book, Item, AFTER, BEFORE, DEPTH};
use crate::pathcomp;
use anyhow::Result;
use crossterm::event::{
    Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use futures_util::StreamExt;
use serde_json::Value;
use ratatui::{
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

const DIM: Color = Color::Rgb(128, 128, 128);

// ------------------------------------------------------------------- state

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Books,
    Book,
    Entry,
}

/// The settings of a book.
#[derive(Clone, Copy, PartialEq)]
enum BF {
    Name,
    Description,
    Global,
    Characters,
    ScanDepth,
    TokenBudget,
}

/// The settings of an entry.
#[derive(Clone, Copy, PartialEq)]
enum EF {
    Title,
    Enabled,
    Keys,
    Secondary,
    Logic,
    Constant,
    Case,
    Position,
    Depth,
    Role,
    Order,
    Content,
}

/// A row of the book screen.
#[derive(Clone, Copy, PartialEq)]
enum BRow {
    Header(&'static str),
    Setting(BF),
    Entry(usize),
    NoEntries,
}

/// What a text window is for.
#[derive(Clone, Copy, PartialEq)]
enum Target {
    Book(BF),
    Item(EF),
    NewBook,
    ImportPath,
    /// `true`: as SillyTavern World Info.
    ExportPath(bool),
}

#[derive(Clone, Copy, PartialEq)]
enum Confirm {
    /// Leave the book with unsaved changes; `true` quits `/lore` altogether.
    Leave(bool),
    DeleteBook,
    DeleteEntry(usize),
}

enum Modal {
    None,
    Edit(Box<MultiEdit>, Target),
    Confirm(Confirm),
    /// Characters whose card has a lorebook: (card file, name, entries), and
    /// the one under the cursor.
    Chars(Vec<(String, String, usize)>, usize),
    /// "On for": every character with a switch (card file, name), and
    /// the one under the cursor.
    Toggle(Vec<(String, String)>, usize),
}

struct Tui {
    /// The chat box: only its vim settings are used.
    cfg: Input,
    screen: Screen,
    books: Vec<Book>,
    /// Files in `lorebooks/` that could not be read.
    problems: Vec<String>,
    /// The selected row of each screen.
    sels: [usize; 3],
    scroll: usize,
    follow: bool,
    /// The book being edited, and what its file holds.
    cur: Option<Book>,
    saved: Option<Book>,
    /// A fingerprint of the file `cur` was read from or last saved to: a save
    /// is refused when the file is no longer like that (another device).
    disk: String,
    /// Earlier states of `cur`, newest last (`u` goes back one).
    undo: Vec<Book>,
    /// The entry being edited (entry screen).
    eidx: usize,
    status: Option<(String, bool)>,
    modal: Modal,
    /// Path completion of the import window: the listing and the current one.
    comp: Option<(Vec<String>, usize)>,
    quit: bool,
    rows_rect: Rect,
    /// The character of the chat `/lore` was opened from.
    avatar: String,
    char_name: String,
}

fn split_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
        .collect()
}

/// The windows that take a number: (is it a number window, may it be negative).
fn numeric_target(t: Target) -> Option<bool> {
    match t {
        Target::Book(BF::ScanDepth) | Target::Book(BF::TokenBudget) | Target::Item(EF::Depth) => Some(false),
        Target::Item(EF::Order) => Some(true),
        _ => None,
    }
}

/// Drop everything but digits (and a leading `-` where allowed).
fn keep_number(text: &str, negative: bool) -> String {
    text.chars()
        .enumerate()
        .filter(|(i, c)| c.is_ascii_digit() || (negative && *i == 0 && *c == '-'))
        .map(|(_, c)| c)
        .collect()
}

/// The text of a number window, cleaned in place (cursor kept in range).
fn clean_number_window(m: &mut MultiEdit, negative: bool) {
    if m.input.mode == Mode::Command {
        return;
    }
    let clean = keep_number(&m.input.text, negative);
    if clean != m.input.text {
        m.input.text = clean;
        m.input.cursor = m.input.cursor.min(m.input.len_chars());
    }
}

/// Shown when "On for" is used while the book is on for every chat.
const CHARS_OFF: &str = "'On for every chat' is on, so this list is not needed: switch it off to choose characters";

fn char_stem(s: &str) -> String {
    let l = s.trim().to_lowercase();
    l.strip_suffix(".png").map(str::to_string).unwrap_or(l)
}

fn parse_number(text: &str, min: i64, what: &str) -> Result<i64, String> {
    match text.trim().parse::<i64>() {
        Ok(n) if n >= min => Ok(n),
        _ if min == 0 => Err(format!("{}: a whole number, 0 or more", what)),
        _ => Err(format!("{}: a whole number", what)),
    }
}

impl Tui {
    fn new(cfg: &Input, avatar: &str, char_name: &str) -> Tui {
        let mut t = Tui {
            cfg: Input::default(),
            screen: Screen::Books,
            books: Vec::new(),
            problems: Vec::new(),
            sels: [0; 3],
            scroll: 0,
            follow: true,
            cur: None,
            saved: None,
            disk: String::new(),
            undo: Vec::new(),
            eidx: 0,
            status: None,
            modal: Modal::None,
            comp: None,
            quit: false,
            rows_rect: Rect::default(),
            avatar: avatar.to_string(),
            char_name: char_name.to_string(),
        };
        t.cfg.copy_config(cfg);
        t.reload();
        t
    }

    fn say(&mut self, msg: impl Into<String>, is_error: bool) {
        self.status = Some((msg.into(), is_error));
    }

    /// Read the list of books again.
    fn reload(&mut self) {
        let (books, problems) = lorestore::list();
        self.books = books;
        self.problems = problems;
        let last = self.books.len().saturating_sub(1);
        if self.sels[0] > last {
            self.sels[0] = last;
        }
    }

    /// Select the book with this id in the list.
    fn select_book(&mut self, id: &str) {
        if let Some(i) = self.books.iter().position(|b| b.id == id) {
            self.sels[0] = i;
            self.follow = true;
        }
    }

    fn dirty(&self) -> bool {
        match (&self.cur, &self.saved) {
            (Some(a), Some(b)) => a != b,
            _ => false,
        }
    }

    fn snapshot(&mut self) {
        if let Some(b) = &self.cur {
            if self.undo.len() >= 100 {
                self.undo.remove(0);
            }
            self.undo.push(b.clone());
        }
    }

    /// Change the open book; the change can be undone (`u`) when it did
    /// something.
    fn change(&mut self, f: impl FnOnce(&mut Book)) {
        let Some(b) = self.cur.as_ref() else {
            return;
        };
        let mut next = b.clone();
        f(&mut next);
        if &next != b {
            self.snapshot();
            self.cur = Some(next);
        }
    }

    /// Change the entry being edited.
    fn change_item(&mut self, f: impl FnOnce(&mut Item)) {
        let i = self.eidx;
        self.change(|b| {
            if let Some(e) = b.entries.get_mut(i) {
                f(e);
            }
        });
    }

    fn undo_last(&mut self) {
        match self.undo.pop() {
            Some(prev) => {
                self.cur = Some(prev);
                self.fix_selection();
                self.say("undone", false);
            }
            None => self.say("nothing to undo", true),
        }
    }

    /// Keep the selection (and the entry being edited) inside what exists.
    fn fix_selection(&mut self) {
        let n = self.cur.as_ref().map(|b| b.entries.len()).unwrap_or(0);
        if self.eidx >= n {
            self.eidx = n.saturating_sub(1);
            if self.screen == Screen::Entry && n == 0 {
                self.screen = Screen::Book;
            }
        }
        let rows = self.row_count();
        let i = self.si();
        if self.sels[i] >= rows {
            self.sels[i] = rows.saturating_sub(1);
        }
    }

    // ----------------------------------------------------------- rows

    fn si(&self) -> usize {
        match self.screen {
            Screen::Books => 0,
            Screen::Book => 1,
            Screen::Entry => 2,
        }
    }

    fn sel(&self) -> usize {
        self.sels[self.si()]
    }

    fn set_sel(&mut self, v: usize) {
        let i = self.si();
        self.sels[i] = v;
    }

    fn book_rows(&self) -> Vec<BRow> {
        let mut r = vec![
            BRow::Header("Lorebook"),
            BRow::Setting(BF::Name),
            BRow::Setting(BF::Description),
            BRow::Setting(BF::Global),
            BRow::Setting(BF::Characters),
            BRow::Setting(BF::ScanDepth),
            BRow::Setting(BF::TokenBudget),
            BRow::Header("Entries"),
        ];
        match &self.cur {
            Some(b) if !b.entries.is_empty() => {
                for i in 0..b.entries.len() {
                    r.push(BRow::Entry(i));
                }
            }
            _ => r.push(BRow::NoEntries),
        }
        r
    }

    fn entry_fields(&self) -> Vec<EF> {
        let place = self
            .cur
            .as_ref()
            .and_then(|b| b.entries.get(self.eidx))
            .map(|e| e.place)
            .unwrap_or(BEFORE);
        let mut v = vec![
            EF::Title,
            EF::Enabled,
            EF::Keys,
            EF::Secondary,
            EF::Logic,
            EF::Constant,
            EF::Case,
            EF::Position,
        ];
        if place == DEPTH {
            v.push(EF::Depth);
            v.push(EF::Role);
        }
        v.push(EF::Order);
        v.push(EF::Content);
        v
    }

    fn row_count(&self) -> usize {
        match self.screen {
            Screen::Books => self.books.len(),
            Screen::Book => self.book_rows().len(),
            Screen::Entry => self.entry_fields().len(),
        }
    }

    fn selectable(&self, i: usize) -> bool {
        match self.screen {
            Screen::Books => i < self.books.len(),
            Screen::Book => matches!(
                self.book_rows().get(i),
                Some(BRow::Setting(_)) | Some(BRow::Entry(_))
            ),
            Screen::Entry => i < self.entry_fields().len(),
        }
    }

    fn move_row(&mut self, dir: i64) {
        let n = self.row_count() as i64;
        let mut r = self.sel() as i64;
        loop {
            r += dir;
            if r < 0 || r >= n {
                return;
            }
            if self.selectable(r as usize) {
                self.set_sel(r as usize);
                self.follow = true;
                return;
            }
        }
    }

    fn first_row(&mut self) {
        let n = self.row_count();
        if let Some(i) = (0..n).find(|i| self.selectable(*i)) {
            self.set_sel(i);
            self.follow = true;
        }
    }

    fn last_row(&mut self) {
        let n = self.row_count();
        if let Some(i) = (0..n).rev().find(|i| self.selectable(*i)) {
            self.set_sel(i);
            self.follow = true;
        }
    }

    fn book_row(&self) -> Option<BRow> {
        self.book_rows().get(self.sels[1]).copied()
    }

    fn field(&self) -> Option<EF> {
        self.entry_fields().get(self.sels[2]).copied()
    }

    // ---------------------------------------------------- list screen

    fn open_book(&mut self, i: usize) {
        let Some(listed) = self.books.get(i).cloned() else {
            return;
        };
        // The file as it is now (the list may be older than a sync).
        let (b, disk) = match lorestore::load(&listed.id) {
            Ok(x) => x,
            Err(_) => {
                let h = lorestore::disk_hash(&listed.id).unwrap_or_default();
                (listed, h)
            }
        };
        self.disk = disk;
        self.saved = Some(b.clone());
        self.cur = Some(b);
        self.undo.clear();
        self.screen = Screen::Book;
        self.sels[1] = 1;
        self.scroll = 0;
        self.follow = true;
    }

    /// Back to the list (the open book is dropped, saved or not).
    fn leave_book(&mut self, quit_all: bool) {
        self.cur = None;
        self.saved = None;
        self.undo.clear();
        self.screen = Screen::Books;
        self.scroll = 0;
        self.follow = true;
        self.reload();
        if quit_all {
            self.quit = true;
        }
    }

    fn try_leave(&mut self) {
        if self.dirty() {
            self.modal = Modal::Confirm(Confirm::Leave(false));
        } else {
            self.leave_book(false);
        }
    }

    fn try_quit(&mut self) {
        if self.dirty() {
            self.modal = Modal::Confirm(Confirm::Leave(true));
        } else {
            self.quit = true;
        }
    }

    /// Write the open book. `true` when it is saved.
    fn save(&mut self) -> bool {
        let Some(b) = self.cur.as_mut() else {
            return false;
        };
        match lorestore::save_checked(b, &self.disk) {
            Ok(hash) => {
                self.disk = hash;
                self.saved = self.cur.clone();
                self.say("saved: chats use it from the next reply on", false);
                true
            }
            Err(e) => {
                self.say(format!("could not save: {:#}", e), true);
                false
            }
        }
    }

    /// Space in the list: switch a book on or off as a whole (off, it is never
    /// used, whatever its own settings say).
    fn toggle_enabled(&mut self) {
        let Some(listed) = self.books.get(self.sels[0]).cloned() else {
            return;
        };
        // Change the file as it is now, not the list's older copy.
        let (mut b, seen) = lorestore::load(&listed.id).unwrap_or_else(|_| {
            let h = lorestore::disk_hash(&listed.id).unwrap_or_default();
            (listed, h)
        });
        b.enabled = !b.enabled;
        match lorestore::save_checked(&mut b, &seen) {
            Ok(_) => {
                let msg = if b.enabled {
                    format!("'{}' is on: used as its settings say", b.name)
                } else {
                    format!("'{}' is off: not used at all", b.name)
                };
                self.reload();
                self.select_book(&b.id);
                self.say(msg, false);
            }
            Err(e) => self.say(format!("could not save: {:#}", e), true),
        }
    }

    fn duplicate_book(&mut self) {
        let Some(mut b) = self.books.get(self.sels[0]).cloned() else {
            return;
        };
        b.id = String::new();
        b.name = format!("{} copy", b.name);
        b.active = false;
        match lorestore::save(&mut b) {
            Ok(()) => {
                self.reload();
                self.select_book(&b.id);
                self.say(format!("copied as '{}' (off)", b.name), false);
            }
            Err(e) => self.say(format!("could not copy: {:#}", e), true),
        }
    }

    fn delete_book(&mut self) {
        let Some(b) = self.books.get(self.sels[0]).cloned() else {
            return;
        };
        match lorestore::delete(&b.id) {
            Ok(()) => {
                self.reload();
                self.say(format!("deleted '{}'", b.name), false);
            }
            Err(e) => self.say(format!("could not delete: {:#}", e), true),
        }
    }

    /// Was the lorebook of this card already copied? Books made here remember
    /// their character; older ones are recognised by their name.
    fn already_imported(&self, card: &Value) -> bool {
        let avatar = jstr(card, "avatar");
        let book_name = card
            .get("data")
            .and_then(|d| d.get("character_book"))
            .map(|b| jstr(b, "name"))
            .unwrap_or_default();
        let name = if book_name.trim().is_empty() {
            format!("{} lorebook", jstr(card, "name").trim())
        } else {
            book_name
        };
        self.books.iter().any(|b| {
            (!b.source.is_empty() && char_stem(&b.source) == char_stem(&avatar))
                || b.name.trim().to_lowercase() == name.trim().to_lowercase()
        })
    }

    /// `c`: pick a character whose card carries a lorebook that is not in
    /// your lorebooks yet, and copy it.
    fn open_chars(&mut self) {
        let mut found = Vec::new();
        let mut copied = 0;
        for v in crate::charstore::list() {
            let n = v
                .get("data")
                .and_then(|d| d.get("character_book"))
                .and_then(|b| b.get("entries"))
                .and_then(|e| e.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            if n == 0 {
                continue;
            }
            if self.already_imported(&v) {
                copied += 1;
            } else {
                found.push((jstr(&v, "avatar"), jstr(&v, "name"), n));
            }
        }
        if found.is_empty() && copied > 0 {
            self.say("every lorebook in your characters' cards is already copied here", false);
        } else if found.is_empty() {
            self.say("none of your characters has a lorebook in its card", true);
        } else {
            self.modal = Modal::Chars(found, 0);
        }
    }

    /// Enter on "On for": every character, each with a switch.
    fn open_toggle(&mut self) {
        let Some(b) = &self.cur else {
            return;
        };
        let mut list: Vec<(String, String)> = crate::charstore::list()
            .iter()
            .map(|v| (jstr(v, "avatar"), jstr(v, "name")))
            .filter(|(a, _)| !a.is_empty())
            .collect();
        // Names in the book that no card has any more stay in the list, so
        // they can be taken out.
        for c in &b.characters {
            if !list.iter().any(|(a, _)| char_stem(a) == char_stem(c)) {
                list.push((c.clone(), format!("{} (no such card)", c)));
            }
        }
        if list.is_empty() {
            self.say("there are no characters yet", true);
            return;
        }
        let at = list
            .iter()
            .position(|(a, _)| !self.avatar.trim().is_empty() && char_stem(a) == char_stem(&self.avatar))
            .unwrap_or(0);
        self.modal = Modal::Toggle(list, at);
    }

    fn import_character(&mut self, avatar: &str) {
        let result = crate::charstore::get(avatar)
            .and_then(|card| lorestore::from_card(&card, avatar))
            .and_then(lorestore::add);
        match result {
            Ok(b) => {
                self.reload();
                self.select_book(&b.id);
                self.say(
                    format!(
                        "copied the lorebook of {} as '{}' ({} entries, off: press a to switch it on)",
                        avatar.trim_end_matches(".png"),
                        b.name,
                        b.entries.len()
                    ),
                    false,
                );
            }
            Err(e) => self.say(format!("{:#}", e), true),
        }
    }

    // --------------------------------------------------- text windows

    fn start_edit(&mut self, target: Target, text: String, single: bool, hint: &'static str) {
        let mut m = MultiEdit::open(&self.cfg, text, single);
        m.hint = Some(hint);
        // A window that starts empty (a name, a path) is for typing.
        if matches!(
            target,
            Target::NewBook | Target::ImportPath | Target::ExportPath(_)
        ) {
            m.input.enter_insert();
        }
        self.comp = None;
        self.modal = Modal::Edit(Box::new(m), target);
    }

    fn begin_import(&mut self) {
        self.start_edit(
            Target::ImportPath,
            String::new(),
            true,
            "Tab complete  Up/Down next  Enter import  or paste JSON",
        );
    }

    fn begin_export(&mut self, st: bool) {
        let book = match self.screen {
            Screen::Books => self.books.get(self.sels[0]).cloned(),
            _ => self.cur.clone(),
        };
        let Some(b) = book else {
            return;
        };
        let id = lorestore::slug(&b.name);
        let name = if st { format!("{}-st.json", id) } else { format!("{}.json", id) };
        let dir = std::env::current_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|_| ".".to_string());
        self.start_edit(
            Target::ExportPath(st),
            format!("{}/{}", dir.trim_end_matches('/'), name),
            true,
            "Enter write the file  Esc cancel  (a folder or a new file)",
        );
    }

    fn target_title(&self, t: Target) -> &'static str {
        match t {
            Target::NewBook => "New lorebook: name",
            Target::ImportPath => {
                if self.screen == Screen::Books {
                    "Import: lorebook or card file (.json / .png)"
                } else {
                    "Import entries into this book: file"
                }
            }
            Target::ExportPath(true) => "Export as SillyTavern World Info",
            Target::ExportPath(false) => "Export as a V2 lorebook",
            Target::Book(BF::Name) => "Name",
            Target::Book(BF::Description) => "Description",
            Target::Book(BF::Characters) => "Characters (comma-separated)",
            Target::Book(BF::ScanDepth) => "Scan depth (messages)",
            Target::Book(BF::TokenBudget) => "Token budget (0 = no limit)",
            Target::Book(BF::Global) => "Lorebook",
            Target::Item(f) => ef_label(f),
        }
    }

    fn begin_book_edit(&mut self, f: BF) {
        let Some(b) = &self.cur else {
            return;
        };
        if f == BF::Characters && b.active {
            self.say(CHARS_OFF, true);
            return;
        }
        let (text, single) = match f {
            BF::Name => (b.name.clone(), true),
            BF::Description => (b.description.clone(), false),
            BF::Characters => {
                self.open_toggle();
                return;
            }
            BF::ScanDepth => (b.scan_depth.to_string(), true),
            BF::TokenBudget => (b.token_budget.to_string(), true),
            BF::Global => {
                self.adjust_book(1);
                return;
            }
        };
        let hint = if single {
            "Enter accept  Esc cancel"
        } else {
            "Ctrl+S accept  :w accept  Esc leaves INSERT, then cancels"
        };
        self.start_edit(Target::Book(f), text, single, hint);
    }

    fn begin_item_edit(&mut self, f: EF) {
        let Some(e) = self.cur.as_ref().and_then(|b| b.entries.get(self.eidx)) else {
            return;
        };
        let (text, single) = match f {
            EF::Title => (e.name.clone(), true),
            EF::Keys => (e.keys.join(", "), true),
            EF::Secondary => (e.secondary.join(", "), true),
            EF::Order => (e.order.to_string(), true),
            EF::Depth => (e.depth.to_string(), true),
            EF::Content => (e.content.replace("\r\n", "\n").replace('\r', "\n"), false),
            _ => {
                self.adjust_item(1);
                return;
            }
        };
        let hint = if single {
            "Enter accept  Esc cancel"
        } else {
            "Ctrl+S accept  :w accept  Esc leaves INSERT, then cancels"
        };
        self.start_edit(Target::Item(f), text, single, hint);
    }

    /// A window was accepted. `Err`: the window stays open and says why.
    fn apply_edit(&mut self, target: Target, text: &str) -> Result<(), String> {
        match target {
            Target::Book(f) => self.apply_book(f, text),
            Target::Item(f) => self.apply_item(f, text),
            Target::NewBook => {
                let name = text.trim();
                if name.is_empty() {
                    return Err("a lorebook needs a name".to_string());
                }
                let mut b = Book::new(name);
                lorestore::save(&mut b).map_err(|e| format!("{:#}", e))?;
                self.reload();
                self.select_book(&b.id);
                let i = self.sels[0];
                self.open_book(i);
                self.say("new lorebook: n adds an entry, s saves", false);
                Ok(())
            }
            Target::ImportPath => self.apply_import(text),
            Target::ExportPath(st) => {
                let book = match self.screen {
                    Screen::Books => self.books.get(self.sels[0]).cloned(),
                    _ => self.cur.clone(),
                };
                let Some(b) = book else {
                    return Ok(());
                };
                let p = lorestore::export_to(&b, text, st).map_err(|e| format!("{:#}", e))?;
                self.say(format!("wrote {}", p.display()), false);
                Ok(())
            }
        }
    }

    fn apply_book(&mut self, f: BF, text: &str) -> Result<(), String> {
        match f {
            BF::Name => {
                let t = text.trim().to_string();
                if t.is_empty() {
                    return Err("a lorebook needs a name".to_string());
                }
                self.change(|b| b.name = t);
            }
            BF::Description => {
                let t = text.trim_end().to_string();
                self.change(|b| b.description = t);
            }
            BF::Characters => {
                let list = split_list(text);
                self.change(|b| b.characters = list);
            }
            BF::ScanDepth => {
                let n = parse_number(text, 0, "scan depth")?;
                self.change(|b| b.scan_depth = n);
            }
            BF::TokenBudget => {
                let n = parse_number(text, 0, "token budget")?;
                self.change(|b| b.token_budget = n);
            }
            BF::Global => {}
        }
        Ok(())
    }

    fn apply_item(&mut self, f: EF, text: &str) -> Result<(), String> {
        match f {
            EF::Title => {
                let t = text.trim().to_string();
                self.change_item(|e| e.name = t);
            }
            EF::Keys => {
                let l = split_list(text);
                self.change_item(|e| e.keys = l);
            }
            EF::Secondary => {
                let l = split_list(text);
                self.change_item(|e| e.secondary = l);
            }
            EF::Order => {
                let n = text
                    .trim()
                    .parse::<i64>()
                    .map_err(|_| "order: a whole number".to_string())?;
                self.change_item(|e| e.order = n);
            }
            EF::Depth => {
                let n = parse_number(text, 0, "depth")?;
                self.change_item(|e| e.depth = n);
            }
            EF::Content => {
                let t = text.to_string();
                self.change_item(|e| e.content = t);
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_import(&mut self, text: &str) -> Result<(), String> {
        let imp = lorestore::import_text(text).map_err(|e| format!("{:#}", e))?;
        let n = imp.book.entries.len();
        let name = imp.book.name.clone();
        let mut msg = String::new();
        if !imp.notes.is_empty() {
            msg = format!("  (not supported, simplified: {})", imp.notes.join("; "));
        }
        if self.screen == Screen::Books {
            let b = lorestore::add(imp).map_err(|e| format!("{:#}", e))?;
            self.reload();
            self.select_book(&b.id);
            self.say(
                format!(
                    "imported '{}': {} entries, off until you press a{}",
                    name, n, msg
                ),
                false,
            );
        } else {
            let list = imp.book.entries;
            self.change(|b| b.entries.extend(list));
            self.say(format!("added {} entries from '{}' (s saves){}", n, name, msg), false);
        }
        Ok(())
    }

    /// Tab in the import window: list what the typed path can continue with
    /// and fill in the first one (Up / Down choose another, Tab goes into a
    /// folder).
    fn complete(&mut self, m: &mut MultiEdit) {
        let list: Vec<String> = pathcomp::entries(&m.input.text)
            .into_iter()
            .map(|e| e.insert)
            .collect();
        if list.is_empty() {
            self.comp = None;
            m.note = Some(("nothing here to complete".to_string(), true));
            return;
        }
        m.input.text = list[0].clone();
        m.input.cursor = m.input.len_chars();
        self.comp = Some((list, 0));
    }

    /// Up / Down after a Tab: another entry of the same listing.
    fn cycle(&mut self, m: &mut MultiEdit, forward: bool) {
        let Some((list, i)) = self.comp.take() else {
            return;
        };
        let n = list.len();
        let next = if forward { (i + 1) % n } else { (i + n - 1) % n };
        m.input.text = list[next].clone();
        m.input.cursor = m.input.len_chars();
        self.comp = Some((list, next));
    }

    // ------------------------------------------------------ changes

    /// Left / Right on a setting of the book.
    fn adjust_book(&mut self, dir: i64) {
        let Some(BRow::Setting(f)) = self.book_row() else {
            return;
        };
        match f {
            BF::Global => self.change(|b| b.active = !b.active),
            BF::Characters => {
                if self.cur.as_ref().map(|b| b.active).unwrap_or(false) {
                    self.say(CHARS_OFF, true);
                    return;
                }
                if self.avatar.trim().is_empty() {
                    self.say("no character in this chat", true);
                    return;
                }
                let avatar = self.avatar.clone();
                let name = self.char_name.clone();
                let had = self.cur.as_ref().map(|b| b.has_character(&avatar)).unwrap_or(false);
                self.change(|b| b.toggle_character(&avatar));
                let msg = if had {
                    format!("{} taken out of the list", name)
                } else {
                    format!("{} added: the book is on for this character", name)
                };
                self.say(msg, false);
            }
            BF::ScanDepth => self.change(|b| b.scan_depth = (b.scan_depth + dir).max(0)),
            BF::TokenBudget => self.change(|b| b.token_budget = (b.token_budget + dir * 100).max(0)),
            BF::Name | BF::Description => {}
        }
    }

    /// Left / Right on a setting of the entry.
    fn adjust_item(&mut self, dir: i64) {
        let Some(f) = self.field() else {
            return;
        };
        match f {
            EF::Enabled => self.change_item(|e| e.enabled = !e.enabled),
            EF::Logic => self.change_item(|e| step_logic(e, dir)),
            EF::Constant => self.change_item(|e| e.constant = !e.constant),
            EF::Case => self.change_item(|e| e.case_sensitive = !e.case_sensitive),
            EF::Position => self.change_item(|e| {
                e.place = (e.place as i64 + dir).rem_euclid(3) as u8;
            }),
            EF::Role => self.change_item(|e| {
                e.role = (e.role as i64 + dir).rem_euclid(3) as u8;
            }),
            EF::Depth => self.change_item(|e| e.depth = (e.depth + dir).max(0)),
            EF::Order => self.change_item(|e| e.order += dir * 10),
            EF::Title | EF::Keys | EF::Secondary | EF::Content => {}
        }
        // Depth and Role only exist at a depth: the list may have changed.
        self.fix_selection();
    }

    fn new_entry(&mut self) {
        let Some(b) = &self.cur else {
            return;
        };
        let at = match self.book_row() {
            Some(BRow::Entry(i)) => i + 1,
            _ => b.entries.len(),
        };
        self.change(|b| {
            let at = at.min(b.entries.len());
            b.entries.insert(at, Item::new());
        });
        self.open_entry(at);
        self.say("new entry: Enter on a row edits it (Keys, Text ...)", false);
    }

    fn open_entry(&mut self, i: usize) {
        if self.cur.as_ref().map(|b| i < b.entries.len()).unwrap_or(false) {
            self.eidx = i;
            self.screen = Screen::Entry;
            self.sels[2] = 0;
            self.scroll = 0;
            self.follow = true;
        }
    }

    fn back_to_book(&mut self) {
        self.screen = Screen::Book;
        // Land on the entry that was open.
        let target = self.eidx;
        if let Some(r) = self
            .book_rows()
            .iter()
            .position(|r| *r == BRow::Entry(target))
        {
            self.sels[1] = r;
        }
        self.scroll = 0;
        self.follow = true;
    }

    fn duplicate_entry(&mut self, i: usize) {
        self.change(|b| {
            if let Some(e) = b.entries.get(i).cloned() {
                b.entries.insert(i + 1, e);
            }
        });
        self.move_row(1);
        self.say("entry copied", false);
    }

    fn delete_entry(&mut self, i: usize) {
        self.change(|b| {
            if i < b.entries.len() {
                b.entries.remove(i);
            }
        });
        self.fix_selection();
        self.say("entry deleted (u undoes)", false);
    }

    // ------------------------------------------------------- events

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(k.code, KeyCode::Char('c')) {
            self.try_quit();
            return;
        }
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::Edit(m, target) => {
                if let Some(m) = self.edit_key(*m, target, k) {
                    self.modal = Modal::Edit(Box::new(m), target);
                }
            }
            Modal::Confirm(what) => self.confirm_key(what, k),
            Modal::Chars(list, i) => self.chars_key(list, i, k),
            Modal::Toggle(list, i) => self.toggle_key(list, i, k),
            Modal::None => self.on_key_plain(k),
        }
    }

    fn edit_key(&mut self, mut m: MultiEdit, target: Target, k: KeyEvent) -> Option<MultiEdit> {
        self.status = None;
        if target == Target::ImportPath && m.input.mode != Mode::Command {
            match k.code {
                KeyCode::Tab => {
                    self.complete(&mut m);
                    return Some(m);
                }
                KeyCode::Up | KeyCode::Down if self.comp.is_some() => {
                    self.cycle(&mut m, k.code == KeyCode::Down);
                    return Some(m);
                }
                _ => {}
            }
        }
        self.comp = None;
        let number = numeric_target(target);
        match multi_edit_key(&mut m, k) {
            EditEnd::Keep => {
                if let Some(neg) = number {
                    clean_number_window(&mut m, neg);
                }
                Some(m)
            }
            EditEnd::Cancel => None,
            EditEnd::Accept(text) => {
                let text = match number {
                    Some(neg) => keep_number(&text, neg),
                    None => text,
                };
                match self.apply_edit(target, &text) {
                    Ok(()) => None,
                    Err(e) => {
                        m.note = Some((e, true));
                        Some(m)
                    }
                }
            }
        }
    }

    fn confirm_key(&mut self, what: Confirm, k: KeyEvent) {
        let yes = matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y'));
        match what {
            Confirm::Leave(quit_all) => match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('s') => {
                    if self.save() {
                        self.leave_book(quit_all);
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') => self.leave_book(quit_all),
                _ => {}
            },
            Confirm::DeleteBook => {
                if yes {
                    self.delete_book();
                }
            }
            Confirm::DeleteEntry(i) => {
                if yes {
                    self.delete_entry(i);
                }
            }
        }
    }

    fn chars_key(&mut self, list: Vec<(String, String, usize)>, mut i: usize, k: KeyEvent) {
        let last = list.len().saturating_sub(1);
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
                if let Some((avatar, _, _)) = list.get(i) {
                    let avatar = avatar.clone();
                    self.import_character(&avatar);
                }
                keep = false;
            }
            _ => {}
        }
        if keep {
            self.modal = Modal::Chars(list, i);
        }
    }

    fn toggle_key(&mut self, list: Vec<(String, String)>, mut i: usize, k: KeyEvent) {
        let last = list.len().saturating_sub(1);
        let mut keep = true;
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => keep = false,
            KeyCode::Up | KeyCode::Char('k') => i = i.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => i = (i + 1).min(last),
            KeyCode::PageUp => i = i.saturating_sub(8),
            KeyCode::PageDown => i = (i + 8).min(last),
            KeyCode::Home => i = 0,
            KeyCode::End => i = last,
            KeyCode::Enter
            | KeyCode::Char(' ')
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Char('h')
            | KeyCode::Char('l') => {
                if let Some((avatar, _)) = list.get(i) {
                    let avatar = avatar.clone();
                    self.change(|b| b.toggle_character(&avatar));
                }
            }
            _ => {}
        }
        if keep {
            self.modal = Modal::Toggle(list, i);
        }
    }

    /// The keys that move through the rows, on every screen.
    fn nav(&mut self, k: KeyEvent) -> bool {
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_row(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_row(1),
            KeyCode::PageUp => (0..8).for_each(|_| self.move_row(-1)),
            KeyCode::PageDown => (0..8).for_each(|_| self.move_row(1)),
            KeyCode::Home => self.first_row(),
            KeyCode::End => self.last_row(),
            _ => return false,
        }
        true
    }

    fn on_key_plain(&mut self, k: KeyEvent) {
        self.status = None;
        if self.nav(k) {
            return;
        }
        match self.screen {
            Screen::Books => self.key_books(k),
            Screen::Book => self.key_book(k),
            Screen::Entry => self.key_entry(k),
        }
    }

    fn key_books(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                let i = self.sels[0];
                self.open_book(i);
            }
            KeyCode::Char('n') => self.start_edit(
                Target::NewBook,
                String::new(),
                true,
                "Enter create  Esc cancel",
            ),
            KeyCode::Char('i') => self.begin_import(),
            KeyCode::Char('c') => self.open_chars(),
            KeyCode::Char(' ') => self.toggle_enabled(),
            KeyCode::Char('x') => self.begin_export(false),
            KeyCode::Char('X') => self.begin_export(true),
            KeyCode::Char('D') => self.duplicate_book(),
            KeyCode::Char('d') | KeyCode::Delete => {
                if !self.books.is_empty() {
                    self.modal = Modal::Confirm(Confirm::DeleteBook);
                }
            }
            _ => {}
        }
    }

    fn key_book(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.try_leave(),
            KeyCode::Char('s') => {
                self.save();
            }
            KeyCode::Char('u') => self.undo_last(),
            KeyCode::Char('n') => self.new_entry(),
            KeyCode::Char('i') => self.begin_import(),
            KeyCode::Char('x') => self.begin_export(false),
            KeyCode::Char('X') => self.begin_export(true),
            KeyCode::Enter => match self.book_row() {
                Some(BRow::Setting(f)) => self.begin_book_edit(f),
                Some(BRow::Entry(i)) => self.open_entry(i),
                _ => {}
            },
            KeyCode::Left | KeyCode::Char('h') => self.adjust_book(-1),
            KeyCode::Right | KeyCode::Char('l') => match self.book_row() {
                Some(BRow::Entry(i)) => self.open_entry(i),
                _ => self.adjust_book(1),
            },
            KeyCode::Char(' ') => match self.book_row() {
                Some(BRow::Entry(i)) => self.change(|b| {
                    if let Some(e) = b.entries.get_mut(i) {
                        e.enabled = !e.enabled;
                    }
                }),
                // Space switches "On for every chat"; on "On for" it opens the list.
                Some(BRow::Setting(BF::Global)) => self.adjust_book(1),
                Some(BRow::Setting(BF::Characters)) => self.begin_book_edit(BF::Characters),
                _ => {}
            },
            KeyCode::Char('D') => {
                if let Some(BRow::Entry(i)) = self.book_row() {
                    self.duplicate_entry(i);
                }
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                if let Some(BRow::Entry(i)) = self.book_row() {
                    self.modal = Modal::Confirm(Confirm::DeleteEntry(i));
                }
            }
            _ => {}
        }
    }

    fn key_entry(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.back_to_book(),
            KeyCode::Char('s') => {
                self.save();
            }
            KeyCode::Char('u') => self.undo_last(),
            KeyCode::Enter => {
                if let Some(f) = self.field() {
                    self.begin_item_edit(f);
                }
            }
            KeyCode::Left | KeyCode::Char('h') => self.adjust_item(-1),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => self.adjust_item(1),
            KeyCode::Tab | KeyCode::BackTab => {
                let n = self.cur.as_ref().map(|b| b.entries.len()).unwrap_or(0);
                if n > 1 {
                    let step = if k.code == KeyCode::Tab { 1 } else { n - 1 };
                    self.eidx = (self.eidx + step) % n;
                    self.fix_selection();
                }
            }
            _ => {}
        }
    }

    fn on_paste(&mut self, s: &str) {
        if let Modal::Edit(m, target) = &mut self.modal {
            if m.input.mode == Mode::Command {
                return;
            }
            if let Some(neg) = numeric_target(*target) {
                let digits = keep_number(s.trim(), neg && m.input.cursor == 0);
                m.input.paste_text(&digits);
                clean_number_window(m, neg);
                return;
            }
            let text = s.replace("\r\n", "\n").replace('\r', "\n");
            if m.single {
                // One line: a pasted line break is a space.
                let one: String = text
                    .chars()
                    .map(|c| if c == '\n' { ' ' } else { c })
                    .filter(|c| !c.is_control())
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
                if r < self.row_count() && self.selectable(r) {
                    // A click selects; a click on the selected row acts on it.
                    let was = self.sel() == r;
                    self.set_sel(r);
                    self.follow = true;
                    self.status = None;
                    if was {
                        self.on_key_plain(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                    }
                }
            }
            _ => {}
        }
    }
}

// ------------------------------------------------------------- labels

fn ef_label(f: EF) -> &'static str {
    match f {
        EF::Title => "Title",
        EF::Enabled => "Enabled",
        EF::Keys => "Keys",
        EF::Secondary => "Secondary keys",
        EF::Logic => "Secondary logic",
        EF::Constant => "Always on",
        EF::Case => "Case sensitive",
        EF::Position => "Position",
        EF::Depth => "Depth",
        EF::Role => "Role",
        EF::Order => "Order",
        EF::Content => "Text",
    }
}

fn ef_help(f: EF) -> &'static str {
    match f {
        EF::Title => "Just a name so you can tell entries apart in the list. The model never sees it.",
        EF::Enabled => "Off: the entry stays in the book but is never used. Handy to pause one without deleting it.",
        EF::Keys => "Trigger words, separated by commas. If any of them appears in the latest messages, this entry's text is added to the prompt. Plain words only.",
        EF::Secondary => "Extra words, separated by commas. 'Secondary logic' decides how they count; they are ignored while it is off.",
        EF::Logic => "How Secondary keys count once a Key matched. Off: ignored. Needs any one / every one: they must appear. Blocked if any / all appear: they stop the entry.",
        EF::Constant => "On: the entry is sent every time, whatever is said. Keys are ignored.",
        EF::Case => "Off (default): 'Dragon' and 'dragon' both match. On: capital letters must match exactly. Applies to Keys and Secondary keys.",
        EF::Position => "Where the text goes: before or after the character's description, or inside the chat history among the messages.",
        EF::Depth => "Only for 'in the chat'. How many messages from the end the text is placed. 0 = after the last message, 1 = just before it.",
        EF::Role => "Only for 'in the chat'. Who the text appears to be said by: system (a note), user, or assistant (the character).",
        EF::Order => "Decides the sequence in the prompt: the lowest number comes first. If the token budget runs out, the highest numbers are kept.",
        EF::Content => "The text that is added to the prompt when the entry is used. {{char}} and {{user}} become the names. Enter to edit.",
    }
}

fn bf_label(f: BF) -> &'static str {
    match f {
        BF::Name => "Name",
        BF::Description => "Description",
        BF::Global => "On for every chat",
        BF::Characters => "On for",
        BF::ScanDepth => "Scan depth",
        BF::TokenBudget => "Token budget",
    }
}

fn bf_help(f: BF, char_name: &str, active: bool) -> String {
    match f {
        BF::Name => "The name shown in the list of lorebooks.".to_string(),
        BF::Description => "A note for yourself about this book. The model never sees it.".to_string(),
        BF::Global => "On: the book is used in every chat with every character. Off: only for the characters chosen below. Left / Right switches it.".to_string(),
        BF::Characters if active => {
            "Disabled: the book is already on for every chat. Switch 'On for every chat' off to choose characters.".to_string()
        }
        BF::Characters => format!(
            "Characters this book is also used for, even when it is not on for everyone. Enter: pick from a list. Left / Right: add or remove {}.",
            if char_name.is_empty() { "this chat's character" } else { char_name }
        ),
        BF::ScanDepth => "How many of the latest messages are searched for trigger words. Higher finds older mentions. Left / Right: 1, Enter: type a number.".to_string(),
        BF::TokenBudget => "The most text (in tokens) the book may add to one prompt. 0 = no limit. Left / Right: 100, Enter: type a number.".to_string(),
    }
}

fn place_text(e: &Item) -> String {
    match e.place {
        AFTER => "after the character".to_string(),
        DEPTH => "in the chat".to_string(),
        _ => "before the character".to_string(),
    }
}

fn role_text(role: u8) -> &'static str {
    match role {
        1 => "user",
        2 => "assistant",
        _ => "system",
    }
}

/// The choices of the "Secondary logic" row, in the order Left / Right goes
/// through them: (secondary keys used, SillyTavern's `selectiveLogic`).
const LOGIC_STEPS: [(bool, u8); 5] = [(false, 0), (true, 0), (true, 3), (true, 2), (true, 1)];

fn logic_index(e: &Item) -> usize {
    LOGIC_STEPS
        .iter()
        .position(|(on, l)| *on == e.selective && (!e.selective || *l == e.logic))
        .unwrap_or(0)
}

fn step_logic(e: &mut Item, dir: i64) {
    let n = LOGIC_STEPS.len() as i64;
    let (on, l) = LOGIC_STEPS[(logic_index(e) as i64 + dir.signum()).rem_euclid(n) as usize];
    e.selective = on;
    e.logic = l;
}

fn logic_text(e: &Item) -> &'static str {
    match LOGIC_STEPS[logic_index(e)] {
        (false, _) => "off (secondary keys ignored)",
        (true, 3) => "needs every one of them",
        (true, 2) => "blocked if any of them appears",
        (true, 1) => "blocked if all of them appear",
        _ => "needs any one of them",
    }
}

/// A value that Left / Right change, as `‹ value ›` (like the /note window).
fn arrows(v: &str) -> String {
    format!("\u{2039} {} \u{203a}", v)
}

fn on_off(b: bool) -> &'static str {
    if b {
        "[x] on"
    } else {
        "[ ] off"
    }
}

/// A single line of what a text holds (a line break shows as `↵`).
fn one_line(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n").replace('\n', " \u{21b5} ")
}

// ----------------------------------------------------------------- drawing

/// A row: `label` and `value`, the label in a column; the selected row is
/// drawn as a bar.
fn pair_line(label: &str, label_w: usize, value: &str, empty: bool, selected: bool, width: usize) -> Line<'static> {
    let l = format!("  {:<w$} ", clip(label, label_w), w = label_w);
    let v = clip(value, width.saturating_sub(label_w + 3));
    if selected {
        let line = format!("{}{}", l, v);
        let pad = " ".repeat(width.saturating_sub(dwidth(&line)));
        Line::from(Span::styled(
            format!("{}{}", line, pad),
            Style::default().fg(Color::Black).bg(Color::Cyan),
        ))
    } else {
        let vs = if empty { Style::default().fg(DIM) } else { Style::default() };
        Line::from(vec![Span::raw(l), Span::styled(v, vs)])
    }
}

fn bar_line(text: &str, selected: bool, dim: bool, width: usize) -> Line<'static> {
    let t = clip(text, width);
    if selected {
        let pad = " ".repeat(width.saturating_sub(dwidth(&t)));
        Line::from(Span::styled(
            format!("{}{}", t, pad),
            Style::default().fg(Color::Black).bg(Color::Cyan),
        ))
    } else if dim {
        Line::from(Span::styled(t, Style::default().fg(DIM)))
    } else {
        Line::from(Span::raw(t))
    }
}

impl Tui {
    fn books_line(&self, i: usize, width: usize) -> Line<'static> {
        let b = &self.books[i];
        let mark = if b.enabled { "[x]" } else { "[ ]" };
        let name_w = (width / 2).clamp(10, 32);
        let text = format!(
            " {} {:<nw$} {:>4} entries  {}",
            mark,
            clip(&b.name, name_w),
            b.entries.len(),
            b.scope(),
            nw = name_w
        );
        bar_line(&text, i == self.sels[0], !b.enabled, width)
    }

    fn book_setting_value(&self, f: BF, b: &Book) -> (String, bool) {
        match f {
            BF::Name => (b.name.clone(), false),
            BF::Description => {
                let s = one_line(&b.description);
                if s.trim().is_empty() {
                    ("(empty)".to_string(), true)
                } else {
                    (s, false)
                }
            }
            BF::Global => (on_off(b.active).to_string(), false),
            BF::Characters => {
                if b.active {
                    ("(not needed: on for every chat)".to_string(), true)
                } else if b.characters.is_empty() {
                    ("(none)".to_string(), true)
                } else {
                    (b.characters.join(", "), false)
                }
            }
            BF::ScanDepth => (arrows(&format!("{} latest messages", b.scan_depth)), false),
            BF::TokenBudget => {
                if b.token_budget == 0 {
                    (arrows("0 (no limit)"), false)
                } else {
                    (arrows(&format!("{} tokens", b.token_budget)), false)
                }
            }
        }
    }

    fn book_line(&self, r: BRow, idx: usize, width: usize) -> Line<'static> {
        let Some(b) = &self.cur else {
            return Line::from("");
        };
        let selected = idx == self.sels[1];
        match r {
            BRow::Header(h) => Line::from(Span::styled(
                format!(" {}", h),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )),
            BRow::Setting(f) => {
                let (v, empty) = self.book_setting_value(f, b);
                pair_line(bf_label(f), 18, &v, empty, selected, width)
            }
            BRow::NoEntries => Line::from(Span::styled(
                "  (no entries: n adds one, i imports some from a file)".to_string(),
                Style::default().fg(DIM),
            )),
            BRow::Entry(i) => {
                let Some(e) = b.entries.get(i) else {
                    return Line::from("");
                };
                let mark = if e.enabled { "[x]" } else { "[ ]" };
                let tag = if e.constant {
                    "always".to_string()
                } else {
                    match e.place {
                        AFTER => "after".to_string(),
                        DEPTH => format!("depth {}", e.depth),
                        _ => "before".to_string(),
                    }
                };
                let label_w = (width / 3).clamp(10, 28);
                let keys_w = width.saturating_sub(label_w + tag.chars().count() + 12);
                let text = format!(
                    "  {} {:<lw$}  {:<kw$}  {}",
                    mark,
                    clip(&e.label(), label_w),
                    clip(&e.keys.join(", "), keys_w),
                    tag,
                    lw = label_w,
                    kw = keys_w
                );
                bar_line(&text, selected, !e.enabled, width)
            }
        }
    }

    fn field_line(&self, f: EF, idx: usize, width: usize) -> Line<'static> {
        let Some(e) = self.cur.as_ref().and_then(|b| b.entries.get(self.eidx)) else {
            return Line::from("");
        };
        let (v, empty) = match f {
            EF::Title => (e.name.clone(), e.name.trim().is_empty()),
            EF::Enabled => (on_off(e.enabled).to_string(), false),
            EF::Keys => (e.keys.join(", "), e.keys.is_empty()),
            EF::Secondary => (e.secondary.join(", "), e.secondary.is_empty()),
            EF::Logic => (arrows(logic_text(e)), false),
            EF::Constant => (on_off(e.constant).to_string(), false),
            EF::Case => (on_off(e.case_sensitive).to_string(), false),
            EF::Position => (arrows(&place_text(e)), false),
            EF::Depth => (arrows(&format!("{} messages from the end", e.depth)), false),
            EF::Role => (arrows(role_text(e.role)), false),
            EF::Order => (arrows(&e.order.to_string()), false),
            EF::Content => (one_line(&e.content), e.content.trim().is_empty()),
        };
        let v = if empty && v.is_empty() { "(empty)".to_string() } else { v };
        pair_line(ef_label(f), 22, &v, empty, idx == self.sels[2], width)
    }

    /// The text under the list: the description of the selected book, or the
    /// text of the selected entry.
    fn detail(&self) -> String {
        match self.screen {
            Screen::Books => self
                .books
                .get(self.sels[0])
                .map(|b| {
                    if b.description.trim().is_empty() {
                        String::new()
                    } else {
                        b.description.clone()
                    }
                })
                .unwrap_or_default(),
            Screen::Book => match (self.book_row(), &self.cur) {
                (Some(BRow::Entry(i)), Some(b)) => {
                    b.entries.get(i).map(|e| e.content.clone()).unwrap_or_default()
                }
                _ => String::new(),
            },
            Screen::Entry => self
                .cur
                .as_ref()
                .and_then(|b| b.entries.get(self.eidx))
                .map(|e| e.content.clone())
                .unwrap_or_default(),
        }
    }

    fn help(&self) -> String {
        match self.screen {
            Screen::Books => match self.books.get(self.sels[0]) {
                Some(b) => format!(
                    "[x] the book is used as its own settings say, [ ] it is never used.  {}: {} of {} entries can fire.",
                    b.name,
                    b.usable_count(),
                    b.entries.len()
                ),
                None => "A lorebook adds text to the prompt when its keys show up in the chat.".to_string(),
            },
            Screen::Book => match self.book_row() {
                Some(BRow::Setting(f)) => {
                    bf_help(f, &self.char_name, self.cur.as_ref().map(|b| b.active).unwrap_or(false))
                }
                Some(BRow::Entry(_)) => {
                    "Enter opens the entry, Space switches it on / off, D copies it, d deletes it.".to_string()
                }
                _ => String::new(),
            },
            Screen::Entry => self.field().map(|f| ef_help(f).to_string()).unwrap_or_default(),
        }
    }
}

fn draw(f: &mut Frame, t: &mut Tui) {
    let area = f.area();
    if area.width < 30 || area.height < 8 {
        f.render_widget(Paragraph::new("too small"), area);
        return;
    }
    let detail_h: u16 = if area.height >= 22 { 3 } else { 0 };
    let help_h: u16 = if area.height >= 14 { 2 } else { 1 };
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(detail_h),
        Constraint::Length(help_h),
        Constraint::Length(1),
    ])
    .split(area);

    // ---- title
    let crumb = match (t.screen, &t.cur) {
        (Screen::Books, _) => " Lorebooks ".to_string(),
        (Screen::Book, Some(b)) => format!(" Lorebook: {} ", b.name),
        (Screen::Entry, Some(b)) => {
            let label = b
                .entries
                .get(t.eidx)
                .map(|e| e.label())
                .unwrap_or_default();
            format!(" {} \u{203a} {} ({}/{}) ", b.name, label, t.eidx + 1, b.entries.len())
        }
        _ => " Lorebooks ".to_string(),
    };
    let mut title = vec![Span::styled(
        clip(&crumb, area.width.saturating_sub(20) as usize),
        Style::default().fg(Color::Black).bg(Color::White),
    )];
    if t.dirty() {
        title.push(Span::styled("  unsaved changes", Style::default().fg(Color::Yellow)));
    }
    f.render_widget(Paragraph::new(Line::from(title)), chunks[0]);

    // ---- rows
    let block_title = match t.screen {
        Screen::Books => " Books ",
        Screen::Book => " Settings and entries ",
        Screen::Entry => " Entry ",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(block_title);
    let inner = block.inner(chunks[1]);
    f.render_widget(block, chunks[1]);
    t.rows_rect = inner;

    let h = inner.height as usize;
    let w = inner.width as usize;
    let count = t.row_count();
    if t.follow {
        let mut first = t.sel();
        // Keep the heading of the group in view too.
        if t.screen == Screen::Book && first > 0 {
            if let Some(BRow::Header(_)) = t.book_rows().get(first - 1) {
                first -= 1;
            }
        }
        if first < t.scroll {
            t.scroll = first;
        } else if t.sel() >= t.scroll + h {
            t.scroll = t.sel() + 1 - h;
        }
    }
    t.scroll = t.scroll.min(count.saturating_sub(h));
    let end = (t.scroll + h).min(count);
    let lines: Vec<Line> = match t.screen {
        Screen::Books => {
            if t.books.is_empty() {
                vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        "  No lorebooks yet.".to_string(),
                        Style::default().fg(DIM),
                    )),
                    Line::from(Span::styled(
                        "  n  write a new one".to_string(),
                        Style::default().fg(DIM),
                    )),
                    Line::from(Span::styled(
                        "  i  import a SillyTavern World Info file, a V2 lorebook or a card".to_string(),
                        Style::default().fg(DIM),
                    )),
                    Line::from(Span::styled(
                        "  c  copy the lorebook that is inside a character's card".to_string(),
                        Style::default().fg(DIM),
                    )),
                ]
            } else {
                (t.scroll..end).map(|i| t.books_line(i, w)).collect()
            }
        }
        Screen::Book => {
            let rows = t.book_rows();
            (t.scroll..end)
                .filter_map(|i| rows.get(i).map(|r| t.book_line(*r, i, w)))
                .collect()
        }
        Screen::Entry => {
            let fields = t.entry_fields();
            (t.scroll..end)
                .filter_map(|i| fields.get(i).map(|fl| t.field_line(*fl, i, w)))
                .collect()
        }
    };
    f.render_widget(Paragraph::new(lines), inner);

    // ---- the text of the selected entry / the book's description
    if detail_h > 0 {
        let text = t.detail();
        if !text.trim().is_empty() {
            f.render_widget(
                Paragraph::new(text)
                    .wrap(Wrap { trim: false })
                    .style(Style::default().fg(Color::Gray)),
                Rect::new(
                    chunks[2].x + 1,
                    chunks[2].y,
                    chunks[2].width.saturating_sub(2),
                    chunks[2].height,
                ),
            );
        }
    }

    // ---- help line
    f.render_widget(
        Paragraph::new(format!(" {}", t.help()))
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(DIM)),
        chunks[3],
    );

    // ---- footer: question, message or key hints
    let footer = chunks[4];
    let question = match &t.modal {
        Modal::Confirm(Confirm::Leave(_)) => {
            Some(" Save changes? [y] save and leave  [n] discard  [any] cancel".to_string())
        }
        Modal::Confirm(Confirm::DeleteBook) => t
            .books
            .get(t.sels[0])
            .map(|b| format!(" Delete '{}' for good? [y/N]", b.name)),
        Modal::Confirm(Confirm::DeleteEntry(_)) => Some(" Delete this entry? [y/N]".to_string()),
        _ => None,
    };
    if let Some(q) = question {
        f.render_widget(
            Paragraph::new(Span::styled(
                clip(&q, area.width as usize),
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
    } else if !t.problems.is_empty() && t.screen == Screen::Books {
        f.render_widget(
            Paragraph::new(Span::styled(
                format!(
                    " could not read: {}",
                    clip(&t.problems.join("; "), area.width.saturating_sub(18) as usize)
                ),
                Style::default().fg(Color::Red),
            )),
            footer,
        );
    } else {
        let hints = match t.screen {
            Screen::Books => " \u{2191}\u{2193} select  Enter open  n new  i import  c from a character  Space on/off  D copy  x/X export  d delete  q quit",
            Screen::Book => " \u{2191}\u{2193} select  Enter edit/open  Space on/off  n new entry  i import  x/X export  d delete  u undo  s save  q back",
            Screen::Entry => " \u{2191}\u{2193} select  Enter edit  \u{2190}\u{2192} change  Tab next entry  u undo  s save  q back",
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                clip(hints, area.width as usize),
                Style::default().fg(DIM),
            )),
            footer,
        );
    }

    // ---- text window
    let target = if let Modal::Edit(_, tg) = &t.modal { Some(*tg) } else { None };
    if let Some(tg) = target {
        let title = t.target_title(tg);
        if let Modal::Edit(m, _) = &mut t.modal {
            draw_multi(f, area, title, m);
        }
    }

    // ---- "On for": every character with a switch
    if let Modal::Toggle(list, sel) = &t.modal {
        let book = t.cur.as_ref();
        let rect = centered(area, 60, (list.len() as u16 + 4).min(area.height));
        f.render_widget(Clear, rect);
        let inner_w = rect.width.saturating_sub(2) as usize;
        let vis = (rect.height as usize).saturating_sub(3).max(1);
        let start = if *sel >= vis { *sel + 1 - vis } else { 0 };
        let mut lines: Vec<Line> = list
            .iter()
            .enumerate()
            .skip(start)
            .take(vis)
            .map(|(i, (avatar, name))| {
                let on = book.map(|b| b.has_character(avatar)).unwrap_or(false);
                let style = if i == *sel {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else if on {
                    Style::default()
                } else {
                    Style::default().fg(DIM)
                };
                Line::from(Span::styled(
                    clip(&format!(" {} {}", if on { "[x]" } else { "[ ]" }, name), inner_w),
                    style,
                ))
            })
            .collect();
        lines.push(Line::from(Span::styled(
            clip(" Enter / Space switch  \u{2191}\u{2193} select  Esc done", inner_w),
            Style::default().fg(DIM),
        )));
        f.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(" On for "),
            ),
            rect,
        );
    }

    // ---- characters with a lorebook
    if let Modal::Chars(list, sel) = &t.modal {
        let rect = centered(area, 70, list.len() as u16 + 4);
        f.render_widget(Clear, rect);
        let inner_w = rect.width.saturating_sub(2) as usize;
        let vis = (rect.height as usize).saturating_sub(3).max(1);
        let start = if *sel >= vis { *sel + 1 - vis } else { 0 };
        let mut lines: Vec<Line> = list
            .iter()
            .enumerate()
            .skip(start)
            .take(vis)
            .map(|(i, (_, name, n))| {
                let style = if i == *sel {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default()
                };
                Line::from(Span::styled(
                    clip(&format!(" {:<30} {} entries", clip(name, 30), n), inner_w),
                    style,
                ))
            })
            .collect();
        lines.push(Line::from(Span::styled(
            clip(" Enter copy it as a lorebook  Esc cancel", inner_w),
            Style::default().fg(DIM),
        )));
        f.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(" Lorebook of a character "),
            ),
            rect,
        );
    }
}

// ------------------------------------------------------------------- entry

/// Open the manager in the chat's own terminal and run it until it is quit.
/// `avatar` / `char_name`: the character of the chat it was opened from.
/// Whatever was saved is in `~/.config/tarpi/lorebooks/` when this returns.
pub async fn manage(
    term: &mut crate::Term,
    events: &mut EventStream,
    cfg: &Input,
    avatar: &str,
    char_name: &str,
) -> Result<()> {
    let mut t = Tui::new(cfg, avatar, char_name);
    if !t.problems.is_empty() {
        let msg = format!("could not read: {}", t.problems.join("; "));
        t.say(msg, true);
    } else if t.books.is_empty() {
        t.say("no lorebooks yet: n writes one, i imports one, c copies a card's", false);
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

    /// A manager with one open book (nothing is written to disk).
    fn open() -> Tui {
        let mut t = Tui::new(&Input::default(), "Sofia.png", "Sofia");
        let b = Book::new("Test");
        t.saved = Some(b.clone());
        t.cur = Some(b);
        t.screen = Screen::Book;
        t.sels[1] = 1;
        t
    }

    #[test]
    fn a_change_is_unsaved_until_it_is_undone() {
        let mut t = open();
        assert!(!t.dirty());
        t.apply_book(BF::Name, " Realm ").unwrap();
        assert_eq!(t.cur.as_ref().unwrap().name, "Realm");
        assert!(t.dirty());
        t.undo_last();
        assert!(!t.dirty());
        // The same value again is not a change.
        t.apply_book(BF::Name, "Test").unwrap();
        assert!(!t.dirty());
    }

    #[test]
    fn numbers_must_be_numbers() {
        let mut t = open();
        assert!(t.apply_book(BF::ScanDepth, "x").is_err());
        assert!(t.apply_book(BF::TokenBudget, "-3").is_err());
        t.apply_book(BF::ScanDepth, " 7 ").unwrap();
        assert_eq!(t.cur.as_ref().unwrap().scan_depth, 7);
        assert!(t.apply_book(BF::Name, "  ").is_err());
    }

    #[test]
    fn entries_are_added_edited_and_deleted() {
        let mut t = open();
        t.change(|b| b.entries.push(Item::new()));
        t.eidx = 0;
        t.apply_item(EF::Keys, "dragon, , wyrm").unwrap();
        t.apply_item(EF::Content, "Hoards.").unwrap();
        t.apply_item(EF::Order, "-5").unwrap();
        assert!(t.apply_item(EF::Order, "x").is_err());
        assert!(t.apply_item(EF::Depth, "-1").is_err());
        let e = &t.cur.as_ref().unwrap().entries[0];
        assert_eq!(e.keys, vec!["dragon", "wyrm"]);
        assert_eq!((e.content.as_str(), e.order), ("Hoards.", -5));
        t.delete_entry(0);
        assert!(t.cur.as_ref().unwrap().entries.is_empty());
        t.undo_last();
        assert_eq!(t.cur.as_ref().unwrap().entries.len(), 1);
    }

    #[test]
    fn depth_and_role_only_show_for_a_depth_entry() {
        let mut t = open();
        t.change(|b| b.entries.push(Item::new()));
        t.eidx = 0;
        t.screen = Screen::Entry;
        assert!(!t.entry_fields().contains(&EF::Depth));
        t.sels[2] = t.entry_fields().iter().position(|f| *f == EF::Position).unwrap();
        t.adjust_item(1); // before -> after
        t.adjust_item(1); // after -> depth
        assert_eq!(t.cur.as_ref().unwrap().entries[0].place, DEPTH);
        assert!(t.entry_fields().contains(&EF::Depth));
        assert!(t.entry_fields().contains(&EF::Role));
        t.adjust_item(1); // depth -> before again
        assert!(!t.entry_fields().contains(&EF::Role));
    }

    #[test]
    fn the_characters_row_toggles_the_chats_character() {
        let mut t = open();
        t.sels[1] = t
            .book_rows()
            .iter()
            .position(|r| *r == BRow::Setting(BF::Characters))
            .unwrap();
        t.adjust_book(1);
        assert_eq!(t.cur.as_ref().unwrap().characters, vec!["Sofia"]);
        t.adjust_book(1);
        assert!(t.cur.as_ref().unwrap().characters.is_empty());
    }

    #[test]
    fn number_windows_drop_what_is_not_a_digit() {
        assert_eq!(keep_number("1a2-3 ", false), "123");
        assert_eq!(keep_number("-1x2", true), "-12");
        assert_eq!(keep_number("1-2", true), "12");
        assert_eq!(numeric_target(Target::Book(BF::ScanDepth)), Some(false));
        assert_eq!(numeric_target(Target::Item(EF::Order)), Some(true));
        assert_eq!(numeric_target(Target::Book(BF::Name)), None);
    }

    #[test]
    fn a_new_entry_opens_without_a_text_window() {
        let mut t = open();
        t.new_entry();
        assert_eq!(t.screen, Screen::Entry);
        assert!(matches!(t.modal, Modal::None));
        assert_eq!(t.cur.as_ref().unwrap().entries.len(), 1);
    }

    #[test]
    fn the_characters_menu_switches_a_character() {
        let mut t = open();
        t.modal = Modal::Toggle(vec![("Ann.png".into(), "Ann".into()), ("Bo.png".into(), "Bo".into())], 0);
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        t.on_key(key(KeyCode::Enter));
        t.on_key(key(KeyCode::Down));
        t.on_key(key(KeyCode::Char(' ')));
        assert_eq!(t.cur.as_ref().unwrap().characters, vec!["Ann", "Bo"]);
        t.on_key(key(KeyCode::Enter));
        assert_eq!(t.cur.as_ref().unwrap().characters, vec!["Ann"]);
        t.on_key(key(KeyCode::Esc));
        assert!(matches!(t.modal, Modal::None));
    }

    #[test]
    fn on_for_is_disabled_while_the_book_is_on_for_every_chat() {
        let mut t = open();
        t.change(|b| b.active = true);
        t.sels[1] = t
            .book_rows()
            .iter()
            .position(|r| *r == BRow::Setting(BF::Characters))
            .unwrap();
        t.adjust_book(1);
        t.begin_book_edit(BF::Characters);
        assert!(t.cur.as_ref().unwrap().characters.is_empty());
        assert!(matches!(t.modal, Modal::None));
        assert!(t.status.as_ref().map(|s| s.1).unwrap_or(false));
        t.change(|b| b.active = false);
        t.begin_book_edit(BF::Characters);
        assert!(matches!(t.modal, Modal::Toggle(..)) || t.status.is_some());
    }

    #[test]
    fn secondary_logic_steps_through_its_choices() {
        let mut e = Item::new();
        assert_eq!(logic_text(&e), "off (secondary keys ignored)");
        let mut seen = Vec::new();
        for _ in 0..5 {
            step_logic(&mut e, 1);
            seen.push((e.selective, e.logic));
        }
        assert_eq!(seen, vec![(true, 0), (true, 3), (true, 2), (true, 1), (false, 0)]);
        step_logic(&mut e, -1);
        assert_eq!((e.selective, e.logic), (true, 1));
        assert_eq!(logic_text(&e), "blocked if all of them appear");
        assert!(!Item::new().case_sensitive);
    }

    #[test]
    fn space_switches_on_for_every_chat() {
        let mut t = open();
        t.sels[1] = t
            .book_rows()
            .iter()
            .position(|r| *r == BRow::Setting(BF::Global))
            .unwrap();
        let space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        t.on_key(space);
        assert!(t.cur.as_ref().unwrap().active);
        t.on_key(space);
        assert!(!t.cur.as_ref().unwrap().active);
    }

    #[test]
    fn headers_cannot_be_selected() {
        let mut t = open();
        t.change(|b| b.entries.push(Item::new()));
        t.sels[1] = 1;
        t.move_row(-1);
        assert_eq!(t.sels[1], 1); // the header above is skipped over
        t.last_row();
        assert_eq!(t.book_row(), Some(BRow::Entry(0)));
        assert!(!t.selectable(0));
        assert!(!t.selectable(7));
    }

    #[test]
    fn lists_are_split_on_commas() {
        assert_eq!(split_list("a, b ,, c"), vec!["a", "b", "c"]);
        assert!(split_list(" , ").is_empty());
        assert_eq!(split_list("  red   dragon ,\tgold  "), vec!["red dragon", "gold"]);
    }
}
