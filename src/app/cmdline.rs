//! The command line (the input box): the `/command` menu, path completion
//! after `/import `, and the command history.

use super::*;

impl App {
    /// The commands the menu offers for `word`.
    pub(super) fn menu_commands(&self, word: &str) -> Vec<&'static cmds::Cmd> {
        cmds::matches(word)
    }

    /// The completion menu, when it should show: the box is in INSERT mode
    /// and holds one `/word` and nothing else. The entries are the commands
    /// that word starts; then the highlighted entry (the top one until Tab /
    /// the arrows move it) and whether it has been filled into the box yet.
    pub fn completion(&self) -> Option<(Vec<&'static cmds::Cmd>, usize, bool)> {
        if !self.command_menu
            || self.editing.is_some()
            || self.comp.done
            || self.input.mode != Mode::Insert
        {
            return None;
        }
        // While cycling, the list stays that of the word typed at the start,
        // even though the box now holds the full command name.
        if let Some(base) = &self.comp.base {
            let list = self.menu_commands(base);
            if let Some(c) = list.get(self.comp.sel) {
                // The box holds the filled-in name (and the space after it,
                // for a command that takes an argument).
                if self.input.text == c.fill() {
                    return Some((list, self.comp.sel, true));
                }
            }
        }
        let list = self.menu_commands(cmds::typed_word(&self.input.text)?);
        if list.is_empty() {
            None
        } else {
            Some((list, 0, false))
        }
    }

    /// Command history. With nothing but a `/` in the box, Up puts the
    /// newest command that was run before in the box; then Up goes further
    /// back and Down forward, and Down past the newest puts the `/` back.
    /// Typing or editing anything ends the walk (the keys then do what they
    /// always did). Tab / Shift+Tab stay the command menu's keys.
    /// Returns false when the key is not for the history.
    pub(super) fn history_key(&mut self, k: &KeyEvent) -> bool {
        if self.input.mode != Mode::Insert
            || self.editing.is_some()
            || k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        let up = match k.code {
            KeyCode::Up => true,
            KeyCode::Down => false,
            _ => return false,
        };
        // A walk only goes on while the box still shows the entry it put there.
        if let Some(p) = self.hist_pos {
            if self.cmd_hist.get(p) != Some(&self.input.text) {
                self.hist_pos = None;
            }
        }
        // Only Up starts a walk, and only from a bare `/`.
        if self.hist_pos.is_none() && self.input.text != "/" {
            return false;
        }
        let Some(step) = crate::history::walk(self.hist_pos, up, self.cmd_hist.len()) else {
            return false;
        };
        let to = match step {
            crate::history::Walk::To(i) => i,
            crate::history::Walk::Back => {
                // Past the newest: back to what was there before.
                let draft = std::mem::take(&mut self.hist_draft);
                self.hist_pos = None;
                self.comp = Comp::default();
                self.set_input_text(draft);
                return true;
            }
        };
        if self.hist_pos.is_none() {
            self.hist_draft = self.input.text.clone();
        }
        let Some(cmd) = self.cmd_hist.get(to).cloned() else {
            return false;
        };
        self.hist_pos = Some(to);
        self.comp = Comp::default();
        self.set_input_text(cmd);
        true
    }

    /// The top command is highlighted from the start. Tab / Down move the
    /// highlight to the next one and Shift+Tab / Up to the previous one
    /// (wrapping), filling it into the box; the first press already leaves
    /// the top one. Esc only hides the menu (see `hide_menu_key`).
    /// Returns false when the key is not for the menu.
    pub(super) fn completion_key(&mut self, k: &KeyEvent) -> bool {
        if k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            return false;
        }
        let step: i32 = match k.code {
            KeyCode::Tab | KeyCode::Down => 1,
            KeyCode::BackTab | KeyCode::Up => -1,
            _ => return false,
        };
        let Some((list, sel, picked)) = self.completion() else {
            return false;
        };
        let n = list.len() as i32;
        let (base, idx) = if picked {
            (
                self.comp.base.clone().unwrap_or_default(),
                (sel as i32 + step).rem_euclid(n) as usize,
            )
        } else {
            let typed = cmds::typed_word(&self.input.text).unwrap_or("").to_string();
            // Nothing filled in yet: the top entry is only highlighted, so
            // the first Tab (or arrow) already moves on from it.
            (typed, step.rem_euclid(n) as usize)
        };
        let fill = list[idx].fill();
        self.set_input_text(fill);
        self.comp = Comp {
            base: Some(base),
            sel: idx,
            done: false,
        };
        true
    }

    /// Enter on a half-typed command chooses the highlighted one. A command
    /// that takes an argument (`/imp` -> `/import `) is filled in with a space
    /// after it and nothing is run yet; any other (`/ch` -> `/chats`) is
    /// filled in and runs at once (this returns false so the key goes on to
    /// run it). A word that already is a command or short form (`/c`, `/nc`)
    /// is left as typed and runs. Returns true when it only completed.
    pub(super) fn apply_top_completion(&mut self) -> bool {
        let Some((list, sel, picked)) = self.completion() else {
            return false;
        };
        // Already filled in with Tab / the arrows: it runs as it stands.
        if picked {
            return false;
        }
        let word = cmds::typed_word(&self.input.text)
            .unwrap_or("")
            .to_lowercase();
        let complete = cmds::COMMANDS
            .iter()
            .any(|c| c.name == word || (!c.alias.is_empty() && c.alias == word));
        if complete {
            return false;
        }
        let top = list[sel];
        self.set_input_text(top.fill());
        self.comp = Comp::default();
        top.args
    }

    /// Esc or Ctrl+C while a menu above the input box is showing (the
    /// `/command` menu, the `/import` path menu or the spelling menu) hides
    /// that menu and does nothing else: no quitting, no stopping a reply, no
    /// leaving the mode. The command menu stays shut until the next key (like
    /// after a click) and the path menu until Tab opens it again; what is in
    /// the box is left as it is. Returns
    /// false when no menu is showing (or for any other key).
    pub(super) fn hide_menu_key(&mut self, k: &KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let esc = k.code == KeyCode::Esc && !ctrl && !alt;
        let ctrl_c = k.code == KeyCode::Char('c') && ctrl && !alt;
        if !esc && !ctrl_c {
            return false;
        }
        if self.spell_menu.is_some() {
            self.spell_menu = None;
            return true;
        }
        if self.card_modal.is_none() {
            if self.path_completion().is_some() {
                self.path_open = false;
                self.comp = Comp::default();
                return true;
            }
            if self.completion().is_some() {
                self.comp = Comp {
                    base: None,
                    sel: 0,
                    done: true,
                };
                return true;
            }
        }
        false
    }

    /// Whether the box is in a state where the path menu may be used: INSERT
    /// mode, no message being edited, no command history walk going on.
    pub(super) fn path_usable(&self) -> bool {
        self.editing.is_none() && self.hist_pos.is_none() && self.input.mode == Mode::Insert
    }

    /// The path menu of `/import`, when it should show: Tab opened it (see
    /// `path_key`), the box is in INSERT mode and holds `/import ` and a path
    /// (see `pathcomp::typed_arg`) that has entries. Gives the entries, the
    /// highlighted one (the top one until Tab / the arrows move it) and
    /// whether it has been filled into the box. It does not depend on
    /// `command_menu`: it never opens by itself.
    pub fn path_completion(&self) -> Option<(Vec<crate::pathcomp::Entry>, usize, bool)> {
        if !self.path_open || !self.path_usable() {
            return None;
        }
        let arg = crate::pathcomp::typed_arg(&self.input.text)?;
        // While cycling, the list stays that of the path typed at the start,
        // even though the box now holds one of its entries.
        if let Some(base) = &self.comp.base {
            let list = self.path_cache.entries(base);
            if let Some(e) = list.get(self.comp.sel) {
                if arg == e.insert {
                    return Some((list, self.comp.sel, true));
                }
            }
        }
        let list = self.path_cache.entries(arg);
        if list.is_empty() {
            None
        } else {
            Some((list, 0, false))
        }
    }

    /// Keys of the path menu. After `/import ` (and a path, if one is typed
    /// already) **Tab opens** the menu and fills in its first entry at once; it
    /// never opens by itself. Once open, Tab (or Down) goes on to the next
    /// entry on each press; Shift+Tab (or Up) starts from the last entry and moves
    /// back; both wrap. Enter chooses the highlighted entry like the command
    /// menu does: a folder is opened (the menu lists what is inside), a file
    /// is filled in and imported at once; with an entry already filled in,
    /// Enter on a folder opens it and on a file imports it. Esc / Ctrl+C
    /// only hide the menu (see `hide_menu_key`).
    /// Returns false when the key is not for the menu.
    pub(super) fn path_key(&mut self, k: &KeyEvent) -> bool {
        if k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            return false;
        }
        if !self.path_open {
            // Tab on `/import <path>` opens the menu. While the command menu
            // is cycling, Tab belongs to it, except on `/import ` itself
            // (filled in by Tab): that goes on to the paths.
            let cmd_menu_has_tab = match self.completion() {
                Some((list, sel, picked)) => !(picked && list[sel].name == "import"),
                None => false,
            };
            if k.code == KeyCode::Tab && self.path_usable() && !cmd_menu_has_tab {
                let typed = crate::pathcomp::typed_arg(&self.input.text).map(str::to_string);
                if let Some(arg) = typed {
                    let list = self.path_cache.entries(&arg);
                    if let Some(first) = list.first() {
                        // Opens the menu and fills in the very first entry
                        // right away; the next Tab goes on to the second.
                        self.path_open = true;
                        self.set_input_text(format!("/import {}", first.insert));
                        self.comp = Comp {
                            base: Some(arg),
                            sel: 0,
                            done: false,
                        };
                        return true;
                    }
                }
            }
            return false;
        }
        let step: i32 = match k.code {
            KeyCode::Tab | KeyCode::Down => 1,
            KeyCode::BackTab | KeyCode::Up => -1,
            KeyCode::Enter => {
                let Some((list, sel, picked)) = self.path_completion() else {
                    return false;
                };
                let e = &list[sel];
                if picked {
                    if e.is_dir {
                        // The folder is already in the box; forgetting the
                        // cycle makes the menu list its contents.
                        self.comp = Comp::default();
                        return true;
                    }
                    // A file that is filled in: import it.
                    return false;
                }
                // Nothing filled in yet: choose the highlighted entry.
                let (insert, is_dir) = (e.insert.clone(), e.is_dir);
                self.set_input_text(format!("/import {}", insert));
                self.comp = Comp::default();
                // A folder: the menu goes on with what is inside. A file: the
                // key goes on to send the command, which imports it.
                return is_dir;
            }
            _ => return false,
        };
        let Some((list, sel, picked)) = self.path_completion() else {
            return false;
        };
        let n = list.len() as i32;
        let (base, idx) = if picked {
            (
                self.comp.base.clone().unwrap_or_default(),
                (sel as i32 + step).rem_euclid(n) as usize,
            )
        } else {
            let typed = crate::pathcomp::typed_arg(&self.input.text)
                .unwrap_or("")
                .to_string();
            (typed, if step > 0 { 0 } else { (n - 1) as usize })
        };
        self.set_input_text(format!("/import {}", list[idx].insert));
        self.comp = Comp {
            base: Some(base),
            sel: idx,
            done: false,
        };
        true
    }

    /// An entry was clicked in the path menu: fill it in. A folder is opened
    /// (the menu goes on with what is inside); a file closes the menu.
    pub(super) fn accept_path(&mut self, i: usize) {
        let Some((list, _, _)) = self.path_completion() else {
            return;
        };
        let Some(e) = list.get(i) else {
            return;
        };
        let (insert, is_dir) = (e.insert.clone(), e.is_dir);
        self.set_input_text(format!("/import {}", insert));
        self.comp = Comp::default();
        // A folder keeps the menu open on its contents; a file closes it.
        self.path_open = is_dir;
    }

    /// A command was clicked in the menu: fill it in and close the menu.
    /// Returns true when the command takes nothing more, so it should run now.
    pub(super) fn accept_completion(&mut self, name: &str) -> bool {
        let args = cmds::COMMANDS
            .iter()
            .find(|c| c.name == name)
            .map_or(true, |c| c.args);
        self.set_input_text(if args {
            format!("/{} ", name)
        } else {
            format!("/{}", name)
        });
        self.comp = Comp {
            base: None,
            sel: 0,
            done: true,
        };
        !args
    }

    /// Replace the whole box with `text`, cursor at the end.
    pub(super) fn set_input_text(&mut self, text: String) {
        self.input.clear();
        self.input.insert_str(&text);
        self.input.cursor = self.input.len_chars();
        self.scroll = 0;
    }
}
