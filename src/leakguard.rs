//! Catches a model that starts writing the chat machinery itself: a turn
//! marker (`<|im_start|>`, `[INST]`, `### Instruction:`), a stop string the
//! server failed to honour, or the user's own `\nName:` line. The moment one
//! shows up the reply is cut there and the request is dropped, so the person
//! never sees it.
//!
//! It sits between the stream parser and the screen and works on text, so it
//! is the same for every backend. The rules that keep it invisible:
//!
//! * **Hold back.** A tail that could still grow into a marker (`<|im_`,
//!   `\nYo`) is withheld until the next chunk settles it. Trailing white space
//!   is withheld too, so a marker's lead-in line break never reaches the
//!   screen. What is shown is therefore final: nothing is ever taken back,
//!   and the result does not depend on how the stream happened to be cut into
//!   chunks (the tests check both with random cuts).
//! * **Cut clean.** On a match, everything from the marker on is thrown away
//!   and the text before it loses its trailing white space.
//! * **Broken markers.** A control token that is cut short at the end of the
//!   stream (`<|im_en`) is dropped instead of flushed.
//! * **Bounded.** At most a marker's length (or [`MAX_WS_HOLD`] bytes of white
//!   space) is ever held, however long the reply is.
//!
//! What counts as a marker:
//!
//! * a **line marker**: a stop string or role line that starts with a line
//!   break (`\nYou:`, `\n### Response:`). It also matches when the line is
//!   indented or wrapped in markdown (`\n  You:`, `\n**You:`).
//! * a **plain marker**: any other stop string, matched exactly as a server
//!   would.
//! * a **control token**: any `<|...|>` or `<｜...｜>` token (ChatML, Llama 3,
//!   DeepSeek, Harmony), with stray spaces and full-width brackets allowed
//!   (`< | im_start | >`), plus a few built-in ones (`</s>`, `[INST]`,
//!   `<start_of_turn>`) matched without regard to letter case.

/// Longest body, in characters, of a `<|...|>` token. Real ones are short;
/// the limit keeps a stray `<|` in prose from holding text back for long.
const MAX_TOKEN_BODY: usize = 40;

/// Characters read to judge one token: the body, the brackets and some padding.
const TOKEN_WINDOW: usize = MAX_TOKEN_BODY + 12;

/// White space is held only up to this many bytes; a longer run is shown.
const MAX_WS_HOLD: usize = 1024;

/// What may sit between a line break and the name of a line marker:
/// indentation and markdown emphasis / quoting.
const DECORATION: &[char] = &[' ', '\t', '*', '_', '>'];

/// Control tokens and role headers that are never part of a reply.
/// Lower case: they are compared ignoring ASCII case.
const BUILT_IN: &[&str] = &[
    "</s>",
    "[inst]",
    "[/inst]",
    "<<sys>>",
    "<</sys>>",
    "<start_of_turn>",
    "<end_of_turn>",
    "\n### instruction:",
    "\n### response:",
    "\n### input:",
];

struct Marker {
    /// Lower case when `fold`. For a line marker, without the line break.
    text: String,
    /// Compare without regard to ASCII case.
    fold: bool,
    /// Matches at the start of a line, whatever indents or marks it.
    line: bool,
}

impl Marker {
    fn new(raw: &str, fold: bool) -> Marker {
        let text = if fold { raw.to_ascii_lowercase() } else { raw.to_string() };
        // `\n` + a name. A bare `\n` or `\n\n` stays an exact string.
        let body = text.strip_prefix('\n').unwrap_or("");
        let line = !body.is_empty()
            && !body.starts_with(char::is_whitespace)
            && !body.contains('\n');
        if line {
            Marker { text: body.to_string(), fold, line }
        } else {
            Marker { text, fold, line }
        }
    }

    /// Byte offset of the first match in `hay` (already lower case if `fold`).
    fn find(&self, hay: &str) -> Option<usize> {
        if !self.line {
            return hay.find(self.text.as_str());
        }
        hay.match_indices('\n').map(|(p, _)| p).find(|p| {
            hay[p + 1..]
                .trim_start_matches(DECORATION)
                .starts_with(self.text.as_str())
        })
    }

    /// Where a tail of `hay` that could still grow into this marker starts,
    /// and how many characters of the marker it already has.
    fn partial(&self, hay: &str) -> Option<(usize, usize)> {
        if !self.line {
            let from = hay.len().saturating_sub(self.text.len().saturating_sub(1));
            return (from..hay.len())
                .filter(|i| hay.is_char_boundary(*i))
                .find(|i| self.text.len() > hay.len() - i && self.text.starts_with(&hay[*i..]))
                .map(|i| (i, hay[i..].chars().count()));
        }
        let p = hay.rfind('\n')?;
        let t = hay[p + 1..].trim_start_matches(DECORATION);
        if t.len() < self.text.len() && self.text.starts_with(t) {
            Some((p, t.chars().count()))
        } else {
            None
        }
    }
}

/// What one call hands on.
#[derive(Debug, PartialEq, Eq)]
pub struct Out {
    /// Text that is safe to show.
    pub text: String,
    /// A marker was found: nothing more should be read from the model.
    pub stop: bool,
}

pub struct LeakGuard {
    markers: Vec<Marker>,
    held: String,
    stopped: bool,
}

/// A `<|...|>` style token at some place in a string.
enum Token {
    /// A complete token starting at this byte offset.
    Full(usize),
    /// Open at the end of the text and still completable, from this offset.
    /// `true` once the `|` after the `<` has been seen: that much is a
    /// control token for certain, a bare `<` could be a less-than sign.
    Partial(usize, bool),
    None,
}

impl LeakGuard {
    /// `stops` are the stop strings sent to the server, `extra` more markers
    /// (the user's name line, the instruct template's own sequences).
    pub fn new(stops: &[String], extra: &[String]) -> LeakGuard {
        let mut markers: Vec<Marker> = BUILT_IN.iter().map(|s| Marker::new(s, true)).collect();
        for s in stops.iter().chain(extra) {
            if s.is_empty() {
                continue;
            }
            let m = Marker::new(s, false);
            if markers.iter().any(|o| !o.fold && o.line == m.line && o.text == m.text) {
                continue;
            }
            markers.push(m);
        }
        LeakGuard { markers, held: String::new(), stopped: false }
    }

    /// Feed the next piece of the reply.
    pub fn push(&mut self, chunk: &str) -> Out {
        if self.stopped {
            return Out { text: String::new(), stop: true };
        }
        self.held.push_str(chunk);
        if let Some(pos) = self.find_match() {
            let text = self.held[..pos].trim_end().to_string();
            self.held.clear();
            self.stopped = true;
            return Out { text, stop: true };
        }
        let at = self.hold_from();
        let text = self.held[..at].to_string();
        self.held.drain(..at);
        Out { text, stop: false }
    }

    /// The stream ended: what is still held, minus any broken control token.
    pub fn finish(&mut self) -> String {
        if self.stopped {
            return String::new();
        }
        let mut held = std::mem::take(&mut self.held);
        let mut cut = held.len();
        if let Token::Partial(i, true) = token_at(&held) {
            cut = i;
        }
        let lc = held.to_ascii_lowercase();
        for m in self.markers.iter().filter(|m| m.fold) {
            // One stray character is far more likely to be text.
            if let Some((i, n)) = m.partial(&lc) {
                if n >= 2 {
                    cut = cut.min(i);
                }
            }
        }
        if cut < held.len() {
            held.truncate(cut);
            held.truncate(held.trim_end().len());
        }
        held
    }

    /// Byte offset of the first marker in the held text.
    fn find_match(&self) -> Option<usize> {
        let lc = self.held.to_ascii_lowercase();
        let mut best: Option<usize> = None;
        for m in &self.markers {
            let hay = if m.fold { &lc } else { &self.held };
            if let Some(i) = m.find(hay) {
                best = Some(best.map_or(i, |b| b.min(i)));
            }
        }
        // A marker inside what looks like the start of a token (`<|[INST]`):
        // the token is the leak, so the cut goes before it. Only the text up
        // to the marker is looked at, so the answer is the same whether the
        // rest of the token has arrived or not. Not when the marker begins
        // with a character a token cannot hold (`<` of `</s>`, a line break):
        // that ends the open token, so it was never a leak.
        if let Some(b) = best {
            let ends_token = self.held[b..]
                .chars()
                .next()
                .map_or(true, |c| c == '\n' || c == '\r' || is_open(c) || is_close(c));
            if !ends_token {
                if let Token::Partial(i, true) = token_at(&self.held[..b]) {
                    best = Some(i);
                }
            }
        }
        if let Token::Full(i) = token_at(&self.held) {
            best = Some(best.map_or(i, |b| b.min(i)));
        }
        best
    }

    /// Where the withheld tail of the held text starts.
    fn hold_from(&self) -> usize {
        let lc = self.held.to_ascii_lowercase();
        // Start of a tail that could still become a marker.
        let mut tail = self.held.len();
        for m in &self.markers {
            let hay = if m.fold { &lc } else { &self.held };
            if let Some((i, _)) = m.partial(hay) {
                tail = tail.min(i);
            }
        }
        if let Token::Partial(i, _) = token_at(&self.held) {
            tail = tail.min(i);
        }
        // The white space in front of it goes with it: a cut trims it. A
        // very long run is shown instead, so the hold stays small.
        let at = self.held[..tail].trim_end().len();
        if tail - at > MAX_WS_HOLD {
            tail
        } else {
            at
        }
    }
}

fn is_open(c: char) -> bool {
    c == '<' || c == '\u{ff1c}'
}
fn is_close(c: char) -> bool {
    c == '>' || c == '\u{ff1e}'
}
fn is_bar(c: char) -> bool {
    c == '|' || c == '\u{ff5c}'
}
fn is_pad(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// The first `<|...|>` / `<｜...｜>` token in `s` (full-width brackets and
/// spaces next to the bars allowed): a whole one, or one still open at the
/// end of the text that could yet be completed.
fn token_at(s: &str) -> Token {
    'outer: for (start, c) in s.char_indices() {
        if !is_open(c) {
            continue;
        }
        let rest: Vec<char> = s[start..].chars().take(TOKEN_WINDOW).collect();
        // Reached the end of the text, not just of the window.
        let at_end = rest.len() < TOKEN_WINDOW;
        let open_or_skip = |weak_or_strong: bool| {
            if at_end {
                Some(Token::Partial(start, weak_or_strong))
            } else {
                None
            }
        };

        let mut i = 1;
        while i < rest.len() && is_pad(rest[i]) {
            i += 1;
        }
        if i >= rest.len() {
            match open_or_skip(false) {
                Some(t) => return t,
                None => continue 'outer,
            }
        }
        if !is_bar(rest[i]) {
            continue 'outer;
        }
        i += 1;
        let mut body = 0usize;
        loop {
            if i >= rest.len() {
                match open_or_skip(true) {
                    Some(t) => return t,
                    None => continue 'outer,
                }
            }
            let c = rest[i];
            if is_bar(c) {
                let mut j = i + 1;
                while j < rest.len() && is_pad(rest[j]) {
                    j += 1;
                }
                if j >= rest.len() {
                    match open_or_skip(true) {
                        Some(t) => return t,
                        None => continue 'outer,
                    }
                }
                if is_close(rest[j]) {
                    return Token::Full(start);
                }
            }
            if c == '\n' || c == '\r' || is_open(c) || is_close(c) {
                continue 'outer;
            }
            body += 1;
            if body > MAX_TOKEN_BODY {
                continue 'outer;
            }
            i += 1;
        }
    }
    Token::None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> LeakGuard {
        LeakGuard::new(&["\nYou:".to_string()], &["\nBob:".to_string()])
    }

    /// Run the chunks through the guard: what was shown, and whether it
    /// stopped on a marker.
    fn run(g: &mut LeakGuard, chunks: &[&str]) -> (String, bool) {
        let mut shown = String::new();
        let mut stopped = false;
        for c in chunks {
            let o = g.push(c);
            shown.push_str(&o.text);
            if o.stop {
                stopped = true;
                break;
            }
        }
        if !stopped {
            shown.push_str(&g.finish());
        }
        (shown, stopped)
    }

    fn run_split(text: &str, size: usize) -> (String, bool) {
        let cs: Vec<char> = text.chars().collect();
        let owned: Vec<String> = cs.chunks(size).map(|c| c.iter().collect()).collect();
        let refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
        run(&mut guard(), &refs)
    }

    #[test]
    fn plain_text_passes_untouched() {
        let (s, stopped) = run(&mut guard(), &["Hello ", "there, ", "friend.\n\n", "How are you?"]);
        assert_eq!(s, "Hello there, friend.\n\nHow are you?");
        assert!(!stopped);
    }

    #[test]
    fn user_turn_is_cut_even_when_split() {
        for size in 1..=8 {
            let (s, stopped) = run_split("She waves.\nYou: hi there", size);
            assert_eq!(s, "She waves.", "chunk size {size}");
            assert!(stopped, "chunk size {size}");
        }
    }

    #[test]
    fn control_tokens_are_cut_in_every_split() {
        for text in [
            "Fine.<|im_end|>\n<|im_start|>user\nhi",
            "Fine.\n<|im_start|>user",
            "Fine.<|eot_id|><|start_header_id|>user",
            "Fine.<｜end▁of▁sentence｜>",
            "Fine. </s>",
            "Fine.\n[INST] more [/INST]",
            "Fine.\n### Instruction:\nmore",
            "Fine.<|channel|>analysis",
            "Fine.<start_of_turn>user",
        ] {
            for size in 1..=6 {
                let (s, stopped) = run_split(text, size);
                assert_eq!(s, "Fine.", "{text:?} chunk size {size}");
                assert!(stopped, "{text:?} chunk size {size}");
            }
        }
    }

    #[test]
    fn spaced_and_full_width_tokens_are_cut() {
        for text in [
            "Fine.< |im_start|>user",
            "Fine.<| im_start |>user",
            "Fine.< | im_end | >",
            "Fine.＜｜im_end｜＞",
            "Fine.＜|im_start|>",
            "Fine.<\t|im_end|>",
            "Fine.<||>",
        ] {
            for size in 1..=5 {
                let (s, stopped) = run_split(text, size);
                assert_eq!(s, "Fine.", "{text:?} chunk size {size}");
                assert!(stopped, "{text:?} chunk size {size}");
            }
        }
    }

    #[test]
    fn indented_or_marked_up_user_line_is_cut() {
        for text in [
            "Fine.\n  You: hi",
            "Fine.\n\tYou: hi",
            "Fine.\n**You:** hi",
            "Fine.\n> You: hi",
            "Fine.\n\n_You: hi",
            "Fine.\r\nYou: hi",
            "Fine.\n### Response:\nmore",
            "Fine.\n  ### instruction:",
        ] {
            for size in 1..=5 {
                let (s, stopped) = run_split(text, size);
                assert_eq!(s, "Fine.", "{text:?} chunk size {size}");
                assert!(stopped, "{text:?} chunk size {size}");
            }
        }
    }

    #[test]
    fn nothing_is_ever_shown_and_taken_back() {
        let text = "He nods.\n\n\"Yes.\"\n<|im_start|>user\nhi";
        let want = "He nods.\n\n\"Yes.\"";
        let cs: Vec<char> = text.chars().collect();
        for size in 1..=5 {
            let mut g = guard();
            let mut shown = String::new();
            for c in cs.chunks(size) {
                let o = g.push(&c.iter().collect::<String>());
                shown.push_str(&o.text);
                assert!(want.starts_with(&shown), "showed {shown:?} at chunk size {size}");
                if o.stop {
                    break;
                }
            }
            assert_eq!(shown, want);
        }
    }

    #[test]
    fn template_markers_and_case() {
        let (s, st) = run(&mut guard(), &["ok\n", "Bob: and then"]);
        assert_eq!((s.as_str(), st), ("ok", true));
        let (s, st) = run(&mut guard(), &["ok [", "inst] go"]);
        assert_eq!((s.as_str(), st), ("ok", true));
        let (s, st) = run(&mut guard(), &["ok [", "InSt] go"]);
        assert_eq!((s.as_str(), st), ("ok", true));
    }

    #[test]
    fn near_misses_are_released() {
        let (s, st) = run(&mut guard(), &["a < b and ", "x |> y, You: yes, ", "<|not a\ntoken|>"]);
        assert!(!st);
        assert_eq!(s, "a < b and x |> y, You: yes, <|not a\ntoken|>");
        let (s, st) = run(&mut guard(), &["Hold on\nYo", "ung master"]);
        assert_eq!((s.as_str(), st), ("Hold on\nYoung master", false));
        let (s, st) = run(&mut guard(), &["a <", "3 b"]);
        assert_eq!((s.as_str(), st), ("a <3 b", false));
        // Roleplay: emphasis and quotes at the start of a line are text.
        let text = "*She smiles.*\n*You nod.*\n> quoted\n_aside_\n  indented";
        for size in 1..=4 {
            let (s, st) = run_split(text, size);
            assert_eq!((s.as_str(), st), (text, false), "chunk size {size}");
        }
    }

    #[test]
    fn broken_token_at_end_of_stream_is_dropped() {
        let (s, st) = run(&mut guard(), &["Goodbye.", "<|im_en"]);
        assert_eq!((s.as_str(), st), ("Goodbye.", false));
        let (s, st) = run(&mut guard(), &["Goodbye. [IN"]);
        assert_eq!((s.as_str(), st), ("Goodbye.", false));
        let (s, st) = run(&mut guard(), &["Goodbye.\n### Instr"]);
        assert_eq!((s.as_str(), st), ("Goodbye.", false));
        let (s, st) = run(&mut guard(), &["Goodbye. < | im_"]);
        assert_eq!((s.as_str(), st), ("Goodbye.", false));
    }

    #[test]
    fn trailing_text_that_only_looks_like_a_marker_is_kept() {
        for text in ["She sighs.\nYo", "Is it < ", "Is it <", "a\n*", "a\n  ", "x [", "x <"] {
            let (s, st) = run(&mut guard(), &[text]);
            assert_eq!((s.as_str(), st), (text, false), "{text:?}");
        }
    }

    #[test]
    fn marker_first_in_reply_leaves_nothing() {
        let (s, st) = run(&mut guard(), &["<|im_start|>assistant"]);
        assert_eq!((s.as_str(), st), ("", true));
    }

    #[test]
    fn stays_stopped() {
        let mut g = guard();
        assert!(g.push("x</s>y").stop);
        assert_eq!(g.push("more"), Out { text: String::new(), stop: true });
        assert_eq!(g.finish(), "");
    }

    #[test]
    fn multibyte_text_is_safe() {
        let text = "こんにちは、世界。✨ 'tis fine. 🎉<|im_end|>";
        for size in 1..=4 {
            let (s, st) = run_split(text, size);
            assert_eq!(s, "こんにちは、世界。✨ 'tis fine. 🎉");
            assert!(st);
        }
        let (s, st) = run(&mut guard(), &["世界\n　You: x"]);
        assert_eq!((s.as_str(), st), ("世界\n　You: x", false));
    }

    #[test]
    fn server_stop_strings_are_honoured_as_given() {
        let mut g = LeakGuard::new(&["OOC:".to_string()], &[]);
        let (s, st) = run(&mut g, &["Nice. OO", "C: note"]);
        assert_eq!((s.as_str(), st), ("Nice.", true));
        // A bare line break (single-line mode) stays exact.
        let mut g = LeakGuard::new(&["\n".to_string()], &[]);
        let (s, st) = run(&mut g, &["One line.", "\nTwo"]);
        assert_eq!((s.as_str(), st), ("One line.", true));
    }

    #[test]
    fn held_text_stays_small() {
        let mut g = guard();
        let mut shown = 0;
        for _ in 0..200 {
            shown += g.push(&" ".repeat(100)).text.len();
            assert!(g.held.len() <= MAX_WS_HOLD + 100, "held {}", g.held.len());
        }
        assert!(shown > 0);
        // A stray "<" is not held for ever either.
        let mut g = guard();
        let out = g.push("a <| ").text + &g.push(&"x".repeat(100)).text;
        assert!(out.contains("xxxxx"), "{out:?}");
        assert!(g.held.len() < 100);
    }

    #[test]
    fn long_non_stream_text_is_quick() {
        let text = "word <b> and\n*more* text < here, ".repeat(20_000);
        let mut g = guard();
        let o = g.push(&text);
        assert!(!o.stop);
        assert!(o.text.len() + g.finish().len() >= text.trim_end().len());
    }

    /// Small deterministic generator, so the test needs no crate.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    #[test]
    fn result_never_depends_on_how_the_stream_is_cut() {
        let pieces = [
            "She", " says", "\n", "\n\n", " ", "*", "_", ">", "\"", "hi", "<", "|", "｜", "＜", ">",
            "im_start", "im_end", "<|", "|>", "You", ":", "You:", "Bob:", "[", "INST", "]", "[INST]",
            "</s>", "###", " Response", ":", "\r\n", "世界", "🎉", "  ", "\t", "OOC",
        ];
        let mut rng = Rng(0x9e3779b97f4a7c15);
        for case in 0..20_000 {
            let n = 1 + rng.below(14);
            let text: String = (0..n).map(|_| pieces[rng.below(pieces.len())]).collect();
            // Reference: the whole text as one chunk.
            let (want, want_stop) = run(&mut guard(), &[&text]);
            // The same text in random chunks, char-aligned.
            let cs: Vec<char> = text.chars().collect();
            let mut g = guard();
            let mut shown = String::new();
            let mut stopped = false;
            let mut i = 0;
            while i < cs.len() {
                let len = 1 + rng.below(4);
                let chunk: String = cs[i..(i + len).min(cs.len())].iter().collect();
                i += len;
                let o = g.push(&chunk);
                shown.push_str(&o.text);
                assert!(
                    want.starts_with(&shown),
                    "case {case} {text:?}: showed {shown:?}, final {want:?}"
                );
                if o.stop {
                    stopped = true;
                    break;
                }
            }
            if !stopped {
                shown.push_str(&g.finish());
            }
            assert_eq!(
                (shown.as_str(), stopped),
                (want.as_str(), want_stop),
                "case {case} {text:?}"
            );
        }
    }
}
