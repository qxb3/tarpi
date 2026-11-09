//! `/char-img`: the character card's picture in a square window.
//!
//! The picture is fetched from SillyTavern (`GET /characters/<avatar>`, the
//! card PNG itself), decoded here and drawn with half-block cells (`▀`, the
//! top pixel as foreground and the bottom pixel as background), so it works in
//! any truecolor terminal, over SSH and in Termux, with no image protocol.
//! A terminal cell is about twice as tall as wide, so `w` columns by `w / 2`
//! rows show a square of `w` x `w` pixels. State lives in `App::image_modal`,
//! drawing in `ui::draw_image_modal`.

use anyhow::{Context, Result};
use image::{imageops::FilterType, RgbImage};
use ratatui::layout::Rect;
use ratatui::style::Color;

/// How the picture reaches the screen.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Proto {
    /// Coloured half-block cells (1 x 2 points per cell): works everywhere,
    /// but is coarse.
    Blocks,
    /// Sixel graphics (foot, Windows Terminal, mlterm, xterm, ...): full resolution.
    Sixel,
    /// Kitty graphics protocol (kitty, WezTerm, Ghostty, ...): full resolution.
    Kitty,
    /// iTerm2 inline images (iTerm2, WezTerm): full resolution.
    Iterm,
}

/// Pick the best way to show a picture in this terminal. `ST_CLI_IMAGE`
/// (`blocks`, `kitty`, `iterm` or `sixel`) overrides the detection.
pub fn detect() -> Proto {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    match env("ST_CLI_IMAGE").to_lowercase().as_str() {
        "blocks" | "block" | "off" => return Proto::Blocks,
        "sixel" => return Proto::Sixel,
        "kitty" => return Proto::Kitty,
        "iterm" | "iterm2" => return Proto::Iterm,
        _ => {}
    }
    // Inside tmux / screen the escape codes do not get through.
    if !env("TMUX").is_empty() || env("TERM").starts_with("screen") {
        return Proto::Blocks;
    }
    let prog = env("TERM_PROGRAM");
    if !env("KITTY_WINDOW_ID").is_empty()
        || env("TERM").contains("kitty")
        || prog == "ghostty"
        || prog == "WezTerm"
    {
        Proto::Kitty
    } else if prog == "iTerm.app" {
        Proto::Iterm
    } else if env("TERM").starts_with("foot")
        || env("TERM").contains("mlterm")
        || !env("WT_SESSION").is_empty()
        || prog.to_lowercase().contains("contour")
    {
        Proto::Sixel
    } else {
        Proto::Blocks
    }
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len() / 3 * 4 + 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Escape code that removes a Kitty picture.
pub const KITTY_DELETE: &[u8] = b"\x1b_Ga=d,d=A,q=2\x1b\\";

pub struct ImageModal {
    pub title: String,
    pub proto: Proto,
    /// Set by the drawing code each frame: where the picture goes (graphics only).
    pub want: Option<Rect>,
    /// Where the picture was last sent to the terminal.
    pub placed: Option<Rect>,
    png: Vec<u8>,
    src: RgbImage,
    /// The picture scaled for the last size drawn: (side in pixels, pixels).
    cache: Option<(u32, RgbImage)>,
}

impl ImageModal {
    /// Decode a PNG / JPEG / WebP. Transparent areas are put on black.
    pub fn new(title: &str, bytes: &[u8]) -> Result<ImageModal> {
        let img = image::load_from_memory(bytes).context("could not read the card image")?;
        let png = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            bytes.to_vec()
        } else {
            let mut v = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Png)
                .context("could not convert the card image")?;
            v
        };
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        let mut rgb = RgbImage::new(w, h);
        for (x, y, p) in rgba.enumerate_pixels() {
            let a = p[3] as u32;
            let c = |v: u8| ((v as u32 * a) / 255) as u8;
            rgb.put_pixel(x, y, image::Rgb([c(p[0]), c(p[1]), c(p[2])]));
        }
        Ok(ImageModal {
            title: title.to_string(),
            proto: detect(),
            want: None,
            placed: None,
            png,
            src: rgb,
            cache: None,
        })
    }

    /// The picture fitted inside an `n` x `n` square of points (aspect ratio
    /// kept, centred, black around).
    fn square(&mut self, n: u32) -> &RgbImage {
        let n = n.max(2);
        if self.cache.as_ref().map(|c| c.0) != Some(n) {
            let (w, h) = self.src.dimensions();
            let scale = (n as f32 / w as f32).min(n as f32 / h as f32);
            let nw = ((w as f32 * scale).round() as u32).clamp(1, n);
            let nh = ((h as f32 * scale).round() as u32).clamp(1, n);
            let small = image::imageops::resize(&self.src, nw, nh, FilterType::Lanczos3);
            let mut sq = RgbImage::new(n, n);
            let (ox, oy) = ((n - nw) / 2, (n - nh) / 2);
            for (x, y, p) in small.enumerate_pixels() {
                sq.put_pixel(x + ox, y + oy, *p);
            }
            self.cache = Some((n, sq));
        }
        &self.cache.as_ref().unwrap().1
    }

    /// Half-block cells for a square of `side` x `side` points (`side` even):
    /// `side / 2` rows of `side` (top, bottom) colour pairs.
    pub fn rows(&mut self, side: u32) -> Vec<Vec<(Color, Color)>> {
        let side = side.max(2) & !1;
        let sq = self.square(side);
        let px = |x: u32, y: u32| {
            let p = sq.get_pixel(x, y);
            Color::Rgb(p[0], p[1], p[2])
        };
        (0..side / 2)
            .map(|r| (0..side).map(|x| (px(x, r * 2), px(x, r * 2 + 1))).collect())
            .collect()
    }

    /// Whether the terminal itself draws the picture.
    pub fn is_graphics(&self) -> bool {
        matches!(self.proto, Proto::Kitty | Proto::Iterm | Proto::Sixel)
    }

    /// Pixel size of a cell; 8 x 16 when the terminal does not say.
    fn cell_px() -> (f32, f32) {
        match crossterm::terminal::window_size() {
            Ok(w) if w.width > 0 && w.height > 0 && w.columns > 0 && w.rows > 0 => (
                w.width as f32 / w.columns as f32,
                w.height as f32 / w.rows as f32,
            ),
            _ => (8.0, 16.0),
        }
    }

    /// The part of `inner` (cells) the picture covers when drawn by a
    /// graphics protocol: its aspect ratio kept, centred.
    pub fn fit(&self, inner: Rect) -> Rect {
        let (cw, ch) = Self::cell_px();
        let (iw, ih) = self.src.dimensions();
        let (bw, bh) = (inner.width as f32 * cw, inner.height as f32 * ch);
        let scale = (bw / iw as f32).min(bh / ih as f32);
        let cols = ((iw as f32 * scale / cw).round() as u16).clamp(1, inner.width);
        let rows = ((ih as f32 * scale / ch).round() as u16).clamp(1, inner.height);
        Rect::new(
            inner.x + (inner.width - cols) / 2,
            inner.y + (inner.height - rows) / 2,
            cols,
            rows,
        )
    }

    /// Terminal bytes that draw the picture over the cells of `r`.
    pub fn escape(&self, r: Rect) -> Vec<u8> {
        let mut out = format!("\x1b[{};{}H", r.y + 1, r.x + 1).into_bytes();
        match self.proto {
            Proto::Kitty => {
                let data = b64(&self.png);
                let mut parts = data.as_bytes().chunks(4096).peekable();
                let mut first = true;
                while let Some(c) = parts.next() {
                    let more = if parts.peek().is_some() { 1 } else { 0 };
                    if first {
                        out.extend(
                            format!("\x1b_Ga=T,f=100,c={},r={},C=1,q=2,m={};", r.width, r.height, more)
                                .into_bytes(),
                        );
                        first = false;
                    } else {
                        out.extend(format!("\x1b_Gm={};", more).into_bytes());
                    }
                    out.extend_from_slice(c);
                    out.extend_from_slice(b"\x1b\\");
                }
            }
            Proto::Iterm => {
                out.extend(
                    format!(
                        "\x1b]1337;File=inline=1;size={};width={};height={};preserveAspectRatio=0:{}\x07",
                        self.png.len(),
                        r.width,
                        r.height,
                        b64(&self.png)
                    )
                    .into_bytes(),
                );
            }
            Proto::Sixel => {
                let (cw, ch) = Self::cell_px();
                let mut pw = (r.width as f32 * cw) as u32;
                let mut ph = (r.height as f32 * ch) as u32;
                // keep the data size reasonable
                let big = pw.max(ph);
                if big > 1000 {
                    pw = pw * 1000 / big;
                    ph = ph * 1000 / big;
                }
                let img = image::imageops::resize(&self.src, pw.max(1), ph.max(1), FilterType::Lanczos3);
                out.extend(sixel(&img));
            }
            Proto::Blocks => {}
        }
        out
    }
}

/// Encode a picture as a Sixel image (a fixed 6 x 6 x 6 colour cube).
fn sixel(img: &RgbImage) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let q = |v: u8| ((v as u32 * 5 + 127) / 255) as usize;
    let mut out = format!("\x1bPq\"1;1;{};{}", w, h).into_bytes();
    for i in 0..216usize {
        let (r, g, b) = (i / 36, (i / 6) % 6, i % 6);
        out.extend(format!("#{};2;{};{};{}", i, r * 20, g * 20, b * 20).into_bytes());
    }
    for y0 in (0..h).step_by(6) {
        // one sixel column of bits per colour, for this band of 6 rows
        let mut cols: Vec<Option<Vec<u8>>> = vec![None; 216];
        for x in 0..w {
            for dy in 0..6u32 {
                let y = y0 + dy;
                if y >= h {
                    break;
                }
                let p = img.get_pixel(x, y);
                let c = q(p[0]) * 36 + q(p[1]) * 6 + q(p[2]);
                let v = cols[c].get_or_insert_with(|| vec![0u8; w as usize]);
                v[x as usize] |= 1 << dy;
            }
        }
        for (c, bits) in cols.iter().enumerate() {
            let Some(bits) = bits else { continue };
            out.extend(format!("#{}", c).into_bytes());
            let mut i = 0;
            while i < bits.len() {
                let b = bits[i];
                let mut n = 1;
                while i + n < bits.len() && bits[i + n] == b {
                    n += 1;
                }
                let ch = 63 + b;
                if n > 3 {
                    out.extend(format!("!{}", n).into_bytes());
                    out.push(ch);
                } else {
                    out.extend(std::iter::repeat(ch).take(n));
                }
                i += n;
            }
            out.push(b'$'); // back to the left edge for the next colour
        }
        out.push(b'-'); // next band
    }
    out.extend_from_slice(b"\x1b\\");
    out
}
