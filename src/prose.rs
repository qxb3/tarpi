//! Auto-closing of unfinished prose while a reply streams in, like
//! SillyTavern's "auto-fix markdown", but stricter about when it acts.
//!
//! A reply that is still being generated often has an open `*narration`,
//! `**bold` or `"speech`. The closing marker is added to what is *shown*
//! (the stored text is untouched), so the colours stay readable as the
//! stream goes on. When the model writes the real closer, there is simply
//! nothing left to add: the TUI's marker is replaced by the model's.
//!
//! The parser follows the same rules as `ui::style_text`, with a few extra
//! guards so it never adds noise:
//!
//! * A marker is only closed once something visible follows its opener
//!   (a lone trailing `*` is not turned into `**`).
//! * `2 * 3`, `* bullets`, a `*` glued to the end of a word (`5*3`) and
//!   other asterisks that cannot open anything are left alone.
//! * Only the current paragraph is looked at; styles never cross a blank line.
//! * Markers that were opened in one order are closed in the reverse
//!   order, and ones the model already closed are not touched.
//! * A half-typed `**` closer (`**bold*`) is completed with a single `*`.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Italic,
    Bold,
    /// A quote; the char is the one that closes it.
    Quote(char),
}

struct Open {
    kind: Kind,
    /// Something other than whitespace has been written since the opener.
    content: bool,
}

/// The part of `text` after its last blank line.
fn last_paragraph(text: &str) -> &str {
    let mut start = 0usize;
    let mut pos = 0usize;
    for line in text.split('\n') {
        if line.trim().is_empty() {
            start = pos + line.len() + 1;
        }
        pos += line.len() + 1;
    }
    text.get(start.min(text.len())..).unwrap_or("")
}

/// What is left open at the end of `text`: whether the text ends in a loose
/// `*` (after a space) that has to be hidden before a closer can follow it,
/// and the string that closes the open markers (empty: nothing to close).
fn analyze(text: &str) -> (bool, String) {
    let trimmed = text.trim_end();
    let para = last_paragraph(trimmed);
    let ch: Vec<char> = para.chars().collect();
    let n = ch.len();

    let mut open: Vec<Open> = Vec::new();
    let mut trailing_single_star = false;
    let mut trailing_star_after_space = false;
    let mut i = 0usize;
    while i < n {
        let c = ch[i];
        if c == '*' {
            let double = i + 1 < n && ch[i + 1] == '*';
            let w = if double { 2 } else { 1 };
            let prev = if i == 0 { None } else { Some(ch[i - 1]) };
            let prev_ws = prev.map(|p| p.is_whitespace()).unwrap_or(true);
            let next_ws = i + w >= n || ch[i + w].is_whitespace();
            let kind = if double { Kind::Bold } else { Kind::Italic };
            let is_open = open.iter().rposition(|o| o.kind == kind);
            if let Some(p) = is_open {
                if !prev_ws {
                    // The model closed it itself.
                    open.remove(p);
                    i += w;
                    continue;
                }
            } else if !next_ws && !prev.map(|p| p.is_alphanumeric()).unwrap_or(false) {
                open.push(Open {
                    kind,
                    content: false,
                });
                i += w;
                continue;
            }
            // An asterisk that opens and closes nothing.
            if !double && i + 1 == n {
                trailing_single_star = true;
                trailing_star_after_space = prev_ws;
            }
            i += w;
            continue;
        }

        match c {
            '"' => {
                if let Some(p) = open.iter().rposition(|o| matches!(o.kind, Kind::Quote(_))) {
                    open.remove(p);
                } else {
                    open.push(Open {
                        kind: Kind::Quote('"'),
                        content: false,
                    });
                }
            }
            '\u{201C}' => {
                if !open.iter().any(|o| matches!(o.kind, Kind::Quote(_))) {
                    open.push(Open {
                        kind: Kind::Quote('\u{201D}'),
                        content: false,
                    });
                }
            }
            '\u{201D}' => {
                if let Some(p) = open.iter().rposition(|o| matches!(o.kind, Kind::Quote(_))) {
                    open.remove(p);
                }
            }
            _ => {
                if !c.is_whitespace() {
                    for o in open.iter_mut() {
                        o.content = true;
                    }
                }
            }
        }
        i += 1;
    }

    // Innermost (last opened) first; markers with nothing in them are skipped.
    let pending: Vec<Kind> = open
        .iter()
        .rev()
        .filter(|o| o.content)
        .map(|o| o.kind)
        .collect();

    // `**bold*`: the model's next `*` completes the closer, so add just one.
    let half_closer = trailing_single_star && !trailing_star_after_space;
    let mut out = String::new();
    for (k, kind) in pending.iter().enumerate() {
        match kind {
            Kind::Bold if k == 0 && half_closer => out.push('*'),
            Kind::Bold => out.push_str("**"),
            Kind::Italic => out.push('*'),
            Kind::Quote(close) => out.push(*close),
        }
    }
    (trailing_single_star && trailing_star_after_space, out)
}

/// The markers that are still open at the end of `text`, as the string that
/// closes them. Empty when nothing needs closing.
#[cfg(test)]
pub fn closers(text: &str) -> String {
    analyze(text).1
}

/// `text` with its open markers closed. A loose trailing `*` (`... 2 *`) is
/// left out, since a closer cannot follow a space; it comes back as soon as
/// the next character shows what it is.
fn close(text: &str) -> Option<(String, String)> {
    let (dangling, c) = analyze(text);
    if c.is_empty() {
        return None;
    }
    let mut base = text.trim_end();
    if dangling {
        base = base[..base.len() - 1].trim_end();
    }
    Some((format!("{}{}", base, c), c))
}

/// What to draw for a reply that is still streaming: the text with its
/// open markers closed. Trailing whitespace is dropped only when a closer is
/// added (a closer after a space would not close anything).
pub fn close_for_display(text: &str) -> String {
    match close(text) {
        Some((shown, _)) => shown,
        None => text.to_string(),
    }
}

/// A finished reply with its open markers closed, plus the closer that was
/// added (empty when the text was already balanced).
pub fn close_final(text: &str) -> (String, String) {
    match close(text) {
        Some(done) => done,
        None => (text.to_string(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closes_open_italic() {
        assert_eq!(close_for_display("She smiles *and waves"), "She smiles *and waves*");
    }

    #[test]
    fn leaves_balanced_text_alone() {
        assert_eq!(closers("*hi* \"yo\" **b**"), "");
    }

    #[test]
    fn model_closer_replaces_ours() {
        assert_eq!(close_for_display("*hi"), "*hi*");
        assert_eq!(close_for_display("*hi*"), "*hi*");
    }

    #[test]
    fn lone_opener_adds_nothing() {
        assert_eq!(closers("She smiles *"), "");
        assert_eq!(closers("She smiles * "), "");
        assert_eq!(closers("\"" ), "");
    }

    #[test]
    fn plain_asterisks_ignored() {
        assert_eq!(closers("2 * 3 = 6"), "");
        assert_eq!(closers("5*3 is 15"), "");
        assert_eq!(closers("* a bullet"), "");
    }

    #[test]
    fn nested_and_interleaved() {
        assert_eq!(closers("*she says \"hi"), "\"*");
        assert_eq!(closers("*she says \"hi*"), "\"");
        assert_eq!(closers("**big *small"), "***");
    }

    #[test]
    fn half_typed_bold_closer() {
        assert_eq!(close_for_display("**bold*"), "**bold**");
    }

    #[test]
    fn styles_stop_at_blank_line() {
        assert_eq!(closers("*never closed\n\nnext paragraph"), "");
        assert_eq!(closers("*never closed\n\nnext *one"), "*");
    }

    #[test]
    fn loose_star_after_space_is_hidden() {
        assert_eq!(close_for_display("*he mutters, 2 *"), "*he mutters, 2*");
        assert_eq!(close_for_display("2 * 3"), "2 * 3");
    }

    #[test]
    fn trailing_whitespace_before_closer() {
        assert_eq!(close_for_display("*hi \n"), "*hi*");
    }
}
