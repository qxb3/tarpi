//! Rendering: message wrapping/styling and the ratatui layout.

use crate::app::{App, ChatStats};
use crate::cardedit::{CardModal, Hit};
use crate::input::Mode;
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
/// foreground faded towards the background, speech is the theme's yellow, and
/// prose nested in the other kind is a blend of the two. `quote_outer` says
/// which one was opened first when both are active. They are RGB, so "bold"
/// doesn't turn them bright in terminals that render bold as bright.
fn mk_style(italic: bool, bold: bool, quote: bool, quote_outer: bool) -> Style {
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
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
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
        }
        let mut line: Vec<StyledChar> = Vec::new();
        let mut i = 0usize;
        while i < chars.len() {
            let c = chars[i];
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
                    let st = mk_style(in_italic, in_bold, in_quote, quote_outer);
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

            line.push((c, mk_style(in_italic, in_bold, in_quote, quote_outer)));
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

/// Render one chat message: a name header, the wrapped body, and a blank line.
pub fn message_lines(
    name: &str,
    _is_user: bool,
    text: &str,
    width: usize,
    swipe: Option<(usize, usize)>,
    branch: Option<(usize, usize)>,
    streaming: bool,
    highlight: bool,
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
    // Thin, faint dashed rule between messages (one row, same as the old blank line).
    out.push(Line::from(Span::styled(
        "\u{254C}".repeat(width.max(1)),
        Style::default().fg(Color::Rgb(52, 52, 52)),
    )));
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
) -> Line<'static> {
    if sel.is_none() && found.is_empty() {
        return Line::from(row.to_string());
    }
    // VISUAL selection: reverse video. `:s` matches: black on yellow.
    let sel_style = Style::default().add_modifier(Modifier::REVERSED);
    let find_style = Style::default()
        .fg(Color::Black)
        .bg(if replaced { Color::Green } else { Color::Yellow });
    let style_at = |i: usize| -> Style {
        if sel.map_or(false, |(s, e)| i >= s && i < e) {
            sel_style
        } else if found.iter().any(|&(s, e)| i >= s && i < e) {
            find_style
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
    "Esc              stop generating / cancel an edit",
    "",
    "Edit any message: click it. It opens in INSERT mode.",
    "Enter (or [Save]) saves, Esc cancels.",
    "Your unsent draft comes back afterwards.",
    "",
    "PgUp/PgDn, Up/Down, wheel  scroll chat (wheel on box: box)",
    "Swipe sideways (slanted) / tap bar: other swipes",
    "Ctrl+A/E/W/U     line start / end / delete word / clear",
    "",
    "Vim: qq (INSERT -> NORMAL), i a I A o O (-> INSERT)",
    "     (keys and on/off: ~/.config/st-cli/config.yaml)",
    "  h j k l w W b B e E 0 ^ $ gg G, x X r dd D cc C yy p P u",
    "  <leader>y{motion}, <leader>yy, <leader>Y, VISUAL <leader>y:",
    "  yank to the system clipboard (leader: config vim.leader)",
    "  Ctrl+U / Ctrl+D: half a box up / down (in /character, on",
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
    "/undo [n] (/u)   delete the last message, or the last n",
    "/new             new chat with this character",
    "/newchat (/nc)   delete THIS chat and start a new one",
    "/summarize       write an in-depth summary of the shown branch as",
    "                 a NEW summary branch (branch 1 is the whole",
    "                 conversation; nothing is replaced; a copy is also",
    "                 saved under another name)",
    "/deletebranch    delete the branch that is showing (also /db)",
    "/chats           open, create or delete chats of this character",
    "/characters      switch to another character",
    "/character       view and edit this character's card",
    "/help            this screen",
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

/// The `/character` window: every field of the card as a scrollable form,
/// the focused field editable, with Cancel and Save buttons at the bottom.
fn draw_card_modal(f: &mut Frame, area: Rect, m: &mut CardModal) {
    // Small screens: the window takes the whole screen.
    let w = if area.width < 60 { area.width } else { (area.width - 4).min(100) };
    let h = if area.height < 20 { area.height } else { area.height - 2 };
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);

    let title = if m.dirty() {
        " character card (modified) "
    } else {
        " character card "
    };
    // Bottom edge: the vim mode (when vim modes are on) and the main keys.
    let mut bottom: Vec<Span<'static>> = Vec::new();
    if m.input.vim_enabled() {
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
    bottom.push(Span::raw(" Tab next \u{b7} Ctrl+S save \u{b7} Esc cancel "));
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
    let text_w = (body.width as usize).saturating_sub(2).max(1);
    m.text_w = text_w;
    m.text_x = body.x + 1;
    m.view_h = body.height as usize;

    let dim = Style::default().fg(Color::DarkGray);
    // A `:s/pat/new` being typed: matches highlighted, and once the
    // replacement has begun, the text as it would look afterwards. It only
    // ever concerns the focused field: that is all the editor holds, so the
    // other fields are never touched.
    let live = m.input.command_live();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut owners: Vec<(usize, Option<usize>)> = Vec::new();
    let mut cursor_line = 0usize;
    let mut cursor_col = 0usize;
    let mut cursor_row0 = false;

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
        let found: &[(usize, usize)] = if focused {
            live.as_ref().map_or(&[], |l| l.marks.as_slice())
        } else {
            &[]
        };
        let cur = if focused { m.input.cursor.min(view.chars().count()) } else { 0 };
        let (rows, (crow, ccol), starts) = layout_input(view, cur, text_w);
        // The focused field is drawn with the chat box's row renderer, so a
        // VISUAL selection shows in reverse video.
        let chars: Vec<char> = if focused { view.chars().collect() } else { Vec::new() };
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
            for (r, row) in rows.iter().enumerate() {
                if focused && r == crow {
                    cursor_line = lines.len();
                    cursor_col = ccol;
                    cursor_row0 = r == 0;
                }
                if focused {
                    lines.push(input_row(row, starts[r], &chars, sel, found, preview.is_some()));
                } else {
                    lines.push(Line::from(Span::styled(row.clone(), style)));
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

    if let Some(pos) = cmd_cursor {
        f.set_cursor_position(pos);
    } else if cursor_line >= m.scroll && cursor_line < end {
        f.set_cursor_position((
            m.text_x + cursor_col as u16,
            body.y + (cursor_line - m.scroll) as u16,
        ));
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    // Title + one message row + a 3-row input box + status row.
    if area.width < 10 || area.height < 6 {
        f.render_widget(Paragraph::new("too small"), area);
        return;
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
    let badge_full: &'static str = concat!(" st-cli ", env!("CARGO_PKG_VERSION"), " ");
    let badge_short: &'static str = " st-cli ";
    let who = format!(" {}/{}", app.character.name, app.chat.file);
    let opts = [
        format!(
            "{} \u{2022} {} \u{2022} {}",
            who,
            app.persona_label(),
            app.model_label()
        ),
        format!("{} \u{2022} {}", who, app.persona_label()),
        who.clone(),
    ];
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
    let title = Line::from(vec![
        Span::styled(
            badge,
            Style::default()
                .fg(Color::Black)
                .bg(NAME_WHITE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(rest, Style::default().fg(Color::Gray)),
    ]);
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
    let lines = app.build_lines(msg_area.width as usize);
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
    let shown: Vec<Line<'static>> = (first_row..first_row + visible_rows)
        .map(|r| {
            input_row(
                &input_lines[r],
                row_starts[r],
                &text_chars,
                selection,
                found,
                replaced_view,
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
        let left_w = status_line.chars().count();
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(status_line, style))),
            chunks[3],
        );
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

    // ---- help overlay
    if app.show_help {
        // Narrow screens: the window takes the full width and long lines wrap.
        let w = 62u16.min(area.width);
        let iw = (w as usize).saturating_sub(2);
        let wrapped: Vec<String> = HELP.iter().flat_map(|l| wrap_help(l, iw)).collect();
        let hh = (wrapped.len() as u16 + 2).min(area.height);
        let rect = centered(area, w, hh);
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
    } else if app.card_modal.is_some() {
        if let Some(m) = app.card_modal.as_mut() {
            draw_card_modal(f, area, m);
        }
    } else if let Some(x) = cmd_cursor_x {
        f.set_cursor_position((x, chunks[3].y));
    } else if cur_row >= first_row && cur_row < first_row + visible_rows {
        let x = input_area.x + 1 + cur_col as u16;
        let y = input_area.y + 1 + (cur_row - first_row) as u16;
        f.set_cursor_position((x, y));
    }
}
