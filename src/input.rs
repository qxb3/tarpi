//! Minimal multi-line text input with a cursor measured in characters,
//! plus a small vim-style NORMAL / VISUAL mode (`qq` leaves INSERT mode) and a
//! `:` command line (`:%s/find/replace/` with a live preview of the matches).

use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Mode {
    #[default]
    Insert,
    Normal,
    Visual,
    /// `:` command line (typed text lives in `Input::cmd`, not in the box).
    Command,
}

#[derive(Default)]
pub struct Input {
    pub text: String,
    /// `:w` was run; the screen that owns the box takes it with
    /// `take_write_req` and saves.
    write_req: bool,
    /// `:wq` was run (together with `write_req`): save, then close the window.
    quit_req: bool,
    /// Cursor position in chars (not bytes).
    pub cursor: usize,
    pub mode: Mode,
    /// Operator / prefix waiting for its second key (`d`, `c`, `y`, `g`, `r`; `I` / `A`
    /// after `i` / `a` of a text object; `i` / `a` in VISUAL mode).
    pending: Option<char>,
    /// Count typed before `pending`.
    pend_n: usize,
    /// The operator (`d`, `c` or `y`) waiting for a text object, while
    /// `pending` is `I` (after `i`) or `A` (after `a`).
    obj_op: char,
    /// Digits typed so far (`3` in `3w`).
    count: String,
    /// Unnamed register.
    reg: String,
    reg_lines: bool,
    undo: Vec<(String, usize)>,
    /// Text and cursor when the current INSERT session began. Leaving INSERT
    /// mode pushes it on the undo stack (if the text changed), so one `u`
    /// undoes everything typed in that session, like in vim.
    insert_base: Option<(String, usize)>,
    /// A snapshot was taken by the NORMAL-mode key being handled (`i`, `o`,
    /// `cw` ...), so the INSERT session it starts needs no base of its own.
    just_snap: bool,
    /// Leader key (config `vim.leader`, default space); `None` turns the
    /// `<leader>` mappings off.
    leader: Option<char>,
    /// The leader key was just typed; the next key completes the mapping.
    leader_wait: bool,
    /// The pending `y` came from `<leader>y`: copy the yank to the system clipboard.
    sys_yank: bool,
    /// The last `operator` call wrote the register (it was a successful yank).
    yanked: bool,
    /// Text waiting to be sent to the system clipboard (see `take_clipboard`).
    clip: Option<String>,
    /// `true`: no vim modes, the box is plain text editing (config `vim.enabled`).
    vim_off: bool,
    /// Auto-close quotes, `*`, `` ` `` and brackets while typing (config `autopair`).
    autopair: bool,
    /// Keys typed quickly in INSERT mode to reach NORMAL mode (default `qq`).
    normal_keys: Vec<char>,
    /// How many of `normal_keys` were just typed, and when the last one was.
    seq_hits: usize,
    seq_at: Option<Instant>,
    /// VISUAL mode: where the selection started (the cursor is the other end).
    anchor: usize,
    /// VISUAL mode: whole lines (`V`) instead of characters (`v`).
    vis_line: bool,
    /// COMMAND mode: the text typed after the `:`.
    cmd: String,
    /// COMMAND mode started from VISUAL mode: first and last selected line
    /// (0-based), what `'<` and `'>` stand for.
    cmd_marks: Option<(usize, usize)>,
}

// ------------------------------------------------------------ char helpers

fn class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

fn line_start(ch: &[char], i: usize) -> usize {
    let mut i = i.min(ch.len());
    while i > 0 && ch[i - 1] != '\n' {
        i -= 1;
    }
    i
}

/// Index of the newline ending the line (or the text length).
fn line_end(ch: &[char], i: usize) -> usize {
    let mut i = i.min(ch.len());
    while i < ch.len() && ch[i] != '\n' {
        i += 1;
    }
    i
}

/// The cursor one line (not one display row) up (`dir < 0`) or down, at the
/// same offset in that line or at its last char when it is shorter. It stays
/// where it is on the first / last line.
fn line_vertical(ch: &[char], cursor: usize, dir: i32) -> usize {
    let cur = cursor.min(ch.len());
    let ls = line_start(ch, cur);
    let col = cur - ls;
    let (ts, te) = if dir < 0 {
        if ls == 0 {
            return cur;
        }
        let nl = ls - 1;
        (line_start(ch, nl), nl)
    } else {
        let le = line_end(ch, cur);
        if le >= ch.len() {
            return cur;
        }
        (le + 1, line_end(ch, le + 1))
    };
    ts + col.min((te - ts).saturating_sub(1))
}

fn first_nonblank(ch: &[char], i: usize) -> usize {
    let mut j = line_start(ch, i);
    while j < ch.len() && (ch[j] == ' ' || ch[j] == '\t') {
        j += 1;
    }
    j
}

/// Character class; with `big` (the `W` `B` `E` WORD motions) everything
/// that is not whitespace counts as one class.
fn cls(c: char, big: bool) -> u8 {
    if big && !c.is_whitespace() {
        1
    } else {
        class(c)
    }
}

fn word_fwd(ch: &[char], i: usize, big: bool) -> usize {
    let n = ch.len();
    let mut i = i.min(n);
    if i < n && cls(ch[i], big) != 0 {
        let cl = cls(ch[i], big);
        while i < n && cls(ch[i], big) == cl {
            i += 1;
        }
    }
    while i < n && cls(ch[i], big) == 0 {
        i += 1;
    }
    i
}

fn word_back(ch: &[char], i: usize, big: bool) -> usize {
    let mut i = i.min(ch.len());
    while i > 0 && cls(ch[i - 1], big) == 0 {
        i -= 1;
    }
    if i == 0 {
        return 0;
    }
    let cl = cls(ch[i - 1], big);
    while i > 0 && cls(ch[i - 1], big) == cl {
        i -= 1;
    }
    i
}

fn word_end(ch: &[char], i: usize, big: bool) -> usize {
    let n = ch.len();
    if n == 0 {
        return 0;
    }
    let mut i = i + 1;
    while i < n && cls(ch[i], big) == 0 {
        i += 1;
    }
    if i >= n {
        return n - 1;
    }
    let cl = cls(ch[i], big);
    while i + 1 < n && cls(ch[i + 1], big) == cl {
        i += 1;
    }
    i
}

/// 0-based number of the line `i` is on.
fn line_no(ch: &[char], i: usize) -> usize {
    ch[..i.min(ch.len())].iter().filter(|&&c| c == '\n').count()
}

/// Start and end (the newline, or text end) of the 0-based line `n`.
fn line_bounds(ch: &[char], n: usize) -> (usize, usize) {
    let mut s = 0;
    for _ in 0..n {
        match ch[s..].iter().position(|&c| c == '\n') {
            Some(p) => s += p + 1,
            None => break,
        }
    }
    (s, line_end(ch, s))
}

// ----------------------------------------------------------- text objects

/// Start and end (the newline, or text end) of the paragraph `cur` is in
/// (lines up to the next blank line).
fn para_bounds(ch: &[char], cur: usize) -> (usize, usize) {
    let blank = |ls: usize| {
        let le = line_end(ch, ls);
        ch[ls..le].iter().all(|c| c.is_whitespace())
    };
    let mut s = line_start(ch, cur);
    while s > 0 {
        let ps = line_start(ch, s - 1);
        if blank(ps) {
            break;
        }
        s = ps;
    }
    let mut e = line_end(ch, cur);
    while e < ch.len() {
        let ns = e + 1;
        if ns >= ch.len() || blank(ns) {
            break;
        }
        e = line_end(ch, ns);
    }
    (s, e)
}

/// `iw` / `aw` (`iW` / `aW` with `big`): the word under the cursor on its line.
/// `aw` also takes the whitespace after it (before it, when nothing follows).
fn obj_word(ch: &[char], cur: usize, around: bool, big: bool) -> Option<(usize, usize)> {
    if cur >= ch.len() || ch[cur] == '\n' {
        return None;
    }
    let ls = line_start(ch, cur);
    let le = line_end(ch, cur);
    let same = |a: usize, b: usize| cls(ch[a], big) == cls(ch[b], big);
    let run = |from: usize| -> (usize, usize) {
        let mut s = from;
        while s > ls && same(s - 1, from) {
            s -= 1;
        }
        let mut e = from + 1;
        while e < le && same(e, from) {
            e += 1;
        }
        (s, e)
    };
    let (s, e) = run(cur);
    if !around {
        return Some((s, e));
    }
    if cls(ch[cur], big) == 0 {
        // On whitespace: it, and the word after it.
        if e < le {
            return Some((s, run(e).1));
        }
        return Some((s, e));
    }
    if e < le && cls(ch[e], big) == 0 {
        return Some((s, run(e).1));
    }
    if s > ls && cls(ch[s - 1], big) == 0 {
        return Some((run(s - 1).0, e));
    }
    Some((s, e))
}

/// `i"` `a"` `i'` `i`` and `i*` `a*`: the text between a pair of `q` on the
/// cursor's line (for `*`, in its paragraph, like the prose styling). The
/// pair around the cursor is used, else the next one after it. `a` also takes
/// the spaces after the pair (before it, when nothing follows).
fn obj_quote(ch: &[char], cur: usize, q: char, around: bool) -> Option<(usize, usize)> {
    if cur > ch.len() {
        return None;
    }
    let (lo, hi) = if q == '*' {
        para_bounds(ch, cur)
    } else {
        (line_start(ch, cur), line_end(ch, cur))
    };
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let mut open: Option<usize> = None;
    for i in lo..hi {
        if ch[i] != q {
            continue;
        }
        if q == '*' {
            // `**` is bold, not narration. A `*` opens when text follows it and
            // closes when text precedes it; others (`2 * 3`) are left alone.
            if (i + 1 < hi && ch[i + 1] == '*') || (i > lo && ch[i - 1] == '*') {
                continue;
            }
            match open {
                None => {
                    if i + 1 < hi && !ch[i + 1].is_whitespace() {
                        open = Some(i);
                    }
                }
                Some(s) => {
                    if !ch[i - 1].is_whitespace() {
                        pairs.push((s, i));
                        open = None;
                    }
                }
            }
        } else {
            match open {
                None => open = Some(i),
                Some(s) => {
                    pairs.push((s, i));
                    open = None;
                }
            }
        }
    }
    let &(s, e) = pairs
        .iter()
        .find(|(s, e)| *s <= cur && cur <= *e)
        .or_else(|| pairs.iter().find(|(s, _)| *s > cur))?;
    if !around {
        return Some((s + 1, e));
    }
    let blank = |c: char| c == ' ' || c == '\t';
    let mut end = e + 1;
    while end < hi && blank(ch[end]) {
        end += 1;
    }
    let mut start = s;
    if end == e + 1 {
        while start > lo && blank(ch[start - 1]) {
            start -= 1;
        }
    }
    Some((start, end))
}

/// `i(` `a(` `ib` `i[` `i{` `iB` `i<` ...: the text between the pair of
/// brackets around the cursor, nesting and several lines allowed.
fn obj_bracket(ch: &[char], cur: usize, o: char, cl: char, around: bool) -> Option<(usize, usize)> {
    let len = ch.len();
    if cur >= len {
        return None;
    }
    let start = if ch[cur] == o {
        cur
    } else {
        let mut depth = 0usize;
        let mut j = cur;
        let mut found = None;
        while j > 0 {
            j -= 1;
            if ch[j] == cl {
                depth += 1;
            } else if ch[j] == o {
                if depth == 0 {
                    found = Some(j);
                    break;
                }
                depth -= 1;
            }
        }
        found?
    };
    let mut depth = 0usize;
    let mut end = None;
    for k in start + 1..len {
        if ch[k] == o {
            depth += 1;
        } else if ch[k] == cl {
            if depth == 0 {
                end = Some(k);
                break;
            }
            depth -= 1;
        }
    }
    let end = end?;
    Some(if around { (start, end + 1) } else { (start + 1, end) })
}

/// The range (half-open, in chars) of the text object `key` (`w`, `"`, `(`,
/// `*`, ...) around `cur`; `around` is `a` instead of `i`.
fn text_object(ch: &[char], cur: usize, key: char, around: bool) -> Option<(usize, usize)> {
    match key {
        'w' => obj_word(ch, cur, around, false),
        'W' => obj_word(ch, cur, around, true),
        '"' | '\'' | '`' | '*' => obj_quote(ch, cur, key, around),
        '(' | ')' | 'b' => obj_bracket(ch, cur, '(', ')', around),
        '[' | ']' => obj_bracket(ch, cur, '[', ']', around),
        '{' | '}' | 'B' => obj_bracket(ch, cur, '{', '}', around),
        '<' | '>' => obj_bracket(ch, cur, '<', '>', around),
        _ => None,
    }
}

// ---------------------------------------------------------- command line

/// One piece of a `:s` replacement.
#[derive(Clone, Debug, PartialEq)]
enum Rep {
    Lit(char),
    /// `&`: the text that was found.
    Matched,
}

/// A parsed `:[range]s/pat/rep/[flags]`.
#[derive(Clone, Debug)]
struct Subst {
    /// First and last line (0-based, inclusive).
    lines: (usize, usize),
    pat: Vec<char>,
    rep: Vec<Rep>,
    global: bool,
    ignore_case: bool,
    /// The closing delimiter of the pattern was typed, so the replacement
    /// part has begun (the live preview then shows the replaced text).
    rep_started: bool,
}

/// What `Subst::apply` makes of the text.
struct Applied {
    /// The text after all replacements.
    text: Vec<char>,
    /// Where the replacements ended up in `text` (half-open, never empty).
    spans: Vec<(usize, usize)>,
    /// Start (in `text`) of the last replacement.
    last_at: usize,
    count: usize,
    lines: usize,
}

/// The live view of a `:s` being typed.
pub struct Live {
    /// The text as it would be after the replacement, once the replacement
    /// part has begun; `None` while only the pattern is being typed.
    pub text: Option<String>,
    /// What to highlight: matches (in the old text) or, with `text`, the
    /// replaced spans (in the new text).
    pub marks: Vec<(usize, usize)>,
    /// How many matches there are.
    pub count: usize,
}

enum Cmd {
    /// `:5` — go to a line.
    Goto(usize),
    Subst(Subst),
    /// `:w` / `:write` — save (what Ctrl+S does where the box is).
    Write,
    /// `:wq` / `:x` — save, then close the window.
    WriteQuit,
}

impl Subst {
    /// Do the replacements. `None` when nothing matches.
    fn apply(&self, ch: &[char]) -> Option<Applied> {
        let found = self.find(ch);
        if found.is_empty() {
            return None;
        }
        let mut out: Vec<char> = Vec::with_capacity(ch.len());
        let mut spans = Vec::new();
        let mut last = 0;
        let mut last_at = 0;
        let mut lines = 0;
        let mut prev_line = usize::MAX;
        for &(s, e) in &found {
            out.extend_from_slice(&ch[last..s]);
            last_at = out.len();
            for t in &self.rep {
                match t {
                    Rep::Lit(c) => out.push(*c),
                    Rep::Matched => out.extend_from_slice(&ch[s..e]),
                }
            }
            if out.len() > last_at {
                spans.push((last_at, out.len()));
            }
            last = e;
            let ln = line_start(ch, s);
            if ln != prev_line {
                lines += 1;
                prev_line = ln;
            }
        }
        out.extend_from_slice(&ch[last..]);
        Some(Applied {
            text: out,
            spans,
            last_at,
            count: found.len(),
            lines,
        })
    }

    /// Every match, as half-open char ranges, in the order `:s` replaces
    /// them: all of them with the `g` flag, else the first one per line.
    fn find(&self, ch: &[char]) -> Vec<(usize, usize)> {
        let (rs, _) = line_bounds(ch, self.lines.0);
        let (_, re) = line_bounds(ch, self.lines.1);
        let m = self.pat.len();
        let mut out = Vec::new();
        if m == 0 {
            return out;
        }
        let eq = |a: char, b: char| {
            a == b
                || (self.ignore_case
                    && a.to_lowercase().eq(b.to_lowercase()))
        };
        let mut i = rs;
        while i + m <= re {
            if (0..m).all(|k| eq(ch[i + k], self.pat[k])) {
                out.push((i, i + m));
                i = if self.global {
                    i + m
                } else {
                    // Next line only (a match that swallowed a newline has
                    // already used up the start of the next one).
                    line_end(ch, i + m) + 1
                };
            } else {
                i += 1;
            }
        }
        out
    }
}

/// An address: `.`, `$`, a line number, `'<` or `'>`. `Ok(None)` when there
/// is no address at `i`.
fn parse_addr(
    s: &[char],
    i: &mut usize,
    nlines: usize,
    cur: usize,
    marks: Option<(usize, usize)>,
) -> Result<Option<usize>, String> {
    match s.get(*i) {
        Some('.') => {
            *i += 1;
            Ok(Some(cur))
        }
        Some('$') => {
            *i += 1;
            Ok(Some(nlines - 1))
        }
        Some('\'') => {
            let which = s.get(*i + 1).copied();
            if which != Some('<') && which != Some('>') {
                return Err("E20: Mark not set".to_string());
            }
            let Some((a, b)) = marks else {
                return Err("E20: Mark not set".to_string());
            };
            *i += 2;
            Ok(Some(if which == Some('<') { a } else { b }))
        }
        Some(d) if d.is_ascii_digit() => {
            let mut n = 0usize;
            while let Some(d) = s.get(*i).and_then(|c| c.to_digit(10)) {
                n = n.saturating_mul(10).saturating_add(d as usize);
                *i += 1;
            }
            if n > nlines {
                return Err("E16: Invalid range".to_string());
            }
            Ok(Some(n.saturating_sub(1)))
        }
        _ => Ok(None),
    }
}

/// Read up to the next unescaped `d`. `\d` gives a plain `d`, other escapes
/// are kept for later. Also says whether the closing `d` was there.
fn take_part(s: &[char], i: &mut usize, d: char) -> (Vec<char>, bool) {
    let mut out = Vec::new();
    while *i < s.len() {
        let c = s[*i];
        if c == '\\' && *i + 1 < s.len() {
            let n = s[*i + 1];
            if n != d {
                out.push('\\');
            }
            out.push(n);
            *i += 2;
            continue;
        }
        *i += 1;
        if c == d {
            return (out, true);
        }
        out.push(c);
    }
    (out, false)
}

/// Parse a command line. `Ok(None)` is an empty command (or one that is not
/// complete enough to say what it is yet).
fn parse_command(
    cmd: &str,
    ch: &[char],
    cur: usize,
    marks: Option<(usize, usize)>,
) -> Result<Option<Cmd>, String> {
    let s: Vec<char> = cmd.chars().collect();
    let nlines = ch.iter().filter(|&&c| c == '\n').count() + 1;
    let mut i = 0;
    while i < s.len() && (s[i] == ' ' || s[i] == ':') {
        i += 1;
    }
    if i >= s.len() {
        return Ok(None);
    }

    // Range: `%`, `N`, `N,M`, `.`, `$`, `'<,'>`.
    let mut range: Option<(usize, usize)> = None;
    if s[i] == '%' {
        i += 1;
        range = Some((0, nlines - 1));
    } else if let Some(a) = parse_addr(&s, &mut i, nlines, cur, marks)? {
        let mut b = a;
        if s.get(i) == Some(&',') {
            i += 1;
            b = parse_addr(&s, &mut i, nlines, cur, marks)?
                .ok_or_else(|| "E14: Invalid address".to_string())?;
        }
        range = Some((a.min(b), a.max(b)));
    }
    while i < s.len() && s[i] == ' ' {
        i += 1;
    }

    let name_start = i;
    while i < s.len() && s[i].is_ascii_alphabetic() {
        i += 1;
    }
    let name: String = s[name_start..i].iter().collect();
    if name.is_empty() {
        return Ok(match (range, s.get(i)) {
            (Some((_, b)), None) => Some(Cmd::Goto(b)),
            _ => None,
        });
    }
    if name == "w" || name == "write" {
        if range.is_some() || s[i..].iter().any(|c| !c.is_whitespace()) {
            return Err(format!("E488: Trailing characters: {}", cmd.trim()));
        }
        return Ok(Some(Cmd::Write));
    }
    if matches!(name.as_str(), "wq" | "x" | "xit" | "exit") {
        if range.is_some() || s[i..].iter().any(|c| !c.is_whitespace()) {
            return Err(format!("E488: Trailing characters: {}", cmd.trim()));
        }
        return Ok(Some(Cmd::WriteQuit));
    }
    if !"substitute".starts_with(name.as_str()) {
        return Err(format!("E492: Not an editor command: {}", cmd.trim()));
    }
    while i < s.len() && s[i] == ' ' {
        i += 1;
    }
    let Some(&d) = s.get(i) else {
        return Ok(None);
    };
    if d.is_alphanumeric() || d.is_whitespace() || matches!(d, '\\' | '"' | '|') {
        return Err("E146: Regular expressions can't be delimited by letters".to_string());
    }
    i += 1;

    let (raw_pat, closed) = take_part(&s, &mut i, d);
    let mut raw_rep = Vec::new();
    let mut flags: Vec<char> = Vec::new();
    if closed {
        let (r, closed2) = take_part(&s, &mut i, d);
        raw_rep = r;
        if closed2 {
            flags = s[i..].iter().filter(|c| !c.is_whitespace()).copied().collect();
        }
    }
    let mut global = false;
    let mut ignore_case = false;
    for f in &flags {
        match f {
            'g' => global = true,
            'i' => ignore_case = true,
            'I' => ignore_case = false,
            _ => {
                return Err(format!(
                    "E488: Trailing characters: {}",
                    flags.iter().collect::<String>()
                ))
            }
        }
    }

    // `\\` -> `\`, `\n` -> newline, any other `\x` -> x.
    let mut pat = Vec::new();
    let mut it = raw_pat.iter().copied();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => pat.push('\n'),
                Some(x) => pat.push(x),
                None => pat.push('\\'),
            }
        } else {
            pat.push(c);
        }
    }
    // `&` and `\0` are the match; `\n` / `\r` a line break; `\x` is x.
    let mut rep = Vec::new();
    let mut it = raw_rep.iter().copied();
    while let Some(c) = it.next() {
        match c {
            '&' => rep.push(Rep::Matched),
            '\\' => match it.next() {
                Some('0') => rep.push(Rep::Matched),
                Some('n') | Some('r') => rep.push(Rep::Lit('\n')),
                Some(x) => rep.push(Rep::Lit(x)),
                None => rep.push(Rep::Lit('\\')),
            },
            x => rep.push(Rep::Lit(x)),
        }
    }

    Ok(Some(Cmd::Subst(Subst {
        lines: range.unwrap_or((cur, cur)),
        pat,
        rep,
        global,
        ignore_case,
        rep_started: closed,
    })))
}

impl Input {
    fn byte_idx(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len())
    }

    pub fn len_chars(&self) -> usize {
        self.text.chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    pub fn insert_char(&mut self, c: char) {
        self.guard_edit();
        let b = self.byte_idx(self.cursor);
        self.text.insert(b, c);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        let s: String = s
            .replace('\t', "    ")
            .chars()
            .filter(|c| *c == '\n' || !c.is_control())
            .collect();
        let b = self.byte_idx(self.cursor);
        self.text.insert_str(b, &s);
        self.cursor += s.chars().count();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.guard_edit();
        // Between an auto-closed pair (`"|"`, `*|*`, `(|)`): remove both halves.
        if self.autopair && self.mode == Mode::Insert {
            let ch = self.chars();
            if let (Some(&a), Some(&b)) = (ch.get(self.cursor - 1), ch.get(self.cursor)) {
                if matches!(
                    (a, b),
                    ('(', ')') | ('[', ']') | ('{', '}') | ('"', '"') | ('\'', '\'') | ('*', '*') | ('`', '`')
                ) {
                    let start = self.byte_idx(self.cursor - 1);
                    let end = self.byte_idx(self.cursor + 1);
                    self.text.replace_range(start..end, "");
                    self.cursor -= 1;
                    return;
                }
            }
        }
        let start = self.byte_idx(self.cursor - 1);
        let end = self.byte_idx(self.cursor);
        self.text.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.len_chars() {
            return;
        }
        self.guard_edit();
        let start = self.byte_idx(self.cursor);
        let end = self.byte_idx(self.cursor + 1);
        self.text.replace_range(start..end, "");
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        if self.cursor < self.len_chars() {
            self.cursor += 1;
        }
    }

    /// Start of the current line.
    pub fn home(&mut self) {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor.min(chars.len());
        while i > 0 && chars[i - 1] != '\n' {
            i -= 1;
        }
        self.cursor = i;
    }

    /// End of the current line.
    pub fn end(&mut self) {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor.min(chars.len());
        while i < chars.len() && chars[i] != '\n' {
            i += 1;
        }
        self.cursor = i;
    }

    pub fn delete_word_back(&mut self) {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor.min(chars.len());
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        if i >= self.cursor.min(chars.len()) {
            return;
        }
        self.guard_edit();
        let start = self.byte_idx(i);
        let end = self.byte_idx(self.cursor);
        self.text.replace_range(start..end, "");
        self.cursor = i;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    // ------------------------------------------------------------ vim mode

    fn chars(&self) -> Vec<char> {
        self.text.chars().collect()
    }

    /// Forget undo history and half-typed commands (new text is being edited).
    pub fn reset_vim(&mut self) {
        self.pending = None;
        self.count.clear();
        self.cmd.clear();
        self.cmd_marks = None;
        self.undo.clear();
        self.just_snap = false;
        self.insert_base = Some((self.text.clone(), self.cursor));
        self.seq_hits = 0;
        self.seq_at = None;
    }

    /// Apply the `vim:` config: with `enabled` false the box never leaves
    /// INSERT mode; otherwise `keys` (two or more chars) typed quickly in
    /// INSERT mode switch to NORMAL mode.
    pub fn configure(&mut self, enabled: bool, keys: Vec<char>) {
        self.vim_off = !enabled;
        self.normal_keys = if keys.len() >= 2 { keys } else { vec!['q', 'q'] };
        if self.vim_off {
            self.enter_insert();
        } else if self.insert_base.is_none() {
            self.insert_base = Some((self.text.clone(), self.cursor));
        }
    }

    /// Take over the vim settings of another box (the `/character` window
    /// edits with the same modes and NORMAL-mode keys as the chat box).
    /// Was `:w` run since the last call? (Clears the flag.)
    pub fn take_write_req(&mut self) -> bool {
        std::mem::take(&mut self.write_req)
    }

    /// Was `:wq` run since the last call? (Clears the flag; `take_write_req`
    /// is true then too.)
    pub fn take_quit_req(&mut self) -> bool {
        std::mem::take(&mut self.quit_req)
    }

    pub fn copy_config(&mut self, other: &Input) {
        self.leader = other.leader;
        self.vim_off = other.vim_off;
        self.autopair = other.autopair;
        self.normal_keys = other.normal_keys.clone();
    }

    /// Turn auto-pairing of quotes, `*` and brackets on or off (config `autopair`).
    pub fn set_autopair(&mut self, on: bool) {
        self.autopair = on;
    }

    /// Set the `<leader>` key (config `vim.leader`).
    pub fn set_leader(&mut self, leader: char) {
        self.leader = Some(leader);
    }

    /// An operator or `g` / `r` was typed and waits for its next key.
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// `g` was typed and the next key finishes the command (`gg`).
    pub fn pending_g(&self) -> bool {
        self.pending == Some('g')
    }

    /// The leader key was typed and the next key finishes the mapping.
    pub fn leader_waiting(&self) -> bool {
        self.leader_wait
    }

    /// Text yanked with `<leader>y` that still has to go to the system
    /// clipboard (taken once; the caller does the copying).
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.clip.take()
    }

    /// Vim modes are on (config `vim.enabled`).
    pub fn vim_enabled(&self) -> bool {
        !self.vim_off
    }

    /// Type a character in INSERT mode. Typing the NORMAL-mode key sequence
    /// quickly removes the keys again and switches to NORMAL mode.
    pub fn type_char(&mut self, c: char) {
        // Never swallow a key that is part of the NORMAL-mode sequence.
        let is_seq_key = !self.vim_off && self.normal_keys.contains(&c);
        if self.autopair && !is_seq_key && self.try_autopair(c) {
            self.seq_hits = 0;
            self.seq_at = None;
            return;
        }
        if self.vim_off || self.normal_keys.len() < 2 {
            self.insert_char(c);
            return;
        }
        let recent = self
            .seq_at
            .map(|t| t.elapsed() < Duration::from_millis(600))
            .unwrap_or(false);
        let mut hits = if recent { self.seq_hits } else { 0 };
        // The keys typed so far must still be right before the cursor
        // (it may have been moved since).
        if hits > 0 {
            let ch = self.chars();
            let ok = self.cursor >= hits
                && self.cursor <= ch.len()
                && ch[self.cursor - hits..self.cursor] == self.normal_keys[..hits];
            if !ok {
                hits = 0;
            }
        }
        if hits > 0 && self.normal_keys[hits] == c {
            if hits + 1 == self.normal_keys.len() {
                for _ in 0..hits {
                    self.backspace();
                }
                self.enter_normal();
            } else {
                self.insert_char(c);
                self.seq_hits = hits + 1;
                self.seq_at = Some(Instant::now());
            }
            return;
        }
        self.insert_char(c);
        if self.normal_keys[0] == c {
            self.seq_hits = 1;
            self.seq_at = Some(Instant::now());
        } else {
            self.seq_hits = 0;
            self.seq_at = None;
        }
    }

    /// Auto-pairing for one typed character. Returns `true` when it handled
    /// the key (inserted a pair, or stepped over an existing closer); `false`
    /// means "insert the character normally".
    ///
    /// - `(` `[` `{` insert the pair when what follows is blank, the end or a closer.
    /// - `)` `]` `}` step over the matching closer that is already there.
    /// - `"`, `'`, `*` and `` ` `` open a pair, close an open one (step over it), or stay
    ///   single next to a word (`2*3`, `5"`, `don't`) or when the paragraph
    ///   already has an unclosed one. `*|*` followed by `*` becomes `**|**` (bold).
    ///   For `'`, an apostrophe inside a word (`don't`) does not count as a quote.
    fn try_autopair(&mut self, c: char) -> bool {
        let ch = self.chars();
        let cur = self.cursor.min(ch.len());
        let prev = cur.checked_sub(1).and_then(|i| ch.get(i)).copied();
        let next = ch.get(cur).copied();
        let blank_or_closer = |n: Option<char>, extra: &str| match n {
            None => true,
            Some(n) => n.is_whitespace() || ")]}".contains(n) || extra.contains(n),
        };
        let insert_pair = |me: &mut Self, open: &str, close: &str| {
            let b = me.byte_idx(me.cursor);
            me.text.insert_str(b, &format!("{}{}", open, close));
            me.cursor += open.chars().count();
        };
        match c {
            '(' | '[' | '{' => {
                if !blank_or_closer(next, "\"'*.,;:!?") {
                    return false;
                }
                let close = match c {
                    '(' => ")",
                    '[' => "]",
                    _ => "}",
                };
                insert_pair(self, &c.to_string(), close);
                true
            }
            ')' | ']' | '}' => {
                if next == Some(c) {
                    self.cursor += 1;
                    true
                } else {
                    false
                }
            }
            '"' | '\'' | '*' | '`' => {
                // `` ` `` right after a backtick is a longer run (``` ``code`` ```):
                // leave it single.
                if c == '`' && prev == Some('`') && next != Some('`') {
                    return false;
                }
                // `*|*` + `*` -> `**|**`
                if c == '*' && prev == Some('*') && next == Some('*') {
                    let before = cur.checked_sub(2).and_then(|i| ch.get(i)).copied();
                    if before != Some('*')
                        && !before.map(|b| b.is_alphanumeric()).unwrap_or(false)
                        && ch.get(cur + 1).copied() != Some('*')
                    {
                        insert_pair(self, "*", "*");
                        return true;
                    }
                }
                // Closing: step over the one that is already there.
                if next == Some(c) {
                    self.cursor += 1;
                    return true;
                }
                // An unclosed one earlier in this paragraph: this is the closer.
                let para_start = ch[..cur]
                    .windows(2)
                    .rposition(|w| w == ['\n', '\n'])
                    .map(|i| i + 2)
                    .unwrap_or(0);
                let open = ch[para_start..cur]
                    .iter()
                    .enumerate()
                    .filter(|(i, x)| {
                        if **x != c {
                            return false;
                        }
                        // An apostrophe inside a word (`don't`) is not a quote.
                        if c == '\'' {
                            let at = para_start + i;
                            let inside = at > 0
                                && ch[at - 1].is_alphanumeric()
                                && ch.get(at + 1).map(|n| n.is_alphanumeric()).unwrap_or(false);
                            return !inside;
                        }
                        true
                    })
                    .count();
                if open % 2 == 1 {
                    return false;
                }
                if prev.map(|p| p.is_alphanumeric()).unwrap_or(false) {
                    return false;
                }
                if !blank_or_closer(next, "\"'*.,;:!?") {
                    return false;
                }
                insert_pair(self, &c.to_string(), &c.to_string());
                true
            }
            _ => false,
        }
    }

    /// Cursor one word left / right (Alt+Left / Alt+Right).
    pub fn word_left(&mut self, width: usize) {
        self.motion('b', 1, width);
    }

    pub fn word_right(&mut self, width: usize) {
        self.motion('w', 1, width);
    }

    pub fn enter_insert(&mut self) {
        self.insert_base = if self.just_snap {
            None
        } else {
            Some((self.text.clone(), self.cursor))
        };
        self.just_snap = false;
        self.mode = Mode::Insert;
        self.pending = None;
        self.leader_wait = false;
        self.sys_yank = false;
        self.count.clear();
        self.cmd.clear();
        self.cmd_marks = None;
    }

    /// Back to NORMAL mode; the cursor steps left like in vim.
    pub fn enter_normal(&mut self) {
        if self.vim_off {
            return;
        }
        // The INSERT session becomes one undo step.
        self.flush_base();
        let ch = self.chars();
        if self.mode == Mode::Insert && self.cursor > 0 && ch.get(self.cursor - 1) != Some(&'\n') {
            self.cursor -= 1;
        }
        self.mode = Mode::Normal;
        self.pending = None;
        self.count.clear();
        self.seq_hits = 0;
        self.seq_at = None;
        self.clamp_normal();
    }

    /// In NORMAL mode the cursor sits on a character, never past the end.
    pub fn clamp_normal(&mut self) {
        if self.mode != Mode::Normal && self.mode != Mode::Visual {
            return;
        }
        let ch = self.chars();
        if ch.is_empty() {
            self.cursor = 0;
            return;
        }
        if self.cursor >= ch.len() {
            self.cursor = ch.len() - 1;
        }
        if ch[self.cursor] == '\n' && self.cursor > 0 && ch[self.cursor - 1] != '\n' {
            self.cursor -= 1;
        }
    }

    /// Esc in NORMAL mode: drop a half-typed command.
    pub fn cancel_pending(&mut self) {
        self.pending = None;
        self.leader_wait = false;
        self.sys_yank = false;
        self.count.clear();
    }

    /// Put a state on the undo stack (unless it is the one on top already).
    fn push_undo(&mut self, text: String, cursor: usize) {
        if self.undo.last().map(|(t, _)| t != &text).unwrap_or(true) {
            self.undo.push((text, cursor));
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
        }
    }

    /// The INSERT session that is running becomes one undo step: the text it
    /// started from goes on the stack (if anything was typed since). Does
    /// nothing outside INSERT mode.
    fn flush_base(&mut self) {
        if self.mode != Mode::Insert {
            return;
        }
        if let Some((t, cu)) = self.insert_base.take() {
            if t != self.text {
                self.push_undo(t, cu);
            }
        }
    }

    fn snapshot(&mut self) {
        self.just_snap = true;
        self.push_undo(self.text.clone(), self.cursor);
    }

    /// A key that edits the text outside of INSERT mode (Backspace, Delete,
    /// Ctrl+W, Ctrl+J in NORMAL / VISUAL mode) is an undo step of its own.
    /// Called by the editing methods just before they change something.
    fn guard_edit(&mut self) {
        if !self.vim_off && matches!(self.mode, Mode::Normal | Mode::Visual) {
            self.push_undo(self.text.clone(), self.cursor);
        }
    }

    /// Start a new undo step here: what was typed so far stays one step, and
    /// the change that follows (and, in INSERT mode, what is typed after it)
    /// becomes the next one. Used for edits made by the program, not by
    /// vim keys (a spelling fix, a paste, Ctrl+U).
    pub fn checkpoint(&mut self) {
        if self.vim_off {
            return;
        }
        self.flush_base();
        self.push_undo(self.text.clone(), self.cursor);
    }

    /// Call after a `checkpoint` and the change that followed it: INSERT
    /// mode goes on from here as a new session.
    fn begin_step(&mut self) {
        if !self.vim_off && self.mode == Mode::Insert {
            self.insert_base = Some((self.text.clone(), self.cursor));
        }
    }

    /// Replace the characters `start..end` with `new` as one undo step. The
    /// cursor ends up right after the new text. (The spelling menu uses it.)
    pub fn replace_chars(&mut self, start: usize, end: usize, new: &str) {
        let len = self.len_chars();
        let (s, e) = (start.min(len), end.min(len));
        let (s, e) = (s.min(e), s.max(e));
        self.checkpoint();
        self.remove(s, e);
        self.insert_at(s, new);
        self.cursor = s + new.chars().count();
        self.begin_step();
    }

    /// Insert pasted text at the cursor as one undo step.
    pub fn paste_text(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.checkpoint();
        self.insert_str(s);
        self.begin_step();
    }

    /// Empty the box (Ctrl+U) as one undo step, so `u` brings the text back.
    pub fn clear_undoable(&mut self) {
        if self.text.is_empty() {
            return;
        }
        self.checkpoint();
        self.clear();
        self.begin_step();
    }

    /// Take the undo history out of the box (the `/char` window keeps one
    /// per field). The INSERT session that is running is closed first.
    pub fn take_undo(&mut self) -> Vec<(String, usize)> {
        self.flush_base();
        std::mem::take(&mut self.undo)
    }

    /// Put an undo history back (after `reset_vim`, which empties it).
    pub fn set_undo(&mut self, undo: Vec<(String, usize)>) {
        self.undo = undo;
    }

    /// `u`: go back `n` steps. Entries that would change nothing (an
    /// operator that did not delete anything, `r` with the same letter) are
    /// skipped, so every `u` visibly undoes something while there is
    /// something to undo.
    fn undo_steps(&mut self, n: usize) {
        for _ in 0..n {
            let mut done = false;
            while let Some((t, cu)) = self.undo.pop() {
                if t != self.text {
                    self.text = t;
                    self.cursor = cu;
                    done = true;
                    break;
                }
            }
            if !done {
                break;
            }
        }
        self.clamp_normal();
    }

    fn remove(&mut self, s: usize, e: usize) -> String {
        let bs = self.byte_idx(s);
        let be = self.byte_idx(e);
        let removed = self.text[bs..be].to_string();
        self.text.replace_range(bs..be, "");
        removed
    }

    fn insert_at(&mut self, at: usize, s: &str) {
        let b = self.byte_idx(at);
        self.text.insert_str(b, s);
    }

    fn paste(&mut self, after: bool) {
        if self.reg.is_empty() && !self.reg_lines {
            return;
        }
        self.snapshot();
        let ch = self.chars();
        let reg = self.reg.clone();
        if self.reg_lines {
            if after {
                let le = line_end(&ch, self.cursor);
                self.insert_at(le, &format!("\n{}", reg));
                self.cursor = le + 1;
            } else {
                let ls = line_start(&ch, self.cursor);
                self.insert_at(ls, &format!("{}\n", reg));
                self.cursor = ls;
            }
        } else {
            let at = if after && self.cursor < ch.len() && ch[self.cursor] != '\n' {
                self.cursor + 1
            } else {
                self.cursor
            };
            self.insert_at(at, &reg);
            self.cursor = at + reg.chars().count().saturating_sub(1);
        }
    }

    /// Cursor motions shared by NORMAL and VISUAL mode. Returns false when
    /// `c` is not a motion.
    fn motion(&mut self, c: char, n: usize, width: usize) -> bool {
        let ch = self.chars();
        let len = ch.len();
        let cur = self.cursor.min(len);
        match c {
            'h' => {
                for _ in 0..n {
                    if self.cursor > 0 && ch[self.cursor - 1] != '\n' {
                        self.cursor -= 1;
                    }
                }
            }
            'l' => {
                for _ in 0..n {
                    if self.cursor + 1 < len && ch[self.cursor + 1] != '\n' {
                        self.cursor += 1;
                    }
                }
            }
            'j' | 'k' => {
                let dir = if c == 'j' { 1 } else { -1 };
                let whole_lines = self.mode == Mode::Visual && self.vis_line;
                for _ in 0..n {
                    self.cursor = if whole_lines {
                        // `V` selects whole lines, and as in vim `j` / `k`
                        // there go to the next / previous line of the text,
                        // however many rows a long line is wrapped over.
                        line_vertical(&ch, self.cursor, dir)
                    } else {
                        crate::ui::vertical_target(&self.text, self.cursor, width, dir)
                    };
                }
            }
            'w' | 'W' => {
                for _ in 0..n {
                    self.cursor = word_fwd(&ch, self.cursor, c == 'W');
                }
            }
            'b' | 'B' => {
                for _ in 0..n {
                    self.cursor = word_back(&ch, self.cursor, c == 'B');
                }
            }
            'e' | 'E' => {
                for _ in 0..n {
                    self.cursor = word_end(&ch, self.cursor, c == 'E');
                }
            }
            '0' => self.cursor = line_start(&ch, cur),
            '^' => self.cursor = first_nonblank(&ch, cur),
            '$' => {
                let e = line_end(&ch, cur);
                self.cursor = if e > line_start(&ch, cur) { e - 1 } else { e };
            }
            'G' => self.cursor = first_nonblank(&ch, line_start(&ch, len)),
            _ => return false,
        }
        true
    }


    // ----------------------------------------------------- command line (:)

    /// `:` in NORMAL mode, or in VISUAL mode (then the command starts with
    /// `'<,'>`, the selected lines).
    fn enter_command(&mut self, from_visual: bool) {
        if self.vim_off {
            return;
        }
        let ch = self.chars();
        self.cmd.clear();
        self.cmd_marks = None;
        if from_visual {
            let a = self.anchor.min(ch.len());
            let b = self.cursor.min(ch.len());
            let (lo, hi) = (a.min(b), a.max(b));
            self.cmd_marks = Some((line_no(&ch, lo), line_no(&ch, hi)));
            self.cmd = "'<,'>".to_string();
            self.cursor = lo;
        }
        self.mode = Mode::Command;
        self.pending = None;
        self.count.clear();
    }

    /// The text after the `:` (shown on the status line while typing).
    pub fn command_text(&self) -> &str {
        &self.cmd
    }

    /// Esc: leave the command line without running anything.
    pub fn cancel_command(&mut self) {
        if self.mode == Mode::Command {
            self.mode = Mode::Normal;
            self.cmd.clear();
            self.cmd_marks = None;
            self.clamp_normal();
        }
    }

    pub fn command_push(&mut self, c: char) {
        if !c.is_control() {
            self.cmd.push(c);
        }
    }

    /// Pasted text goes to the command line (first line only).
    pub fn command_paste(&mut self, s: &str) {
        for c in s.lines().next().unwrap_or("").chars() {
            self.command_push(c);
        }
    }

    /// Backspace; on an empty command line it leaves COMMAND mode (like vim).
    pub fn command_backspace(&mut self) {
        if self.cmd.is_empty() {
            self.cancel_command();
        } else {
            self.cmd.pop();
        }
    }

    /// Ctrl+U: clear the command line.
    pub fn command_clear(&mut self) {
        self.cmd.clear();
    }

    /// Ctrl+W: delete the word before the end of the command line.
    pub fn command_delete_word(&mut self) {
        while self.cmd.ends_with(' ') {
            self.cmd.pop();
        }
        while !self.cmd.is_empty() && !self.cmd.ends_with(' ') {
            self.cmd.pop();
        }
    }

    /// What the command line would find right now, for the live highlight:
    /// `Some(ranges)` (half-open, in chars, maybe none) while a `:s` pattern
    /// is being typed, `None` for anything else.
    #[cfg(test)]
    pub fn command_preview(&self) -> Option<Vec<(usize, usize)>> {
        if self.mode != Mode::Command {
            return None;
        }
        let ch = self.chars();
        let cur = line_no(&ch, self.cursor.min(ch.len()));
        match parse_command(&self.cmd, &ch, cur, self.cmd_marks) {
            Ok(Some(Cmd::Subst(sub))) if !sub.pat.is_empty() => Some(sub.find(&ch)),
            _ => None,
        }
    }

    /// Live view of a `:s` being typed: the matches, and (once the
    /// replacement part has begun) the text as it would look afterwards.
    pub fn command_live(&self) -> Option<Live> {
        if self.mode != Mode::Command {
            return None;
        }
        let ch = self.chars();
        let cur = line_no(&ch, self.cursor.min(ch.len()));
        let Ok(Some(Cmd::Subst(sub))) = parse_command(&self.cmd, &ch, cur, self.cmd_marks) else {
            return None;
        };
        if sub.pat.is_empty() {
            return None;
        }
        match sub.apply(&ch) {
            None => Some(Live {
                text: None,
                marks: Vec::new(),
                count: 0,
            }),
            Some(done) if sub.rep_started => Some(Live {
                text: Some(done.text.iter().collect()),
                marks: done.spans,
                count: done.count,
            }),
            Some(done) => Some(Live {
                text: None,
                marks: sub.find(&ch),
                count: done.count,
            }),
        }
    }

    /// Enter: run the command line and go back to NORMAL mode. `Ok` carries
    /// a message to show (maybe empty), `Err` an error message.
    pub fn run_command(&mut self) -> Result<String, String> {
        let cmd = std::mem::take(&mut self.cmd);
        let marks = self.cmd_marks.take();
        self.mode = Mode::Normal;
        let res = self.run_command_inner(&cmd, marks);
        self.clamp_normal();
        res
    }

    fn run_command_inner(
        &mut self,
        cmd: &str,
        marks: Option<(usize, usize)>,
    ) -> Result<String, String> {
        let ch = self.chars();
        let cur = line_no(&ch, self.cursor.min(ch.len()));
        let Some(parsed) = parse_command(cmd, &ch, cur, marks)? else {
            return Ok(String::new());
        };
        match parsed {
            Cmd::Goto(l) => {
                let (s, _) = line_bounds(&ch, l);
                self.cursor = first_nonblank(&ch, s);
                Ok(String::new())
            }
            Cmd::Write => {
                self.write_req = true;
                Ok(String::new())
            }
            Cmd::WriteQuit => {
                self.write_req = true;
                self.quit_req = true;
                Ok(String::new())
            }
            Cmd::Subst(sub) => {
                if sub.pat.is_empty() {
                    return Err("E35: No previous regular expression".to_string());
                }
                let Some(done) = sub.apply(&ch) else {
                    let p: String = sub.pat.iter().collect();
                    return Err(format!("E486: Pattern not found: {}", p.replace('\n', "\\n")));
                };
                self.snapshot();
                self.text = done.text.iter().collect();
                self.cursor = first_nonblank(&done.text, done.last_at);
                let (n, lines) = (done.count, done.lines);
                Ok(format!(
                    "{} substitution{} on {} line{}",
                    n,
                    if n == 1 { "" } else { "s" },
                    lines,
                    if lines == 1 { "" } else { "s" }
                ))
            }
        }
    }

    // ---------------------------------------------------------- visual mode

    fn enter_visual(&mut self, linewise: bool) {
        if self.vim_off {
            return;
        }
        self.mode = Mode::Visual;
        self.vis_line = linewise;
        self.anchor = self.cursor;
        self.pending = None;
        self.count.clear();
    }

    /// Leave VISUAL mode (back to NORMAL).
    pub fn exit_visual(&mut self) {
        if self.mode == Mode::Visual {
            self.mode = Mode::Normal;
            self.pending = None;
            self.count.clear();
            self.clamp_normal();
        }
    }

    /// `true` while VISUAL mode selects whole lines (`V`).
    pub fn visual_linewise(&self) -> bool {
        self.mode == Mode::Visual && self.vis_line
    }

    /// Start and end (the newline, or text end) of the lines the selection touches.
    fn visual_lines(&self, ch: &[char]) -> (usize, usize) {
        let len = ch.len();
        let a = self.anchor.min(len);
        let b = self.cursor.min(len);
        (line_start(ch, a.min(b)), line_end(ch, a.max(b)))
    }

    /// The selection as a half-open char range, and whether it is linewise.
    fn visual_bounds(&self, ch: &[char]) -> (usize, usize, bool) {
        if self.vis_line {
            let (ls, le) = self.visual_lines(ch);
            return (ls, le, true);
        }
        let len = ch.len();
        let a = self.anchor.min(len);
        let b = self.cursor.min(len);
        let lo = a.min(b);
        let hi = a.max(b);
        (lo, (hi + 1).min(len), false)
    }

    /// Range to highlight while in VISUAL mode (half-open, in chars).
    pub fn selection(&self) -> Option<(usize, usize)> {
        if self.mode != Mode::Visual {
            return None;
        }
        let ch = self.chars();
        let (s, e, lines) = self.visual_bounds(&ch);
        // A linewise selection also covers the newline that ends it.
        let e = if lines && e < ch.len() { e + 1 } else { e };
        Some((s, e))
    }

    /// Remove whole lines `ls..le` (like `dd`) and put them in the register.
    fn delete_lines(&mut self, ch: &[char], ls: usize, le: usize) {
        let len = ch.len();
        self.reg = ch[ls..le].iter().collect();
        self.reg_lines = true;
        let (s, e) = if le < len {
            (ls, le + 1)
        } else if ls > 0 {
            (ls - 1, le)
        } else {
            (0, le)
        };
        self.remove(s, e);
        let ch2 = self.chars();
        self.cursor = first_nonblank(&ch2, s.min(ch2.len()));
    }

    fn visual_key_inner(&mut self, c: char, width: usize) {
        if let Some(p) = self.pending.take() {
            if p == 'g' && c == 'g' {
                let ch = self.chars();
                self.cursor = first_nonblank(&ch, 0);
            } else if p == 'i' || p == 'a' {
                // `vi"`, `vaw`, `va(`: select the text object.
                let ch = self.chars();
                let cur = self.cursor.min(ch.len());
                if let Some((s, e)) = text_object(&ch, cur, c, p == 'a') {
                    if e > s {
                        self.anchor = s;
                        self.cursor = e - 1;
                    }
                }
            }
            return;
        }
        if c.is_ascii_digit() && (c != '0' || !self.count.is_empty()) {
            self.count.push(c);
            return;
        }
        // `<leader>y` / `<leader>Y`: yank the selection to the system clipboard.
        let mut sys = false;
        if self.leader_wait {
            self.leader_wait = false;
            if c != 'y' && c != 'Y' {
                return;
            }
            sys = true;
        } else if self.leader == Some(c) {
            self.leader_wait = true;
            return;
        }
        let n = self.count.parse::<usize>().unwrap_or(1).clamp(1, 10_000);
        self.count.clear();
        if self.motion(c, n, width) {
            return;
        }

        let ch = self.chars();
        let (s, e, was_lines) = self.visual_bounds(&ch);
        let (ls, le) = self.visual_lines(&ch);
        match c {
            'g' => {
                self.pending = Some('g');
                self.pend_n = n;
            }
            'v' => {
                if self.vis_line {
                    self.vis_line = false;
                } else {
                    self.exit_visual();
                }
            }
            'V' => {
                if self.vis_line {
                    self.exit_visual();
                } else {
                    self.vis_line = true;
                }
            }
            'o' | 'O' => std::mem::swap(&mut self.anchor, &mut self.cursor),
            'i' | 'a' => {
                self.pending = Some(c);
                self.pend_n = n;
            }
            ':' => self.enter_command(true),
            'y' | 'Y' => {
                let lines = was_lines || c == 'Y';
                let (a, b) = if lines { (ls, le) } else { (s, e) };
                self.reg = ch[a..b].iter().collect();
                self.reg_lines = lines;
                self.cursor = a;
                self.exit_visual();
                if sys {
                    self.queue_clipboard();
                }
            }
            'd' | 'x' | 'D' | 'X' => {
                let lines = was_lines || c == 'D' || c == 'X';
                self.snapshot();
                if lines {
                    self.delete_lines(&ch, ls, le);
                } else {
                    self.reg = ch[s..e].iter().collect();
                    self.reg_lines = false;
                    self.remove(s, e);
                    self.cursor = s;
                }
                self.exit_visual();
            }
            'c' | 's' | 'C' | 'S' | 'R' => {
                let lines = was_lines || matches!(c, 'C' | 'S' | 'R');
                let (a, b) = if lines { (ls, le) } else { (s, e) };
                self.snapshot();
                self.reg = ch[a..b].iter().collect();
                self.reg_lines = lines;
                self.remove(a, b);
                self.cursor = a;
                self.enter_insert();
            }
            'p' | 'P' => {
                if self.reg.is_empty() && !self.reg_lines {
                    return;
                }
                let reg = self.reg.clone();
                let reg_lines = self.reg_lines;
                let old: String = ch[s..e].iter().collect();
                self.snapshot();
                self.remove(s, e);
                // Linewise text pasted over a character selection gets its own line.
                let wrap = reg_lines && !was_lines;
                let text = if wrap { format!("\n{}\n", reg) } else { reg };
                self.insert_at(s, &text);
                self.cursor = if wrap { s + 1 } else { s };
                if c == 'p' {
                    self.reg = old;
                    self.reg_lines = was_lines;
                }
                self.exit_visual();
            }
            _ => {}
        }
    }

    /// `d`/`c`/`y` followed by `c2`: `dd`, `cw`, `y$`, ...
    fn operator(&mut self, p: char, c2: char, n: usize) {
        let ch = self.chars();
        let len = ch.len();
        let cur = self.cursor.min(len);

        if c2 == p {
            // Whole line(s).
            let ls = line_start(&ch, cur);
            let mut le = line_end(&ch, cur);
            for _ in 1..n {
                if le < len {
                    le = line_end(&ch, le + 1);
                }
            }
            self.reg = ch[ls..le].iter().collect();
            self.reg_lines = true;
            self.yanked = p == 'y';
            match p {
                'd' => {
                    self.snapshot();
                    let (s, e) = if le < len {
                        (ls, le + 1)
                    } else if ls > 0 {
                        (ls - 1, le)
                    } else {
                        (0, le)
                    };
                    self.remove(s, e);
                    let ch2 = self.chars();
                    self.cursor = first_nonblank(&ch2, s.min(ch2.len()));
                }
                'c' => {
                    self.snapshot();
                    self.remove(ls, le);
                    self.cursor = ls;
                    self.enter_insert();
                }
                _ => {}
            }
            return;
        }

        let range: Option<(usize, usize)> = match c2 {
            'w' | 'W' if p == 'c' => {
                let big = c2 == 'W';
                let mut t = cur;
                for i in 0..n {
                    let at_word_end = cur < len
                        && cls(ch[t], big) != 0
                        && (t + 1 >= len || cls(ch[t + 1], big) != cls(ch[t], big));
                    if i == 0 && at_word_end {
                        continue;
                    }
                    t = word_end(&ch, t, big);
                }
                Some((cur, (t + 1).min(len)))
            }
            'w' | 'W' => {
                let big = c2 == 'W';
                let mut t = cur;
                for _ in 0..n {
                    t = word_fwd(&ch, t, big);
                }
                // `dw` never joins lines.
                if let Some(nl) = ch[cur..].iter().position(|&x| x == '\n') {
                    t = t.min(cur + nl);
                }
                Some((cur, t))
            }
            'e' | 'E' => {
                let big = c2 == 'E';
                let mut t = cur;
                for _ in 0..n {
                    t = word_end(&ch, t, big);
                }
                Some((cur, (t + 1).min(len)))
            }
            'b' | 'B' => {
                let big = c2 == 'B';
                let mut t = cur;
                for _ in 0..n {
                    t = word_back(&ch, t, big);
                }
                Some((t, cur))
            }
            '$' => Some((cur, line_end(&ch, cur))),
            '0' => Some((line_start(&ch, cur), cur)),
            '^' => {
                let f = first_nonblank(&ch, cur);
                Some((f.min(cur), f.max(cur)))
            }
            'h' => {
                let mut s = cur;
                for _ in 0..n {
                    if s > 0 && ch[s - 1] != '\n' {
                        s -= 1;
                    }
                }
                Some((s, cur))
            }
            'l' => {
                let mut e = cur;
                for _ in 0..n {
                    if e < len && ch[e] != '\n' {
                        e += 1;
                    }
                }
                Some((cur, e))
            }
            _ => None,
        };
        let Some((s, e)) = range else { return };
        self.apply_range(p, &ch, s, e);
    }

    /// `d` / `c` / `y` + a text object (`diw`, `ci"`, `ya(`, ...).
    fn operator_obj(&mut self, p: char, around: bool, key: char) {
        let ch = self.chars();
        let cur = self.cursor.min(ch.len());
        let Some((s, e)) = text_object(&ch, cur, key, around) else {
            return;
        };
        self.apply_range(p, &ch, s, e);
    }

    /// Run operator `p` (`d`, `c` or `y`) on the chars `s..e`.
    fn apply_range(&mut self, p: char, ch: &[char], s: usize, e: usize) {
        if s >= e {
            if p == 'c' {
                self.snapshot();
                self.cursor = s.min(ch.len());
                self.enter_insert();
            }
            return;
        }
        self.reg = ch[s..e].iter().collect();
        self.reg_lines = false;
        self.yanked = p == 'y';
        match p {
            'y' => self.cursor = s,
            'd' => {
                self.snapshot();
                self.remove(s, e);
                self.cursor = s;
            }
            _ => {
                self.snapshot();
                self.remove(s, e);
                self.cursor = s;
                self.enter_insert();
            }
        }
    }

    /// Queue the register for the system clipboard. A linewise yank ends in a
    /// newline, like vim's `"+yy`.
    fn queue_clipboard(&mut self) {
        let mut t = self.reg.clone();
        if self.reg_lines {
            t.push('\n');
        }
        self.clip = Some(t);
    }

    /// Ctrl+U / Ctrl+D: move the cursor `rows` display rows up (`dir < 0`)
    /// or down, stopping at the first / last row. Returns how many rows it
    /// moved. Works in NORMAL and VISUAL mode (the selection follows).
    pub fn move_rows(&mut self, dir: i32, rows: usize, width: usize) -> usize {
        let mut moved = 0;
        for _ in 0..rows {
            let (lines, (row, _), _) = crate::ui::layout_input(&self.text, self.cursor, width);
            if (dir < 0 && row == 0) || (dir > 0 && row + 1 >= lines.len()) {
                break;
            }
            self.cursor = crate::ui::vertical_target(&self.text, self.cursor, width, dir);
            moved += 1;
        }
        self.pending = None;
        self.count.clear();
        self.clamp_normal();
        moved
    }

    /// One key typed in NORMAL or VISUAL mode. `width` is the input box
    /// width, used so `j`/`k` follow wrapped rows.
    pub fn normal_key(&mut self, c: char, width: usize) {
        self.just_snap = false;
        if self.mode == Mode::Visual {
            self.visual_key_inner(c, width);
        } else {
            self.normal_key_inner(c, width);
        }
        self.clamp_normal();
    }

    fn normal_key_inner(&mut self, c: char, width: usize) {
        let ch = self.chars();
        let len = ch.len();

        if let Some(p) = self.pending.take() {
            let n = self.pend_n.max(1);
            match p {
                'r' => {
                    if self.cursor < len && ch[self.cursor] != '\n' {
                        self.snapshot();
                        let at = self.cursor;
                        self.remove(at, at + 1);
                        self.insert_at(at, &c.to_string());
                        self.cursor = at;
                    }
                }
                'g' => {
                    if c == 'g' {
                        self.cursor = first_nonblank(&ch, 0);
                    }
                }
                // `di"`, `caw`, `ya(`: the text object's key.
                'I' | 'A' => {
                    self.yanked = false;
                    let op = self.obj_op;
                    self.operator_obj(op, p == 'A', c);
                    if op == 'y' && self.sys_yank && self.yanked {
                        self.queue_clipboard();
                    }
                }
                _ => {
                    // `i` / `a` after an operator start a text object.
                    if (c == 'i' || c == 'a') && matches!(p, 'd' | 'c' | 'y') {
                        self.obj_op = p;
                        self.pending = Some(if c == 'i' { 'I' } else { 'A' });
                        self.pend_n = n;
                        return;
                    }
                    self.yanked = false;
                    self.operator(p, c, n);
                    if p == 'y' && self.sys_yank && self.yanked {
                        self.queue_clipboard();
                    }
                }
            }
            self.sys_yank = false;
            self.yanked = false;
            return;
        }

        // `<leader>y{motion}` / `<leader>yy` / `<leader>Y`: yank to the system clipboard.
        if self.leader_wait {
            self.leader_wait = false;
            let n = self.count.parse::<usize>().unwrap_or(1).clamp(1, 10_000);
            self.count.clear();
            match c {
                'y' => {
                    self.pending = Some('y');
                    self.pend_n = n;
                    self.sys_yank = true;
                }
                'Y' => {
                    self.yanked = false;
                    self.operator('y', 'y', n);
                    if self.yanked {
                        self.queue_clipboard();
                    }
                    self.yanked = false;
                }
                _ => {}
            }
            return;
        }
        if self.leader == Some(c) {
            self.leader_wait = true;
            return;
        }

        if c.is_ascii_digit() && (c != '0' || !self.count.is_empty()) {
            self.count.push(c);
            return;
        }
        let n = self.count.parse::<usize>().unwrap_or(1).clamp(1, 10_000);
        self.count.clear();
        let cur = self.cursor.min(len);

        if self.motion(c, n, width) {
            return;
        }

        match c {
            'x' | 'X' => {
                let (s, e) = if c == 'x' {
                    (cur, (cur + n).min(line_end(&ch, cur)))
                } else {
                    let mut s = cur;
                    for _ in 0..n {
                        if s > 0 && ch[s - 1] != '\n' {
                            s -= 1;
                        }
                    }
                    (s, cur)
                };
                if s < e {
                    self.snapshot();
                    self.reg = self.remove(s, e);
                    self.reg_lines = false;
                    self.cursor = s;
                }
            }
            'Y' => self.operator('y', 'y', n),
            'D' => self.operator('d', '$', 1),
            'C' => self.operator('c', '$', 1),
            's' => self.operator('c', 'l', n),
            'S' => self.operator('c', 'c', 1),
            'i' => {
                self.snapshot();
                self.enter_insert();
            }
            'a' => {
                self.snapshot();
                if cur < len && ch[cur] != '\n' {
                    self.cursor = cur + 1;
                }
                self.enter_insert();
            }
            'I' => {
                self.snapshot();
                self.cursor = first_nonblank(&ch, cur);
                self.enter_insert();
            }
            'A' => {
                self.snapshot();
                self.cursor = line_end(&ch, cur);
                self.enter_insert();
            }
            'o' => {
                self.snapshot();
                let le = line_end(&ch, cur);
                self.insert_at(le, "\n");
                self.cursor = le + 1;
                self.enter_insert();
            }
            'O' => {
                self.snapshot();
                let ls = line_start(&ch, cur);
                self.insert_at(ls, "\n");
                self.cursor = ls;
                self.enter_insert();
            }
            'v' => self.enter_visual(false),
            'V' => self.enter_visual(true),
            ':' => self.enter_command(false),
            'p' => self.paste(true),
            'P' => self.paste(false),
            'u' => self.undo_steps(n),
            'd' | 'c' | 'y' | 'g' | 'r' => {
                self.pending = Some(c);
                self.pend_n = n;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vim(text: &str) -> Input {
        let mut i = Input::default();
        i.insert_str(text);
        i.enter_normal();
        i
    }

    fn type_cmd(i: &mut Input, cmd: &str) {
        i.normal_key(':', 40);
        assert_eq!(i.mode, Mode::Command);
        for c in cmd.chars() {
            i.command_push(c);
        }
    }

    fn run(text: &str, cmd: &str) -> (String, Result<String, String>) {
        let mut i = vim(text);
        type_cmd(&mut i, cmd);
        let r = i.run_command();
        assert_eq!(i.mode, Mode::Normal);
        (i.text.clone(), r)
    }

    fn keys(i: &mut Input, ks: &str) {
        for c in ks.chars() {
            i.normal_key(c, 80);
        }
    }

    fn at(text: &str, cursor: usize) -> Input {
        let mut i = vim(text);
        i.cursor = cursor;
        i
    }

    #[test]
    fn inner_and_around_quotes() {
        let mut i = at("say \"hello world\" ok", 8);
        keys(&mut i, "ci\"");
        assert_eq!(i.text, "say \"\" ok");
        assert_eq!(i.mode, Mode::Insert);
        assert_eq!(i.cursor, 5);
        let mut i = at("say \"hello\" ok", 7);
        keys(&mut i, "da\"");
        assert_eq!(i.text, "say ok");
        let mut i = at("say \"hello\"", 7);
        keys(&mut i, "da\"");
        assert_eq!(i.text, "say");
    }

    #[test]
    fn quote_object_finds_next_pair_and_handles_empty() {
        let mut i = at("go \"a\" and \"b\"", 0);
        keys(&mut i, "di\"");
        assert_eq!(i.text, "go \"\" and \"b\"");
        let mut i = at("x \"\" y", 2);
        keys(&mut i, "ci\"");
        assert_eq!(i.cursor, 3);
        assert_eq!(i.mode, Mode::Insert);
    }

    #[test]
    fn narration_object() {
        let mut i = at("\"hi *you* there\"", 6);
        keys(&mut i, "ci*");
        assert_eq!(i.text, "\"hi ** there\"");
        let mut i = at("*waves* \"hi\"", 2);
        keys(&mut i, "da*");
        assert_eq!(i.text, "\"hi\"");
        // `2 * 3` is not narration.
        let mut i = at("2 * 3 *x*", 8);
        keys(&mut i, "di*");
        assert_eq!(i.text, "2 * 3 **");
    }

    #[test]
    fn word_objects() {
        let mut i = at("one two three", 5);
        keys(&mut i, "diw");
        assert_eq!(i.text, "one  three");
        let mut i = at("one two three", 5);
        keys(&mut i, "daw");
        assert_eq!(i.text, "one three");
        let mut i = at("one two", 5);
        keys(&mut i, "daw");
        assert_eq!(i.text, "one");
        let mut i = at("foo.bar baz", 1);
        keys(&mut i, "ciW");
        assert_eq!(i.text, " baz");
    }

    #[test]
    fn bracket_objects_nest() {
        let mut i = at("f(a, (b), c) z", 4);
        keys(&mut i, "di(");
        assert_eq!(i.text, "f() z");
        let mut i = at("f(a, (b), c) z", 7);
        keys(&mut i, "da(");
        assert_eq!(i.text, "f(a, , c) z");
        let mut i = at("x [one\ntwo] y", 8);
        keys(&mut i, "ci]");
        assert_eq!(i.text, "x [] y");
    }

    #[test]
    fn yank_object_and_paste() {
        let mut i = at("say \"hello\"", 6);
        keys(&mut i, "yi\"$p");
        assert_eq!(i.text, "say \"hello\"hello");
    }

    #[test]
    fn unknown_object_does_nothing() {
        let mut i = at("one two", 1);
        keys(&mut i, "diz");
        assert_eq!(i.text, "one two");
        assert_eq!(i.mode, Mode::Normal);
        keys(&mut i, "x");
        assert_eq!(i.text, "ne two");
    }

    #[test]
    fn visual_objects() {
        let mut i = at("say \"hello\" ok", 7);
        keys(&mut i, "vi\"");
        assert_eq!(i.selection(), Some((5, 10)));
        keys(&mut i, "d");
        assert_eq!(i.text, "say \"\" ok");
        let mut i = at("f(a, b) z", 3);
        keys(&mut i, "va(");
        assert_eq!(i.selection(), Some((1, 7)));
        let mut i = at("one two", 5);
        keys(&mut i, "viw");
        assert_eq!(i.selection(), Some((4, 7)));
    }

    #[test]
    fn percent_s_first_per_line() {
        let (t, r) = run("foo foo\nbar foo", "%s/foo/X/");
        assert_eq!(t, "X foo\nbar X");
        assert_eq!(r.unwrap(), "2 substitutions on 2 lines");
    }

    #[test]
    fn global_flag_and_ampersand() {
        let (t, r) = run("a.b.c", "%s/./[&]/g");
        assert_eq!(t, "a[.]b[.]c");
        assert_eq!(r.unwrap(), "2 substitutions on 1 line");
    }

    #[test]
    fn current_line_only_and_ranges() {
        // The cursor sits on the last line after typing the text.
        let (t, _) = run("a\na\na", "s/a/b/");
        assert_eq!(t, "a\na\nb");
    }

    #[test]
    fn ranges() {
        let (t, _) = run("a\na\na\na", "2,3s/a/b/");
        assert_eq!(t, "a\nb\nb\na");
        let (t, _) = run("a\na\na\na", ".,$s/a/b/");
        assert_eq!(t, "a\na\na\nb");
        let (t, r) = run("a\na", "9s/a/b/");
        assert_eq!(t, "a\na");
        assert_eq!(r.unwrap_err(), "E16: Invalid range");
    }

    #[test]
    fn ignore_case_and_alt_delimiter() {
        let (t, _) = run("Foo fOO", "%s#foo#x#gi");
        assert_eq!(t, "x x");
        let (t, _) = run("Foo", "%s/foo/x/");
        assert_eq!(t, "Foo");
    }

    #[test]
    fn escapes() {
        let (t, _) = run("a/b", "%s/\\//-/");
        assert_eq!(t, "a-b");
        let (t, _) = run("a b", "%s/ /\\n/");
        assert_eq!(t, "a\nb");
        let (t, _) = run("a\nb", "%s/\\n/ /");
        assert_eq!(t, "a b");
        let (t, _) = run("a", "%s/a/\\&/");
        assert_eq!(t, "&");
    }

    #[test]
    fn missing_replacement_deletes() {
        let (t, _) = run("hello world", "%s/o");
        assert_eq!(t, "hell world");
        let (t, _) = run("hello world", "%s/o//g");
        assert_eq!(t, "hell wrld");
    }

    #[test]
    fn errors() {
        let (t, r) = run("abc", "%s/x/y/");
        assert_eq!(t, "abc");
        assert_eq!(r.unwrap_err(), "E486: Pattern not found: x");
        assert!(run("abc", "foo").1.unwrap_err().starts_with("E492"));
        assert!(run("abc", "%s/a/b/q").1.unwrap_err().starts_with("E488"));
        assert!(run("abc", "%s//b/").1.is_err());
    }

    #[test]
    fn w_saves_and_wq_saves_and_closes() {
        let mut i = vim("abc");
        type_cmd(&mut i, "w");
        i.run_command().unwrap();
        assert!(i.take_write_req());
        assert!(!i.take_quit_req());
        type_cmd(&mut i, "wq");
        i.run_command().unwrap();
        assert!(i.take_write_req());
        assert!(i.take_quit_req());
        type_cmd(&mut i, "wq now");
        assert!(i.run_command().unwrap_err().starts_with("E488"));
    }

    #[test]
    fn undo_restores() {
        let mut i = vim("foo foo");
        type_cmd(&mut i, "%s/foo/x/g");
        i.run_command().unwrap();
        assert_eq!(i.text, "x x");
        i.normal_key('u', 40);
        assert_eq!(i.text, "foo foo");
    }

    #[test]
    fn live_preview() {
        let mut i = vim("foo bar\nfoo foo");
        type_cmd(&mut i, "%s");
        assert_eq!(i.command_preview(), None);
        i.command_push('/');
        assert_eq!(i.command_preview(), None); // empty pattern
        i.command_push('f');
        assert_eq!(i.command_preview(), Some(vec![(0, 1), (8, 9)]));
        i.command_push('o');
        i.command_push('o');
        assert_eq!(i.command_preview(), Some(vec![(0, 3), (8, 11)]));
        for c in "/x/g".chars() {
            i.command_push(c);
        }
        assert_eq!(i.command_preview(), Some(vec![(0, 3), (8, 11), (12, 15)]));
        i.command_push('z'); // bad flag: no preview, no panic
        assert_eq!(i.command_preview(), None);
        i.cancel_command();
        assert_eq!(i.mode, Mode::Normal);
        assert_eq!(i.text, "foo bar\nfoo foo");
        assert_eq!(i.command_preview(), None);
    }

    #[test]
    fn live_replacement_view() {
        let mut i = vim("foo bar\nfoo");
        type_cmd(&mut i, "%s/foo");
        let l = i.command_live().unwrap();
        assert_eq!((l.text, l.marks, l.count), (None, vec![(0, 3), (8, 11)], 2));
        i.command_push('/');
        let l = i.command_live().unwrap();
        // Replacement part started but empty: the matches are gone from the view.
        assert_eq!(l.text.as_deref(), Some(" bar\n"));
        assert!(l.marks.is_empty());
        for c in "quux".chars() {
            i.command_push(c);
            let l = i.command_live().unwrap();
            assert_eq!(l.count, 2);
        }
        let l = i.command_live().unwrap();
        assert_eq!(l.text.as_deref(), Some("quux bar\nquux"));
        assert_eq!(l.marks, vec![(0, 4), (9, 13)]);
        // The box text itself is untouched until Enter.
        assert_eq!(i.text, "foo bar\nfoo");
        i.command_push('/');
        i.command_push('g');
        i.run_command().unwrap();
        assert_eq!(i.text, "quux bar\nquux");
        // No match: no text, count 0.
        let mut i = vim("abc");
        type_cmd(&mut i, "%s/zzz/y");
        let l = i.command_live().unwrap();
        assert!(l.text.is_none() && l.count == 0);
    }

    #[test]
    fn v_mode_j_and_k_move_by_whole_lines_not_wrapped_rows() {
        // The first line wraps over several rows at width 10.
        let mut i = vim("aaaaaaaaaaaaaaaaaaaaaaaaa\nbb\ncc");
        i.cursor = 0;
        i.normal_key('V', 10);
        i.normal_key('j', 10);
        assert_eq!(i.cursor, 26, "one `j` reaches the next line");
        assert_eq!(i.selection(), Some((0, 29)));
        i.normal_key('j', 10);
        assert_eq!(i.cursor, 29);
        i.normal_key('j', 10);
        assert_eq!(i.cursor, 29, "the last line stays");
        i.normal_key('k', 10);
        i.normal_key('k', 10);
        assert_eq!(i.cursor, 0);
        // Outside V mode `j` still follows the wrapped rows.
        i.normal_key('V', 10); // leave VISUAL
        i.normal_key('j', 10);
        assert!(i.cursor > 0 && i.cursor < 26, "still inside the first line: {}", i.cursor);
    }

    #[test]
    fn visual_range() {
        let mut i = vim("a\na\na\na");
        i.cursor = 2; // second line
        i.normal_key('V', 40);
        i.cursor = 4; // third line (`j` needs the real layout code)
        i.normal_key(':', 40);
        assert_eq!(i.command_text(), "'<,'>");
        for c in "s/a/b/".chars() {
            i.command_push(c);
        }
        assert_eq!(i.command_preview(), Some(vec![(2, 3), (4, 5)]));
        i.run_command().unwrap();
        assert_eq!(i.text, "a\nb\nb\na");
    }

    #[test]
    fn backspace_and_goto() {
        let mut i = vim("a\nb\nc");
        i.normal_key(':', 40);
        i.command_backspace();
        assert_eq!(i.mode, Mode::Normal);
        type_cmd(&mut i, "2");
        i.run_command().unwrap();
        assert_eq!(i.cursor, 2);
    }

    fn typed(text: &str, on: bool) -> Input {
        let mut i = Input::default();
        i.set_autopair(on);
        i.reset_vim();
        for c in text.chars() {
            i.type_char(c);
        }
        i
    }

    #[test]
    fn autopair_quotes_and_narration() {
        let i = typed("\"", true);
        assert_eq!((i.text.as_str(), i.cursor), ("\"\"", 1));
        // typing the closer steps over it
        let i = typed("\"hi\"", true);
        assert_eq!((i.text.as_str(), i.cursor), ("\"hi\"", 4));
        let i = typed("*walks* \"hi\" ok", true);
        assert_eq!(i.text, "*walks* \"hi\" ok");
        // bold
        let i = typed("**b**", true);
        assert_eq!((i.text.as_str(), i.cursor), ("**b**", 5));
        let i = typed("**", true);
        assert_eq!((i.text.as_str(), i.cursor), ("****", 2));
    }

    #[test]
    fn autopair_stays_single_next_to_words() {
        assert_eq!(typed("2*3", true).text, "2*3");
        assert_eq!(typed("5\"", true).text, "5\"");
        // second quote in a paragraph that already has one open is the closer
        let mut i = typed("say \"a", true);
        i.cursor = i.len_chars();
        assert_eq!(i.text, "say \"a\"");
    }

    #[test]
    fn autopair_brackets_and_backspace() {
        let i = typed("(a)", true);
        assert_eq!((i.text.as_str(), i.cursor), ("(a)", 3));
        let mut i = typed("[", true);
        assert_eq!(i.text, "[]");
        i.backspace();
        assert_eq!((i.text.as_str(), i.cursor), ("", 0));
        let mut i = typed("*", true);
        i.backspace();
        assert_eq!(i.text, "");
        let mut i = typed("**", true);
        i.backspace();
        assert_eq!(i.text, "**");
    }

    #[test]
    fn autopair_single_quotes() {
        let i = typed("'", true);
        assert_eq!((i.text.as_str(), i.cursor), ("''", 1));
        // typing the closer steps over it
        let i = typed("'hi'", true);
        assert_eq!((i.text.as_str(), i.cursor), ("'hi'", 4));
        // backspace between an empty pair removes both
        let mut i = typed("'", true);
        i.backspace();
        assert_eq!((i.text.as_str(), i.cursor), ("", 0));
        // contractions and possessives stay single
        assert_eq!(typed("don't", true).text, "don't");
        assert_eq!(typed("Sofia's", true).text, "Sofia's");
        // a contraction inside a quote still ends up right
        assert_eq!(typed("'I don't know'", true).text, "'I don't know'");
        // mixed with double quotes
        assert_eq!(typed("\"it's 'fine'\"", true).text, "\"it's 'fine'\"");
    }

    #[test]
    fn autopair_backtick() {
        let i = typed("`", true);
        assert_eq!((i.text.as_str(), i.cursor), ("``", 1));
        // typing the closer steps over it
        let i = typed("`ls -la`", true);
        assert_eq!((i.text.as_str(), i.cursor), ("`ls -la`", 8));
        let i = typed("run `ls` now", true);
        assert_eq!(i.text, "run `ls` now");
        // backspace between an empty pair removes both
        let mut i = typed("`", true);
        i.backspace();
        assert_eq!((i.text.as_str(), i.cursor), ("", 0));
        // stays single next to a word, and a third backtick makes a run
        assert_eq!(typed("a`", true).text, "a`");
        assert_eq!(typed("```", true).text, "```");
    }

    #[test]
    fn autopair_off_changes_nothing() {
        assert_eq!(typed("\"*('", false).text, "\"*('");
        assert_eq!(typed("`", false).text, "`");
        let mut i = typed("\"", false);
        i.backspace();
        assert_eq!(i.text, "");
    }

    #[test]
    fn unicode_and_cursor() {
        let (t, _) = run("héllo wörld", "%s/ö/o/");
        assert_eq!(t, "héllo world");
    }

    #[test]
    fn undo_after_insert_session() {
        // Fresh box, type, leave INSERT mode, undo: back to empty.
        let mut i = Input::default();
        i.reset_vim();
        for c in "hello world".chars() {
            i.type_char(c);
        }
        i.enter_normal();
        i.normal_key('u', 40);
        assert_eq!(i.text, "");
        // `A` then typing: one `u` takes the typed part away.
        let mut i = vim("abc");
        i.normal_key('A', 40);
        i.insert_str("def");
        i.enter_normal();
        assert_eq!(i.text, "abcdef");
        i.normal_key('u', 40);
        assert_eq!(i.text, "abc");
        // `o` + typing: one `u` for the whole thing.
        let mut i = vim("abc");
        i.normal_key('o', 40);
        i.insert_str("x");
        i.enter_normal();
        i.normal_key('u', 40);
        assert_eq!(i.text, "abc");
    }

    #[test]
    fn half_page_rows() {
        let mut i = vim("a\nb\nc\nd\ne");
        i.cursor = 0;
        assert_eq!(i.move_rows(1, 2, 40), 2);
        assert_eq!(i.cursor, 4);
        assert_eq!(i.move_rows(1, 10, 40), 2);
        assert_eq!(i.cursor, 8);
        assert_eq!(i.move_rows(-1, 3, 40), 3);
        assert_eq!(i.cursor, 2);
    }

    #[test]
    fn yank_and_leader_yank() {
        let mut i = vim("one two\nthree");
        i.set_leader(' ');
        i.cursor = 0;
        // plain yw: register only
        i.normal_key('y', 40);
        i.normal_key('w', 40);
        assert_eq!(i.take_clipboard(), None);
        i.cursor = 7;
        i.normal_key('p', 40);
        assert!(i.text.starts_with("one two") && i.text.contains("one "));
        // <leader>yy: whole line to the clipboard
        let mut i = vim("one two\nthree");
        i.set_leader(' ');
        i.cursor = 0;
        for c in " yy".chars() {
            i.normal_key(c, 40);
        }
        assert_eq!(i.take_clipboard().as_deref(), Some("one two\n"));
        // <leader>yw
        for c in " yw".chars() {
            i.normal_key(c, 40);
        }
        assert_eq!(i.take_clipboard().as_deref(), Some("one "));
        // <leader>y then a key that is no motion: nothing queued
        for c in " yz".chars() {
            i.normal_key(c, 40);
        }
        assert_eq!(i.take_clipboard(), None);
        // and the leftover state is clean: plain yw still doesn't touch the clipboard
        i.normal_key('y', 40);
        i.normal_key('w', 40);
        assert_eq!(i.take_clipboard(), None);
        // <leader>Y
        for c in " Y".chars() {
            i.normal_key(c, 40);
        }
        assert_eq!(i.take_clipboard().as_deref(), Some("one two\n"));
        // VISUAL: <leader>y
        let mut i = vim("hello world");
        i.set_leader(' ');
        i.cursor = 0;
        i.normal_key('v', 40);
        i.normal_key('e', 40);
        i.normal_key(' ', 40);
        i.normal_key('y', 40);
        assert_eq!(i.take_clipboard().as_deref(), Some("hello"));
        assert_eq!(i.mode, Mode::Normal);
        // VISUAL plain y: no clipboard
        i.cursor = 0;
        i.normal_key('v', 40);
        i.normal_key('e', 40);
        i.normal_key('y', 40);
        assert_eq!(i.take_clipboard(), None);
        // Leader unset: space does nothing special
        let mut i = vim("ab");
        i.normal_key(' ', 40);
        i.normal_key('y', 40);
        i.normal_key('y', 40);
        assert_eq!(i.take_clipboard(), None);
    }

    // ------------------------------------------------------------ undo

    #[test]
    fn undo_spelling_fix_in_insert_mode() {
        // Type, fix a word (as the spelling menu does), keep typing, leave INSERT:
        // every `u` takes back one thing, newest first.
        let mut i = Input::default();
        i.reset_vim();
        for c in "teh cat".chars() {
            i.type_char(c);
        }
        i.replace_chars(0, 3, "the");
        assert_eq!(i.text, "the cat");
        assert_eq!(i.cursor, 3);
        i.cursor = i.len_chars();
        for c in " sat".chars() {
            i.type_char(c);
        }
        i.enter_normal();
        i.normal_key('u', 40);
        assert_eq!(i.text, "the cat");
        i.normal_key('u', 40);
        assert_eq!(i.text, "teh cat");
        i.normal_key('u', 40);
        assert_eq!(i.text, "");
    }

    #[test]
    fn undo_spelling_fix_in_normal_mode() {
        let mut i = vim("one teh two");
        i.cursor = 0;
        i.normal_key('x', 40); // an earlier change
        assert_eq!(i.text, "ne teh two");
        i.replace_chars(3, 6, "the");
        i.clamp_normal();
        assert_eq!(i.text, "ne the two");
        i.normal_key('u', 40);
        assert_eq!(i.text, "ne teh two");
        i.normal_key('u', 40);
        assert_eq!(i.text, "one teh two");
    }

    #[test]
    fn undo_paste() {
        let mut i = vim("abc");
        i.paste_text("XYZ");
        assert_eq!(i.text, "abXYZc");
        i.normal_key('u', 40);
        assert_eq!(i.text, "abc");
        // In INSERT mode a paste is a step of its own too.
        let mut i = Input::default();
        i.reset_vim();
        i.type_char('a');
        i.paste_text("BC");
        i.type_char('d');
        i.enter_normal();
        i.normal_key('u', 40);
        assert_eq!(i.text, "aBC");
        i.normal_key('u', 40);
        assert_eq!(i.text, "a");
        i.normal_key('u', 40);
        assert_eq!(i.text, "");
    }

    #[test]
    fn undo_clear_box() {
        let mut i = Input::default();
        i.reset_vim();
        for c in "hello".chars() {
            i.type_char(c);
        }
        i.clear_undoable();
        assert_eq!(i.text, "");
        i.enter_normal();
        i.normal_key('u', 40);
        assert_eq!(i.text, "hello");
    }

    #[test]
    fn undo_keys_that_edit_in_normal_mode() {
        let mut i = vim("abc def");
        i.normal_key('$', 40);
        i.backspace();
        assert_eq!(i.text, "abc df");
        i.normal_key('u', 40);
        assert_eq!(i.text, "abc def");
        i.cursor = 0;
        i.delete();
        assert_eq!(i.text, "bc def");
        i.normal_key('u', 40);
        assert_eq!(i.text, "abc def");
        i.cursor = 7;
        i.delete_word_back();
        assert_eq!(i.text, "abc ");
        i.normal_key('u', 40);
        assert_eq!(i.text, "abc def");
        // A key that changes nothing leaves nothing to undo.
        i.cursor = 0;
        i.backspace();
        i.delete_word_back();
        assert_eq!(i.text, "abc def");
        assert!(i.undo.is_empty());
    }

    #[test]
    fn undo_with_count_and_past_the_oldest() {
        let mut i = vim("abcdef");
        i.cursor = 0;
        for _ in 0..3 {
            i.normal_key('x', 40);
        }
        assert_eq!(i.text, "def");
        i.normal_key('2', 40);
        i.normal_key('u', 40);
        assert_eq!(i.text, "bcdef");
        i.normal_key('9', 40);
        i.normal_key('u', 40);
        assert_eq!(i.text, "abcdef");
        // Nothing left: `u` does nothing.
        i.normal_key('u', 40);
        assert_eq!(i.text, "abcdef");
    }

    #[test]
    fn undo_skips_steps_that_changed_nothing() {
        let mut i = vim("abc");
        i.cursor = 0;
        i.normal_key('x', 40);
        assert_eq!(i.text, "bc");
        // `r` with the letter that is already there changes nothing.
        i.normal_key('r', 40);
        i.normal_key('b', 40);
        assert_eq!(i.text, "bc");
        i.normal_key('u', 40);
        assert_eq!(i.text, "abc");
    }

    #[test]
    fn undo_puts_the_cursor_on_a_letter() {
        // The INSERT session after a fix started with the cursor right after
        // the last letter; in NORMAL mode it has to sit on that letter.
        let mut i = Input::default();
        i.reset_vim();
        for c in "abc".chars() {
            i.type_char(c);
        }
        i.replace_chars(0, 3, "xyz");
        assert_eq!(i.cursor, 3);
        i.type_char('d');
        i.enter_normal();
        i.normal_key('u', 40);
        assert_eq!(i.text, "xyz");
        assert_eq!(i.cursor, 2);
    }
}
