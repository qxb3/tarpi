//! The `/char` card window and the `/char-img` picture window.

use super::*;

impl App {
    /// `/char-img`: show the card's picture in a square window.
    pub(super) async fn open_image_modal(&mut self) {
        let avatar = if self.character.avatar.trim().is_empty() {
            self.chat.avatar.clone()
        } else {
            self.character.avatar.clone()
        };
        let res = match charstore::image(&avatar) {
            Ok(bytes) => ImageModal::new(&self.character.name, &bytes),
            Err(e) => Err(e),
        };
        match res {
            Ok(m) => self.image_modal = Some(m),
            Err(e) => self.set_status(format!("{:#}", e), true),
        }
    }

    /// `/character`: open the card of this character as an editable form.
    pub(super) async fn open_card_modal(&mut self) {
        // Read the card fresh: it may have been edited since the chat opened.
        let avatar = self.character.avatar.clone();
        let shown = match charstore::get(&avatar) {
            Ok(v) => {
                let mut ch = Character::from_value(&v);
                if ch.avatar.trim().is_empty() {
                    ch.avatar = avatar;
                }
                ch
            }
            Err(_) => self.character.clone(),
        };
        self.card_modal = Some(CardModal::new(&shown, &self.input));
    }

    pub(super) async fn cancel_card_modal(&mut self) {
        let dirty = self.card_modal.as_ref().map(|m| m.dirty()).unwrap_or(false);
        self.card_modal = None;
        if self.persona_edit.take().is_some() {
            // Back to the persona window.
            self.open_persona_picker().await;
            if dirty {
                self.set_status("persona: changes discarded", false);
            }
            return;
        }
        if self.sys_edit.take().is_some() {
            // Back to the system prompt window.
            self.open_sysprompt_picker().await;
            if dirty {
                self.set_status("system prompt: changes discarded", false);
            }
            return;
        }
        if self.note_edit {
            self.note_edit = false;
            if dirty {
                self.set_status("author's note: changes discarded", false);
            }
            return;
        }
        if self.vector_edit {
            self.vector_edit = false;
            if dirty {
                self.set_status("vector memory: changes discarded", false);
            }
            return;
        }
        if self.config_edit {
            self.config_edit = false;
            if dirty {
                self.set_status("config: changes discarded", false);
            }
            return;
        }
        if dirty {
            self.set_status("character card: changes discarded", false);
        }
    }

    /// Write every changed field of the form to the card. The form stays open
    /// (Ctrl+S, `:w`, Save) unless `close` is set (`:wq`). On a failure it
    /// stays open either way (what was already written counts as saved).
    pub(super) async fn save_card_modal(&mut self, close: bool) {
        if self.persona_edit.is_some() {
            self.save_persona_form(close).await;
            return;
        }
        if self.sys_edit.is_some() {
            self.save_sysprompt_form(close).await;
            return;
        }
        if self.note_edit {
            self.save_note_form(close).await;
            return;
        }
        if self.vector_edit {
            self.save_vector_form(close).await;
            return;
        }
        if self.config_edit {
            self.save_config_form(close).await;
            return;
        }
        let changes = match self.card_modal.as_mut() {
            Some(m) => {
                m.sync();
                m.changes()
            }
            None => return,
        };
        if changes.is_empty() {
            if close {
                self.card_modal = None;
            } else {
                self.card_note("no changes to save", false);
            }
            self.set_status("character card: no changes to save", false);
            return;
        }
        let avatar = self.character.avatar.clone();
        let total = changes.len();
        for (key, value) in changes {
            if let Err(e) = charstore::set_field(&avatar, key, &value) {
                self.card_note(format!("{:#}", e), true);
                self.set_status(format!("{:#}", e), true);
                return;
            }
            self.apply_card_field(key, &value);
            if let Some(m) = self.card_modal.as_mut() {
                m.mark_saved(key);
            }
        }
        let msg = format!(
            "saved to the character card ({} field{})",
            total,
            if total == 1 { "" } else { "s" }
        );
        if close {
            self.card_modal = None;
        } else {
            self.card_note(msg.clone(), false);
        }
        self.set_status(msg, false);
    }

    /// Keep the card held in memory in step with what was just saved.
    pub(super) fn apply_card_field(&mut self, key: &str, value: &Value) {
        let text = value.as_str().unwrap_or("").to_string();
        let ch = &mut self.character;
        match key {
            "name" => ch.name = text,
            "description" => ch.description = text,
            "personality" => ch.personality = text,
            "scenario" => ch.scenario = text,
            "first_mes" => ch.first_mes = text,
            "mes_example" => ch.mes_example = text,
            "system_prompt" => ch.system_prompt = text,
            "post_history_instructions" => ch.post_history_instructions = text,
            "depth_prompt" => {
                let prompt = value.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
                ch.depth_prompt = if prompt.trim().is_empty() {
                    None
                } else {
                    Some(crate::character::DepthPrompt {
                        prompt: prompt.to_string(),
                        depth: value.get("depth").and_then(|v| v.as_u64()).unwrap_or(4) as usize,
                        role: value
                            .get("role")
                            .and_then(|v| v.as_str())
                            .unwrap_or("system")
                            .to_string(),
                    })
                };
            }
            "alternate_greetings" => {
                ch.alternate_greetings = value
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str())
                            .map(|x| x.to_string())
                            .collect::<Vec<String>>()
                    })
                    .unwrap_or_default();
            }
            _ => {}
        }
    }

    /// A one-line message inside the `/character` window.
    pub(super) fn card_note(&mut self, msg: impl Into<String>, is_error: bool) {
        if let Some(m) = self.card_modal.as_mut() {
            m.note = msg.into();
            m.note_err = is_error;
        }
    }

    /// Keys in the `/character` window. The focused field is edited with the
    /// chat box's own editor, so INSERT / NORMAL / VISUAL / `:` all work.
    pub(super) async fn on_card_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        // Esc / Ctrl+C with the spelling menu open only hide the menu.
        if self.hide_menu_key(&k) {
            return;
        }
        if ctrl {
            match k.code {
                KeyCode::Char('c') => {
                    self.quit().await;
                    return;
                }
                KeyCode::Char('s') => {
                    // Saves and stays; `:wq` saves and closes.
                    self.save_card_modal(false).await;
                    return;
                }
                _ => {}
            }
        }
        // A row of choices (`/note`, `/memory`): arrows change it, nothing is typed.
        if self.card_modal.as_ref().map_or(false, |c| c.pick_of(c.focus).is_some()) {
            self.on_pick_key(k).await;
            return;
        }
        // Tab opens the spelling menu on a misspelled word.
        if self.spell_key(&k) {
            return;
        }
        let half_dir = match k.code {
            KeyCode::Char('u') if ctrl && !alt => -1,
            KeyCode::Char('d') if ctrl && !alt => 1,
            _ => 0,
        };

        // Only the `/config` window: Tab types two spaces (YAML indent).
        let tab_spaces = self.config_edit;
        let mut cancel = false;
        let mut cmd_result: Option<Result<String, String>> = None;
        let mut write = false;
        let mut quit = false;
        let mut clip: Option<String> = None;
        {
            let Some(c) = self.card_modal.as_mut() else {
                return;
            };
            c.note.clear();
            c.note_err = false;
            let was_armed = std::mem::replace(&mut c.discard_armed, false);
            c.follow = true;
            let width = c.text_w.max(1);
            let page = c.view_h.saturating_sub(1).max(1);

            if c.input.mode == Mode::Command {
                // `:` command line (`:%s/find/new/g` and friends).
                match k.code {
                    KeyCode::Char('u') if ctrl => c.input.command_clear(),
                    KeyCode::Char('w') if ctrl => c.input.command_delete_word(),
                    KeyCode::Char(ch) if !ctrl && !alt => c.input.command_push(ch),
                    KeyCode::Backspace => c.input.command_backspace(),
                    KeyCode::Esc => c.input.cancel_command(),
                    KeyCode::Enter => {
                        cmd_result = Some(c.input.run_command());
                        write = c.input.take_write_req();
                        quit = c.input.take_quit_req();
                    }
                    _ => {}
                }
            } else if half_dir != 0 && matches!(c.input.mode, Mode::Normal | Mode::Visual) {
                // Ctrl+U / Ctrl+D: half a window up / down; reaching the
                // edge of the field moves on into the neighbouring one.
                c.half_page(half_dir);
            } else if matches!(c.input.mode, Mode::Normal | Mode::Visual)
                && !ctrl
                && !alt
                && matches!(k.code, KeyCode::Char(_))
            {
                // NORMAL / VISUAL mode: letters are vim commands.
                if let KeyCode::Char(ch) = k.code {
                    c.normal_key(ch);
                    clip = c.input.take_clipboard();
                }
            } else {
                let mode = c.input.mode;
                match k.code {
                    KeyCode::Esc => match mode {
                        Mode::Visual => c.input.exit_visual(),
                        // Like vim: Esc leaves INSERT mode.
                        Mode::Insert if c.input.vim_enabled() => c.input.enter_normal(),
                        _ => {
                            c.input.cancel_pending();
                            if c.dirty() && !was_armed {
                                c.discard_armed = true;
                                c.note = "unsaved changes: Esc again discards, Ctrl+S saves"
                                    .to_string();
                                c.note_err = true;
                            } else {
                                cancel = true;
                            }
                        }
                    },
                    KeyCode::Enter => match mode {
                        Mode::Insert => c.input.insert_char('\n'),
                        Mode::Normal => c.down(),
                        _ => {}
                    },
                    KeyCode::Backspace => c.input.backspace(),
                    KeyCode::Delete => c.input.delete(),
                    KeyCode::Left if alt && !ctrl => c.input.word_left(width),
                    KeyCode::Right if alt && !ctrl => c.input.word_right(width),
                    KeyCode::Left => c.input.left(),
                    KeyCode::Right => c.input.right(),
                    KeyCode::Up => c.up(),
                    KeyCode::Down => c.down(),
                    KeyCode::Home => c.input.home(),
                    KeyCode::End => c.input.end(),
                    KeyCode::PageUp => {
                        for _ in 0..page {
                            c.up();
                        }
                    }
                    KeyCode::PageDown => {
                        for _ in 0..page {
                            c.down();
                        }
                    }
                    KeyCode::Tab if tab_spaces && !ctrl && !alt => {
                        if mode == Mode::Insert {
                            c.input.insert_char(' ');
                            c.input.insert_char(' ');
                        }
                    }
                    KeyCode::Char('a') if ctrl => c.input.home(),
                    KeyCode::Char('e') if ctrl => c.input.end(),
                    KeyCode::Char('w') if ctrl => c.input.delete_word_back(),
                    KeyCode::Char('j') if ctrl => c.input.insert_char('\n'),
                    // INSERT mode; typing the NORMAL-mode keys (`qq`) quickly
                    // switches to NORMAL mode.
                    KeyCode::Char(ch) if (!ctrl && !alt) || (ctrl && alt) => {
                        c.input.type_char(ch)
                    }
                    _ => {}
                }
            }
            c.input.clamp_normal();
            c.sync();
        }
        if let Some(text) = clip {
            let (msg, err) = copy_to_clipboard(&text);
            self.card_note(msg.clone(), err);
            self.set_status(msg, err);
        }
        if cancel {
            self.cancel_card_modal().await;
            return;
        }
        if write {
            // `:w` = Ctrl+S / the Save button (stays open); `:wq` closes too.
            self.save_card_modal(quit).await;
            return;
        }
        match cmd_result {
            Some(Ok(msg)) if !msg.is_empty() => {
                self.card_note(msg.clone(), false);
                self.set_status(msg, false);
            }
            Some(Err(msg)) => {
                self.card_note(msg.clone(), true);
                self.set_status(msg, true);
            }
            _ => {}
        }
    }

    pub(super) async fn on_card_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if let Some(c) = self.card_modal.as_mut() {
                    c.follow = false;
                    c.scroll = if up {
                        c.scroll.saturating_sub(3)
                    } else {
                        c.scroll.saturating_add(3)
                    };
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let pos = Position::new(m.column, m.row);
                if self.spell_menu.is_some() {
                    // A click on a suggestion puts it in; anywhere else closes the menu.
                    if self.spell_area.contains(pos) {
                        if let Some(&(_, i)) = self.spell_rows.iter().find(|(y, _)| *y == m.row) {
                            self.spell_accept(i);
                        }
                        return;
                    }
                    self.spell_menu = None;
                }
                let (cancel, save) = match self.card_modal.as_ref() {
                    Some(c) => (c.cancel_btn.contains(pos), c.save_btn.contains(pos)),
                    None => return,
                };
                if cancel {
                    self.cancel_card_modal().await;
                } else if save {
                    self.save_card_modal(false).await;
                } else if let Some(c) = self.card_modal.as_mut() {
                    c.click(m.column, m.row);
                }
            }
            _ => {}
        }
    }
}
