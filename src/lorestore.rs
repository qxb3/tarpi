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
        let mut place = match ji64(ext, "position", -1) {
            4 => DEPTH,
            1 | 3 | 6 => AFTER,
            0 | 2 | 5 => BEFORE,
            _ => {
                if jstr(e, "position") == "after_char" {
                    AFTER
                } else {
                    BEFORE
                }
            }
        };
        if place > DEPTH {
            place = BEFORE;
        }
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

    /// SillyTavern's number for the place.
    fn place_number(&self) -> i64 {
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
        ext.insert(
            "tarpi".into(),
            json!({
                "enabled": self.enabled,
                "active": self.active,
                "characters": self.characters,
                "source": self.source,
            }),
        );
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

/// Ids become file names, so only plain characters are allowed in them.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
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

/// Every book, by name, and a line for each file that could not be read.
pub fn list() -> (Vec<Book>, Vec<String>) {
    let mut books = Vec::new();
    let mut errors = Vec::new();
    let Ok(d) = dir() else {
        return (books, errors);
    };
    let Ok(rd) = std::fs::read_dir(&d) else {
        return (books, errors);
    };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    files.sort();
    for p in files {
        let Some(id) = p.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
            continue;
        };
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
    if !book.id.is_empty() && disk_hash(&book.id).unwrap_or_default() != seen {
        bail!(
            "not saved: this lorebook was changed or deleted somewhere else (another device?) \
             since you opened it. Leave without saving (q, then n) and open it again"
        );
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
        .iter()
        .filter_map(|p| cached(p))
        .filter(|b| b.applies_to(avatar))
        .filter_map(|b| b.engine())
        .collect()
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
    Ok(Imported { book: b, notes: Vec::new() })
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
        let comment = jstr(e, "comment");
        let v = json!({
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
        entries.insert(
            i.to_string(),
            json!({
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
            }),
        );
    }
    json!({
        "name": book.name,
        "description": book.description,
        "entries": Value::Object(entries),
    })
}

/// Write the book to `path_text` (a file, or a folder: then `<id>.json` goes
/// there) as SillyTavern World Info (`st`) or as tarpi's own V2 file. An
/// existing file is never overwritten. The path written is returned.
pub fn export_to(book: &Book, path_text: &str, st: bool) -> Result<PathBuf> {
    let t = path_text.trim();
    if t.is_empty() {
        bail!("type where to write the file");
    }
    let mut p = crate::import::expand_path(t);
    if p.is_dir() {
        p = p.join(format!("{}.json", slug(&book.name)));
    }
    if p.exists() {
        bail!("{} already exists: pick another name", p.display());
    }
    let value = if st { to_st(book) } else { book.to_value() };
    let bytes = serde_json::to_vec_pretty(&value)?;
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            bail!("the folder {} does not exist", parent.display());
        }
    }
    std::fs::write(&p, bytes).with_context(|| format!("writing {}", p.display()))?;
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
