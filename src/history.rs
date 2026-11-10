//! History of the `/commands` typed in a chat (Up on an empty `/` walks
//! through it, see `App::history_key`).
//!
//! It is kept in `~/.config/tarpi/command_history`, one command per line
//! (a backslash is written `\\` and a line break `\n`, so a command with a
//! line break in it still takes one line), oldest first. Every chat screen
//! starts from that file, so the history survives quitting and carries over
//! when you switch chat or character. Adding a command reads the file again
//! first, so several terminals do not wipe each other's commands.

use std::path::PathBuf;

/// How many commands are kept.
const MAX: usize = 200;

pub struct History {
    entries: Vec<String>,
    path: Option<PathBuf>,
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// The commands in the file, `None` when it cannot be read (not there yet).
fn read(path: &Option<PathBuf>) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(path.as_ref()?).ok()?;
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let cmd = unescape(line);
        if cmd.starts_with('/') && cmd.trim().len() > 1 {
            // A command that is in the file twice counts once, at its newest place.
            out.retain(|e| e != &cmd);
            out.push(cmd);
        }
    }
    let extra = out.len().saturating_sub(MAX);
    out.drain(..extra);
    Some(out)
}

/// Where one Up / Down press takes a walk through `n` entries.
#[derive(Debug, PartialEq, Eq)]
pub enum Walk {
    /// Show this entry (0 = oldest).
    To(usize),
    /// Down past the newest entry: put back what was in the box before.
    Back,
}

/// The next step of a walk. `pos` is the entry being shown, `None` before
/// the walk started; only Up starts one. Up stops at the oldest entry.
pub fn walk(pos: Option<usize>, up: bool, n: usize) -> Option<Walk> {
    match pos {
        None if up && n > 0 => Some(Walk::To(n - 1)),
        None => None,
        Some(p) if up => Some(Walk::To(p.saturating_sub(1))),
        Some(p) if p + 1 < n => Some(Walk::To(p + 1)),
        Some(_) => Some(Walk::Back),
    }
}

impl History {
    /// The history in the config directory.
    pub fn load() -> History {
        History::at(crate::config::history_path())
    }

    /// The history kept in `path` (`None`: only in memory).
    pub fn at(path: Option<PathBuf>) -> History {
        let entries = read(&path).unwrap_or_default();
        History { entries, path }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Entry `i`, oldest first.
    pub fn get(&self, i: usize) -> Option<&String> {
        self.entries.get(i)
    }

    /// Remember a command that was run. Only `/commands` are kept; running
    /// the same one again moves it to the newest place instead of adding a
    /// copy. Failing to write the file is not an error.
    pub fn add(&mut self, cmd: &str) {
        let cmd = cmd.trim();
        if !cmd.starts_with('/') || cmd.len() < 2 {
            return;
        }
        if let Some(disk) = read(&self.path) {
            self.entries = disk;
        }
        self.entries.retain(|e| e != cmd);
        self.entries.push(cmd.to_string());
        let extra = self.entries.len().saturating_sub(MAX);
        self.entries.drain(..extra);
        self.write();
    }

    fn write(&self) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut text: String = self
            .entries
            .iter()
            .map(|e| escape(e))
            .collect::<Vec<_>>()
            .join("\n");
        text.push('\n');
        let _ = std::fs::write(path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file() -> PathBuf {
        std::env::temp_dir().join(format!("tarpi-hist-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn walking() {
        // Only Up starts a walk, and only when there is something to show.
        assert_eq!(walk(None, true, 3), Some(Walk::To(2)));
        assert_eq!(walk(None, false, 3), None);
        assert_eq!(walk(None, true, 0), None);
        // Up goes back and stops at the oldest; Down comes forward.
        assert_eq!(walk(Some(2), true, 3), Some(Walk::To(1)));
        assert_eq!(walk(Some(0), true, 3), Some(Walk::To(0)));
        assert_eq!(walk(Some(0), false, 3), Some(Walk::To(1)));
        // Down past the newest goes back to the draft.
        assert_eq!(walk(Some(2), false, 3), Some(Walk::Back));
        assert_eq!(walk(Some(0), false, 1), Some(Walk::Back));
    }

    #[test]
    fn escaping_round_trips() {
        for s in ["/regen a", "/regen a\nb", "/regen back\\slash", "/x \\n literal", "/x\\"] {
            assert_eq!(unescape(&escape(s)), s);
        }
        assert!(!escape("/regen a\nb").contains('\n'));
    }

    #[test]
    fn only_commands_are_kept_and_repeats_move_to_the_end() {
        let mut h = History::at(None);
        h.add("hello there");
        h.add("/");
        h.add("  ");
        assert_eq!(h.len(), 0);
        h.add("/regen softer");
        h.add("/chats");
        h.add("/regen softer");
        assert_eq!(h.len(), 2);
        assert_eq!(h.get(0).unwrap(), "/chats");
        assert_eq!(h.get(1).unwrap(), "/regen softer");
    }

    #[test]
    fn keeps_the_newest_two_hundred() {
        let mut h = History::at(None);
        for i in 0..250 {
            h.add(&format!("/undo {}", i + 1));
        }
        assert_eq!(h.len(), MAX);
        assert_eq!(h.get(0).unwrap(), "/undo 51");
        assert_eq!(h.get(MAX - 1).unwrap(), "/undo 250");
    }

    #[test]
    fn saved_to_a_file_and_shared() {
        let p = temp_file();
        let mut a = History::at(Some(p.clone()));
        a.add("/regen one\ntwo");
        a.add("/chats");
        // A second box (another terminal / the next chat screen) sees both.
        let mut b = History::at(Some(p.clone()));
        assert_eq!(b.len(), 2);
        assert_eq!(b.get(0).unwrap(), "/regen one\ntwo");
        // Its own command does not wipe what `a` wrote meanwhile.
        a.add("/persona");
        b.add("/api");
        let c = History::at(Some(p.clone()));
        let all: Vec<&str> = (0..c.len()).map(|i| c.get(i).unwrap().as_str()).collect();
        assert_eq!(all, ["/regen one\ntwo", "/chats", "/persona", "/api"]);
        let _ = std::fs::remove_file(p);
    }
}
