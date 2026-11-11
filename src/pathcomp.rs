//! Path completion for `/import`: what the menu above the input box lists
//! once Tab has opened it after `/import ` (see `App::path_completion`
//! and `ui::draw_path_menu`).
//!
//! The folder the typed text names is read and the entries that start with
//! what has been typed after the last `/` are offered: folders (always) and
//! card files (`.png` / `.json`). Hidden entries only show once the name being
//! typed starts with a `.`. A path is read the way `import::expand_path` reads
//! it: `~` is the home folder, a relative path starts from the folder tarpi
//! was started in, and spaces may be written `\ ` or the whole path put in
//! quotes. The text an entry fills in follows the style that was typed
//! (escaped spaces, or the open quote), so what ends up in the box is what
//! `/import` understands.
//!
//! Nothing here touches the app: it is plain text and file-system work.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The menu never lists more than this many entries of one folder.
const MAX_ENTRIES: usize = 500;
/// How long a folder listing is reused (the menu asks for it every frame).
const CACHE_FOR: Duration = Duration::from_secs(1);

/// One entry of the menu.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The file or folder name, as it is on disk.
    pub name: String,
    pub is_dir: bool,
    /// Size in bytes (0 for a folder).
    pub size: u64,
    /// Everything after `/import ` once this entry is filled in: a folder
    /// ends with `/`; a file closes the quote if one was opened.
    pub insert: String,
}

/// The path being typed, if `text` is `/import` followed by one: a single
/// line that is not JSON text (`{`) and not a link. It may be empty (just
/// `/import ` lists the current folder).
pub fn typed_arg(text: &str) -> Option<&str> {
    if text.contains(['\n', '\r']) {
        return None;
    }
    let head = text.get(..7)?;
    if !head.eq_ignore_ascii_case("/import") {
        return None;
    }
    let rest = &text[7..];
    if !rest.chars().next()?.is_whitespace() {
        return None;
    }
    let arg = rest.trim_start();
    let lower = arg.to_ascii_lowercase();
    if lower.starts_with('{')
        || lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("file://")
    {
        return None;
    }
    Some(arg)
}

/// The typed path, split.
struct Parsed {
    /// The quote the path was opened with, if any.
    quote: Option<char>,
    /// Everything up to and including the last `/`, as typed.
    dir_text: String,
    /// What has been typed of the name (unescaped).
    prefix: String,
}

fn unescape(s: &str) -> String {
    s.replace("\\ ", " ")
}

fn parse(arg: &str) -> Option<Parsed> {
    let (quote, body) = match arg.chars().next() {
        Some(q @ ('"' | '\'')) => (Some(q), &arg[1..]),
        _ => (None, arg),
    };
    // A path whose quote is already closed is finished.
    if let Some(q) = quote {
        if body.contains(q) {
            return None;
        }
    }
    let (dir_text, name_part): (String, String) = if quote.is_none() && body == "~" {
        ("~/".to_string(), String::new())
    } else {
        match body.rfind('/') {
            Some(i) => (body[..=i].to_string(), body[i + 1..].to_string()),
            None => (String::new(), body.to_string()),
        }
    };
    let prefix = if quote.is_some() {
        name_part
    } else {
        unescape(&name_part)
    };
    Some(Parsed {
        quote,
        dir_text,
        prefix,
    })
}

/// The folder the typed path names.
fn dir_path(p: &Parsed) -> PathBuf {
    let raw = if p.quote.is_some() {
        p.dir_text.clone()
    } else {
        unescape(&p.dir_text)
    };
    if raw.is_empty() {
        return PathBuf::from(".");
    }
    if raw.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(raw[2..].trim_start_matches('/'));
        }
    }
    PathBuf::from(raw)
}

fn is_card(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".png") || lower.ends_with(".json")
}

fn insert_text(p: &Parsed, name: &str, is_dir: bool) -> String {
    let mut s = String::new();
    if let Some(q) = p.quote {
        s.push(q);
        s.push_str(&p.dir_text);
        s.push_str(name);
        if is_dir {
            s.push('/');
        } else {
            s.push(q);
        }
    } else {
        s.push_str(&p.dir_text);
        s.push_str(&name.replace(' ', "\\ "));
        if is_dir {
            s.push('/');
        }
    }
    s
}

/// The entries the menu offers for the typed path `arg` (see `typed_arg`):
/// folders first, then card files, each sorted by name ignoring case. Empty
/// when the folder cannot be read or nothing matches.
pub fn entries(arg: &str) -> Vec<Entry> {
    let Some(p) = parse(arg) else {
        return Vec::new();
    };
    let Ok(read) = std::fs::read_dir(dir_path(&p)) else {
        return Vec::new();
    };
    let want = p.prefix.to_lowercase();
    let show_hidden = p.prefix.starts_with('.');
    let mut out: Vec<Entry> = Vec::new();
    for e in read.flatten() {
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        if (!show_hidden && name.starts_with('.')) || !name.to_lowercase().starts_with(&want) {
            continue;
        }
        // A name with the quote in it could not be written inside that quote.
        if p.quote.is_some_and(|q| name.contains(q)) {
            continue;
        }
        // `metadata` follows links, so a link to a folder is a folder.
        let Ok(meta) = std::fs::metadata(e.path()) else {
            continue;
        };
        let is_dir = meta.is_dir();
        if !is_dir && !(meta.is_file() && is_card(&name)) {
            continue;
        }
        out.push(Entry {
            insert: insert_text(&p, &name, is_dir),
            size: if is_dir { 0 } else { meta.len() },
            name,
            is_dir,
        });
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    out.truncate(MAX_ENTRIES);
    out
}

/// A size for the menu: `812 B`, `184 KB`, `2.3 MB`.
pub fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{} B", n)
    } else if n < 1024 * 1024 {
        format!("{:.0} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

/// The last folder listing, kept for a second: the menu asks for the entries
/// several times per frame and per key.
#[derive(Default)]
pub struct Cache(Mutex<Option<(String, Instant, Vec<Entry>)>>);

impl Cache {
    pub fn entries(&self, arg: &str) -> Vec<Entry> {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((a, at, v)) = g.as_ref() {
            if a == arg && at.elapsed() < CACHE_FOR {
                return v.clone();
            }
        }
        let v = entries(arg);
        *g = Some((arg.to_string(), Instant::now(), v.clone()));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch folder with a few files; removed again on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Scratch {
            let dir = std::env::temp_dir().join(format!("tarpi-pathcomp-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(dir.join("cards")).unwrap();
            for f in ["Sofia.png", "my card.json", "notes.txt", ".hidden.png"] {
                std::fs::write(dir.join(f), b"x").unwrap();
            }
            Scratch(dir)
        }

        fn arg(&self, tail: &str) -> String {
            format!("{}/{}", self.0.display(), tail)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn names(v: &[Entry]) -> Vec<&str> {
        v.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn only_a_path_after_import_is_completed() {
        assert_eq!(typed_arg("/import "), Some(""));
        assert_eq!(typed_arg("/import ~/Dow"), Some("~/Dow"));
        assert_eq!(typed_arg("/IMPORT   sofia"), Some("sofia"));
        assert_eq!(typed_arg("/import"), None);
        assert_eq!(typed_arg("/importer x"), None);
        assert_eq!(typed_arg("/imp"), None);
        assert_eq!(typed_arg("/regen x"), None);
        assert_eq!(typed_arg("hello /import x"), None);
        assert_eq!(typed_arg("/import {\"name\": \"x\"}"), None);
        assert_eq!(typed_arg("/import https://chub.ai/characters/a/b"), None);
        assert_eq!(typed_arg("/import HTTP://x"), None);
        assert_eq!(typed_arg("/import file:///tmp/a.png"), None);
        assert_eq!(typed_arg("/import a\nb"), None);
    }

    #[test]
    fn folders_come_first_then_cards_and_other_files_are_left_out() {
        let s = Scratch::new();
        let list = entries(&s.arg(""));
        assert_eq!(names(&list), vec!["cards", "my card.json", "Sofia.png"]);
        assert!(list[0].is_dir && !list[1].is_dir);
    }

    #[test]
    fn the_name_typed_so_far_filters_ignoring_case() {
        let s = Scratch::new();
        assert_eq!(names(&entries(&s.arg("so"))), vec!["Sofia.png"]);
        assert_eq!(names(&entries(&s.arg("SO"))), vec!["Sofia.png"]);
        assert!(entries(&s.arg("zzz")).is_empty());
    }

    #[test]
    fn hidden_entries_need_a_dot() {
        let s = Scratch::new();
        assert!(!names(&entries(&s.arg(""))).contains(&".hidden.png"));
        assert_eq!(names(&entries(&s.arg("."))), vec![".hidden.png"]);
    }

    #[test]
    fn a_folder_fills_in_with_a_slash_and_a_space_is_escaped() {
        let s = Scratch::new();
        let list = entries(&s.arg(""));
        assert_eq!(list[0].insert, s.arg("cards/"));
        assert_eq!(list[1].insert, s.arg("my\\ card.json"));
        // An escaped space in what was typed is understood.
        assert_eq!(names(&entries(&s.arg("my\\ c"))), vec!["my card.json"]);
    }

    #[test]
    fn a_quoted_path_keeps_its_quote() {
        let s = Scratch::new();
        let list = entries(&format!("\"{}", s.arg("my")));
        assert_eq!(names(&list), vec!["my card.json"]);
        assert_eq!(list[0].insert, format!("\"{}\"", s.arg("my card.json")));
        let dirs = entries(&format!("'{}", s.arg("ca")));
        assert_eq!(dirs[0].insert, format!("'{}", s.arg("cards/")));
        // Once the quote is closed the path is finished.
        assert!(entries(&format!("\"{}\"", s.arg("my card.json"))).is_empty());
    }

    #[test]
    fn a_missing_folder_gives_nothing() {
        let s = Scratch::new();
        assert!(entries(&s.arg("nope/so")).is_empty());
    }

    #[test]
    fn the_inserted_path_is_one_import_understands() {
        // The entry that goes into the box has to name the same file again.
        let s = Scratch::new();
        let list = entries(&s.arg("my"));
        let again = entries(&list[0].insert);
        assert_eq!(names(&again), vec!["my card.json"]);
    }

    #[test]
    fn sizes_are_short() {
        assert_eq!(human_size(812), "812 B");
        assert_eq!(human_size(188_416), "184 KB");
        assert_eq!(human_size(2_411_724), "2.3 MB");
    }

    #[test]
    fn the_cache_reuses_a_listing() {
        let s = Scratch::new();
        let c = Cache::default();
        let a = c.entries(&s.arg(""));
        std::fs::write(s.0.join("Late.png"), b"x").unwrap();
        // Same text within the second: the old listing.
        assert_eq!(c.entries(&s.arg("")), a);
        // Another text: read again.
        assert!(names(&c.entries(&s.arg("la"))).contains(&"Late.png"));
    }
}
