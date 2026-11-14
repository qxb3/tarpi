//! Standalone lorebooks: tarpi's own folder, `~/.config/tarpi/lorebooks/`.
//!
//! A lorebook is one `<id>.json` file there, written as the `character_book`
//! of a V2 card (so another tool can read it as it is) with one addition,
//! `extensions.tarpi`: whether the book is on for every chat (`active`) and
//! the characters it is also on for (`characters`, card file names without
//! `.png`). Fields tarpi does not know about are kept when a book is saved.
//!
//! What a book does in a prompt is decided by `lorebook` (keys, positions,
//! budget); this file is the model that `/lore` (`loretui`) edits, the
//! reading and writing of the files, and the import of other formats:
//! SillyTavern World Info files, V2 `character_book` files and the lorebook
//! inside a character card (a `.json` or `.png` file, or a character already
//! in tarpi). `active_books` is what `textgen` asks for when it builds a prompt.

use crate::charstore;
use crate::config;
use crate::jv::*;
use crate::lorebook::{strings, Entry as LoreEntry, Lorebook, Place};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// `Item::place`: ahead of the character's description.
pub const BEFORE: u8 = 0;
/// `Item::place`: after the character's description and scenario.
pub const AFTER: u8 = 1;
/// `Item::place`: in the chat, `Item::depth` messages from the end.
pub const DEPTH: u8 = 2;

pub const DEFAULT_SCAN_DEPTH: i64 = 4;

/// Largest file `/lore` imports (a book of thousands of entries is a few MB).
const MAX_IMPORT: u64 = 64 * 1024 * 1024;

// ------------------------------------------------------------------ model

/// One entry of a book.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// A title for the list (`comment` in the file); not used in the prompt.
    pub name: String,
    pub enabled: bool,
    pub keys: Vec<String>,
    pub secondary: Vec<String>,
    pub selective: bool,
    /// How the secondary keys count when `selective` (SillyTavern's
    /// `selectiveLogic`): 0 any one must appear, 3 every one must, 2 none may
    /// appear, 1 they must not all appear.
    pub logic: u8,
    pub constant: bool,
    pub case_sensitive: bool,
    pub order: i64,
    /// `BEFORE`, `AFTER` or `DEPTH`.
    pub place: u8,
    pub depth: i64,
    /// 0 system, 1 user, 2 assistant (used with `DEPTH`).
    pub role: u8,
    pub content: String,
    /// The fields of the entry tarpi has no use for, kept as they were.
    raw: Map<String, Value>,
}

/// SillyTavern's number for a place, read the way tarpi does: 0 before, 1
/// after, 4 at a depth; the Author's Note and example-message slots (2, 3, 5,
/// 6) fall to the nearest of before / after. `None` for any other number.
fn place_of_number(n: i64) -> Option<u8> {
    match n {
        4 => Some(DEPTH),
        1 | 3 | 6 => Some(AFTER),
        0 | 2 | 5 => Some(BEFORE),
        _ => None,
    }
}

/// SillyTavern entry fields tarpi does not act on. They are kept in the
/// entry's `extensions` (under SillyTavern's own names) when a World Info
/// file is imported, so that saving the book and `to_st` hand them back
/// instead of losing them.
const ST_KEPT: &[&str] = &[
    "scanDepth",
    "matchWholeWords",
    "characterFilter",
    "triggers",
    "ignoreBudget",
    "excludeRecursion",
    "preventRecursion",
    "delayUntilRecursion",
    "probability",
    "useProbability",
    "sticky",
    "cooldown",
    "delay",
    "group",
    "groupOverride",
    "groupWeight",
    "useGroupScoring",
];

impl Item {
    pub fn new() -> Item {
        Item {
            name: String::new(),
            enabled: true,
            keys: Vec::new(),
            secondary: Vec::new(),
            selective: false,
            logic: 0,
            constant: false,
            case_sensitive: false,
            order: 100,
            place: BEFORE,
            depth: 4,
            role: 0,
            content: String::new(),
            raw: Map::new(),
        }
    }

    /// One entry of a V2 `character_book.entries` (read the way
    /// `lorebook::Entry` reads it, but a switched-off or empty one is kept).
    pub fn from_value(e: &Value) -> Item {
        let null = Value::Null;
        let ext = e.get("extensions").unwrap_or(&null);
        let place = place_of_number(ji64(ext, "position", -1)).unwrap_or_else(|| {
            if jstr(e, "position") == "after_char" {
                AFTER
            } else {
                BEFORE
            }
        });
        let mut name = jstr(e, "comment");
        if name.trim().is_empty() {
            name = jstr(e, "name");
        }
        Item {
            name,
            enabled: jbool(e, "enabled", true),
            keys: strings(e.get("keys")),
            secondary: strings(e.get("secondary_keys")),
            selective: jbool(e, "selective", false),
            logic: ji64(ext, "selectiveLogic", 0).clamp(0, 3) as u8,
            constant: jbool(e, "constant", false),
            case_sensitive: jbool(e, "case_sensitive", false),
            order: ji64(e, "insertion_order", 100),
            place,
            depth: ji64(ext, "depth", 4).max(0),
            role: ji64(ext, "role", 0).clamp(0, 2) as u8,
            content: jstr(e, "content"),
            raw: e.as_object().cloned().unwrap_or_default(),
        }
    }

    pub fn to_value(&self) -> Value {
        let mut o = self.raw.clone();
        o.insert("keys".into(), json!(self.keys));
        o.insert("secondary_keys".into(), json!(self.secondary));
        o.insert("content".into(), json!(self.content));
        o.insert("comment".into(), json!(self.name));
        o.insert("name".into(), json!(self.name));
        o.insert("enabled".into(), json!(self.enabled));
        o.insert("insertion_order".into(), json!(self.order));
        o.insert("selective".into(), json!(self.selective));
        o.insert("constant".into(), json!(self.constant));
        o.insert("case_sensitive".into(), json!(self.case_sensitive));
        o.insert(
            "position".into(),
            json!(if self.place == AFTER { "after_char" } else { "before_char" }),
        );
        let mut ext = o
            .get("extensions")
            .and_then(|x| x.as_object())
            .cloned()
            .unwrap_or_default();
        ext.insert("position".into(), json!(self.place_number()));
        ext.insert("depth".into(), json!(self.depth));
        ext.insert("role".into(), json!(self.role));
        ext.insert("selectiveLogic".into(), json!(self.logic));
        o.insert("extensions".into(), Value::Object(ext));
        Value::Object(o)
    }

    /// SillyTavern's number for the place. An entry that came with one of the
    /// slots tarpi folds onto before / after (Author's Note, example
    /// messages) keeps its number as long as its place is not changed, so
    /// saving it or exporting it does not move it.
    fn place_number(&self) -> i64 {
        let kept = self
            .raw
            .get("extensions")
            .map(|x| ji64(x, "position", -1))
            .unwrap_or(-1);
        if place_of_number(kept) == Some(self.place) {
            return kept;
        }
        match self.place {
            AFTER => 1,
            DEPTH => 4,
            _ => 0,
        }
    }

    /// What the title column shows: the title, else the first key.
    pub fn label(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.trim().to_string()
        } else if let Some(k) = self.keys.first() {
            k.clone()
        } else if self.constant {
            "(always on)".to_string()
        } else {
            "(untitled)".to_string()
        }
    }

    /// Can this entry ever add anything to a prompt?
    fn usable(&self) -> bool {
        self.enabled && !self.content.trim().is_empty()
    }

    fn engine(&self) -> LoreEntry {
        LoreEntry {
            keys: self.keys.clone(),
            secondary: self.secondary.clone(),
            selective: self.selective,
            logic: self.logic,
            constant: self.constant,
            case_sensitive: self.case_sensitive,
            order: self.order,
            place: match self.place {
                AFTER => Place::After,
                DEPTH => Place::Depth {
                    depth: self.depth.max(0) as usize,
                    role: self.role.min(2),
                },
                _ => Place::Before,
            },
            content: self.content.clone(),
        }
    }
}

impl Default for Item {
    fn default() -> Item {
        Item::new()
    }
}

/// A lorebook as `/lore` edits it.
#[derive(Debug, Clone, PartialEq)]
pub struct Book {
    /// The file name without `.json`; empty until the book is first saved.
    pub id: String,
    pub name: String,
    pub description: String,
    /// The master switch: off, the book is never used, whatever its other
    /// settings say (`[x]` / `[ ]` in the list). On by default.
    pub enabled: bool,
    /// On for every chat.
    pub active: bool,
    /// Also on for these characters (card file names without `.png`).
    pub characters: Vec<String>,
    /// The character whose card this book was copied from (card file name
    /// without `.png`); empty for any other book.
    pub source: String,
    /// A fingerprint of the card's book as it was when this copy was made
    /// (empty for older copies and other books), to tell when the card's own
    /// book has changed since.
    pub source_hash: String,
    pub scan_depth: i64,
    pub token_budget: i64,
    pub entries: Vec<Item>,
    /// The fields of the book tarpi has no use for, kept as they were.
    raw: Map<String, Value>,
}

/// `Sofia.png` and `sofia` name the same character.
fn char_key(s: &str) -> String {
    let l = s.trim().to_lowercase();
    l.strip_suffix(".png").map(str::to_string).unwrap_or(l)
}

impl Book {
    pub fn new(name: &str) -> Book {
        Book {
            id: String::new(),
            name: name.trim().to_string(),
            description: String::new(),
            enabled: true,
            active: false,
            characters: Vec::new(),
            source: String::new(),
            source_hash: String::new(),
            scan_depth: DEFAULT_SCAN_DEPTH,
            token_budget: 0,
            entries: Vec::new(),
            raw: Map::new(),
        }
    }

    /// A V2 `character_book` object (with `extensions.tarpi`).
    pub fn from_value(id: &str, v: &Value) -> Book {
        let mut raw = v.as_object().cloned().unwrap_or_default();
        raw.remove("entries");
        let entries: Vec<Item> = v
            .get("entries")
            .and_then(|e| e.as_array())
            .map(|a| a.iter().map(Item::from_value).collect())
            .unwrap_or_default();
        let null = Value::Null;
        let tarpi = v
            .get("extensions")
            .and_then(|e| e.get("tarpi"))
            .unwrap_or(&null);
        let mut name = jstr(v, "name");
        if name.trim().is_empty() {
            name = id.to_string();
        }
        Book {
            id: id.to_string(),
            name,
            description: jstr(v, "description"),
            enabled: jbool(tarpi, "enabled", true),
            active: jbool(tarpi, "active", false),
            characters: strings(tarpi.get("characters")),
            source: jstr(tarpi, "source"),
            source_hash: jstr(tarpi, "source_hash"),
            scan_depth: ji64(v, "scan_depth", DEFAULT_SCAN_DEPTH).max(0),
            token_budget: ji64(v, "token_budget", 0).max(0),
            entries,
            raw,
        }
    }

    pub fn to_value(&self) -> Value {
        let mut o = self.raw.clone();
        o.insert("name".into(), json!(self.name));
        o.insert("description".into(), json!(self.description));
        o.insert("scan_depth".into(), json!(self.scan_depth));
        o.insert("token_budget".into(), json!(self.token_budget));
        let entries: Vec<Value> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let mut v = e.to_value();
                v["id"] = json!(i);
                v
            })
            .collect();
        o.insert("entries".into(), Value::Array(entries));
        let mut ext = o
            .get("extensions")
            .and_then(|x| x.as_object())
            .cloned()
            .unwrap_or_default();
        let mut tarpi = json!({
            "enabled": self.enabled,
            "active": self.active,
            "characters": self.characters,
            "source": self.source,
        });
        if !self.source_hash.is_empty() {
            tarpi["source_hash"] = json!(self.source_hash);
        }
        ext.insert("tarpi".into(), tarpi);
        o.insert("extensions".into(), Value::Object(ext));
        Value::Object(o)
    }

    /// Is the book on for this character (a card file name)?
    pub fn applies_to(&self, avatar: &str) -> bool {
        self.enabled
            && (self.active
                || (!avatar.trim().is_empty()
                && self
                    .characters
                    .iter()
                    .any(|c| char_key(c) == char_key(avatar))))
    }

    /// Is this book the copy of the lorebook in this character's card
    /// (`avatar`; `book_name` is the name that book is given)? A book made
    /// from a card remembers its character (`source`) and only that counts,
    /// so two characters whose books share a name (a plain "Lorebook") are
    /// not mistaken for each other. Only older copies, which have no
    /// `source`, are recognised by that name.
    pub fn is_copy_of(&self, avatar: &str, book_name: &str) -> bool {
        if !self.source.is_empty() {
            return char_key(&self.source) == char_key(avatar);
        }
        let name = book_name.trim().to_lowercase();
        !name.is_empty() && self.name.trim().to_lowercase() == name
    }

    /// Has the lorebook in the card (`card` as `charstore::get` gives it)
    /// changed since this copy was made? `false` for a book that is no copy
    /// or an older one that did not note the fingerprint.
    pub fn card_book_changed(&self, card: &Value) -> bool {
        if self.source.is_empty() || self.source_hash.is_empty() {
            return false;
        }
        match card.get("data").and_then(|d| d.get("character_book")) {
            Some(b) => card_book_hash(b) != self.source_hash,
            None => true,
        }
    }

    /// How many entries are bigger than the whole token budget, so that they
    /// can never be added to a prompt (`cpt`: characters per token).
    pub fn over_budget(&self, cpt: f32) -> usize {
        if self.token_budget <= 0 {
            return 0;
        }
        let budget = self.token_budget as usize;
        self.entries
            .iter()
            .filter(|e| e.usable() && crate::prompt::est_tokens(&e.content, cpt) > budget)
            .count()
    }

    /// Is the character in the book's list?
    pub fn has_character(&self, avatar: &str) -> bool {
        self.characters.iter().any(|c| char_key(c) == char_key(avatar))
    }

    /// Add or take out a character (a card file name) from the list.
    pub fn toggle_character(&mut self, avatar: &str) {
        if self.has_character(avatar) {
            self.characters.retain(|c| char_key(c) != char_key(avatar));
        } else {
            self.characters.push(char_key_name(avatar));
        }
    }

    /// The book as the prompt builder uses it; `None` when no entry can ever
    /// add anything (all off or empty).
    pub fn engine(&self) -> Option<Lorebook> {
        let entries: Vec<LoreEntry> = self
            .entries
            .iter()
            .filter(|i| i.usable())
            .map(Item::engine)
            .collect();
        if entries.is_empty() {
            return None;
        }
        Some(Lorebook {
            entries,
            scan_depth: self.scan_depth.max(0) as usize,
            token_budget: self.token_budget.max(0) as usize,
        })
    }

    /// `3 on of 5`-style count for the list.
    pub fn usable_count(&self) -> usize {
        self.entries.iter().filter(|i| i.usable()).count()
    }

    /// Who the book is on for, in a few words.
    pub fn scope(&self) -> String {
        match (self.active, self.characters.len()) {
            (true, _) => "every chat".to_string(),
            (false, 0) => "nobody yet".to_string(),
            (false, 1) => format!("only {}", self.characters[0]),
            (false, n) => format!("{} characters", n),
        }
    }
}

/// A character's file stem as it is written in a book's list.
fn char_key_name(avatar: &str) -> String {
    let t = avatar.trim();
    t.strip_suffix(".png").unwrap_or(t).to_string()
}

// ----------------------------------------------------------------- folder

fn dir() -> Result<PathBuf> {
    config::lorebooks_dir().ok_or_else(|| anyhow!("no config directory on this system"))
}

/// Longest file name (without `.json`) of a book.
const MAX_ID: usize = 150;

/// Ids become file names. Any name a file in the folder (or one that arrives
/// with a sync) can have is fine, spaces and dots included, as long as it
/// cannot reach outside the folder or hide: no `/`, `\\` or `:`, no control
/// characters, no leading dot.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID
        && !id.starts_with('.')
        && !id.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
}

/// A file-name friendly id made from a book's name.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.trim_matches('-').chars().take(40).collect();
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "lorebook".to_string()
    } else {
        out
    }
}

/// `base` and six random characters (`dragons-3fa9c1`). The ending keeps two
/// devices that each make a book with the same name (see `tarpi connect`)
/// from writing the same file.
fn unique_id(dir: &Path, base: &str) -> String {
    loop {
        let rnd = uuid::Uuid::new_v4().simple().to_string();
        let id = format!("{}-{}", base, &rnd[..6]);
        if !dir.join(format!("{}.json", id)).exists() {
            return id;
        }
    }
}

fn path_of(id: &str) -> Result<PathBuf> {
    if !valid_id(id) {
        bail!("bad lorebook file name '{}'", id);
    }
    Ok(dir()?.join(format!("{}.json", id)))
}

fn parse_book(id: &str, text: &str) -> Result<Book> {
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .with_context(|| format!("{}.json is not valid JSON", id))?;
    if !v.is_object() {
        bail!("{}.json is not a lorebook", id);
    }
    Ok(Book::from_value(id, &v))
}

/// The `*.json` files of the folder, by name.
fn book_files() -> Vec<PathBuf> {
    let Ok(d) = dir() else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&d) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    files.sort();
    files
}

/// The id (file name without `.json`) of a file of the folder, when `/lore`
/// can open it.
fn usable_id(p: &Path) -> Option<String> {
    p.file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| valid_id(s))
        .map(str::to_string)
}

/// Every book, by name, and a line for each file that could not be read.
pub fn list() -> (Vec<Book>, Vec<String>) {
    let mut books = Vec::new();
    let mut errors = Vec::new();
    for p in book_files() {
        let Some(id) = p.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
            continue;
        };
        if !valid_id(&id) {
            errors.push(format!(
                "{}: not a usable file name (rename the file: no leading dot, no / \\ or :)",
                id
            ));
            continue;
        }
        match std::fs::read_to_string(&p)
            .map_err(anyhow::Error::from)
            .and_then(|t| parse_book(&id, &t))
        {
            Ok(b) => books.push(b),
            Err(e) => errors.push(format!("{}: {:#}", id, e)),
        }
    }
    books.sort_by_key(|b| b.name.to_lowercase());
    (books, errors)
}

/// A book as its file holds it now, and a fingerprint of that file (what
/// `save_checked` compares against).
pub fn load(id: &str) -> Result<(Book, String)> {
    let p = path_of(id)?;
    let bytes = std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?;
    let text = String::from_utf8_lossy(&bytes).to_string();
    Ok((parse_book(id, &text)?, crate::mirror::hash_bytes(&bytes)))
}

/// The fingerprint of the book's file as it is now; `None` when it is gone.
pub fn disk_hash(id: &str) -> Option<String> {
    let p = path_of(id).ok()?;
    std::fs::read(p).ok().map(|b| crate::mirror::hash_bytes(&b))
}

/// Like `save`, but only when the file is still the one that was read (`seen`
/// is what `load` or an earlier `save_checked` returned): when another device
/// changed or deleted it meanwhile, nothing is written. Returns the new
/// fingerprint.
pub fn save_checked(book: &mut Book, seen: &str) -> Result<String> {
    if !book.id.is_empty() {
        path_of(&book.id)?; // a bad name is said as that, not as "deleted"
        check_seen(disk_hash(&book.id), seen)?;
    }
    let d = dir()?;
    if book.id.is_empty() {
        book.id = unique_id(&d, &slug(&book.name));
    }
    let p = path_of(&book.id)?;
    let bytes = serde_json::to_vec_pretty(&book.to_value())?;
    crate::mirror::write_atomic(&p, &bytes, None)?;
    Ok(crate::mirror::hash_bytes(&bytes))
}

/// The rule of `save_checked`: `now` is the file's fingerprint as it is now
/// (`None`: the file is gone). A file that is gone is not "the same as before":
/// writing it would bring back a book that was deleted on purpose.
fn check_seen(now: Option<String>, seen: &str) -> Result<()> {
    match now {
        Some(h) if h == seen => Ok(()),
        Some(_) => bail!(
            "not saved: this lorebook was changed somewhere else (another device?) since you \
             opened it. Leave without saving (q, then n) and open it again"
        ),
        None => bail!(
            "not saved: this lorebook was deleted somewhere else (another device?) since you \
             opened it, and nothing was written. Leave without saving (q, then n)"
        ),
    }
}

/// Write the book (a new one gets its file name from its name).
pub fn save(book: &mut Book) -> Result<()> {
    let d = dir()?;
    if book.id.is_empty() {
        book.id = unique_id(&d, &slug(&book.name));
    }
    let p = path_of(&book.id)?;
    let bytes = serde_json::to_vec_pretty(&book.to_value())?;
    crate::mirror::write_atomic(&p, &bytes, None)
}

pub fn delete(id: &str) -> Result<()> {
    let p = path_of(id)?;
    std::fs::remove_file(&p).with_context(|| format!("deleting {}", p.display()))
}

// ----------------------------------------------------------- for a prompt

/// (path, size, mtime in ms, book) of the files read before: a prompt is built
/// often (token counts), a book can be big, and the files rarely change.
static CACHE: Mutex<Vec<(PathBuf, u64, u64, Arc<Book>)>> = Mutex::new(Vec::new());

fn cached(path: &Path) -> Option<Arc<Book>> {
    let meta = std::fs::metadata(path).ok()?;
    let (size, mtime) = (meta.len(), crate::mirror::mtime_ms(&meta));
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = g.iter().find(|c| c.0 == path && c.1 == size && c.2 == mtime) {
        return Some(c.3.clone());
    }
    let id = path.file_stem()?.to_str()?.to_string();
    let text = std::fs::read_to_string(path).ok()?;
    let book = Arc::new(parse_book(&id, &text).ok()?);
    g.retain(|c| c.0 != path);
    g.push((path.to_path_buf(), size, mtime, book.clone()));
    Some(book)
}

/// The standalone lorebooks that are on for this character (a card file
/// name), ready for the prompt builder. A book that cannot be read is skipped.
pub fn active_books(avatar: &str) -> Vec<Lorebook> {
    // Tests build prompts too: they must not depend on the books of whoever
    // runs them.
    if cfg!(test) {
        return Vec::new();
    }
    // Only books `/lore` can show: one that is on but cannot be opened to
    // switch it off would be a trap.
    book_files()
        .iter()
        .filter(|p| usable_id(p).is_some())
        .filter_map(|p| cached(p))
        .filter(|b| b.applies_to(avatar))
        .filter_map(|b| b.engine())
        .collect()
}

/// Is a copy of this character's own card lorebook (a book whose `source` is
/// the character) on for it? Then the card's book is left out of the prompt,
/// so its entries are not sent twice.
pub fn replaces_card_book(avatar: &str) -> bool {
    if cfg!(test) || avatar.trim().is_empty() {
        return false;
    }
    book_files()
        .iter()
        .filter(|p| usable_id(p).is_some())
        .filter_map(|p| cached(p))
        .any(|b| {
            !b.source.is_empty()
                && char_key(&b.source) == char_key(avatar)
                && b.applies_to(avatar)
                && b.engine().is_some()
        })
}

// ----------------------------------------------------------------- import

/// A book read from somewhere else, and what was lost on the way.
pub struct Imported {
    pub book: Book,
    pub notes: Vec<String>,
}

/// `/lore` import: a path (`~`, quotes and `\ ` as in `/import`), or JSON
/// text pasted as it is.
pub fn import_text(arg: &str) -> Result<Imported> {
    let t = arg.trim();
    if t.is_empty() {
        bail!("type the path of a lorebook or card file");
    }
    if t.starts_with('{') {
        return parse_import(t.as_bytes(), "Imported lorebook");
    }
    if t.to_lowercase().starts_with("http://") || t.to_lowercase().starts_with("https://") {
        bail!("links are not imported here: download the file, then give its path");
    }
    let p = crate::import::expand_path(t);
    let meta = std::fs::metadata(&p).with_context(|| format!("{} not found", p.display()))?;
    if !meta.is_file() {
        bail!("{} is not a file", p.display());
    }
    if meta.len() > MAX_IMPORT {
        bail!("{} is too big for a lorebook", p.display());
    }
    let bytes = std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?;
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Imported lorebook")
        .to_string();
    parse_import(&bytes, &stem)
}

/// Read a lorebook out of the bytes of a file: a SillyTavern World Info file,
/// a V2 `character_book`, or a character card (JSON or PNG) with a lorebook.
/// `fallback` names a book that has no name of its own.
pub fn parse_import(bytes: &[u8], fallback: &str) -> Result<Imported> {
    let raw = charstore::raw_card(bytes)?;
    if !raw.is_object() {
        bail!("the JSON is not an object");
    }
    let is_card = jstr(&raw, "spec").starts_with("chara_card_v")
        || (raw.get("entries").is_none()
            && (raw.get("name").is_some()
                || raw.get("char_name").is_some()
                || raw.get("data").is_some()));
    if is_card {
        return from_card(&charstore::normalize(&raw), fallback);
    }
    let mut imp = match raw.get("entries") {
        Some(Value::Object(m)) => from_st(&raw, m),
        Some(Value::Array(_)) => Imported {
            book: Book::from_value("", &raw),
            notes: Vec::new(),
        },
        _ => bail!("not a lorebook: there is no `entries` list in it"),
    };
    imp.book.id = String::new();
    // An imported book starts off, whatever the file it came from said.
    imp.book.enabled = true;
    imp.book.active = false;
    imp.book.characters.clear();
    imp.book.source.clear();
    imp.book.source_hash.clear();
    if jstr(&raw, "name").trim().is_empty() {
        imp.book.name = fallback.trim().to_string();
    }
    if imp.book.entries.is_empty() {
        bail!("this lorebook has no entries");
    }
    Ok(imp)
}

/// The lorebook of a card as `charstore::get` / `normalize` gives it.
pub fn from_card(card: &Value, fallback: &str) -> Result<Imported> {
    let book = card
        .get("data")
        .and_then(|d| d.get("character_book"))
        .filter(|b| {
            b.get("entries")
                .and_then(|e| e.as_array())
                .is_some_and(|a| !a.is_empty())
        })
        .ok_or_else(|| anyhow!("this character card has no lorebook"))?;
    let mut b = Book::from_value("", book);
    b.id = String::new();
    if jstr(book, "name").trim().is_empty() {
        let who = jstr(card, "name");
        let who = if who.trim().is_empty() { fallback.trim().to_string() } else { who };
        b.name = format!("{} lorebook", who.trim());
    }
    // A card's book is not one of tarpi's: it starts off.
    b.enabled = true;
    b.active = false;
    b.characters.clear();
    b.source = char_key_name(fallback);
    b.source_hash = card_book_hash(book);
    Ok(Imported { book: b, notes: Vec::new() })
}

/// A fingerprint of the `character_book` of a card, noted in the copy that
/// `from_card` makes (see `Book::card_book_changed`).
fn card_book_hash(book: &Value) -> String {
    crate::mirror::hash_bytes(&serde_json::to_vec(book).unwrap_or_default())
}

/// The names of the copies of this character's card lorebook (a card file
/// name; on or off) that no longer match what the card carries now: the card
/// was imported again, or its book was edited. They stay as they are, in
/// use, until the person replaces them.
pub fn outdated_copies(avatar: &str) -> Vec<String> {
    let Ok(card) = charstore::get(avatar) else {
        return Vec::new();
    };
    list()
        .0
        .into_iter()
        .filter(|b| !b.source.is_empty() && char_key(&b.source) == char_key(avatar))
        .filter(|b| b.card_book_changed(&card))
        .map(|b| b.name)
        .collect()
}

/// A key written as `/regex/` (SillyTavern); tarpi reads keys as plain text.
fn is_regex_key(k: &str) -> bool {
    k.len() > 2 && k.starts_with('/') && k[1..].contains('/')
}

/// A SillyTavern World Info file: `entries` is an object of numbered entries.
fn from_st(raw: &Value, entries: &Map<String, Value>) -> Imported {
    let mut list: Vec<&Value> = entries.values().filter(|e| e.is_object()).collect();
    list.sort_by_key(|e| (ji64(e, "displayIndex", ji64(e, "uid", 0)), ji64(e, "uid", 0)));
    let (mut regex, mut prob, mut slots, mut timed) = (0, 0, 0, 0);
    // Not acted on: per-entry scan depth, whole-word matching, a character
    // filter (the entry is then on for every character), triggers, ignoring
    // the budget, and the recursion flags.
    let (mut scan, mut whole, mut filtered, mut trig, mut nobudget, mut recur) = (0, 0, 0, 0, 0, 0);
    let mut items = Vec::new();
    for e in list {
        let keys = strings(e.get("key"));
        let secondary = strings(e.get("keysecondary"));
        let selective = jbool(e, "selective", false);
        if keys.iter().chain(secondary.iter()).any(|k| is_regex_key(k)) {
            regex += 1;
        }
        if jbool(e, "useProbability", false) && ji64(e, "probability", 100) < 100 {
            prob += 1;
        }
        let position = ji64(e, "position", 0);
        if matches!(position, 2 | 3 | 5 | 6) {
            slots += 1;
        }
        if ji64(e, "sticky", 0) > 0
            || ji64(e, "cooldown", 0) > 0
            || ji64(e, "delay", 0) > 0
            || !jstr(e, "group").trim().is_empty()
        {
            timed += 1;
        }
        if e.get("scanDepth").is_some_and(|x| x.is_number()) {
            scan += 1;
        }
        if jbool(e, "matchWholeWords", false) {
            whole += 1;
        }
        if e.get("characterFilter").is_some_and(|f| {
            ["names", "tags"]
                .iter()
                .any(|k| f.get(*k).and_then(|x| x.as_array()).is_some_and(|a| !a.is_empty()))
        }) {
            filtered += 1;
        }
        if e.get("triggers").and_then(|t| t.as_array()).is_some_and(|a| !a.is_empty()) {
            trig += 1;
        }
        if jbool(e, "ignoreBudget", false) {
            nobudget += 1;
        }
        if jbool(e, "excludeRecursion", false)
            || jbool(e, "preventRecursion", false)
            || jbool(e, "delayUntilRecursion", false)
        {
            recur += 1;
        }
        let comment = jstr(e, "comment");
        let mut v = json!({
            "keys": keys,
            "secondary_keys": secondary,
            "content": jstr(e, "content"),
            "comment": comment,
            "name": comment,
            "enabled": !jbool(e, "disable", false),
            "insertion_order": ji64(e, "order", 100),
            "selective": selective,
            "constant": jbool(e, "constant", false),
            "case_sensitive": jbool(e, "caseSensitive", false),
            "extensions": {
                "position": position,
                "depth": ji64(e, "depth", 4),
                "role": ji64(e, "role", 0),
                "selectiveLogic": ji64(e, "selectiveLogic", 0),
            },
        });
        // What tarpi does not act on stays in the entry, in SillyTavern's own
        // names, for the book file and `to_st` to hand back.
        for k in ST_KEPT {
            if let Some(x) = e.get(*k) {
                v["extensions"][*k] = x.clone();
            }
        }
        items.push(Item::from_value(&v));
    }
    let mut book = Book::new(&jstr(raw, "name"));
    book.description = jstr(raw, "description");
    book.entries = items;
    let mut notes = Vec::new();
    if regex > 0 {
        notes.push(format!("{} with /regex/ keys (kept as plain text)", regex));
    }
    if prob > 0 {
        notes.push(format!("{} with a probability (always on when a key matches)", prob));
    }
    if slots > 0 {
        notes.push(format!(
            "{} for the Author's Note / example slots (moved before / after the character)",
            slots
        ));
    }
    if timed > 0 {
        notes.push(format!("{} with sticky / cooldown / delay / groups (ignored)", timed));
    }
    if scan > 0 {
        notes.push(format!("{} with a scan depth of their own (the book's is used)", scan));
    }
    if whole > 0 {
        notes.push(format!("{} with match whole words (keys match inside words too)", whole));
    }
    if filtered > 0 {
        notes.push(format!(
            "{} limited to certain characters (now on for every character that uses the book)",
            filtered
        ));
    }
    if trig > 0 {
        notes.push(format!("{} with triggers (ignored)", trig));
    }
    if nobudget > 0 {
        notes.push(format!("{} that ignore the token budget (counted like the others)", nobudget));
    }
    if recur > 0 {
        notes.push(format!("{} with recursion flags (there is no recursive scanning)", recur));
    }
    Imported { book, notes }
}

/// Save an imported book as a new file; the book is returned with its id.
pub fn add(imp: Imported) -> Result<Book> {
    let mut b = imp.book;
    b.id = String::new();
    save(&mut b)?;
    Ok(b)
}

// ----------------------------------------------------------------- export

/// The book as a SillyTavern World Info file.
pub fn to_st(book: &Book) -> Value {
    let mut entries = Map::new();
    for (i, e) in book.entries.iter().enumerate() {
        let mut entry = json!({
            "uid": i,
            "key": e.keys,
            "keysecondary": e.secondary,
            "comment": e.name,
            "content": e.content,
            "constant": e.constant,
            "vectorized": false,
            "selective": e.selective,
            "selectiveLogic": e.logic,
            "addMemo": true,
            "order": e.order,
            "position": e.place_number(),
            "disable": !e.enabled,
            "excludeRecursion": false,
            "preventRecursion": false,
            "delayUntilRecursion": false,
            "probability": 100,
            "useProbability": true,
            "depth": e.depth,
            "group": "",
            "groupOverride": false,
            "groupWeight": 100,
            "scanDepth": null,
            "caseSensitive": e.case_sensitive,
            "matchWholeWords": null,
            "role": e.role,
            "displayIndex": i,
        });
        // The fields tarpi keeps but does not act on (see `ST_KEPT`) go back
        // as they were read.
        if let Some(ext) = e.raw.get("extensions").and_then(|x| x.as_object()) {
            for k in ST_KEPT {
                if let Some(x) = ext.get(*k) {
                    entry[*k] = x.clone();
                }
            }
        }
        entries.insert(i.to_string(), entry);
    }
    json!({
        "name": book.name,
        "description": book.description,
        "entries": Value::Object(entries),
    })
}

/// Write the book to `path_text` (a file, or a folder: then `<name>.json`,
/// made from the book's name, goes there) as SillyTavern World Info (`st`) or
/// as tarpi's own V2 file. An existing file is never overwritten, not even
/// by one that appears while this runs. The path written is returned.
pub fn export_to(book: &Book, path_text: &str, st: bool) -> Result<PathBuf> {
    let t = path_text.trim();
    if t.is_empty() {
        bail!("type where to write the file");
    }
    let mut p = crate::import::expand_path(t);
    if p.is_dir() {
        p = p.join(format!("{}.json", slug(&book.name)));
    }
    let value = if st { to_st(book) } else { book.to_value() };
    let bytes = serde_json::to_vec_pretty(&value)?;
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            bail!("the folder {} does not exist", parent.display());
        }
    }
    // `create_new` makes the file only when there is none: one check and one
    // write in a single step, so nothing can slip in between them.
    let mut f = match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            bail!("{} already exists: pick another name", p.display())
        }
        Err(e) => return Err(e).with_context(|| format!("writing {}", p.display())),
    };
    if let Err(e) = f.write_all(&bytes) {
        drop(f);
        let _ = std::fs::remove_file(&p); // no half-written file left behind
        return Err(e).with_context(|| format!("writing {}", p.display()));
    }
    Ok(p)
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    fn st_file() -> Value {
        json!({
            "name": "World",
            "entries": {
                "1": { "uid": 1, "key": ["/dr.g/i", "wyrm"], "keysecondary": [], "comment": "Dragons",
                       "content": "Dragons hoard.", "constant": false, "selective": false,
                       "order": 50, "position": 4, "depth": 2, "role": 1, "disable": false,
                       "caseSensitive": true, "probability": 50, "useProbability": true },
                "0": { "uid": 0, "key": ["king"], "keysecondary": ["crown"], "comment": "King",
                       "content": "The king is old.", "selective": true, "selectiveLogic": 1,
                       "order": 100, "position": 1, "disable": true },
            }
        })
    }

    #[test]
    fn a_sillytavern_file_is_read_in_uid_order_with_notes() {
        let bytes = serde_json::to_vec(&st_file()).unwrap();
        let imp = parse_import(&bytes, "file").unwrap();
        assert_eq!(imp.book.name, "World");
        assert_eq!(imp.book.entries.len(), 2);
        let (king, dragon) = (&imp.book.entries[0], &imp.book.entries[1]);
        assert_eq!(king.name, "King");
        assert!(!king.enabled);
        assert!(king.selective);
        assert_eq!(king.place, AFTER);
        assert_eq!(dragon.place, DEPTH);
        assert_eq!((dragon.depth, dragon.role, dragon.order), (2, 1, 50));
        assert!(dragon.case_sensitive);
        assert_eq!(dragon.keys, vec!["/dr.g/i", "wyrm"]);
        // regex key, probability (selective logic is kept)
        assert_eq!(imp.notes.len(), 2);
        assert_eq!(king.logic, 1);
        // not on until the person says so
        assert!(!imp.book.active);
    }

    #[test]
    fn a_book_without_a_name_gets_the_file_name() {
        let v = json!({ "entries": [{ "keys": ["a"], "content": "x" }] });
        let imp = parse_import(&serde_json::to_vec(&v).unwrap(), "my file").unwrap();
        assert_eq!(imp.book.name, "my file");
    }

    #[test]
    fn a_card_gives_its_lorebook() {
        let card = json!({ "spec": "chara_card_v2", "spec_version": "2.0", "name": "Sofia",
            "data": { "name": "Sofia", "description": "d", "character_book": {
                "entries": [{ "keys": ["a"], "content": "x" }] } } });
        let imp = parse_import(&serde_json::to_vec(&card).unwrap(), "f").unwrap();
        assert_eq!(imp.book.name, "Sofia lorebook");
        assert_eq!(imp.book.entries.len(), 1);
        let bare = json!({ "spec": "chara_card_v2", "spec_version": "2.0", "name": "Sofia",
            "data": { "name": "Sofia", "description": "d" } });
        assert!(parse_import(&serde_json::to_vec(&bare).unwrap(), "f").is_err());
    }

    #[test]
    fn other_json_is_refused() {
        assert!(parse_import(b"[1, 2]", "f").is_err());
        assert!(parse_import(b"{\"foo\": 1}", "f").is_err());
        assert!(parse_import(b"not json", "f").is_err());
    }

    #[test]
    fn a_book_survives_saving_and_reading() {
        let mut b = Book::new("Test");
        b.active = true;
        b.characters = vec!["Sofia".into()];
        b.token_budget = 300;
        let mut e = Item::new();
        e.name = "Dragons".into();
        e.keys = vec!["dragon".into(), "wyrm".into()];
        e.content = "Hoards.".into();
        e.place = DEPTH;
        e.depth = 3;
        e.role = 2;
        e.order = 7;
        b.entries.push(e);
        let again = Book::from_value("", &b.to_value());
        assert_eq!(again.to_value(), b.to_value());
        assert_eq!(again.entries.len(), 1);
        assert_eq!(again.entries[0].keys, vec!["dragon", "wyrm"]);
        assert_eq!(
            (again.entries[0].place, again.entries[0].depth, again.entries[0].role),
            (DEPTH, 3, 2)
        );
        assert!(again.active);
        assert_eq!(again.characters, vec!["Sofia"]);
        assert_eq!(again.token_budget, 300);
    }

    #[test]
    fn a_disabled_book_applies_to_nobody() {
        let mut b = Book::new("Test");
        b.active = true;
        b.characters = vec!["Sofia".into()];
        assert!(b.applies_to("Sofia.png") && b.applies_to("Other.png"));
        b.enabled = false;
        assert!(!b.applies_to("Sofia.png") && !b.applies_to("Other.png"));
        let again = Book::from_value("", &b.to_value());
        assert!(!again.enabled);
        // A file from before the switch existed is on.
        let old = json!({ "name": "Old", "entries": [], "extensions": { "tarpi": { "active": true } } });
        assert!(Book::from_value("o", &old).enabled);
    }

    #[test]
    fn a_book_copied_from_a_card_remembers_the_character() {
        let card = json!({ "name": "Sofia", "data": { "character_book": { "entries": [
            { "keys": ["a"], "content": "x", "enabled": true }
        ] } } });
        let imp = from_card(&card, "Sofia.png").unwrap();
        assert_eq!(imp.book.source, "Sofia");
        assert_eq!(Book::from_value("", &imp.book.to_value()).source, "Sofia");
    }

    #[test]
    fn unknown_fields_are_kept() {
        let v = json!({ "name": "B", "recursive_scanning": true, "entries": [
            { "keys": ["a"], "content": "x", "id": 4, "extensions": { "probability": 80, "position": 0 } }] });
        let b = Book::from_value("b", &v);
        let out = b.to_value();
        assert_eq!(out["recursive_scanning"], json!(true));
        assert_eq!(out["entries"][0]["extensions"]["probability"], json!(80));
    }

    #[test]
    fn the_engine_reads_a_saved_book_like_a_card_book() {
        let mut b = Book::new("Test");
        let mut on = Item::new();
        on.keys = vec!["dragon".into()];
        on.content = "Hoards.".into();
        on.place = DEPTH;
        on.depth = 2;
        on.role = 1;
        let mut off = Item::new();
        off.keys = vec!["x".into()];
        off.content = "never".into();
        off.enabled = false;
        b.entries = vec![on, off, Item::new()];
        let mine = b.engine().unwrap();
        let card = Lorebook::from_card_data(&json!({ "character_book": b.to_value() })).unwrap();
        assert_eq!(mine.entries.len(), 1);
        assert_eq!(card.entries.len(), 1);
        assert_eq!(mine.entries[0].place, card.entries[0].place);
        assert_eq!(mine.entries[0].keys, card.entries[0].keys);
        assert_eq!(mine.scan_depth, card.scan_depth);
        assert!(Book::new("empty").engine().is_none());
    }

    #[test]
    fn a_book_is_on_for_everyone_or_for_listed_characters() {
        let mut b = Book::new("B");
        assert!(!b.applies_to("Sofia.png"));
        b.toggle_character("Sofia.png");
        assert_eq!(b.characters, vec!["Sofia"]);
        assert!(b.applies_to("sofia.png"));
        assert!(!b.applies_to("Other.png"));
        assert!(!b.applies_to(""));
        b.toggle_character("SOFIA.png");
        assert!(b.characters.is_empty());
        b.active = true;
        assert!(b.applies_to("Other.png"));
        assert!(b.applies_to(""));
    }

    #[test]
    fn the_sillytavern_export_reads_back() {
        let imp = parse_import(&serde_json::to_vec(&st_file()).unwrap(), "f").unwrap();
        let out = to_st(&imp.book);
        let again = parse_import(&serde_json::to_vec(&out).unwrap(), "f").unwrap();
        assert_eq!(again.book.entries.len(), imp.book.entries.len());
        for (a, b) in again.book.entries.iter().zip(imp.book.entries.iter()) {
            assert_eq!(a.keys, b.keys);
            assert_eq!(a.content, b.content);
            assert_eq!(a.place, b.place);
            assert_eq!(a.enabled, b.enabled);
            assert_eq!((a.depth, a.role, a.order), (b.depth, b.role, b.order));
        }
    }

    #[test]
    fn ids_are_file_name_safe() {
        assert_eq!(slug("The Realm of  Aldor!"), "the-realm-of-aldor");
        assert_eq!(slug("???"), "lorebook");
    }

    #[test]
    fn file_names_with_spaces_and_dots_are_usable_but_not_ones_that_escape() {
        assert!(valid_id("My Book"));
        assert!(valid_id("world.v2"));
        assert!(valid_id("Café – notes"));
        assert!(!valid_id(".hidden"));
        assert!(!valid_id(".."));
        assert!(!valid_id("a/b"));
        assert!(!valid_id("a\\b"));
        assert!(!valid_id("a:b"));
        assert!(!valid_id("a\u{7}b"));
        assert!(!valid_id(&"x".repeat(MAX_ID + 1)));
        assert!(valid_id(&"x".repeat(MAX_ID)));
        // The id of a file is its name without `.json`, dots and all.
        assert_eq!(usable_id(Path::new("/d/world.v2.json")).as_deref(), Some("world.v2"));
        assert_eq!(usable_id(Path::new("/d/.x.json")), None);
    }

    #[test]
    fn a_book_that_is_gone_is_not_saved_back() {
        assert!(check_seen(Some("abc".into()), "abc").is_ok());
        let changed = check_seen(Some("abd".into()), "abc").unwrap_err().to_string();
        assert!(changed.contains("changed"));
        // Deleted elsewhere: the old fingerprint can be anything, even empty
        // (the list's copy has none), and still nothing is written.
        for seen in ["abc", ""] {
            let gone = check_seen(None, seen).unwrap_err().to_string();
            assert!(gone.contains("deleted"));
        }
    }

    #[test]
    fn copies_are_told_apart_by_their_character_not_by_the_name() {
        let card = |who: &str| {
            json!({ "name": who, "data": { "character_book": {
                "name": "Lorebook", "entries": [{ "keys": ["a"], "content": "x" }] } } })
        };
        let a = from_card(&card("Anna"), "Anna.png").unwrap().book;
        let b = from_card(&card("Ben"), "Ben.png").unwrap().book;
        // Same name, different characters.
        assert_eq!(a.name, b.name);
        assert!(a.is_copy_of("Anna.png", "Lorebook"));
        assert!(a.is_copy_of("anna", "Lorebook"));
        assert!(!a.is_copy_of("Ben.png", "Lorebook"));
        assert!(b.is_copy_of("Ben.png", "Lorebook"));
        // An older copy has no source: the name is all there is to go by.
        let mut old = Book::new("Lorebook");
        assert!(old.is_copy_of("Ben.png", "lorebook"));
        assert!(!old.is_copy_of("Ben.png", "Other"));
        assert!(!old.is_copy_of("Ben.png", " "));
        old.source = "Anna".into();
        assert!(!old.is_copy_of("Ben.png", "Lorebook"));
    }

    #[test]
    fn a_copy_notices_when_the_card_book_changed() {
        let card = |content: &str| {
            json!({ "name": "Anna", "data": { "character_book": {
                "entries": [{ "keys": ["a"], "content": content }] } } })
        };
        let copy = from_card(&card("one"), "Anna.png").unwrap().book;
        assert!(!copy.source_hash.is_empty());
        assert!(!copy.card_book_changed(&card("one")));
        assert!(copy.card_book_changed(&card("two")));
        // The card lost its book.
        assert!(copy.card_book_changed(&json!({ "name": "Anna", "data": {} })));
        // The note survives saving and reading.
        let again = Book::from_value("x", &copy.to_value());
        assert_eq!(again.source_hash, copy.source_hash);
        // A book that is no copy, or an older copy, never reports a change.
        assert!(!Book::new("B").card_book_changed(&card("two")));
        let mut old = copy.clone();
        old.source_hash.clear();
        assert!(!old.card_book_changed(&card("two")));
        // An imported file starts clean, whatever it carried.
        let mut v = copy.to_value();
        v["entries"] = json!([{ "keys": ["a"], "content": "x" }]);
        let imp = parse_import(&serde_json::to_vec(&v).unwrap(), "f").unwrap();
        assert!(imp.book.source.is_empty() && imp.book.source_hash.is_empty());
    }

    #[test]
    fn an_entrys_own_slot_survives_saving_and_export() {
        let st = json!({ "name": "W", "entries": {
            "0": { "uid": 0, "key": ["a"], "content": "c0", "position": 2 },
            "1": { "uid": 1, "key": ["b"], "content": "c1", "position": 3 },
            "2": { "uid": 2, "key": ["c"], "content": "c2", "position": 5 },
            "3": { "uid": 3, "key": ["d"], "content": "c3", "position": 6 },
        } });
        let imp = parse_import(&serde_json::to_vec(&st).unwrap(), "f").unwrap();
        let places: Vec<u8> = imp.book.entries.iter().map(|e| e.place).collect();
        assert_eq!(places, vec![BEFORE, AFTER, BEFORE, AFTER]);
        // Saved and read again, then exported: the numbers are the originals.
        let again = Book::from_value("x", &imp.book.to_value());
        let out = to_st(&again);
        let pos = |i: &str| out["entries"][i]["position"].as_i64().unwrap();
        assert_eq!((pos("0"), pos("1"), pos("2"), pos("3")), (2, 3, 5, 6));
        let saved = again.to_value();
        assert_eq!(saved["entries"][1]["extensions"]["position"], json!(3));
        // Moving the entry to another place gives it that place's own number.
        let mut moved = again.clone();
        moved.entries[1].place = BEFORE;
        assert_eq!(to_st(&moved)["entries"]["1"]["position"], json!(0));
        moved.entries[1].place = DEPTH;
        assert_eq!(to_st(&moved)["entries"]["1"]["position"], json!(4));
        // ... and back where it was: its own slot returns.
        moved.entries[1].place = AFTER;
        assert_eq!(to_st(&moved)["entries"]["1"]["position"], json!(3));
    }

    #[test]
    fn what_tarpi_does_not_act_on_is_reported_and_kept() {
        let st = json!({ "name": "W", "entries": {
            "0": { "uid": 0, "key": ["a"], "content": "x", "scanDepth": 7,
                   "matchWholeWords": true, "ignoreBudget": true,
                   "excludeRecursion": true, "triggers": ["continue"],
                   "characterFilter": { "isExclude": false, "names": ["Anna"], "tags": [] } },
            "1": { "uid": 1, "key": ["b"], "content": "y", "scanDepth": null,
                   "matchWholeWords": null, "triggers": [],
                   "characterFilter": { "isExclude": false, "names": [], "tags": [] } },
        } });
        let imp = parse_import(&serde_json::to_vec(&st).unwrap(), "f").unwrap();
        let has = |w: &str| imp.notes.iter().any(|n| n.contains(w) && n.starts_with("1 "));
        assert!(has("scan depth"));
        assert!(has("whole words"));
        assert!(has("certain characters"));
        assert!(has("triggers"));
        assert!(has("token budget"));
        assert!(has("recursion"));
        // The second entry has none of it: nothing to report for it.
        assert_eq!(imp.notes.len(), 6);
        // Kept in the entry and handed back on export.
        let out = to_st(&Book::from_value("x", &imp.book.to_value()));
        let e0 = &out["entries"]["0"];
        assert_eq!(e0["scanDepth"], json!(7));
        assert_eq!(e0["matchWholeWords"], json!(true));
        assert_eq!(e0["characterFilter"]["names"], json!(["Anna"]));
        assert_eq!(e0["triggers"], json!(["continue"]));
        // An entry without them is exported with the plain defaults.
        assert_eq!(out["entries"]["1"]["probability"], json!(100));
    }

    #[test]
    fn entries_bigger_than_the_budget_are_counted() {
        let mut b = Book::new("B");
        let mut big = Item::new();
        big.content = "x".repeat(64); // ~20 tokens
        let mut small = Item::new();
        small.content = "y".repeat(8);
        let mut off = Item::new();
        off.content = "z".repeat(64);
        off.enabled = false;
        b.entries = vec![big, small, off];
        assert_eq!(b.over_budget(3.2), 0); // no budget: no limit
        b.token_budget = 10;
        assert_eq!(b.over_budget(3.2), 1); // switched-off entries do not count
        b.token_budget = 100;
        assert_eq!(b.over_budget(3.2), 0);
    }

    #[test]
    fn an_export_never_overwrites_a_file() {
        let d = std::env::temp_dir().join(format!("tarpi-export-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&d).unwrap();
        let mut b = Book::new("My World");
        b.entries.push(Item::new());
        // A folder: the file is named after the book.
        let first = export_to(&b, d.to_str().unwrap(), false).unwrap();
        assert_eq!(first, d.join("my-world.json"));
        let before = std::fs::read(&first).unwrap();
        // The same again is refused and the file is as it was.
        b.name = "My World".into();
        b.description = "changed".into();
        let again = export_to(&b, d.to_str().unwrap(), false).unwrap_err().to_string();
        assert!(again.contains("already exists"));
        let explicit = export_to(&b, first.to_str().unwrap(), true).unwrap_err().to_string();
        assert!(explicit.contains("already exists"));
        assert_eq!(std::fs::read(&first).unwrap(), before);
        // A new name in the same folder is fine.
        let other = d.join("other.json");
        assert_eq!(export_to(&b, other.to_str().unwrap(), true).unwrap(), other);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn new_ids_never_repeat_across_devices() {
        let d = std::env::temp_dir();
        let (a, b) = (unique_id(&d, "dragons"), unique_id(&d, "dragons"));
        assert!(a.starts_with("dragons-") && valid_id(&a));
        assert_eq!(a.len(), "dragons-".len() + 6);
        assert_ne!(a, b);
        assert!(valid_id("my-book_2"));
        assert!(!valid_id("../x"));
        assert!(!valid_id(""));
    }
}
