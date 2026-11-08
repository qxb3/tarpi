//! Colours for the prose styling, taken from the terminal's own theme.
//!
//! At start-up (in raw mode, before the event loop reads the keyboard) st-cli
//! asks the terminal for its default foreground (`OSC 10`), background
//! (`OSC 11`) and its palette yellow (`OSC 4;3`), then a `DA1` query as the
//! last one: every terminal answers that, so once its reply is in we know no
//! more answers are coming and there is no fixed wait. A terminal that does
//! not answer (or a non-Unix system) costs at most 300 ms once, and the
//! colours then come from `COLORFGBG` (light / dark) with xterm-like values.
//!
//! The prose colours are *derived* from those three values, so they follow
//! whatever theme the terminal is using:
//!
//! * `*narration*`: a neutral grey between the foreground's and the
//!   background's brightness (readable on both dark and light themes), bold;
//! * `"speech"`: the theme's own yellow;
//! * prose inside other prose (`*he said "hi"*`, `"well *sigh* ok"`): the
//!   inner colour pulled part of the way towards the outer one, so it blends
//!   in but is still a visible step away from both.

use ratatui::style::Color;
use std::sync::OnceLock;

pub type Rgb = (u8, u8, u8);

/// How far the foreground's brightness is faded towards the background's for
/// `*narration*` (0 = plain foreground, 1 = invisible). Higher = greyer and
/// more different from plain text, lower = easier to read.
const NARRATION_FADE: f32 = 0.52;

/// How far the inner colour of nested prose moves towards the outer colour
/// (0 = no change, 1 = the outer colour).
const NEST_BLEND: f32 = 0.4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub fg: Rgb,
    pub bg: Rgb,
    pub yellow: Rgb,
    /// `yellow` was reported by the terminal. When it was not, plain speech
    /// uses the terminal's indexed yellow (so the theme still applies) and
    /// `yellow` is only used to work out blends.
    pub yellow_real: bool,
}

pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

fn rgb(c: Rgb) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

fn luminance(c: Rgb) -> f32 {
    0.299 * c.0 as f32 + 0.587 * c.1 as f32 + 0.114 * c.2 as f32
}

impl Palette {
    /// Colour of `*narration*`: a neutral grey (no tint from the theme's
    /// colours) between the foreground's and the background's brightness.
    pub fn narration(&self) -> Rgb {
        let g = (luminance(self.fg) + (luminance(self.bg) - luminance(self.fg)) * NARRATION_FADE)
            .round()
            .clamp(0.0, 255.0) as u8;
        (g, g, g)
    }

    /// Foreground colour of prose, or `None` for plain text (which keeps the
    /// terminal's own default colour). `quote_outer` tells which of the two
    /// was opened first when both are active.
    pub fn prose_color(&self, italic: bool, quote: bool, quote_outer: bool) -> Option<Color> {
        match (italic, quote) {
            (false, false) => None,
            (true, false) => Some(rgb(self.narration())),
            (false, true) => Some(if self.yellow_real {
                rgb(self.yellow)
            } else {
                Color::Yellow
            }),
            (true, true) => {
                let (inner, outer) = if quote_outer {
                    (self.narration(), self.yellow)
                } else {
                    (self.yellow, self.narration())
                };
                Some(rgb(mix(inner, outer, NEST_BLEND)))
            }
        }
    }
}

/// Guesses when the terminal does not answer.
fn guess(light: bool) -> Palette {
    if light {
        Palette {
            fg: (0, 0, 0),
            bg: (255, 255, 255),
            yellow: (153, 153, 0),
            yellow_real: false,
        }
    } else {
        Palette {
            fg: (229, 229, 229),
            bg: (0, 0, 0),
            yellow: (205, 205, 0),
            yellow_real: false,
        }
    }
}

/// `COLORFGBG` ("15;0" or "15;default;0"): the last field is the background's
/// palette index; 7 and 9-15 are light.
fn colorfgbg_is_light() -> bool {
    std::env::var("COLORFGBG")
        .ok()
        .and_then(|v| v.rsplit(';').next().and_then(|b| b.trim().parse::<u8>().ok()))
        .map(|b| b == 7 || (9..=15).contains(&b))
        .unwrap_or(false)
}

/// `rgb:RRRR/GGGG/BBBB` (1 to 4 hex digits per channel) right after `marker`.
fn find_color(reply: &str, marker: &str) -> Option<Rgb> {
    let i = reply.find(marker)? + marker.len();
    let rest = reply[i..].strip_prefix("rgb:")?;
    let end = rest
        .find(|c: char| !(c.is_ascii_hexdigit() || c == '/'))
        .unwrap_or(rest.len());
    let mut parts = rest[..end].split('/');
    let chan = |s: &str| -> Option<u8> {
        if s.is_empty() || s.len() > 4 {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        let max = 16u32.pow(s.len() as u32) - 1;
        Some(((v * 255 + max / 2) / max) as u8)
    };
    let c = (chan(parts.next()?)?, chan(parts.next()?)?, chan(parts.next()?)?);
    if parts.next().is_some() {
        return None;
    }
    Some(c)
}

/// Build the palette from the terminal's raw reply text.
fn from_reply(reply: &str) -> Palette {
    let fg = find_color(reply, "\x1b]10;");
    let bg = find_color(reply, "\x1b]11;");
    let yellow = find_color(reply, "\x1b]4;3;");
    let light = match bg {
        Some(b) => luminance(b) > 128.0,
        None => colorfgbg_is_light(),
    };
    let base = guess(light);
    let mut p = Palette {
        fg: fg.unwrap_or(base.fg),
        bg: bg.unwrap_or(base.bg),
        yellow: yellow.unwrap_or(base.yellow),
        yellow_real: yellow.is_some(),
    };
    // A theme that reports the same colour for both would make narration
    // vanish: fall back to the guess for the pair.
    if p.fg == p.bg {
        p.fg = base.fg;
        p.bg = base.bg;
    }
    p
}

/// The DA1 answer (`ESC [ ? ... c`) is the last one we asked for.
#[cfg(unix)]
fn has_da1_reply(buf: &[u8]) -> bool {
    let s = String::from_utf8_lossy(buf);
    match s.find("\x1b[?") {
        Some(i) => s[i..].contains('c'),
        None => false,
    }
}

#[cfg(unix)]
fn ask_terminal() -> Option<String> {
    use std::io::{IsTerminal, Write};
    use std::time::{Duration, Instant};

    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return None;
    }
    let mut out = std::io::stdout();
    out.write_all(b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b]4;3;?\x1b\\\x1b[c")
        .ok()?;
    out.flush().ok()?;

    let deadline = Instant::now() + Duration::from_millis(300);
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut pfd = libc::pollfd {
            fd: 0,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `pfd` is a valid pollfd and we pass a count of 1.
        let r = unsafe { libc::poll(&mut pfd, 1, left.as_millis().max(1) as libc::c_int) };
        if r <= 0 {
            break;
        }
        let mut tmp = [0u8; 256];
        // SAFETY: `tmp` is a valid writable buffer of the length we pass.
        let n = unsafe { libc::read(0, tmp.as_mut_ptr() as *mut libc::c_void, tmp.len()) };
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n as usize]);
        if has_da1_reply(&buf) {
            break;
        }
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(not(unix))]
fn ask_terminal() -> Option<String> {
    None
}

static PALETTE: OnceLock<Palette> = OnceLock::new();

/// Ask the terminal for its colours. Call once, in raw mode, before the
/// event loop starts reading input. Safe to skip: `palette()` then uses the
/// guess.
pub fn detect() {
    let p = match ask_terminal() {
        Some(reply) => from_reply(&reply),
        None => guess(colorfgbg_is_light()),
    };
    let _ = PALETTE.set(p);
}

pub fn palette() -> Palette {
    *PALETTE.get_or_init(|| guess(colorfgbg_is_light()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_replies_of_any_digit_count() {
        let r = "\x1b]10;rgb:ffff/ffff/ffff\x1b\\\x1b]11;rgb:00/00/00\x07\x1b]4;3;rgb:c/a/0\x1b\\\x1b[?62;c";
        assert_eq!(find_color(r, "\x1b]10;"), Some((255, 255, 255)));
        assert_eq!(find_color(r, "\x1b]11;"), Some((0, 0, 0)));
        assert_eq!(find_color(r, "\x1b]4;3;"), Some((204, 170, 0)));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(find_color("\x1b]10;rgb:zz/00/00\x07", "\x1b]10;"), None);
        assert_eq!(find_color("\x1b]10;rgb:00/00\x07", "\x1b]10;"), None);
        assert_eq!(find_color("nothing", "\x1b]10;"), None);
    }

    #[test]
    fn mix_ends_and_middle() {
        assert_eq!(mix((0, 0, 0), (200, 100, 50), 0.0), (0, 0, 0));
        assert_eq!(mix((0, 0, 0), (200, 100, 50), 1.0), (200, 100, 50));
        assert_eq!(mix((0, 0, 0), (200, 100, 50), 0.5), (100, 50, 25));
    }

    #[test]
    fn reply_gives_theme_colours() {
        let p = from_reply("\x1b]10;rgb:dcdc/dcdc/dcdc\x07\x1b]11;rgb:1e1e/1e1e/2e2e\x07\x1b]4;3;rgb:e5e5/c0c0/7b7b\x07");
        assert_eq!(p.fg, (220, 220, 220));
        assert_eq!(p.bg, (30, 30, 46));
        assert_eq!(p.yellow, (229, 192, 123));
        assert!(p.yellow_real);
    }

    #[test]
    fn nested_prose_blends_but_stays_distinct() {
        let p = from_reply("\x1b]10;rgb:ffff/ffff/ffff\x07\x1b]11;rgb:0000/0000/0000\x07\x1b]4;3;rgb:ffff/ffff/0000\x07");
        let n = |c: Option<Color>| match c {
            Some(Color::Rgb(r, g, b)) => (r, g, b),
            _ => panic!("expected rgb"),
        };
        let narr = n(p.prose_color(true, false, false));
        let quote = n(p.prose_color(false, true, false));
        let quote_in_narr = n(p.prose_color(true, true, false));
        let narr_in_quote = n(p.prose_color(true, true, true));
        // Each nested colour differs from both pure colours...
        for c in [quote_in_narr, narr_in_quote] {
            assert_ne!(c, narr);
            assert_ne!(c, quote);
        }
        // ...and the inner prose leans towards its own colour.
        assert_ne!(quote_in_narr, narr_in_quote);
        assert!(quote_in_narr.2 < narr_in_quote.2 || quote_in_narr.0 != narr_in_quote.0);
    }

    #[test]
    fn narration_is_a_neutral_grey_between_fg_and_bg() {
        let p = from_reply("\x1b]10;rgb:dcdc/dcdc/ffff\x07\x1b]11;rgb:1e1e/1e1e/2e2e\x07");
        let (r, g, b) = p.narration();
        assert!(r == g && g == b);
        assert!(r > 30 && r < 220);
    }

    #[test]
    fn plain_text_keeps_terminal_default() {
        assert_eq!(guess(false).prose_color(false, false, false), None);
    }

    #[test]
    fn unreported_yellow_stays_indexed() {
        assert_eq!(
            guess(false).prose_color(false, true, false),
            Some(Color::Yellow)
        );
    }
}
