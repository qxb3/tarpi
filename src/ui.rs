//! Rendering: message wrapping/styling and the ratatui layout.

use crate::app::{App, ChatStats};
use crate::cardedit::{CardModal, Hit};
use crate::input::Mode;
use crate::picker::Picker;
use crate::posedit::{self, PosEdit};
use crate::termcolors;
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthChar;

type StyledChar = (char, Style);

/// Colour of the names in the message headers.
const NAME_WHITE: Color = Color::Rgb(255, 255, 255);

/// Width of the Send button next to the input box.
pub const BTN_W: u16 = 10;
/// Wider button column while the extra "Card save" button is shown.
pub const CARD_BTN_W: u16 = 13;
/// Button column on narrow screens (4 letters and the border).
pub const NARROW_BTN_W: u16 = 6;

// ------------------------------------------------------------ text styling

/// Style of one char given the markdown state it sits in. The colours come
/// from the terminal's own theme (see `termcolors`): narration is the
/// foreground faded towards the background, speech is the theme's yellow,
/// speech nested in narration is the plain speech colour, and narration nested
/// in speech is a lighter shade of the speech colour (and always bold). `quote_outer` says
/// which one was opened first when both are active. They are RGB, so "bold"
/// doesn't turn them bright in terminals that render bold as bright.
fn mk_style(italic: bool, bold: bool, quote: bool, quote_outer: bool, code: bool) -> Style {
    let mut style = Style::default();
    // Narration is always bold, also when it sits inside speech. Speech is
    // not bold, also when it sits inside narration.
    if italic && !(quote && !quote_outer) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if let Some(c) = termcolors::palette().prose_color(italic, quote, quote_outer) {
        style = style.fg(c);
    }
    if quote && (!italic || !quote_outer) {
        style = style.remove_modifier(Modifier::BOLD);
    }
    // Narration nested in speech (`"hello *you*"`) is always bold.
    if italic && quote && quote_outer {
        style = style.add_modifier(Modifier::BOLD);
    }
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if code {
        style = style.bg(termcolors::palette().code_bg());
    }
    style
}

/// Length of the run of backticks starting at `from`.
fn tick_run(chars: &[char], from: usize) -> usize {
    chars[from..].iter().take_while(|&&c| c == '`').count()
}

/// Is there a closing run of exactly `n` backticks at or after `(li, from)`,
/// before the paragraph ends (a blank line)?
fn code_closer_ahead(lines: &[Vec<char>], li: usize, from: usize, n: usize) -> bool {
    for (k, line) in lines.iter().enumerate().skip(li) {
        if k > li && line.iter().all(|c| c.is_whitespace()) {
            return false;
        }
        let mut j = if k == li { from } else { 0 };
        while j < line.len() {
            if line[j] == '`' {
                let run = tick_run(line, j);
                if run == n {
                    return true;
                }
                j += run;
            } else {
                j += 1;
            }
        }
    }
    false
}

/// Is there a closing `*` (or `**` when `double`) at or after `(li, from)`,
/// before the paragraph ends (a blank line)?
fn closer_ahead(lines: &[Vec<char>], li: usize, from: usize, double: bool) -> bool {
    for (k, line) in lines.iter().enumerate().skip(li) {
        if k > li && line.iter().all(|c| c.is_whitespace()) {
            return false;
        }
        let mut j = if k == li { from } else { 0 };
        while j < line.len() {
            if line[j] == '*' {
                let dbl = j + 1 < line.len() && line[j + 1] == '*';
                if !double || dbl {
                    return true;
                }
                j += if dbl { 2 } else { 1 };
            } else {
                j += 1;
            }
        }
    }
    false
}

/// Split text into lines of styled chars. `*italic*` and `**bold**` markers
/// are consumed; "quoted speech" is highlighted, similar to the web UI.
///
/// Like markdown, styles never leak past the end of a paragraph, and a `*`
/// that cannot open or close anything (`2 * 3`, a `* ` bullet, an unclosed
/// `*`) is shown as a plain asterisk instead of colouring the rest of the
/// message. Control characters (a stray `\r`, escape sequences) are dropped
/// because they would corrupt the terminal.
fn style_text(text: &str) -> Vec<Vec<StyledChar>> {
    let mut in_italic = false;
    let mut in_bold = false;
    let mut in_quote = false;
    // Inside `code` (the number of backticks that closes it, 0 = not in code):
    // everything is literal, on a lighter background.
    let mut code_ticks = 0usize;
    // Which of quote / italic was opened first (only read while both are on).
    let mut quote_outer = false;
    let mut lines: Vec<Vec<StyledChar>> = Vec::new();

    let raw_lines: Vec<Vec<char>> = text
        .split('\n')
        .map(|l| {
            l.replace('\t', "    ")
                .chars()
                .filter(|c| !c.is_control())
                .collect::<Vec<char>>()
        })
        .collect();

    for (li, chars) in raw_lines.iter().enumerate() {
        if chars.iter().all(|c| c.is_whitespace()) {
            in_italic = false;
            in_bold = false;
            in_quote = false;
            code_ticks = 0;
        }
        let mut line: Vec<StyledChar> = Vec::new();
        let mut i = 0usize;
        while i < chars.len() {
            let c = chars[i];
            if c == '`' {
                let n = tick_run(chars, i);
                let is_marker = if code_ticks > 0 {
                    n == code_ticks
                } else {
                    code_closer_ahead(&raw_lines, li, i + n, n)
                };
                if is_marker {
                    code_ticks = if code_ticks > 0 { 0 } else { n };
                } else {
                    let st = mk_style(in_italic, in_bold, in_quote, quote_outer, code_ticks > 0);
                    for _ in 0..n {
                        line.push(('`', st));
                    }
                }
                i += n;
                continue;
            }
            if code_ticks > 0 {
                line.push((c, mk_style(in_italic, in_bold, in_quote, quote_outer, true)));
                i += 1;
                continue;
            }
            if c == '*' {
                let double = i + 1 < chars.len() && chars[i + 1] == '*';
                let w = if double { 2 } else { 1 };
                let prev_ws = i == 0 || chars[i - 1].is_whitespace();
                let next_ws = i + w >= chars.len() || chars[i + w].is_whitespace();
                let active = if double { in_bold } else { in_italic };
                let is_marker = if active {
                    !prev_ws
                } else {
                    !next_ws && closer_ahead(&raw_lines, li, i + w, double)
                };
                if is_marker {
                    if double {
                        in_bold = !in_bold;
                    } else {
                        if !in_italic {
                            quote_outer = in_quote;
                        }
                        in_italic = !in_italic;
                    }
                } else {
                    let st = mk_style(in_italic, in_bold, in_quote, quote_outer, false);
                    for _ in 0..w {
                        line.push(('*', st));
                    }
                }
                i += w;
                continue;
            }

            let is_quote_char = c == '"' || c == '\u{201C}' || c == '\u{201D}';
            let mut closes_quote = false;
            if is_quote_char {
                if c == '"' {
                    if in_quote {
                        closes_quote = true;
                    } else {
                        in_quote = true;
                        quote_outer = !in_italic;
                    }
                } else if c == '\u{201C}' {
                    in_quote = true;
                    quote_outer = !in_italic;
                } else {
                    closes_quote = true;
                }
            }

            line.push((c, mk_style(in_italic, in_bold, in_quote, quote_outer, false)));
            if closes_quote {
                in_quote = false;
            }
            i += 1;
        }
        lines.push(line);
    }
    lines
}

fn trim_trailing_spaces(line: &mut Vec<StyledChar>) {
    while let Some((c, _)) = line.last() {
        if *c == ' ' {
            line.pop();
        } else {
            break;
        }
    }
}

fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// Greedy word wrap that keeps per-character styles.
fn wrap_styled(chars: &[StyledChar], width: usize) -> Vec<Vec<StyledChar>> {
    let width = width.max(1);
    let mut out: Vec<Vec<StyledChar>> = Vec::new();
    let mut cur: Vec<StyledChar> = Vec::new();
    let mut cur_w = 0usize;
    let mut i = 0usize;

    while i < chars.len() {
        let is_space = chars[i].0 == ' ';
        let mut j = i;
        while j < chars.len() && (chars[j].0 == ' ') == is_space {
            j += 1;
        }
        let token = &chars[i..j];
        let tw: usize = token.iter().map(|(c, _)| char_width(*c)).sum();

        if is_space {
            if cur_w + tw <= width {
                cur.extend_from_slice(token);
                cur_w += tw;
            } else {
                // Spaces at a wrap point are dropped.
                out.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
        } else if cur_w + tw <= width {
            cur.extend_from_slice(token);
            cur_w += tw;
        } else if tw <= width {
            trim_trailing_spaces(&mut cur);
            out.push(std::mem::take(&mut cur));
            cur.extend_from_slice(token);
            cur_w = tw;
        } else {
            // A single word wider than the screen: break it by character.
            if !cur.is_empty() {
                trim_trailing_spaces(&mut cur);
                out.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            for &(c, s) in token {
                let cw = char_width(c);
                if cur_w + cw > width && !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                cur.push((c, s));
                cur_w += cw;
            }
        }
        i = j;
    }
    out.push(cur);
    out
}

fn chars_to_line(chars: &[StyledChar]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut cur_style = Style::default();
    let mut started = false;
    for &(c, s) in chars {
        if started && s != cur_style {
            spans.push(Span::styled(std::mem::take(&mut buf), cur_style));
        }
        cur_style = s;
        started = true;
        buf.push(c);
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, cur_style));
    }
    Line::from(spans)
}

/// Text of the branch counter in the first message's header, `summary branch 1/2`.
/// (Not a swipe: `‹n/m›` is for swipes and greetings.)
fn branch_text(cur: usize, total: usize) -> String {
    format!("summary branch {}/{}", cur + 1, total)
}

/// Columns (relative to the message area) that count as a tap on the
/// `summary branch n/m` counter in the first message's header, with two columns each
/// side like `swipe_zone`.
pub fn branch_zone(name: &str, cur: usize, total: usize) -> (usize, usize) {
    let name_w: usize = name.chars().map(|c| c.width().unwrap_or(0)).sum();
    let counter = branch_text(cur, total).chars().count();
    let start = name_w + 2 + 2;
    (start.saturating_sub(2), start + counter + 2)
}

/// Columns (relative to the message area) that count as a tap on the
/// `‹n/m›` swipe counter in a message header: the counter itself plus two
/// columns each side, so a fingertip can hit it. `branch` is the branch
/// counter that sits before it, if any.
pub fn swipe_zone(
    name: &str,
    cur: usize,
    total: usize,
    branch: Option<(usize, usize)>,
) -> (usize, usize) {
    let name_w: usize = name.chars().map(|c| c.width().unwrap_or(0)).sum();
    let counter = format!("\u{2039}{}/{}\u{203A}", cur + 1, total).chars().count();
    // The name sits in a badge padded with one space each side, then two
    // spaces before the counter.
    let mut start = name_w + 2 + 2;
    if let Some((bc, bt)) = branch {
        start += branch_text(bc, bt).chars().count() + 2;
    }
    (start.saturating_sub(2), start + counter + 2)
}

/// Render one chat message: a name header, the wrapped body, and the row that
/// separates it from the next message: a faint line of `sep` (config
/// `msg_seperator`), or an empty row when that is `None`.
pub fn message_lines(
    name: &str,
    _is_user: bool,
    text: &str,
    width: usize,
    swipe: Option<(usize, usize)>,
    branch: Option<(usize, usize)>,
    streaming: bool,
    highlight: bool,
    sep: Option<char>,
) -> Vec<Line<'static>> {
    let dim = Style::default().fg(Color::DarkGray);
    // Names (yours and the character's) are black on white, bold.
    // RGB white, because ANSI "white" is a light grey in many terminals.
    let mut name_style = Style::default()
        .fg(Color::Black)
        .bg(NAME_WHITE)
        .add_modifier(Modifier::BOLD);
    if highlight {
        name_style = name_style.add_modifier(Modifier::REVERSED);
    }
    let mut header: Vec<Span<'static>> = vec![Span::styled(format!(" {} ", name), name_style)];
    if let Some((cur, total)) = branch {
        header.push(Span::styled(
            format!("  {}", branch_text(cur, total)),
            Style::default().fg(Color::Cyan),
        ));
    }
    if let Some((cur, total)) = swipe {
        if total > 1 {
            header.push(Span::styled(
                format!("  \u{2039}{}/{}\u{203A}", cur + 1, total),
                dim,
            ));
        }
    }
    if streaming {
        header.push(Span::styled("  \u{2026}".to_string(), dim));
    }

    let mut out: Vec<Line<'static>> = vec![Line::from(header)];
    if text.is_empty() && !streaming {
        out.push(Line::from(Span::styled("(empty message)".to_string(), dim)));
    } else {
        for para in style_text(text) {
            for wrapped in wrap_styled(&para, width) {
                out.push(chars_to_line(&wrapped));
            }
        }
    }
    // Thin, faint rule between messages (one row), or an empty row.
    out.push(match sep {
        Some(c) => {
            // A wide character takes two columns: fewer of them fill the row.
            let cols = c.width().unwrap_or(1).max(1);
            Line::from(Span::styled(
                c.to_string().repeat((width.max(1) / cols).max(1)),
                Style::default().fg(Color::Rgb(52, 52, 52)),
            ))
        }
        None => Line::from(""),
    });
    out
}

// ------------------------------------------------------------------- input

/// Hard-wrap the input text to `width` columns and locate the cursor.
/// Returns the display lines, the cursor as (row, column), and the char index
/// at which each display row starts.
pub fn layout_input(
    text: &str,
    cursor: usize,
    width: usize,
) -> (Vec<String>, (usize, usize), Vec<usize>) {
    let width = width.max(1);
    let mut starts: Vec<usize> = vec![0];
    let mut lines: Vec<String> = vec![String::new()];
    let mut col = 0usize;
    let mut cur_pos = (0usize, 0usize);
    let mut total = 0usize;

    for (i, ch) in text.chars().enumerate() {
        total = i + 1;
        if ch == '\n' {
            if i == cursor {
                cur_pos = (lines.len() - 1, col);
            }
            lines.push(String::new());
            starts.push(i + 1);
            col = 0;
            continue;
        }
        let w = char_width(ch);
        if col + w > width && col > 0 {
            lines.push(String::new());
            starts.push(i);
            col = 0;
        }
        if i == cursor {
            cur_pos = (lines.len() - 1, col);
        }
        if let Some(last) = lines.last_mut() {
            last.push(ch);
        }
        col += w;
    }
    if cursor >= total {
        cur_pos = (lines.len() - 1, col);
    }
    // A cursor sitting exactly at the right edge would fall on the border.
    cur_pos.1 = cur_pos.1.min(width.saturating_sub(1));
    (lines, cur_pos, starts)
}

/// New cursor (char index) after pressing Up (`dir < 0`) or Down in the
/// wrapped input box, keeping the on-screen column.
pub fn vertical_target(text: &str, cursor: usize, width: usize, dir: i32) -> usize {
    let (lines, (row, col), starts) = layout_input(text, cursor, width);
    let total = text.chars().count();
    let target = if dir < 0 {
        if row == 0 {
            return 0;
        }
        row - 1
    } else {
        if row + 1 >= lines.len() {
            return total;
        }
        row + 1
    };
    let start = starts[target];
    let row_len = lines[target].chars().count();
    let mut w = 0usize;
    let mut n = 0usize;
    for c in lines[target].chars() {
        if w >= col {
            break;
        }
        w += char_width(c);
        n += 1;
    }
    // A row that ended by wrapping has no spot of its own past its last char.
    let wrapped = target + 1 < starts.len() && starts[target + 1] == start + row_len;
    if wrapped && n >= row_len && row_len > 0 {
        n = row_len - 1;
    }
    start + n
}

/// Char index of the spot a mouse click landed on in the wrapped input box.
/// `row` is the display row (counted from the first row of the text, not of
/// the visible part) and `col` the column inside the box.
pub fn index_at(text: &str, width: usize, row: usize, col: usize) -> usize {
    let (lines, _, starts) = layout_input(text, 0, width);
    let row = row.min(lines.len() - 1);
    let start = starts[row];
    let row_len = lines[row].chars().count();
    let mut w = 0usize;
    let mut n = 0usize;
    for c in lines[row].chars() {
        let cw = char_width(c);
        if w + cw > col {
            break;
        }
        w += cw;
        n += 1;
    }
    // A row that ended by wrapping has no spot of its own past its last char.
    let wrapped = row + 1 < starts.len() && starts[row + 1] == start + row_len;
    if wrapped && n >= row_len && row_len > 0 {
        n = row_len - 1;
    }
    start + n
}

/// One display row of the input box, with the VISUAL selection (if any)
/// drawn in reverse video. `start` is the char index the row starts at.
fn input_row(
    row: &str,
    start: usize,
    text: &[char],
    sel: Option<(usize, usize)>,
    found: &[(usize, usize)],
    replaced: bool,
    misspelled: &[(usize, usize)],
    comment: &[(usize, usize)],
) -> Line<'static> {
    if sel.is_none() && found.is_empty() && misspelled.is_empty() && comment.is_empty() {
        return Line::from(row.to_string());
    }
    // VISUAL selection: reverse video. `:s` matches: black on yellow.
    let sel_style = Style::default().add_modifier(Modifier::REVERSED);
    let find_style = Style::default()
        .fg(Color::Black)
        .bg(if replaced { Color::Green } else { Color::Yellow });
    // Misspelled words: underlined in the terminal theme's red.
    let red = termcolors::palette().red_color();
    let spell_style = Style::default()
        .add_modifier(Modifier::UNDERLINED)
        .underline_color(red);
    // `//` and `/* */` notes of a card (`/char`): gray, and no spell marks.
    let note_style = Style::default().fg(Color::DarkGray);
    let style_at = |i: usize| -> Style {
        if sel.map_or(false, |(s, e)| i >= s && i < e) {
            sel_style
        } else if found.iter().any(|&(s, e)| i >= s && i < e) {
            find_style
        } else if comment.iter().any(|&(s, e)| i >= s && i < e) {
            note_style
        } else if misspelled.iter().any(|&(s, e)| i >= s && i < e) {
            spell_style
        } else {
            Style::default()
        }
    };
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut buf_style = Style::default();
    let mut idx = start;
    for c in row.chars() {
        let st = style_at(idx);
        if !buf.is_empty() && st != buf_style {
            spans.push(Span::styled(std::mem::take(&mut buf), buf_style));
        }
        buf_style = st;
        buf.push(c);
        idx += 1;
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, buf_style));
    }
    // A highlighted line break (or an empty highlighted line) shows as a coloured blank.
    if text.get(idx) == Some(&'\n') {
        let st = style_at(idx);
        if st != Style::default() {
            spans.push(Span::styled(" ".to_string(), st));
        }
    }
    Line::from(spans)
}

/// One display row of a card field that is not being edited, with `//` and
/// `/* */` notes drawn gray (`comment` holds their char ranges).
fn comment_row(row: &str, start: usize, base: Style, comment: &[(usize, usize)]) -> Line<'static> {
    if comment.is_empty() {
        return Line::from(Span::styled(row.to_string(), base));
    }
    let note = Style::default().fg(Color::DarkGray);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut cur = base;
    let mut idx = start;
    for c in row.chars() {
        let inside = comment.iter().any(|&(s, e)| idx >= s && idx < e);
        let st = if inside { note } else { base };
        if !buf.is_empty() && st != cur {
            spans.push(Span::styled(std::mem::take(&mut buf), cur));
        }
        cur = st;
        buf.push(c);
        idx += 1;
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, cur));
    }
    Line::from(spans)
}

/// 1234567 -> "1,234,567".
fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

const HELP: &[&str] = &[
    "Enter            send (saves, while editing a message)",
    "Shift+Enter      new line (also Alt+Enter, Ctrl+J)",
    "                 an empty box asks the character to reply",
    "Ctrl+K           continue the last reply (/continue, /c)",
    "Ctrl+R           regenerate last reply (replaces it and its swipes)",
    "Ctrl+P           edit the character's last reply",
    "Ctrl+O           edit your own last message",
    "Ctrl+S           editing the card's greeting: save it to the card",
    "Left/Right       previous / next swipe (or greeting), empty box",
    "Ctrl+Left/Right  previous / next summary branch (wraps)",
    "Tab              spelling suggestions for the word under the cursor",
    "                 (spellcheck: true; also in /char, /persona and /lore)",
    "Esc              stop generating / cancel an edit",
    "",
    "Edit any message: click it. It opens in INSERT mode.",
    "Enter (or [Save]) saves, Esc cancels.",
    "Your unsent draft comes back afterwards.",
    "",
    "PgUp/PgDn, Up/Down, wheel  scroll chat (wheel on box: box)",
    "Tap the swipe bar: other swipes",
    "Ctrl+A/E/W/U     line start / end / delete word / clear",
    "",
    "Vim: qq (INSERT -> NORMAL), i a I A o O (-> INSERT)",
    "     (keys and on/off: ~/.config/tarpi/config.yaml)",
    "  h j k l w W b B e E 0 ^ $ gg G, x X r dd D cc C yy p P u",
    "  <leader>y{motion}, <leader>yy, <leader>Y, VISUAL <leader>y:",
    "  yank to the system clipboard (leader: config vim.leader)",
    "  Ctrl+U / Ctrl+D: half a box up / down (in /char, on",
    "  into the previous / next field)",
    "  counts and d/c/y + motion work; Enter sends in NORMAL",
    "  v / V: VISUAL (chars / lines), move, then d y c p; o swaps",
    "  ends, Esc leaves VISUAL",
    "  :%s/find/new/[gi]  replace in all lines (live highlight);",
    "  :s (this line), :3,5s, '<,'>s from VISUAL; & = match; Esc",
    "Ctrl+C or Ctrl+D quit",
    "",
    "/continue (/c)   continue the last reply",
    "/regen [nudge]   regenerate the last reply (replaces its swipes);",
    "                 the optional nudge steers just this reply",
    "/swipe [nudge]   write a new swipe of the last reply (the old ones are",
    "                 kept); the optional nudge steers just this one",
    "/undo [n] (/u)   delete the last message, or the last n",
    "/new             new chat with this character",
    "/newchat (/nc)   delete THIS chat and start a new one",
    "/summarize       write an in-depth summary of the shown branch as",
    "                 a NEW summary branch (also /summ; branch 1 is the whole",
    "                 conversation; nothing is replaced; a copy is also",
    "                 saved under another name)",
    "/deletebranch    delete the branch that is showing (also /db)",
    "/chats           window to open, create or delete (d) chats of this",
    "                 character",
    "/chars           window to switch to another character, or delete one",
    "                 (d, asks first; takes all its chats with it)",
    "/import <card>   add a character: a card .png / .json file, a chub.ai",
    "                 or pygmalion.chat link (or a link to a card file)",
    "/persona         window to choose who you are in the chat; p sets where",
    "                 its description goes into the prompt",
    "/system          window to choose, create (+), edit (e) or delete (d)",
    "                 the system prompt",
    "/prompt          show the whole prompt the next reply is sent with",
    "                 (scroll; Esc closes)",
    "/note            edit this chat's Author's Note: text, position",
    "                 (before / after / chat), depth, role, interval (also /an)",
    "/memory          vector memory: switch it on, how many old messages",
    "                 come back, how close, where they go in the prompt",
    "/api             open the API settings; quitting them (q) brings",
    "                 you back to this chat",
    "/formatting      edit the context and instruct templates; quitting",
    "                 them (q) brings you back to this chat",
    "/lore            manage lorebooks: write, edit, import (SillyTavern",
    "                 World Info, cards), switch on for all chats or for",
    "                 some characters; quitting (q) brings you back here",
    "/config          edit config.yaml in a window (vim modes as in the",
    "                 chat box); Ctrl+S saves and applies it right away",
    "/char            view and edit this character's card",
    "/char-img        show the card's picture in a square window",
    "/one-shot        lock THIS chat: no editing, undo, regen, swipes or",
    "                 branches (type it twice to confirm; permanent)",
    "/help            this screen",
    "",
    "Typing / opens a menu of commands above the box, the top one",
    "highlighted. Tab / Down go to the next one, Shift+Tab / Up to the",
    "previous one (filling it in), a click picks one. Enter chooses the",
    "highlighted one and adds a space (/su -> /summarize ).",
    "After /import, Tab opens a menu of folders and card files and fills",
    "in the first one (Tab / Up / Down move on, Enter on a folder opens it,",
    "on a file imports it).",
    "Esc or Ctrl+C hides any menu above the box (command, /import path,",
    "spelling) and does nothing else.",
    "With only a / in the box, Up brings back the commands you ran before",
    "(/regen softer, ...): Up goes further back, Down forward, Down past",
    "the newest puts the / back.",
    "/quit            exit",
];

/// Columns of the terminal (80 when it cannot be read), for the plain-text menus.
pub fn term_cols() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
        .max(20)
}

/// Word-wrap one help line to `w` columns; continuation rows are indented.
fn wrap_help(line: &str, w: usize) -> Vec<String> {
    let w = w.max(1);
    if line.chars().count() <= w {
        return vec![line.to_string()];
    }
    let base: usize = if w >= 40 { 17 } else { 2 };
    let indent = base.min(w / 2);
    let pad = " ".repeat(indent);
    let mut rows: Vec<String> = Vec::new();
    // Keep the line's own leading spaces on its first row.
    let lead = line.len() - line.trim_start().len();
    let mut cur = " ".repeat(lead.min(w / 2));
    for tok in line.trim_start().split(' ') {
        if tok.is_empty() && cur.is_empty() {
            continue;
        }
        let mut tok: String = tok.to_string();
        loop {
            let used = cur.chars().count();
            let tl = tok.chars().count();
            let sep = if cur.is_empty() || cur.trim().is_empty() { 0 } else { 1 };
            if used + sep + tl <= w {
                if sep == 1 {
                    cur.push(' ');
                }
                cur.push_str(&tok);
                break;
            }
            if !cur.trim().is_empty() {
                rows.push(cur.clone());
                cur = pad.clone();
                continue;
            }
            // A single word longer than the row: cut it.
            let room = w.saturating_sub(used).max(1);
            let head: String = tok.chars().take(room).collect();
            let rest: String = tok.chars().skip(room).collect();
            cur.push_str(&head);
            rows.push(cur.clone());
            cur = pad.clone();
            tok = rest;
            if tok.is_empty() {
                break;
            }
        }
    }
    if !cur.trim().is_empty() || rows.is_empty() {
        rows.push(cur);
    }
    rows
}

/// A one-row button: the label centred on a coloured background.
fn draw_flat_button(f: &mut Frame, area: Rect, label: &str, color: Color) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    f.render_widget(
        Paragraph::new(format!("{:^1$}", label, area.width as usize)).style(
            Style::default()
                .fg(Color::Black)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
        area,
    );
}

/// A bordered button with its label centred.
fn draw_button(f: &mut Frame, area: Rect, label: &str, color: Color) {
    let rows = area.height.saturating_sub(2) as usize;
    let lines: Vec<Line<'static>> = (0..rows)
        .map(|r| {
            if r == rows / 2 {
                Line::from(Span::styled(
                    label.to_string(),
                    Style::default()
                        .fg(Color::Black)
                        .bg(color)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from("")
            }
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(color)),
        ),
        area,
    );
}

/// Display width of a string in terminal columns.
fn cols_of(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// `s` cut to at most `w` columns, with an ellipsis when something was left out.
fn fit_cols(s: &str, w: usize) -> String {
    if cols_of(s) <= w {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw + 1 > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    if w > 0 {
        out.push('\u{2026}');
    }
    out
}

/// The `/command` menu: a small list that opens just above the input box
/// while a command word is being typed. Tab / Shift+Tab / the arrows move the
/// highlighted entry and fill it into the box, a click fills one in too.
/// `top` is the first screen row it may cover (below the title bar).
fn draw_completion(f: &mut Frame, app: &mut App, input_area: Rect, top: u16) {
    app.comp_area = Rect::default();
    app.comp_rows.clear();
    app.path_rows.clear();
    // After `/import `, the menu lists paths instead of commands.
    if let Some((list, sel, _)) = app.path_completion() {
        draw_path_menu(f, app, input_area, top, &list, sel);
        return;
    }
    let Some((list, sel, _)) = app.completion() else {
        return;
    };
    let sel = Some(sel);
    // A border row above and below plus at least one entry.
    let avail = input_area.y.saturating_sub(top) as usize;
    if avail < 3 || input_area.width < 8 {
        return;
    }
    let shown = list.len().min(8).min(avail - 2);
    // Keep the highlighted entry in view, at the bottom edge once it scrolls.
    let first = match sel {
        Some(i) if i >= shown => i + 1 - shown,
        _ => 0,
    };
    let label = |c: &crate::cmds::Cmd| -> String {
        if c.alias.is_empty() {
            format!("/{}", c.name)
        } else {
            format!("/{} (/{})", c.name, c.alias)
        }
    };
    let name_w = list.iter().map(|c| cols_of(&label(c))).max().unwrap_or(0);
    let desc_w = list.iter().map(|c| cols_of(c.desc)).max().unwrap_or(0);
    let want = 1 + name_w + 2 + desc_w + 1;
    let w = (want as u16 + 2).min(input_area.width).max(4);
    let iw = (w - 2) as usize;
    let h = shown as u16 + 2;
    let rect = Rect::new(input_area.x, input_area.y - h, w, h);

    let mut rows: Vec<Line<'static>> = Vec::new();
    for (n, c) in list.iter().enumerate().skip(first).take(shown) {
        let on = sel == Some(n);
        let (name_style, desc_style) = if on {
            let st = Style::default().fg(Color::Black).bg(Color::Cyan);
            (st.add_modifier(Modifier::BOLD), st)
        } else {
            (
                Style::default().fg(Color::Cyan),
                Style::default().fg(Color::DarkGray),
            )
        };
        let name_txt = fit_cols(&format!(" {}", label(c)), iw);
        let left = cols_of(&name_txt);
        // Pad the name column so the descriptions line up.
        let pad = (1 + name_w + 2).saturating_sub(left);
        let desc_room = iw.saturating_sub(left + pad);
        let desc_txt = fit_cols(c.desc, desc_room);
        let used = left + pad + cols_of(&desc_txt);
        let mut spans = vec![
            Span::styled(name_txt, name_style),
            Span::styled(" ".repeat(pad.min(iw.saturating_sub(left))), desc_style),
            Span::styled(desc_txt, desc_style),
        ];
        if on && used < iw {
            // The highlight runs to the right edge.
            spans.push(Span::styled(" ".repeat(iw - used), desc_style));
        }
        rows.push(Line::from(spans));
        app.comp_rows.push((rect.y + 1 + (n - first) as u16, c.name));
    }
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    if let (Some(i), true) = (sel, list.len() > shown) {
        block = block.title_bottom(Line::from(format!(" {}/{} ", i + 1, list.len())));
    }
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(rows).block(block), rect);
    app.comp_area = rect;
}

/// The `/import` path menu: drawn like the command menu, just above the input
/// box, but listing the folders and card files that the path typed after
/// `/import ` leads to (name and size; folders end with `/`). Tab / the arrows
/// fill an entry in, a click does too (see `App::path_key`).
fn draw_path_menu(
    f: &mut Frame,
    app: &mut App,
    input_area: Rect,
    top: u16,
    list: &[crate::pathcomp::Entry],
    sel: usize,
) {
    use crate::pathcomp::{human_size, Entry};
    // A border row above and below plus at least one entry.
    let avail = input_area.y.saturating_sub(top) as usize;
    if avail < 3 || input_area.width < 8 {
        return;
    }
    let shown = list.len().min(8).min(avail - 2);
    // Keep the highlighted entry in view, at the bottom edge once it scrolls.
    let first = if sel >= shown { sel + 1 - shown } else { 0 };
    let label = |e: &Entry| -> String {
        if e.is_dir {
            format!("{}/", e.name)
        } else {
            e.name.clone()
        }
    };
    let desc = |e: &Entry| -> String {
        if e.is_dir {
            "folder".to_string()
        } else {
            human_size(e.size)
        }
    };
    let name_w = list.iter().map(|e| cols_of(&label(e))).max().unwrap_or(0);
    let desc_w = list.iter().map(|e| cols_of(&desc(e))).max().unwrap_or(0);
    let want = 1 + name_w + 2 + desc_w + 1;
    let w = (want.min(u16::MAX as usize - 2) as u16 + 2)
        .min(input_area.width)
        .max(4);
    let iw = (w - 2) as usize;
    let h = shown as u16 + 2;
    let rect = Rect::new(input_area.x, input_area.y - h, w, h);

    let mut rows: Vec<Line<'static>> = Vec::new();
    for (n, e) in list.iter().enumerate().skip(first).take(shown) {
        let on = n == sel;
        let (name_style, desc_style) = if on {
            let st = Style::default().fg(Color::Black).bg(Color::Cyan);
            (st.add_modifier(Modifier::BOLD), st)
        } else {
            let name = if e.is_dir {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            };
            (name, Style::default().fg(Color::DarkGray))
        };
        let name_txt = fit_cols(&format!(" {}", label(e)), iw);
        let left = cols_of(&name_txt);
        // Pad the name column so the sizes line up.
        let pad = (1 + name_w + 2).saturating_sub(left);
        let desc_room = iw.saturating_sub(left + pad);
        let desc_txt = fit_cols(&desc(e), desc_room);
        let used = left + pad + cols_of(&desc_txt);
        let mut spans = vec![
            Span::styled(name_txt, name_style),
            Span::styled(" ".repeat(pad.min(iw.saturating_sub(left))), desc_style),
            Span::styled(desc_txt, desc_style),
        ];
        if on && used < iw {
            // The highlight runs to the right edge.
            spans.push(Span::styled(" ".repeat(iw - used), desc_style));
        }
        rows.push(Line::from(spans));
        app.path_rows.push((rect.y + 1 + (n - first) as u16, n));
    }
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    if list.len() > shown {
        block = block.title_bottom(Line::from(format!(" {}/{} ", sel + 1, list.len())));
    }
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(rows).block(block), rect);
    app.comp_area = rect;
}

/// Where the spelling menu goes.
#[derive(Clone, Copy)]
pub(crate) enum SpellPlace {
    /// Next to the text cursor, at this screen cell (the windows).
    At(u16, u16),
    /// Just above the input box (the chat).
    Above { input_area: Rect, top: u16 },
}

/// The spelling menu: suggestions for the misspelled word the cursor is in.
/// In the chat it is drawn like the command menu, just above the input box;
/// in the `/char` and persona windows it sits at the text cursor.
fn draw_spell_menu(f: &mut Frame, app: &mut App, input_area: Rect, top: u16) {
    app.spell_area = Rect::default();
    app.spell_rows.clear();
    let anchor = app.card_modal.as_ref().map(|m| m.cursor_xy);
    // The `/command` menu takes the place when it is showing.
    if anchor.is_none() && (app.completion().is_some() || app.path_completion().is_some()) {
        return;
    }
    if matches!(anchor, Some(None))
        || app.show_help
        || app.prompt_view.is_some()
        || app.image_modal.is_some()
        || app.picker.is_some()
        || app.pos_edit.is_some()
    {
        return;
    }
    let Some(menu) = app.spell_menu.as_ref() else {
        return;
    };
    let place = match anchor {
        Some(Some((cx, cy))) => SpellPlace::At(cx, cy),
        _ => SpellPlace::Above { input_area, top },
    };
    if let Some((rect, hits)) = place_spell_menu(f, menu, place) {
        app.spell_area = rect;
        app.spell_rows = hits;
    }
}

/// Draw `menu` at `place`. Returns where it went and the screen row of each
/// entry (for clicks), or `None` when there is no room for it.
pub(crate) fn place_spell_menu(
    f: &mut Frame,
    menu: &crate::app::SpellMenu,
    place: SpellPlace,
) -> Option<(Rect, Vec<(u16, usize)>)> {
    let screen = f.area();
    // In a window the menu may go above or below the cursor's row: the
    // larger side is the room it has.
    let avail = match place {
        SpellPlace::At(_, cy) => (cy as usize).max(screen.height.saturating_sub(cy + 1) as usize),
        SpellPlace::Above { input_area, top } => input_area.y.saturating_sub(top) as usize,
    };
    if avail < 3 || screen.width < 8 {
        return None;
    }
    let shown = menu.sugg.len().min(8).min(avail - 2);
    let first = if menu.sel >= shown { menu.sel + 1 - shown } else { 0 };
    let want = menu.sugg.iter().map(|s| cols_of(s)).max().unwrap_or(0) + 2;
    let hint = " Tab next \u{b7} Enter pick \u{b7} Esc ";
    let title = format!(" {} ", menu.word);
    let w = (want.max(cols_of(hint)).max(cols_of(&title)) as u16 + 2)
        .min(screen.width)
        .max(4);
    let iw = (w - 2) as usize;
    let h = shown as u16 + 2;
    let rect = match place {
        SpellPlace::At(cx, cy) => {
            // Below the cursor's row when it is in the top half of the
            // screen, above it when in the bottom half; the other side when
            // the menu does not fit where it would go.
            let x = cx.min(screen.width.saturating_sub(w));
            let fits_below = cy + 1 + h <= screen.height;
            let fits_above = cy >= h;
            let below = if cy < screen.height / 2 { fits_below || !fits_above } else { !fits_above };
            let y = (if below { cy + 1 } else { cy - h }).min(screen.height.saturating_sub(h));
            Rect::new(x, y, w, h)
        }
        SpellPlace::Above { input_area, .. } => {
            Rect::new(input_area.x, input_area.y - h, w.min(input_area.width), h)
        }
    };
    let iw = iw.min(rect.width.saturating_sub(2) as usize);
    let red = termcolors::palette().red_color();

    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut hits: Vec<(u16, usize)> = Vec::new();
    for (n, s) in menu.sugg.iter().enumerate().skip(first).take(shown) {
        let txt = fit_cols(&format!(" {}", s), iw);
        let used = cols_of(&txt);
        let line = if n == menu.sel {
            let st = Style::default().fg(Color::Black).bg(Color::Cyan);
            let mut spans = vec![Span::styled(txt, st.add_modifier(Modifier::BOLD))];
            if used < iw {
                spans.push(Span::styled(" ".repeat(iw - used), st));
            }
            Line::from(spans)
        } else {
            Line::from(Span::styled(txt, Style::default().fg(Color::Cyan)))
        };
        rows.push(line);
        hits.push((rect.y + 1 + (n - first) as u16, n));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Line::from(Span::styled(
            title,
            Style::default().add_modifier(Modifier::UNDERLINED).underline_color(red),
        )))
        .title_bottom(Line::from(Span::styled(
            hint.to_string(),
            Style::default().fg(Color::DarkGray),
        )));
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(rows).block(block), rect);
    Some((rect, hits))
}

/// Cut `line` into rows of at most `width` columns, at a space when there is
/// one in the row, anywhere otherwise. An empty line stays one empty row.
fn wrap_rows(line: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut w: usize = 0;
    for c in line.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        while w + cw > width && !cur.is_empty() {
            let cut = cur.rfind(' ').filter(|&p| !cur[..p].trim().is_empty());
            match cut {
                Some(p) => {
                    let rest = cur[p + 1..].to_string();
                    cur.truncate(p);
                    out.push(std::mem::take(&mut cur));
                    w = rest
                        .chars()
                        .map(|x| UnicodeWidthChar::width(x).unwrap_or(0))
                        .sum();
                    cur = rest;
                }
                None => {
                    out.push(std::mem::take(&mut cur));
                    w = 0;
                }
            }
        }
        cur.push(c);
        w += cw;
    }
    out.push(cur);
    out
}

/// The `/prompt` window: the whole prompt, scrollable, over the full area.
fn draw_prompt_modal(f: &mut Frame, area: Rect, v: &mut crate::app::PromptView) {
    f.render_widget(Clear, area);
    let iw = (area.width as usize).saturating_sub(2).max(1);
    let rows = (area.height as usize).saturating_sub(2).max(1);
    if v.wrapped_w != iw || v.wrapped.is_empty() {
        let mut lines: Vec<String> = Vec::new();
        for l in v.texts[v.tab.min(1)].split('\n') {
            lines.extend(wrap_rows(l, iw));
        }
        v.wrapped = lines;
        v.wrapped_w = iw;
    }
    let max_scroll = v.wrapped.len().saturating_sub(rows);
    v.page = rows;
    v.scroll = v.scroll.min(max_scroll);
    let shown: Vec<Line<'static>> = v
        .wrapped
        .iter()
        .skip(v.scroll)
        .take(rows)
        .map(|l| Line::from(l.clone()))
        .collect();
    let bottom = if max_scroll > 0 {
        format!(
            " \u{2191}/\u{2193} PgUp/PgDn scroll \u{b7} Tab page \u{b7} Esc closes ({}/{}) ",
            v.scroll + 1,
            max_scroll + 1
        )
    } else {
        " Tab page \u{b7} Esc closes ".to_string()
    };
    // The two pages as tabs in the title; the one showing is highlighted.
    let on = Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let tab_style = |i: usize| if v.tab == i { on } else { Style::default() };
    let title = Line::from(vec![
        Span::raw(" prompt "),
        Span::styled(" 1 pre-prompt ", tab_style(0)),
        Span::raw(" "),
        Span::styled(" 2 post-prompt ", tab_style(1)),
        Span::raw(v.title.clone()),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_bottom(Line::from(bottom))
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(Paragraph::new(shown).block(block), area);
}

/// The choose-one window of `/characters`, `/chats` and `/persona`: the
/// entries, with the keys on the bottom edge.
/// `full`: the window fills `area` (the start-up character window) instead of
/// being a centred box, and Esc quits instead of cancelling.
pub fn draw_picker(f: &mut Frame, area: Rect, p: &mut Picker, full: bool) {
    let vis = p.visible();
    let rect = if full {
        area
    } else {
        let w = area.width.min(76);
        // Borders (2) + one row per entry, but never the whole screen.
        let want = vis.len().max(1) as u16 + 2;
        let h = want.min(area.height.saturating_sub(2)).max(3).min(area.height);
        centered(area, w, h)
    };
    f.render_widget(Clear, rect);

    let bottom: Line<'static> = if let Some(key) = &p.confirm {
        let shown = p
            .items
            .iter()
            .find(|i| i.key == *key)
            .map(|i| i.label.clone())
            .unwrap_or_else(|| key.clone());
        // Where deleting takes more with it (a character's chats) the window
        // says so. (The window is at most 76 columns wide, so the question is
        // kept short.)
        let text = if p.delete_what.is_empty() {
            format!(" Delete '{}'? y = yes, any other key = no ", shown)
        } else {
            format!(
                " Delete '{}'{}? y = yes, other = no ",
                fit_cols(&shown, 12),
                p.delete_what
            )
        };
        Line::from(Span::styled(text, Style::default().fg(Color::Red)))
    } else if let Some((msg, is_err)) = &p.note {
        Line::from(Span::styled(
            format!(" {} ", msg),
            Style::default().fg(if *is_err { Color::Red } else { Color::Green }),
        ))
    } else {
        let edit = if p.items.iter().any(|i| i.editable) {
            " \u{b7} e edit"
        } else {
            ""
        };
        let del = if p.items.iter().any(|i| i.deletable) {
            " \u{b7} d delete"
        } else {
            ""
        };
        let pos = if p.position_key && p.items.iter().any(|i| i.editable) {
            " \u{b7} p position"
        } else {
            ""
        };
        Line::from(format!(
            " \u{2191}/\u{2193} move \u{b7} Enter choose{}{}{} \u{b7} Esc {} ",
            edit,
            pos,
            del,
            if full { "quit" } else { "cancel" }
        ))
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", p.title))
        .title_bottom(bottom)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height < 1 || inner.width < 4 {
        return;
    }

    // Guide lines at the bottom, only when the list still has room above them.
    let foot_n = p.footer.len() as u16;
    let (list, foot) = if full && foot_n > 0 && inner.height >= foot_n + 1 + vis.len().max(1) as u16 {
        let lh = inner.height - foot_n - 1;
        (
            Rect::new(inner.x, inner.y, inner.width, lh),
            Some(Rect::new(inner.x, inner.y + lh + 1, inner.width, foot_n)),
        )
    } else {
        (inner, None)
    };
    if let Some(fr) = foot {
        let cols = fr.width as usize;
        let lines: Vec<Line<'static>> = p
            .footer
            .iter()
            .map(|t| {
                Line::from(Span::styled(
                    fit_cols(&format!(" {}", t), cols),
                    Style::default().fg(Color::Yellow),
                ))
            })
            .collect();
        f.render_widget(Paragraph::new(lines), fr);
    }
    let rows_n = list.height as usize;
    p.window = rect;
    p.rows = list;
    p.page = rows_n.max(1);

    // Keep the selected row in view.
    if p.sel >= vis.len() {
        p.sel = vis.len().saturating_sub(1);
    }
    if p.sel < p.first {
        p.first = p.sel;
    }
    if p.sel >= p.first + rows_n {
        p.first = p.sel + 1 - rows_n;
    }
    p.first = p.first.min(vis.len().saturating_sub(rows_n));

    // Entries.
    let cols = list.width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    if vis.is_empty() {
        lines.push(Line::from(Span::styled(
            "  nothing to choose",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (i, &idx) in vis.iter().enumerate().skip(p.first).take(rows_n) {
        let it = &p.items[idx];
        let selected = i == p.sel;
        let mut left = format!("{} {}", if selected { "\u{25b8}" } else { " " }, it.label);
        if it.current {
            left.push_str(" (current)");
        }
        let left = fit_cols(&left, cols);
        let used = cols_of(&left);
        let detail = if !it.detail.is_empty() && used + 4 < cols {
            fit_cols(&format!("  {}", it.detail), cols - used)
        } else {
            String::new()
        };
        let pad = " ".repeat(cols.saturating_sub(used + cols_of(&detail)));
        let (base, dim) = if selected {
            let s = Style::default().add_modifier(Modifier::REVERSED);
            (s, s)
        } else {
            (Style::default(), Style::default().fg(Color::DarkGray))
        };
        lines.push(Line::from(vec![
            Span::styled(left, base),
            Span::styled(detail, dim),
            Span::styled(pad, base),
        ]));
    }
    f.render_widget(Paragraph::new(lines), list);
}

/// The window of the `p` key in `/persona`: where the description of a persona
/// is injected into the prompt (position, and for the chat position depth and
/// role).
pub fn draw_pos_edit(f: &mut Frame, area: Rect, e: &mut PosEdit) {
    let rows = e.rows();
    let w = area.width.min(64);
    // Borders (2), the rows, an empty row and a line about the position.
    let want = rows.len() as u16 + 4;
    let h = want.min(area.height).max(3).min(area.height);
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);

    let bottom: Line<'static> = if let Some(msg) = &e.note {
        Line::from(Span::styled(
            format!(" {} ", msg),
            Style::default().fg(Color::Red),
        ))
    } else {
        Line::from(" \u{2191}/\u{2193} row \u{b7} \u{2190}/\u{2192} change \u{b7} Enter save \u{b7} Esc cancel ")
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" position of {} ", fit_cols(&e.label, (w as usize).saturating_sub(16))))
        .title_bottom(bottom)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height < 1 || inner.width < 4 {
        return;
    }
    let cols = inner.width as usize;
    e.window = rect;
    let shown = (rows.len() as u16).min(inner.height);
    e.rows_area = Rect::new(inner.x, inner.y, inner.width, shown);

    let mut lines: Vec<Line<'static>> = Vec::new();
    for r in rows.iter().take(shown as usize) {
        let selected = *r == e.sel;
        let value = match r {
            posedit::Row::Position => posedit::position_name(e.position),
            posedit::Row::Depth => e.depth.to_string(),
            posedit::Row::Role => posedit::role_name(e.role).to_string(),
        };
        let text = fit_cols(
            &format!(
                "{} {:<9}\u{2039} {} \u{203a}",
                if selected { "\u{25b8}" } else { " " },
                r.label(),
                value
            ),
            cols,
        );
        let pad = " ".repeat(cols.saturating_sub(cols_of(&text)));
        let style = if selected {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(text, style),
            Span::styled(pad, style),
        ]));
    }
    // An empty row, then what the chosen position does.
    if inner.height as usize > lines.len() + 1 {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            fit_cols(
                &format!("  {}", posedit::position_note(e.position, e.depth, e.role)),
                cols,
            ),
            Style::default().fg(Color::DarkGray),
        )));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// The question `/import` asks when the card's name is taken: import it again
/// under the numbered name (and its lorebook), or leave it out (Yes / No).
pub fn draw_import_ask(f: &mut Frame, area: Rect, a: &mut crate::importask::ImportAsk) {
    let w = area.width.min(62);
    // Borders (2), the text, an empty row and the buttons.
    let text_rows = if a.lorebook.is_some() { 4 } else { 2 };
    let h = (text_rows + 4) as u16;
    let h = h.min(area.height).max(3).min(area.height);
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);

    let bottom: Line<'static> = if let Some(msg) = &a.note {
        Line::from(Span::styled(format!(" {} ", msg), Style::default().fg(Color::Red)))
    } else {
        Line::from(" y yes \u{b7} n / Esc no \u{b7} \u{2190}/\u{2192} choose \u{b7} Enter ")
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" duplicate character ")
        .title_bottom(bottom)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height < 1 || inner.width < 4 {
        return;
    }
    a.window = rect;
    let cols = inner.width as usize;
    let mut lines: Vec<Line<'static>> = vec![
        Line::from(fit_cols(
            &format!(" \"{}\" is already in your characters.", a.character),
            cols,
        )),
        Line::from(Span::styled(
            fit_cols(&format!(" Import it again as \"{}\"?", a.new_name), cols),
            Style::default().fg(Color::Yellow),
        )),
    ];
    if let Some((book, entries)) = &a.lorebook {
        lines.push(Line::from(fit_cols(
            &format!(
                " Its lorebook ({} {}) is copied too,",
                entries,
                if *entries == 1 { "entry" } else { "entries" }
            ),
            cols,
        )));
        lines.push(Line::from(Span::styled(
            fit_cols(&format!(" as \"{}\".", book), cols),
            Style::default().fg(Color::Yellow),
        )));
    }
    // The buttons go on the last row of the window.
    let button_row = inner.height.saturating_sub(1);
    while (lines.len() as u16) < button_row {
        lines.push(Line::from(""));
    }
    lines.truncate(button_row as usize);
    let yes = "  Yes  ";
    let no = "  No  ";
    let lit = Style::default().add_modifier(Modifier::REVERSED);
    let (ys, ns) = if a.yes {
        (lit, Style::default())
    } else {
        (Style::default(), lit)
    };
    let total = (yes.len() + 2 + no.len()) as u16;
    let x0 = inner.x + inner.width.saturating_sub(total) / 2;
    let y = inner.y + button_row;
    a.yes_area = Rect::new(x0, y, yes.len() as u16, 1).intersection(inner);
    a.no_area = Rect::new(x0 + yes.len() as u16 + 2, y, no.len() as u16, 1).intersection(inner);
    lines.push(Line::from(vec![
        Span::raw(" ".repeat((x0 - inner.x) as usize)),
        Span::styled(yes, ys),
        Span::raw("  "),
        Span::styled(no, ns),
    ]));
    f.render_widget(Paragraph::new(lines), inner);
}

/// The question after `/import` when the card has a lorebook: copy it into
/// `/lore` and link it to the character (Yes / No).
pub fn draw_lore_ask(f: &mut Frame, area: Rect, a: &mut crate::loreask::LoreAsk) {
    let w = area.width.min(60);
    // Borders (2), three lines of text, an empty row and the buttons.
    let h = 7u16.min(area.height).max(3).min(area.height);
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);

    let bottom: Line<'static> = if let Some(msg) = &a.note {
        Line::from(Span::styled(format!(" {} ", msg), Style::default().fg(Color::Red)))
    } else {
        Line::from(" y yes \u{b7} n / Esc no \u{b7} \u{2190}/\u{2192} choose \u{b7} Enter ")
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" lorebook ")
        .title_bottom(bottom)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height < 1 || inner.width < 4 {
        return;
    }
    a.window = rect;
    let cols = inner.width as usize;
    let entries = format!(
        "{} entr{}",
        a.entries,
        if a.entries == 1 { "y" } else { "ies" }
    );
    let mut lines: Vec<Line<'static>> = vec![
        Line::from(fit_cols(&format!(" {} comes with a lorebook:", a.character), cols)),
        Line::from(Span::styled(
            fit_cols(&format!(" {} ({})", a.book, entries), cols),
            Style::default().fg(Color::Yellow),
        )),
        Line::from(fit_cols(&format!(" Import it and link it to {}?", a.character), cols)),
    ];
    // The buttons go on the last row of the window.
    let button_row = inner.height.saturating_sub(1);
    while (lines.len() as u16) < button_row {
        lines.push(Line::from(""));
    }
    lines.truncate(button_row as usize);
    let yes = "  Yes  ";
    let no = "  No  ";
    let lit = Style::default().add_modifier(Modifier::REVERSED);
    let (ys, ns) = if a.yes {
        (lit, Style::default())
    } else {
        (Style::default(), lit)
    };
    let total = (yes.len() + 2 + no.len()) as u16;
    let x0 = inner.x + inner.width.saturating_sub(total) / 2;
    let y = inner.y + button_row;
    a.yes_area = Rect::new(x0, y, yes.len() as u16, 1).intersection(inner);
    a.no_area = Rect::new(x0 + yes.len() as u16 + 2, y, no.len() as u16, 1).intersection(inner);
    lines.push(Line::from(vec![
        Span::raw(" ".repeat((x0 - inner.x) as usize)),
        Span::styled(yes, ys),
        Span::raw("  "),
        Span::styled(no, ns),
    ]));
    f.render_widget(Paragraph::new(lines), inner);
}

/// The `/character` window: every field of the card as a scrollable form,
/// the focused field editable, with Cancel and Save buttons at the bottom.
/// The `/char-img` window: a square (as seen on screen) with the card picture.
fn draw_image_modal(f: &mut Frame, area: Rect, m: &mut crate::charimg::ImageModal) {
    // Inside the border: `side` columns by `side / 2` rows = `side` x `side` pixels.
    let side = (area.width.saturating_sub(2) as u32)
        .min(area.height.saturating_sub(2) as u32 * 2)
        .max(2)
        & !1;
    let rect = centered(area, side as u16 + 2, (side / 2) as u16 + 2);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", m.title))
        .title_bottom(" any key closes ")
        .border_style(Style::default().fg(Color::Cyan));
    if m.is_graphics() {
        // The terminal draws the picture itself (main loop, after this frame).
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        m.want = Some(m.fit(inner));
        return;
    }
    let lines: Vec<Line<'static>> = m
        .rows(side)
        .into_iter()
        .map(|r| {
            Line::from(
                r.into_iter()
                    .map(|(t, b)| Span::styled("\u{2580}", Style::default().fg(t).bg(b)))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn draw_card_modal(
    f: &mut Frame,
    area: Rect,
    m: &mut CardModal,
    mut spell: Option<&mut crate::spell::Checker>,
    rested: bool,
) {
    // `area` is the space between the title bar and the input box, margin
    // included: the window fills its height, and its width on small screens.
    let w = if area.width < 60 { area.width } else { (area.width - 4).min(100) };
    let h = area.height;
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);

    let title = if m.dirty() {
        format!(" {} (modified) ", m.title)
    } else {
        format!(" {} ", m.title)
    };
    // Bottom edge: the vim mode (when vim modes are on) and the main keys.
    let mut bottom: Vec<Span<'static>> = Vec::new();
    let on_pick = m.pick_of(m.focus).is_some();
    if m.input.vim_enabled() && !on_pick {
        let (tag, color) = match m.input.mode {
            Mode::Insert => (" INSERT ", Color::Green),
            Mode::Normal => (" NORMAL ", Color::Magenta),
            Mode::Command => (" COMMAND ", Color::LightBlue),
            Mode::Visual => {
                if m.input.visual_linewise() {
                    (" V-LINE ", Color::Yellow)
                } else {
                    (" VISUAL ", Color::Yellow)
                }
            }
        };
        bottom.push(Span::styled(
            tag,
            Style::default()
                .fg(Color::Black)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if on_pick {
        bottom.push(Span::raw(
            " \u{2190}/\u{2192} change \u{b7} \u{2191}/\u{2193} move \u{b7} Ctrl+S save \u{b7} Esc cancel ",
        ));
    } else {
        bottom.push(Span::raw(" Ctrl+S save \u{b7} :wq save & close \u{b7} Esc cancel "));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_bottom(Line::from(bottom))
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height < 3 || inner.width < 16 {
        return;
    }

    // Body on top, one blank row, then the button row.
    let body = Rect::new(inner.x, inner.y, inner.width, inner.height - 2);
    let btn_y = inner.y + inner.height - 1;
    let btn_w = 12u16.min(inner.width / 2);
    m.cancel_btn = Rect::new(inner.x + 1, btn_y, btn_w, 1);
    m.save_btn = Rect::new(inner.x + inner.width - 1 - btn_w, btn_y, btn_w, 1);
    f.render_widget(
        Paragraph::new(format!("{:^1$}", "Cancel", btn_w as usize))
            .style(Style::default().fg(Color::White).bg(Color::DarkGray)),
        m.cancel_btn,
    );
    f.render_widget(
        Paragraph::new(format!("{:^1$}", "Save", btn_w as usize)).style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        m.save_btn,
    );

    // One column of padding on the left, one spare column on the right so
    // that a cursor at the end of a full row still has a place.
    let live = m.input.command_live();
    // The `/config` window (one YAML field) shows line numbers in a gutter at
    // the left; the text, the cursor and clicks are all shifted by its width.
    let numbered = m.fields.len() == 1 && m.fields[0].key == "config";
    let base_w = (body.width as usize).saturating_sub(2).max(1);
    let gutter = if numbered {
        let n = live
            .as_ref()
            .and_then(|l| l.text.as_deref())
            .unwrap_or(m.fields[0].text.as_str())
            .split('\n')
            .count()
            .max(1);
        let g = n.to_string().len().max(2) + 1;
        // Too narrow for the numbers: leave them out.
        if base_w >= g + 12 { g } else { 0 }
    } else {
        0
    };
    let text_w = (base_w - gutter).max(1);
    m.text_w = text_w;
    m.text_x = body.x + 1 + gutter as u16;
    m.view_h = body.height as usize;

    let dim = Style::default().fg(Color::DarkGray);
    // A `:s/pat/new` being typed: matches highlighted, and once the
    // replacement has begun, the text as it would look afterwards. It only
    // ever concerns the focused field: that is all the editor holds, so the
    // other fields are never touched.
    // The name typed into the Name field counts as spelled right elsewhere.
    if let Some(sp) = spell.as_deref_mut() {
        let name = m.fields.iter().find(|x| x.key == "name").map(|x| x.text.clone());
        sp.set_extra(&[name.as_deref().unwrap_or("")]);
    }
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut owners: Vec<(usize, Option<usize>)> = Vec::new();
    let mut cursor_line = 0usize;
    let mut cursor_col = 0usize;
    let mut cursor_row0 = false;
    let mut lineno = 0usize;

    for (i, fld) in m.fields.iter().enumerate() {
        let focused = i == m.focus;
        let head_style = if focused {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if fld.editable {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            dim.add_modifier(Modifier::BOLD)
        };
        // A row of choices (`/note`): the label and the value between arrows.
        if m.pick_of(i).is_some() {
            if focused {
                cursor_line = lines.len();
                cursor_row0 = false;
            }
            let value_style = if focused {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            lines.push(Line::from(vec![
                Span::styled(format!(" {:<8} ", fld.label), head_style),
                Span::raw("  "),
                Span::styled(format!("\u{2039} {} \u{203a}", m.shown(i)), value_style),
            ]));
            owners.push((i, Some(0)));
            continue;
        }
        let mut head = vec![Span::styled(format!(" {} ", fld.label), head_style)];
        if !fld.editable {
            head.push(Span::styled("  (read only)".to_string(), dim));
        }
        lines.push(Line::from(head));
        owners.push((i, None));

        let preview: Option<String> = if focused {
            live.as_ref().and_then(|l| l.text.clone())
        } else {
            None
        };
        let view: &str = preview.as_deref().unwrap_or(fld.text.as_str());
        let notes = if m.comments {
            crate::cardedit::comment_ranges(view)
        } else {
            Vec::new()
        };
        let found: &[(usize, usize)] = if focused {
            live.as_ref().map_or(&[], |l| l.marks.as_slice())
        } else {
            &[]
        };
        let cur = if focused { m.input.cursor.min(view.chars().count()) } else { 0 };
        let (rows, (crow, ccol), starts) = layout_input(view, cur, text_w);
        // Misspelled words of editable fields (not the Name, which is free
        // text), except while a `:s` preview is shown. The word being typed
        // in the focused field (INSERT mode) is left alone.
        let spell_it = fld.editable
            && fld.key != "name"
            && preview.is_none() && !(focused && live.is_some());
        let misspelled: Vec<(usize, usize)> = match spell.as_deref_mut() {
            Some(sp) if spell_it => {
                let typing = (focused && m.input.mode == Mode::Insert && !rested).then_some(cur);
                sp.marks_typing(view, typing)
            }
            _ => Vec::new(),
        };
        // The focused field is drawn with the chat box's row renderer, so a
        // VISUAL selection shows in reverse video.
        let chars: Vec<char> = if focused || !misspelled.is_empty() {
            view.chars().collect()
        } else {
            Vec::new()
        };
        let sel = if focused { m.input.selection() } else { None };
        if fld.text.is_empty() && !focused {
            lines.push(Line::from(Span::styled("(empty)".to_string(), dim)));
            owners.push((i, Some(0)));
        } else {
            let style = if fld.editable {
                Style::default()
            } else {
                Style::default().fg(Color::Gray)
            };
            let vchars: Vec<char> = if gutter > 0 { view.chars().collect() } else { Vec::new() };
            for (r, row) in rows.iter().enumerate() {
                if focused && r == crow {
                    cursor_line = lines.len();
                    cursor_col = ccol;
                    cursor_row0 = r == 0;
                }
                // Line number on the first row of each line; the rows a long
                // line wraps onto get a blank gutter.
                let num: Option<Span<'static>> = if gutter > 0 {
                    let st = starts[r];
                    let first = st == 0 || vchars.get(st - 1) == Some(&'\n');
                    Some(if first {
                        lineno += 1;
                        Span::styled(format!("{:>1$} ", lineno, gutter - 1), dim)
                    } else {
                        Span::raw(" ".repeat(gutter))
                    })
                } else {
                    None
                };
                let before = lines.len();
                if focused || !misspelled.is_empty() {
                    lines.push(input_row(
                        row,
                        starts[r],
                        &chars,
                        sel,
                        found,
                        preview.is_some(),
                        &misspelled,
                        &notes,
                    ));
                } else {
                    lines.push(comment_row(row, starts[r], style, &notes));
                }
                if let Some(n) = num {
                    lines[before].spans.insert(0, n);
                }
                owners.push((i, Some(r)));
            }
        }
        lines.push(Line::from(""));
        owners.push((i, None));
    }

    // Keep the cursor in view (not while the wheel is scrolling), and the
    // scroll position inside the form.
    let view_h = m.view_h;
    let total = lines.len();
    let max_scroll = total.saturating_sub(view_h);
    if m.follow && view_h > 0 {
        // Show the field's title with its first row.
        let want = if cursor_row0 { cursor_line.saturating_sub(1) } else { cursor_line };
        if want < m.scroll {
            m.scroll = want;
        } else if cursor_line >= m.scroll + view_h {
            m.scroll = cursor_line + 1 - view_h;
        }
    }
    m.scroll = m.scroll.min(max_scroll);

    let end = (m.scroll + view_h).min(total);
    let visible: Vec<Line<'static>> = lines[m.scroll..end].to_vec();
    m.hits.clear();
    for (k, (field, row)) in owners[m.scroll..end].iter().enumerate() {
        m.hits.push(Hit {
            y: body.y + k as u16,
            field: *field,
            row: *row,
        });
    }
    let text_area = Rect::new(body.x + 1, body.y, body.width - 1, body.height);
    f.render_widget(Paragraph::new(visible), text_area);

    // The row above the buttons: the `:` command line, or a message.
    let note_rect = Rect::new(inner.x, inner.y + inner.height - 2, inner.width, 1);
    let mut cmd_cursor: Option<(u16, u16)> = None;
    if m.input.mode == Mode::Command {
        let cs: Vec<char> = m.input.command_text().chars().collect();
        let keep = (inner.width as usize).saturating_sub(2);
        let shown: String = cs[cs.len().saturating_sub(keep)..].iter().collect();
        let used = shown.chars().count();
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(":", Style::default().fg(Color::LightBlue)),
                Span::raw(shown),
            ])),
            note_rect,
        );
        cmd_cursor = Some((note_rect.x + 1 + used as u16, note_rect.y));
        // Right side: which field this applies to, and how many matches.
        if let Some(l) = &live {
            let n = match l.count {
                0 => "no match".to_string(),
                1 => "1 match".to_string(),
                n => format!("{} matches", n),
            };
            let info = format!("{}: {}", m.fields[m.focus].label, n);
            let iw = info.chars().count();
            if used + 2 + iw + 2 <= inner.width as usize {
                let color = if l.count == 0 { Color::Red } else { Color::Yellow };
                f.render_widget(
                    Paragraph::new(Line::from(Span::styled(info, Style::default().fg(color))))
                        .alignment(Alignment::Right),
                    note_rect,
                );
            }
        }
    } else if !m.note.is_empty() {
        let color = if m.note_err { Color::Red } else { Color::DarkGray };
        f.render_widget(
            Paragraph::new(Span::styled(
                format!(" {}", m.note),
                Style::default().fg(color),
            )),
            note_rect,
        );
    }

    m.cursor_xy = None;
    if let Some(pos) = cmd_cursor {
        f.set_cursor_position(pos);
    } else if !on_pick && cursor_line >= m.scroll && cursor_line < end {
        let pos = (
            m.text_x + cursor_col as u16,
            body.y + (cursor_line - m.scroll) as u16,
        );
        f.set_cursor_position(pos);
        m.cursor_xy = Some(pos);
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    // Title + one message row + a 3-row input box + status row.
    if area.width < 10 || area.height < 6 {
        f.render_widget(Paragraph::new("too small"), area);
        return;
    }
    // Start-up (see `App::startup_pick`): the character window is the whole
    // screen; no title bar, chat or input box around it.
    if app.startup_pick {
        if let Some((_, p)) = app.picker.as_mut() {
            draw_picker(f, area, p, true);
            return;
        }
    }
    // Small screens: narrow ones get shorter buttons and a shorter title bar,
    // short ones a flat "Card save" button and a smaller input box.
    let narrow = area.width < 44;
    let short = area.height < 16;

    // Editing a card greeting adds a "Card save" button above "Save".
    let card_btn = app.card_edit_available();
    let flat_card = card_btn && short;
    let btn_w = if narrow {
        NARROW_BTN_W
    } else if card_btn {
        CARD_BTN_W
    } else {
        BTN_W
    };
    let inner_w = area.width.saturating_sub(2 + btn_w) as usize;
    app.input_w = inner_w.max(1);
    // A `:s/pat/new` being typed shows the text as it would look afterwards.
    let live = app.input.command_live();
    let view_text: String = match live.as_ref().and_then(|l| l.text.as_deref()) {
        Some(t) => t.to_string(),
        None => app.input.text.clone(),
    };
    let (input_lines, (cur_row, cur_col), row_starts) =
        layout_input(&view_text, app.input.cursor, inner_w.max(1));
    // Editing an existing message gets a taller box.
    // but never so tall that fewer than two message rows are left.
    let room = (area.height as usize).saturating_sub(6).max(1);
    let wanted_rows = if app.is_editing() {
        ((area.height as usize).saturating_sub(5) * 2 / 3).max(6)
    } else {
        6
    };
    let max_rows = wanted_rows.min(room).max(1);
    // Two stacked bordered buttons need 6 rows (3 each), so the box is at
    // least 4 text rows; flat buttons (short screens) need 2.
    let wanted_min = if flat_card {
        2
    } else if card_btn {
        4
    } else {
        1
    };
    let min_rows = wanted_min.min(max_rows);
    let visible_rows = input_lines.len().clamp(min_rows, max_rows);
    let input_h = visible_rows as u16 + 2;

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(input_h),
        Constraint::Length(1),
    ])
    .split(area);

    // ---- title bar
    // Drops the least important parts until it fits the width: first the
    // API, then the persona, then the version.
    let badge_full: &'static str = concat!(" tarpi ", env!("CARGO_PKG_VERSION"), " ");
    let badge_short: &'static str = " tarpi ";
    // Start-up (see `App::startup_pick`): no chat is open yet, so no name.
    let who = if app.startup_pick {
        String::new()
    } else {
        format!(" {}/{}", app.character.name, app.chat.file)
    };
    let opts = if app.startup_pick {
        [who.clone(), who.clone(), who.clone()]
    } else {
        [
            format!(
                "{} \u{2022} {} \u{2022} {}",
                who,
                app.persona_label(),
                app.model_label()
            ),
            format!("{} \u{2022} {}", who, app.persona_label()),
            who.clone(),
        ]
    };
    let tw = area.width as usize;
    let mut pick: Option<(&'static str, String)> = None;
    for b in [badge_full, badge_short] {
        for r in opts.iter() {
            if b.chars().count() + r.chars().count() <= tw {
                pick = Some((b, r.clone()));
                break;
            }
        }
        if pick.is_some() {
            break;
        }
    }
    let (badge, rest) = pick.unwrap_or(("", who.clone()));
    let mut title_spans = vec![Span::styled(
        badge,
        Style::default()
            .fg(Color::Black)
            .bg(NAME_WHITE)
            .add_modifier(Modifier::BOLD),
    )];
    // The connection string (the last part) is red while the API is not alive.
    let label = app.model_label();
    match rest.strip_suffix(label.as_str()) {
        Some(head) if app.api_down() && !label.is_empty() => {
            title_spans.push(Span::styled(head.to_string(), Style::default().fg(Color::Gray)));
            title_spans.push(Span::styled(label, Style::default().fg(Color::Red)));
        }
        _ => title_spans.push(Span::styled(rest, Style::default().fg(Color::Gray))),
    }
    let title = Line::from(title_spans);
    f.render_widget(Paragraph::new(title), chunks[0]);

    // ---- messages
    // One blank row above and below the messages, to space them off the top
    // bar and the input box (skipped when the terminal is too short).
    let pad: u16 = if chunks[1].height >= 5 { 1 } else { 0 };
    let msg_area = Rect::new(
        chunks[1].x + 1,
        chunks[1].y + pad,
        chunks[1].width.saturating_sub(2),
        chunks[1].height.saturating_sub(2 * pad),
    );
    // Start-up: the character window is all there is; no chat shows behind it.
    let lines = if app.startup_pick {
        Vec::new()
    } else {
        app.build_lines(msg_area.width as usize)
    };
    let h = msg_area.height as usize;
    app.viewport_h = h;
    let total = lines.len();
    // Scrolled up while text streams in below: keep what is on screen where
    // it is (the view grows downwards) instead of letting it ride upwards.
    if app.scroll > 0 && app.last_total > 0 && total != app.last_total {
        app.scroll = (app.scroll as i64 + total as i64 - app.last_total as i64).max(0) as usize;
    }
    app.last_total = total;
    let max_scroll = total.saturating_sub(h);
    if app.scroll > max_scroll {
        app.scroll = max_scroll;
    }
    // Bring a just-selected / just-opened message into view.
    if let Some(idx) = app.reveal.take() {
        if let Some(&(_, s, e)) = app.line_map.iter().find(|(i, _, _)| *i == idx) {
            let end_now = total - app.scroll;
            let start_now = end_now.saturating_sub(h);
            if s < start_now || e - s > h {
                app.scroll = total.saturating_sub(s + h).min(max_scroll);
            } else if e > end_now {
                app.scroll = total - e;
            }
        }
    }
    let end = total - app.scroll;
    let start = end.saturating_sub(h);
    let visible: Vec<Line<'static>> = lines[start..end].to_vec();
    f.render_widget(Paragraph::new(visible), msg_area);
    app.msg_area = msg_area;
    app.view_start = start;

    // ---- input box + send button
    let row = Layout::horizontal([Constraint::Min(1), Constraint::Length(btn_w)]).split(chunks[2]);
    let input_area = row[0];
    let btn_area = row[1];
    let generating = app.is_generating();
    let editing = app.is_editing();
    let border_color = if editing {
        Color::Green
    } else if generating {
        Color::Yellow
    } else {
        Color::Cyan
    };
    let (mode_tag, mode_color) = match app.input.mode {
        Mode::Insert => (" INSERT ", Color::Green),
        Mode::Normal => (" NORMAL ", Color::Magenta),
        Mode::Command => (" COMMAND ", Color::LightBlue),
        Mode::Visual => {
            if app.input.visual_linewise() {
                (" V-LINE ", Color::Yellow)
            } else {
                (" VISUAL ", Color::Yellow)
            }
        }
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    // The mode tag only exists with vim mode (config `vim.enabled`).
    if app.vim {
        block = block.title_bottom(Line::from(Span::styled(
            mode_tag,
            Style::default()
                .fg(Color::Black)
                .bg(mode_color)
                .add_modifier(Modifier::BOLD),
        )));
    }
    let max_first = input_lines.len().saturating_sub(visible_rows);
    let first_row = match app.input_scroll {
        // Scrolled by hand (mouse wheel over the box): the cursor may be out of view.
        Some(s) => s.min(max_first),
        None if input_lines.len() > visible_rows => {
            (cur_row + 1).saturating_sub(visible_rows).min(max_first)
        }
        None => 0,
    };
    app.input_rows = (input_lines.len(), visible_rows);
    let selection = app.input.selection();
    // While a `:s/pattern` is typed on the command line, mark what it finds
    // (yellow) or, once the replacement is being typed, what it puts in (green).
    let found: &[(usize, usize)] = live.as_ref().map_or(&[], |l| l.marks.as_slice());
    let replaced_view = live.as_ref().map_or(false, |l| l.text.is_some());
    let text_chars: Vec<char> = view_text.chars().collect();
    // Spell check only the plain text, not a `:s` preview being typed.
    let misspelled = if live.is_none() {
        app.spell_marks(&view_text)
    } else {
        Vec::new()
    };
    let shown: Vec<Line<'static>> = (first_row..first_row + visible_rows)
        .map(|r| {
            input_row(
                &input_lines[r],
                row_starts[r],
                &text_chars,
                selection,
                found,
                replaced_view,
                &misspelled,
                &[],
            )
        })
        .collect();
    f.render_widget(Paragraph::new(shown).block(block), input_area);

    // Send / Stop / Save button, vertically centred beside the box. While a
    // card greeting is edited, "Card save" sits above it.
    let (label, btn_color) = if editing {
        (if narrow { "Save" } else { " Save " }, Color::Green)
    } else if generating {
        (if narrow { "Stop" } else { " Stop " }, Color::Yellow)
    } else {
        (if narrow { "Send" } else { " Send " }, Color::Cyan)
    };
    if flat_card {
        // Short screen: two one-row buttons instead of two bordered ones.
        let parts = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(btn_area);
        let card_label = if narrow { "Card" } else { " Card save " };
        draw_flat_button(f, parts[0], card_label, Color::Magenta);
        draw_flat_button(f, parts[1], label, btn_color);
        app.card_btn = parts[0];
        app.send_btn = parts[1];
    } else if card_btn {
        let parts = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).split(btn_area);
        draw_button(f, parts[0], if narrow { "Card" } else { " Card save " }, Color::Magenta);
        draw_button(f, parts[1], label, btn_color);
        app.card_btn = parts[0];
        app.send_btn = parts[1];
    } else {
        draw_button(f, btn_area, label, btn_color);
        app.card_btn = Rect::default();
        app.send_btn = btn_area;
    }
    app.input_area = input_area;
    app.input_first_row = first_row;
    draw_completion(f, app, input_area, chunks[1].y);

    // ---- status line
    let (status, is_err) = app.status_line();
    let style = if is_err {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let mut status_line = status;
    if app.scroll > 0 {
        status_line = if status_line.is_empty() {
            format!("\u{2193} {} lines below", app.scroll)
        } else {
            format!(
                "\u{2193} {} lines below  \u{b7}  {}",
                app.scroll, status_line
            )
        };
    }
    // The `:` command line takes over the status row while it is open.
    let mut cmd_cursor_x: Option<u16> = None;
    if app.input.mode == Mode::Command {
        let w = chunks[3].width as usize;
        // Keep the end of a long command in view (one column for the ':').
        let mut shown_cmd: Vec<char> = Vec::new();
        let mut used = 0usize;
        for c in app.input.command_text().chars().rev() {
            let cw = char_width(c);
            if used + cw + 2 > w {
                break;
            }
            used += cw;
            shown_cmd.push(c);
        }
        shown_cmd.reverse();
        let cmd_str: String = shown_cmd.into_iter().collect();
        cmd_cursor_x = Some(chunks[3].x + 1 + used as u16);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(":", Style::default().fg(Color::LightBlue)),
                Span::raw(cmd_str),
            ])),
            chunks[3],
        );
        if let Some(l) = &live {
            let m = &l.count;
            let info = match *m {
                0 => "no match".to_string(),
                1 => "1 match".to_string(),
                n => format!("{} matches", n),
            };
            let iw = info.chars().count();
            if used + 2 + iw + 2 <= w {
                let color = if l.count == 0 { Color::Red } else { Color::Yellow };
                f.render_widget(
                    Paragraph::new(Line::from(Span::styled(info, Style::default().fg(color))))
                        .alignment(Alignment::Right),
                    chunks[3],
                );
            }
        }
    } else {
        // A one-shot chat always shows its mode at the start of this row, in
        // the same grey as the rest; other messages follow after a bullet.
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut left_w = 0usize;
        if app.chat.is_one_shot() {
            let tag = "ONE SHOT MODE";
            left_w += tag.chars().count();
            spans.push(Span::styled(tag, Style::default().fg(Color::DarkGray)));
            if !status_line.is_empty() {
                let sep = "  \u{b7}  ";
                left_w += sep.chars().count();
                spans.push(Span::styled(sep, Style::default().fg(Color::DarkGray)));
            }
        }
        left_w += status_line.chars().count();
        spans.push(Span::styled(status_line, style));
        f.render_widget(Paragraph::new(Line::from(spans)), chunks[3]);
        // Chat totals on the right of the same row, when there is room.
        let ChatStats {
            messages: msgs,
            tokens,
            exact,
        } = app.stats;
        // Exact counts (from the server's tokenizer) show in full; estimates as ~1.2k.
        let tok = if exact {
            group_digits(tokens)
        } else if tokens >= 1_000_000 {
            format!("~{:.2}M", tokens as f32 / 1_000_000.0)
        } else if tokens >= 1000 {
            format!("~{:.1}k", tokens as f32 / 1000.0)
        } else {
            format!("~{}", tokens)
        };
        let stats = format!(
            "{} {} \u{b7} {} tokens",
            msgs,
            if msgs == 1 { "message" } else { "messages" },
            tok
        );
        if left_w + 3 + stats.chars().count() <= chunks[3].width as usize {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    stats,
                    Style::default().fg(Color::DarkGray),
                )))
                .alignment(Alignment::Right),
                chunks[3],
            );
        }
    }

    // ---- windows (help, /char, /chars, /chats, /persona, /char-img)
    // They sit between the title bar and the input box, so both stay visible,
    // with a one-column margin on each side (and one row above and below when
    // there is room). A window that would fill the whole width therefore
    // leaves a margin instead of touching the screen edges. On a very small
    // screen they get the whole screen, as before.
    let modal_area = {
        let c = chunks[1];
        if c.height >= 6 && c.width >= 12 {
            let vm = if c.height >= 8 { 1 } else { 0 };
            Rect::new(c.x + 1, c.y + vm, c.width - 2, c.height - 2 * vm)
        } else {
            area
        }
    };
    // ---- help overlay
    if app.show_help {
        // Narrow screens: the window takes the full width and long lines wrap.
        let w = 62u16.min(modal_area.width);
        let iw = (w as usize).saturating_sub(2);
        let wrapped: Vec<String> = HELP.iter().flat_map(|l| wrap_help(l, iw)).collect();
        let hh = (wrapped.len() as u16 + 2).min(modal_area.height);
        let rect = centered(modal_area, w, hh);
        f.render_widget(Clear, rect);
        // Taller than the screen: the window scrolls.
        let rows = rect.height.saturating_sub(2) as usize;
        let max_scroll = wrapped.len().saturating_sub(rows);
        app.help_page = rows.max(1);
        app.help_scroll = app.help_scroll.min(max_scroll);
        let text: Vec<Line<'static>> = wrapped.iter().map(|l| Line::from(l.clone())).collect();
        let mut block = Block::default()
            .borders(Borders::ALL)
            .title(" help ")
            .border_style(Style::default().fg(Color::Cyan));
        if max_scroll > 0 {
            block = block.title_bottom(Line::from(format!(
                " \u{2191}/\u{2193} scroll \u{b7} any other key closes ({}/{}) ",
                app.help_scroll + 1,
                max_scroll + 1
            )));
        } else {
            block = block.title_bottom(" any key closes ");
        }
        let popup = Paragraph::new(text)
            .block(block)
            .scroll((app.help_scroll as u16, 0));
        f.render_widget(popup, rect);
    } else if let Some(a) = app.import_ask.as_mut() {
        draw_import_ask(f, modal_area, a);
    } else if let Some(a) = app.lore_ask.as_mut() {
        draw_lore_ask(f, modal_area, a);
    } else if let Some(v) = app.prompt_view.as_mut() {
        draw_prompt_modal(f, modal_area, v);
    } else if let Some(m) = app.image_modal.as_mut() {
        draw_image_modal(f, modal_area, m);
    } else if app.card_modal.is_some() {
        app.spell_names();
        app.spell_touch();
        let rested = app.spell_rested();
        // The `/config` window (YAML) is never spell checked.
        let no_spell = app.config_edit;
        if let Some(m) = app.card_modal.as_mut() {
            let sp = if no_spell { None } else { app.spell.as_mut() };
            draw_card_modal(f, modal_area, m, sp, rested);
        }
    } else if let Some(e) = app.pos_edit.as_mut() {
        draw_pos_edit(f, modal_area, e);
    } else if let Some((_, p)) = app.picker.as_mut() {
        draw_picker(f, modal_area, p, false);
    } else if let Some(x) = cmd_cursor_x {
        f.set_cursor_position((x, chunks[3].y));
    } else if cur_row >= first_row && cur_row < first_row + visible_rows {
        let x = input_area.x + 1 + cur_col as u16;
        let y = input_area.y + 1 + (cur_row - first_row) as u16;
        f.set_cursor_position((x, y));
    }
    // The spelling menu goes over everything else.
    draw_spell_menu(f, app, input_area, chunks[1].y);
}

#[cfg(test)]
mod separator_tests {
    use super::*;

    fn last(sep: Option<char>, width: usize) -> String {
        let lines = message_lines("A", false, "hi", width, None, None, false, false, sep);
        lines.last().unwrap().spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn the_row_under_a_message_is_the_chosen_character_or_empty() {
        assert_eq!(last(Some('\u{254C}'), 5), "\u{254C}".repeat(5));
        assert_eq!(last(Some('='), 3), "===");
        assert_eq!(last(None, 5), "");
        // A message is the same height either way.
        let a = message_lines("A", false, "hi", 5, None, None, false, false, Some('-'));
        let b = message_lines("A", false, "hi", 5, None, None, false, false, None);
        assert_eq!(a.len(), b.len());
    }

    #[test]
    fn a_wide_character_fills_the_row_without_overflowing() {
        // two columns each: five fit in ten, and four in nine
        assert_eq!(last(Some('\u{ff5c}'), 10).chars().count(), 5);
        assert_eq!(last(Some('\u{ff5c}'), 9).chars().count(), 4);
    }
}
