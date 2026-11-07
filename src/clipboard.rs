//! Copy text to the system clipboard (`<leader>y`).
//!
//! No extra crates: the text goes to whatever clipboard tool the machine has
//! (wl-copy, xclip, xsel, pbcopy, termux-clipboard-set, clip.exe) and, as a
//! fallback that also works over SSH, to the terminal itself with the OSC 52
//! escape sequence (most modern terminals accept it; tmux needs
//! `set -g set-clipboard on`).

use std::io::Write;
use std::process::{Command, Stdio};

/// Copy `text`; returns a short description of where it went.
pub fn copy(text: &str) -> Result<&'static str, String> {
    for (prog, args) in tools() {
        if pipe_to(prog, args, text) {
            return Ok(prog);
        }
    }
    osc52(text)
}

/// Clipboard programs worth trying on this machine, best guess first.
fn tools() -> Vec<(&'static str, &'static [&'static str])> {
    let mut v: Vec<(&'static str, &'static [&'static str])> = Vec::new();
    if std::env::var_os("TERMUX_VERSION").is_some() {
        v.push(("termux-clipboard-set", &[]));
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        v.push(("wl-copy", &[]));
    }
    if std::env::var_os("DISPLAY").is_some() {
        v.push(("xclip", &["-selection", "clipboard"]));
        v.push(("xsel", &["--clipboard", "--input"]));
    }
    if cfg!(target_os = "macos") {
        v.push(("pbcopy", &[]));
    }
    if std::env::var_os("WSL_DISTRO_NAME").is_some() {
        v.push(("clip.exe", &[]));
    }
    v
}

/// Run `prog` with `text` on its stdin. False when it is missing or fails.
fn pipe_to(prog: &str, args: &[&str], text: &str) -> bool {
    let child = Command::new(prog)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(text.as_bytes()).is_err() {
            let _ = child.wait();
            return false;
        }
        // Dropping stdin closes it so the tool sees the end of the text.
    }
    matches!(child.wait(), Ok(st) if st.success())
}

/// Ask the terminal to set the clipboard (OSC 52).
fn osc52(text: &str) -> Result<&'static str, String> {
    // Terminals cap the payload (xterm: about 100 kB).
    if text.len() > 74_000 {
        return Err("text too long for the terminal clipboard".to_string());
    }
    let seq = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let mut out = std::io::stdout();
    out.write_all(seq.as_bytes())
        .and_then(|_| out.flush())
        .map_err(|e| format!("clipboard: {}", e))?;
    Ok("terminal (OSC 52)")
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
