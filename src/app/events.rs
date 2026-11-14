//! Terminal events: the entry point `on_event` and the keyboard handler
//! `on_key` (mouse and touch live in `mouse`).

use super::*;

/// `<leader>y`: put `text` on the system clipboard; the status-line message
/// and whether it is an error.
pub fn copy_to_clipboard(text: &str) -> (String, bool) {
    let n = text.chars().count();
    match crate::clipboard::copy(text) {
        Ok(how) => (format!("yanked {} chars to the clipboard ({})", n, how), false),
        Err(e) => (e, true),
    }
}

impl App {
    pub async fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k).await,
            Event::Paste(s) => {
                if let Some(c) = self.card_modal.as_mut() {
                    c.paste(&s);
                    c.follow = true;
                    return;
                }
                if self.picker.is_some()
                    || self.pos_edit.is_some()
                    || self.prompt_view.is_some()
                    || self.lore_ask.is_some()
                {
                    return;
                }
                self.input_scroll = None;
                if self.input.mode == Mode::Command {
                    self.input.command_paste(&s);
                } else {
                    self.input.paste_text(&s);
                }
                self.scroll = 0;
            }
            Event::Mouse(m) => self.on_mouse(m).await,
            _ => {}
        }
    }

    /// Without vim mode, Up / Down move the cursor in a multi-line draft like
    /// in any text box; with one line they scroll the chat.
    pub(super) fn text_nav(&self) -> bool {
        !self.vim && self.input_rows.0 > 1
    }

    pub(super) async fn on_key(&mut self, k: KeyEvent) {
        // Typing or moving the cursor brings the box back to the cursor.
        self.input_scroll = None;
        // The path menu only lives while the box holds an `/import` path.
        if self.path_open && crate::pathcomp::typed_arg(&self.input.text).is_none() {
            self.path_open = false;
        }
        if let Some(a) = self.lore_ask.as_mut() {
            let act = a.handle_key(k);
            self.lore_ask_action(act);
            return;
        }
        if self.image_modal.is_some() {
            // Any key closes the picture.
            self.image_modal = None;
            return;
        }
        if self.card_modal.is_some() {
            self.on_card_key(k).await;
            return;
        }
        if let Some(e) = self.pos_edit.as_mut() {
            let act = e.handle_key(k);
            self.pos_action(act).await;
            return;
        }
        if let Some((_, p)) = self.picker.as_mut() {
            let act = p.handle_key(k);
            self.picker_action(act).await;
            return;
        }
        if self.prompt_view.is_some() {
            self.prompt_key(k);
            return;
        }
        if self.show_help {
            // The help window scrolls; any other key closes it.
            let page = self.help_page.max(1);
            match k.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = self.help_scroll.saturating_add(1)
                }
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(page),
                KeyCode::PageDown | KeyCode::Char(' ') => {
                    self.help_scroll = self.help_scroll.saturating_add(page)
                }
                KeyCode::Home | KeyCode::Char('g') => self.help_scroll = 0,
                KeyCode::End | KeyCode::Char('G') => self.help_scroll = usize::MAX,
                _ => {
                    self.show_help = false;
                    self.help_scroll = 0;
                }
            }
            return;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let page = self.viewport_h.saturating_sub(2).max(1);

        // COMMAND mode (`:` command line): keys edit the command, Enter runs
        // it, Esc / Backspace on an empty line leaves.
        if self.input.mode == Mode::Command {
            match k.code {
                KeyCode::Char('c') if ctrl => self.quit().await,
                KeyCode::Char('u') if ctrl => self.input.command_clear(),
                KeyCode::Char('w') if ctrl => self.input.command_delete_word(),
                KeyCode::Char(c) if !ctrl && !alt => self.input.command_push(c),
                KeyCode::Backspace => self.input.command_backspace(),
                KeyCode::Esc => self.input.cancel_command(),
                KeyCode::Enter => {
                    match self.input.run_command() {
                        Ok(msg) if msg.is_empty() => {}
                        Ok(msg) => self.set_status(msg, false),
                        Err(msg) => self.set_status(msg, true),
                    }
                    // `:w` = Ctrl+S.
                    let _ = self.input.take_quit_req();
                    if self.input.take_write_req() {
                        self.commit_edit_to_card().await;
                    }
                }
                _ => {}
            }
            return;
        }

        // Esc / Ctrl+C with a menu above the box open only hide the menu.
        if self.hide_menu_key(&k) {
            return;
        }
        // The path menu of `/import`: Tab opens it, then Tab / arrows / Enter
        // fill paths in. Before the spelling menu, which also uses Tab.
        if self.path_key(&k) {
            return;
        }
        // The spelling menu above the box (Tab opens it on a misspelled word).
        if self.spell_key(&k) {
            return;
        }
        // Up / Down on a bare `/` walk through the commands run before.
        if self.history_key(&k) {
            return;
        }
        // The `/command` menu above the box: Tab / arrows fill commands in.
        if self.completion_key(&k) {
            return;
        }
        self.comp = Comp::default();

        // Ctrl+U / Ctrl+D in NORMAL / VISUAL mode: half a box up / down.
        if ctrl
            && !alt
            && matches!(self.input.mode, Mode::Normal | Mode::Visual)
            && matches!(k.code, KeyCode::Char('u') | KeyCode::Char('d'))
        {
            let dir = if k.code == KeyCode::Char('u') { -1 } else { 1 };
            let half = (self.input_rows.1 / 2).max(1);
            self.input.move_rows(dir, half, self.input_w);
            return;
        }

        // NORMAL / VISUAL mode (vim): letters are commands. Enter sends in
        // NORMAL mode and does nothing in VISUAL mode.
        if matches!(self.input.mode, Mode::Normal | Mode::Visual) && !ctrl && !alt {
            match k.code {
                KeyCode::Char(c) => {
                    self.input.normal_key(c, self.input_w);
                    if let Some(text) = self.input.take_clipboard() {
                        let (msg, err) = copy_to_clipboard(&text);
                        self.set_status(msg, err);
                    }
                    return;
                }
                KeyCode::Enter if self.input.mode == Mode::Normal => {
                    self.submit().await;
                    return;
                }
                KeyCode::Enter => return,
                _ => {}
            }
        }

        match k.code {
            KeyCode::Char('c') if ctrl => self.quit().await,
            KeyCode::Char('d') if ctrl => {
                if self.input.is_empty() {
                    self.quit().await;
                } else {
                    self.input.delete();
                }
            }
            KeyCode::Char('p') if ctrl => self.edit_last(false),
            KeyCode::Char('o') if ctrl => self.edit_last(true),
            KeyCode::Char('s') if ctrl => self.commit_edit_to_card().await,
            KeyCode::Char('r') if ctrl => self.regenerate(None).await,
            KeyCode::Char('k') if ctrl => self.continue_message().await,
            KeyCode::Char('j') if ctrl => {
                self.input.insert_char('\n');
                self.scroll = 0;
            }
            KeyCode::Char('a') if ctrl => self.input.home(),
            KeyCode::Char('e') if ctrl => self.input.end(),
            KeyCode::Char('u') if ctrl => self.input.clear_undoable(),
            KeyCode::Char('w') if ctrl => self.input.delete_word_back(),
            // Plain Left / Right browse swipes while the box has no draft and no
            // message is being edited; otherwise they move the text cursor.
            KeyCode::Left if ctrl && !alt && self.input.is_empty() && self.editing.is_none() => {
                self.switch_branch(-1).await
            }
            KeyCode::Right if ctrl && !alt && self.input.is_empty() && self.editing.is_none() => {
                self.switch_branch(1).await
            }
            KeyCode::Left if !ctrl && !alt && self.input.is_empty() && self.editing.is_none() => {
                self.swipe(-1).await
            }
            KeyCode::Right if !ctrl && !alt && self.input.is_empty() && self.editing.is_none() => {
                self.swipe(1).await
            }
            KeyCode::Esc => {
                if self.input.mode == Mode::Visual {
                    // First Esc only leaves VISUAL mode.
                    self.input.exit_visual();
                } else {
                    self.input.cancel_pending();
                    if self.editing.is_some() {
                        self.cancel_edit();
                    } else if self.is_generating() && !self.stop_blocked() {
                        self.cancel_generation().await;
                    }
                }
            }
            KeyCode::F(1) => self.show_help = true,
            KeyCode::Enter => {
                // Enter sends (or saves an edit). Shift+Enter writes a new
                // line; Alt+Enter and Ctrl+J do too, for terminals that
                // cannot tell Shift+Enter from Enter.
                let shift = k.modifiers.contains(KeyModifiers::SHIFT);
                if (shift || alt) && !ctrl && self.input.mode == Mode::Insert {
                    self.input.insert_char('\n');
                    self.scroll = 0;
                } else {
                    self.submit().await;
                }
            }
            KeyCode::Char(c) if (!ctrl && !alt) || (ctrl && alt) => {
                // The NORMAL-mode keys (default `qq`, typed quickly) leave INSERT mode.
                self.input.type_char(c);
                self.scroll = 0;
            }
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left if alt && !ctrl => self.input.word_left(self.input_w),
            KeyCode::Right if alt && !ctrl => self.input.word_right(self.input_w),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            KeyCode::Up if self.editing.is_some() || self.text_nav() => {
                self.input.cursor =
                    ui::vertical_target(&self.input.text, self.input.cursor, self.input_w, -1);
            }
            KeyCode::Down if self.editing.is_some() || self.text_nav() => {
                self.input.cursor =
                    ui::vertical_target(&self.input.text, self.input.cursor, self.input_w, 1);
            }
            KeyCode::Up => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Down => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_add(page),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(page),
            _ => {}
        }
        self.input.clamp_normal();
    }
}
