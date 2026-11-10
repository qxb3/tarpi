//! Offline spell checking for everything that can be edited: the chat input
//! box and the text fields of the `/char` and persona windows. Misspelled
//! words are drawn with a red underline (see `ui::input_row`).
//!
//! The word list (`dict/en.txt`, built by `tools/gen_dict.py` from the
//! Hunspell en_US dictionary) is compiled into the program: lowercase words,
//! one per line, sorted by byte value. It is never loaded or parsed: a lookup
//! is a binary search over the embedded text itself, so start-up costs
//! nothing and no memory is used beyond the text. The answer for the text in
//! the box is cached and only worked out again when the text changes.
//!
//! Extra words that are never marked: the ones in
//! `~/.config/tarpi/dictionary.txt` (one per line, `#` starts a comment), and
//! the names of the character and of your persona.
//!
//! Short words are held to the same standard: only real one- and two-letter
//! words pass (`a`, `I`, `of`, `up`, ...), so `ho` and `x` are marked. What is
//! left alone: single capital letters (initials), `e.g.`-style abbreviations,
//! ALL-CAPS words, words with a capital inside (`iPhone`), anything with a
//! digit, `_ @ / \ # { } < > =` in it
//! (links, paths, `{{macros}}`), a leading `/command`, and text that is not
//! in Latin letters. Hyphenated words are checked part by part, `'s` is
//! allowed after any known word, and stretched words (`sooo`, `heyyy`) pass
//! when a shorter spelling is a word.

use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

/// The word list: lowercase, one word per line, sorted by byte value.
static WORDS: &str = include_str!("../dict/en.txt");

/// Words that start with an apostrophe in casual writing (`'em`, `'til`).
const CLIPPED: &[&str] = &[
    "em", "til", "cause", "bout", "round", "tis", "twas", "cuz", "sup", "kay", "nuff",
];

/// Is `word` (lowercase) one of the lines of `data`, which is sorted by byte
/// value? Binary search over the text: the region `lo..hi` always starts at a
/// line start and ends at a line start (or at the end of the text).
fn listed(data: &str, word: &str) -> bool {
    let d = data.as_bytes();
    let w = word.as_bytes();
    if w.is_empty() {
        return false;
    }
    let (mut lo, mut hi) = (0usize, d.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let start = d[..mid]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |p| p + 1);
        let end = d[mid..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(d.len(), |p| mid + p);
        match Ord::cmp(&d[start..end], w) {
            Ordering::Equal => return true,
            Ordering::Less => lo = end + 1,
            Ordering::Greater => hi = start,
        }
    }
    false
}

fn is_apostrophe(c: char) -> bool {
    c == '\'' || c == '\u{2019}'
}

/// A letter of the Latin alphabet (with accents). Words in other scripts are
/// not checked, since the list is English.
fn is_latin(c: char) -> bool {
    c.is_ascii_alphabetic() || ('\u{00C0}'..='\u{024F}').contains(&c)
}

/// `sooo` -> `soo` (`max` = 2) or `so` (`max` = 1): runs of the same letter
/// cut down to at most `max` letters.
fn squeeze(word: &str, max: usize) -> String {
    let mut out = String::with_capacity(word.len());
    let mut last: Option<char> = None;
    let mut run = 0usize;
    for c in word.chars() {
        if Some(c) == last {
            run += 1;
        } else {
            last = Some(c);
            run = 1;
        }
        if run <= max {
            out.push(c);
        }
    }
    out
}

/// The `(start, end)` char ranges of the misspelled words of `text`
/// (`end` exclusive). `is_known` gets a lowercase word.
fn find(text: &[char], is_known: impl Fn(&str) -> bool) -> Vec<(usize, usize)> {
    let known = |lc: &str| -> bool {
        if is_known(lc) {
            return true;
        }
        match lc.strip_suffix("'s") {
            Some(base) => !base.is_empty() && is_known(base),
            None => false,
        }
    };
    let mut out = Vec::new();
    let n = text.len();
    let mut i = 0usize;
    let mut first_chunk = true;
    while i < n {
        if text[i].is_whitespace() {
            i += 1;
            continue;
        }
        let s = i;
        while i < n && !text[i].is_whitespace() {
            i += 1;
        }
        let e = i;
        let at_start = first_chunk;
        first_chunk = false;
        let chunk = &text[s..e];
        // `/regen` and friends.
        if at_start && chunk[0] == '/' {
            continue;
        }
        // Links, paths, macros, numbers and the like.
        if chunk.iter().any(|&c| {
            c.is_ascii_digit()
                || matches!(c, '_' | '@' | '/' | '\\' | '#' | '{' | '}' | '<' | '>' | '=')
        }) {
            continue;
        }
        let mut j = s;
        while j < e {
            if !text[j].is_alphabetic() {
                j += 1;
                continue;
            }
            // A run of letters; an apostrophe belongs to it between letters.
            let ws = j;
            while j < e
                && (text[j].is_alphabetic()
                    || (is_apostrophe(text[j]) && j + 1 < e && text[j + 1].is_alphabetic()))
            {
                j += 1;
            }
            let we = j;
            let letters = &text[ws..we];
            if letters.len() == 1 {
                // Initials and grades (`J.`, `Plan B`) and abbreviations like
                // `e.g.` and `p.m.`; any other single letter must be a word.
                if letters[0].is_uppercase() {
                    continue;
                }
                let dot_after = we + 1 < e && text[we] == '.' && text[we + 1].is_alphabetic();
                let dot_before = ws >= s + 2 && text[ws - 1] == '.' && text[ws - 2].is_alphabetic();
                if dot_after || dot_before {
                    continue;
                }
            }
            if !letters.iter().all(|&c| is_apostrophe(c) || is_latin(c)) {
                continue;
            }
            // ALL CAPS (acronyms) and inner capitals (iPhone, McDonald).
            if letters.iter().skip(1).any(|c| c.is_uppercase()) {
                continue;
            }
            let word: String = letters.iter().collect();
            let lc = word.to_lowercase().replace('\u{2019}', "'");
            if ws > s && is_apostrophe(text[ws - 1]) && CLIPPED.contains(&lc.as_str()) {
                continue;
            }
            if known(&lc) {
                continue;
            }
            // A stretched word needs a real word underneath: `sooo` -> `so`,
            // but `aaaa` -> `a` is only the same letter over and over.
            let two = squeeze(&lc, 2);
            let one = squeeze(&lc, 1);
            let real = |s: &str| known(s) && s.chars().any(|c| c != s.chars().next().unwrap());
            if (two != lc && real(&two)) || (one != lc && real(&one)) {
                continue;
            }
            out.push((ws, we));
        }
    }
    out
}

/// Every spelling one edit away from `w`: a letter deleted, two neighbours
/// swapped, a letter changed or a letter added (Norvig's method).
fn edits1(w: &str) -> Vec<String> {
    const ALPHA: &str = "abcdefghijklmnopqrstuvwxyz'";
    let c: Vec<char> = w.chars().collect();
    let mut out: Vec<String> = Vec::new();
    for i in 0..=c.len() {
        if i < c.len() {
            let mut d = c.clone();
            d.remove(i);
            out.push(d.iter().collect());
        }
        if i + 1 < c.len() {
            let mut t = c.clone();
            t.swap(i, i + 1);
            out.push(t.iter().collect());
        }
        for a in ALPHA.chars() {
            if i < c.len() && c[i] != a {
                let mut r = c.clone();
                r[i] = a;
                out.push(r.iter().collect());
            }
            let mut ins = c.clone();
            ins.insert(i, a);
            out.push(ins.iter().collect());
        }
    }
    out
}

/// The spell checker, shared by every editable text (the chat input box and
/// the fields of the `/char` and persona windows).
pub struct Checker {
    /// Words from `dictionary.txt` (lowercase).
    user: HashSet<String>,
    /// Names of the character and the persona (lowercase, one word each).
    names: HashSet<String>,
    names_key: String,
    /// Names of the window that is open (the name field being edited).
    extra: HashSet<String>,
    extra_key: String,
    /// Hash of both name lists: part of every cache key.
    sig: u64,
    /// Marks by hash of (names, text): a window with many fields draws them
    /// all on every key press, and only the changed one is worked out again.
    cache: HashMap<u64, Vec<(usize, usize)>>,
}

/// How many answers are kept before the cache is emptied.
const CACHE_MAX: usize = 128;

/// Every word of every name, lowercase.
fn word_set(names: &[&str]) -> HashSet<String> {
    names
        .iter()
        .flat_map(|n| n.split(|c: char| !c.is_alphabetic() && !is_apostrophe(c)))
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase().replace('\u{2019}', "'"))
        .collect()
}

impl Checker {
    fn empty() -> Checker {
        let mut c = Checker {
            user: HashSet::new(),
            names: HashSet::new(),
            names_key: String::new(),
            extra: HashSet::new(),
            extra_key: String::new(),
            sig: 0,
            cache: HashMap::new(),
        };
        c.resign();
        c
    }

    /// A checker with the user's own dictionary
    /// (`~/.config/tarpi/dictionary.txt`) loaded, if there is one.
    pub fn new() -> Checker {
        let mut c = Checker::empty();
        if let Some(text) = crate::config::dictionary_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
        {
            for line in text.lines() {
                let w = line.trim();
                if w.is_empty() || w.starts_with('#') {
                    continue;
                }
                c.user.insert(w.to_lowercase().replace('\u{2019}', "'"));
            }
        }
        c
    }

    fn resign(&mut self) {
        let mut h = DefaultHasher::new();
        self.names_key.hash(&mut h);
        self.extra_key.hash(&mut h);
        self.sig = h.finish();
    }

    /// Names that are always right (the character, the persona). Every word
    /// of every name counts on its own.
    pub fn set_names(&mut self, names: &[&str]) {
        let key = names.join("\n");
        if key == self.names_key {
            return;
        }
        self.names = word_set(names);
        self.names_key = key;
        self.resign();
    }

    /// More names that are right while a window is drawn (the name being
    /// typed into its name field). Call with `&[]` for none.
    pub fn set_extra(&mut self, names: &[&str]) {
        let key = names.join("\n");
        if key == self.extra_key {
            return;
        }
        self.extra = word_set(names);
        self.extra_key = key;
        self.resign();
    }

    /// The char ranges of the misspelled words of `text`.
    pub fn marks(&mut self, text: &str) -> Vec<(usize, usize)> {
        if text.is_empty() {
            return Vec::new();
        }
        let mut h = DefaultHasher::new();
        self.sig.hash(&mut h);
        text.hash(&mut h);
        let key = h.finish();
        if let Some(v) = self.cache.get(&key) {
            return v.clone();
        }
        let chars: Vec<char> = text.chars().collect();
        let found = {
            let user = &self.user;
            let names = &self.names;
            let extra = &self.extra;
            find(&chars, |w| {
                listed(WORDS, w) || user.contains(w) || names.contains(w) || extra.contains(w)
            })
        };
        if self.cache.len() >= CACHE_MAX {
            self.cache.clear();
        }
        self.cache.insert(key, found.clone());
        found
    }

    fn is_word(&self, w: &str) -> bool {
        listed(WORDS, w)
            || self.user.contains(w)
            || self.names.contains(w)
            || self.extra.contains(w)
    }

    /// Up to `max` spellings for the misspelled `word`, best first: one edit
    /// away (a swapped pair first, then words that keep the first letter and
    /// the length), then two edits away when that leaves too few. The
    /// capital letter at the start of `word` is kept.
    pub fn suggest(&self, word: &str, max: usize) -> Vec<String> {
        let lc = word.to_lowercase().replace('\u{2019}', "'");
        let cl: Vec<char> = lc.chars().collect();
        if cl.is_empty() || cl.len() > 24 {
            return Vec::new();
        }
        // Typo kinds, most likely first: two letters swapped (`teh`), a
        // letter left out (`helo`: the word is `hello` with letters
        // missing), anything else.
        let rank = |cand: &str, dist: u8| -> (u8, u8, bool, usize, String) {
            let cc: Vec<char> = cand.chars().collect();
            let swapped = cc.len() == cl.len()
                && (0..cc.len().saturating_sub(1)).any(|i| {
                    let mut t = cl.clone();
                    t.swap(i, i + 1);
                    t == cc
                });
            let mut it = cc.iter();
            let missing = cc.len() > cl.len() && cl.iter().all(|c| it.any(|d| d == c));
            let kind = if swapped {
                0
            } else if missing {
                1
            } else {
                2
            };
            (
                dist,
                kind,
                cc.first() != cl.first(),
                cc.len().abs_diff(cl.len()),
                cand.to_string(),
            )
        };
        let mut seen: HashSet<String> = HashSet::new();
        seen.insert(lc.clone());
        let mut one: Vec<String> = Vec::new();
        for e in edits1(&lc) {
            if seen.insert(e.clone()) && self.is_word(&e) {
                one.push(e);
            }
        }
        let mut ranked: Vec<(u8, u8, bool, usize, String)> =
            one.iter().map(|w| rank(w, 1)).collect();
        if ranked.len() < max && cl.len() <= 12 {
            let firsts = edits1(&lc);
            for f in firsts {
                for e in edits1(&f) {
                    if seen.insert(e.clone()) && self.is_word(&e) {
                        ranked.push(rank(&e, 2));
                    }
                }
            }
        }
        ranked.sort();
        let upper = word.chars().next().map_or(false, |c| c.is_uppercase());
        ranked
            .into_iter()
            .take(max)
            .map(|r| {
                if upper {
                    let mut ch = r.4.chars();
                    match ch.next() {
                        Some(f) => f.to_uppercase().chain(ch).collect(),
                        None => r.4,
                    }
                } else {
                    r.4
                }
            })
            .collect()
    }

    /// Like `marks`, but the word the cursor (a char index) is in or right
    /// behind is left out when `typing` has it: that word is still being
    /// typed. `None` when the text is not being typed into.
    pub fn marks_typing(&mut self, text: &str, typing: Option<usize>) -> Vec<(usize, usize)> {
        let mut marks = self.marks(text);
        if let Some(c) = typing {
            marks.retain(|&(s, e)| c < s || c > e);
        }
        marks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The misspelled words of `text`.
    fn bad(text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        find(&chars, |w| listed(WORDS, w))
            .into_iter()
            .map(|(s, e)| chars[s..e].iter().collect())
            .collect()
    }

    #[test]
    fn the_list_is_sorted_and_lowercase() {
        let mut prev = "";
        for w in WORDS.lines() {
            assert!(w > prev, "{prev:?} before {w:?}");
            assert_eq!(w, w.to_lowercase());
            prev = w;
        }
    }

    #[test]
    fn lookup_finds_every_word_and_nothing_else() {
        for w in WORDS.lines().step_by(97) {
            assert!(listed(WORDS, w), "{w}");
        }
        assert!(listed(WORDS, "hello"));
        assert!(listed(WORDS, "don't"));
        assert!(!listed(WORDS, "helo"));
        assert!(!listed(WORDS, "zzzzzz"));
        assert!(!listed(WORDS, ""));
        // A part of a line is not a word.
        assert!(!listed(WORDS, "hell o"));
        assert!(listed("a\nb\nc\n", "a"));
        assert!(listed("a\nb\nc\n", "c"));
        assert!(!listed("a\nb\nc\n", "d"));
    }

    #[test]
    fn marks_wrong_words_only() {
        assert_eq!(bad("I recieve teh letter"), vec!["recieve", "teh"]);
        assert!(bad("The quick brown fox jumps over the lazy dog.").is_empty());
    }

    #[test]
    fn ranges_are_char_positions() {
        let chars: Vec<char> = "ok teh".chars().collect();
        assert_eq!(find(&chars, |w| listed(WORDS, w)), vec![(3, 6)]);
        // After a multi-byte character the positions still count chars.
        let chars: Vec<char> = "caf\u{e9} teh".chars().collect();
        assert_eq!(find(&chars, |w| listed(WORDS, w)), vec![(5, 8)]);
    }

    #[test]
    fn apostrophes_and_possessives() {
        assert!(bad("don't can\u{2019}t it's").is_empty());
        assert!(bad("the dog's bone").is_empty());
    }

    #[test]
    fn quotes_around_words_are_not_part_of_them() {
        assert_eq!(bad("\"teh\" and 'teh' and *teh*"), vec!["teh", "teh", "teh"]);
        assert!(bad("'em 'til").is_empty());
    }

    #[test]
    fn hyphenated_words_are_checked_by_part() {
        assert!(bad("well-known").is_empty());
        assert_eq!(bad("well-knwon"), vec!["knwon"]);
    }

    #[test]
    fn things_that_are_not_words_are_left_alone() {
        assert!(bad("NASA iPhone McDonald 3rd abc123 snake_case").is_empty());
        assert!(bad("see https://exmple.com/pge or me@exmple.com").is_empty());
        assert!(bad("{{user}} and {{chr}}").is_empty());
        assert!(bad("/regen she hesitates").is_empty());
        assert_eq!(bad("/regen sehe"), vec!["sehe"]);
        assert!(bad("\u{4f60}\u{597d} \u{43f}\u{440}\u{438}\u{432}\u{435}\u{442}").is_empty());
    }

    #[test]
    fn short_words_must_be_real_words() {
        assert_eq!(bad("ho"), vec!["ho"]);
        assert_eq!(bad("hx wz u x"), vec!["hx", "wz", "u", "x"]);
        assert_eq!(bad("is it ok to go up or on to us"), Vec::<String>::new());
        assert!(bad("I a o").is_empty());
        assert_eq!(bad("ho ho ho"), vec!["ho", "ho", "ho"]);
        // Abbreviations of the book list do not pass any more.
        assert_eq!(bad("cpl ctr"), vec!["cpl", "ctr"]);
        assert!(bad("hmm shh").is_empty());
    }

    #[test]
    fn initials_and_dotted_abbreviations_are_left_alone() {
        assert!(bad("Plan B and Malcolm X, J. R. R. Tolkien").is_empty());
        assert!(bad("e.g. i.e. 5 p.m. and a.m.").is_empty());
        assert_eq!(bad("see u."), vec!["u"]);
    }

    #[test]
    fn stretched_words_pass_when_a_short_form_is_a_word() {
        assert!(bad("sooo heyyy noooo").is_empty());
        assert_eq!(bad("blorrrp"), vec!["blorrrp"]);
        // The same letter repeated is never a word, however short the root.
        assert_eq!(
            bad("a aaaa aaaaa aaaa aaaaaaaaaa"),
            vec!["aaaa", "aaaaa", "aaaa", "aaaaaaaaaa"]
        );
        assert_eq!(bad("bbb xx"), vec!["bbb", "xx"]);
    }

    #[test]
    fn capitals_do_not_matter() {
        assert!(bad("Hello HELLO hello").is_empty());
        assert_eq!(bad("Helo"), vec!["Helo"]);
    }

    #[test]
    fn names_and_the_user_dictionary_count() {
        let mut c = Checker::empty();
        c.user.insert("blorp".to_string());
        assert_eq!(c.marks("Mirei and blorp"), vec![(0, 5)]);
        c.set_names(&["Mirei", "Dr. Sofia (Boss)"]);
        assert!(c.marks("Mirei and blorp and Sofia's boss").is_empty());
    }

    #[test]
    fn extra_names_count_until_they_are_taken_away() {
        let mut c = Checker::empty();
        c.set_extra(&["Kaori"]);
        assert!(c.marks("Kaori").is_empty());
        c.set_extra(&[]);
        assert_eq!(c.marks("Kaori"), vec![(0, 5)]);
    }

    #[test]
    fn suggestions_fix_common_typos() {
        let c = Checker::empty();
        assert_eq!(c.suggest("teh", 6).first().map(|s| s.as_str()), Some("the"));
        assert!(c.suggest("recieve", 6).contains(&"receive".to_string()));
        assert!(c.suggest("helo", 6).contains(&"hello".to_string()));
        assert!(c.suggest("Helo", 6).contains(&"Hello".to_string()));
        assert!(c.suggest("wnat", 6).contains(&"want".to_string()));
        assert!(c.suggest("aaaa", 6).len() <= 6);
        assert!(c.suggest("", 6).is_empty());
    }

    #[test]
    fn suggestions_know_names_and_user_words() {
        let mut c = Checker::empty();
        c.set_names(&["Mirei"]);
        assert!(c.suggest("Mirie", 6).contains(&"Mirei".to_string()));
    }

    #[test]
    fn the_cache_follows_the_text() {
        let mut c = Checker::empty();
        assert_eq!(c.marks("teh").len(), 1);
        assert_eq!(c.marks("teh").len(), 1);
        assert!(c.marks("the").is_empty());
        assert!(c.marks("").is_empty());
    }

    #[test]
    fn the_word_being_typed_is_not_marked() {
        let mut c = Checker::empty();
        // Cursor at the end of `teh`, or inside it: still being typed.
        assert!(c.marks_typing("ok teh", Some(6)).is_empty());
        assert!(c.marks_typing("ok teh", Some(4)).is_empty());
        // Cursor on the space after it, or elsewhere: judged.
        assert_eq!(c.marks_typing("ok teh ", Some(7)), vec![(3, 6)]);
        assert_eq!(c.marks_typing("ok teh", Some(1)), vec![(3, 6)]);
        assert_eq!(c.marks_typing("ok teh", None), vec![(3, 6)]);
    }
}
