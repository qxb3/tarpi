//! The choose-one windows: `/chars`, `/chats` and the start-up character
//! window.

use super::*;

impl App {
    /// Start-up: the character window over an empty screen (see `startup_pick`).
    pub async fn open_startup_picker(&mut self) {
        self.startup_pick = true;
        self.open_characters_picker().await;
        if let Some((_, p)) = self.picker.as_mut() {
            p.title = "choose a character".to_string();
            // Only the default character: a first start, or no card added since.
            let default_avatar = format!("{}.png", charstore::DEFAULT_STEM);
            if p.items.len() == 1 && p.items[0].key == default_avatar {
                p.footer = vec![
                    format!(
                        "First time here? Press Enter on {} to start chatting.",
                        charstore::DEFAULT_STEM
                    ),
                    "To add your own characters: enter Tarpi, then type /import <card file or chub.ai / pygmalion.chat link>."
                        .to_string(),
                ];
            }
        }
    }

    /// `/characters`: choose another character to chat with.
    pub(super) async fn open_characters_picker(&mut self) {
        let list = charstore::list();
        let mut sorted: Vec<&Value> = list.iter().collect();
        sorted.sort_by_key(|c| crate::char_display_name(c).to_lowercase());
        let items: Vec<PickItem> = sorted
            .into_iter()
            .map(|c| {
                let avatar = jstr(c, "avatar");
                PickItem {
                    label: format!(
                        "{}{}",
                        if crate::is_fav(c) { "\u{2605} " } else { "" },
                        crate::char_display_name(c)
                    ),
                    detail: String::new(),
                    current: avatar == self.chat.avatar && !self.startup_pick,
                    deletable: true,
                    editable: false,
                    key: avatar,
                }
            })
            .collect();
        let mut p = Picker::new("characters", items);
        p.delete_what = " and its chats".to_string();
        self.picker = Some((PickKind::Characters, p));
    }

    /// `/chats`: open, create or delete a chat of this character.
    pub(super) async fn open_chats_picker(&mut self) {
        let mut chats = match chatstore::list(&self.chat.avatar) {
            Ok(c) => c,
            Err(e) => {
                self.set_status(format!("{:#}", e), true);
                return;
            }
        };
        crate::sort_chats_newest_first(&mut chats);
        let narrow = ui::term_cols() < 60;
        let mut items = vec![PickItem {
            key: NEW_CHAT.to_string(),
            label: "+ new chat".to_string(),
            detail: String::new(),
            current: false,
            deletable: false,
            editable: false,
        }];
        for c in &chats {
            let id = chat::file_id(c);
            // The `_before-summary` copies are kept out of sight.
            if chat::is_summary_backup(&id) {
                continue;
            }
            items.push(PickItem {
                detail: format!(
                    "{} {} \u{b7} {}",
                    ji64(c, "chat_items", 0),
                    if narrow { "msgs" } else { "messages" },
                    crate::when(c)
                ),
                current: id == self.chat.file,
                deletable: true,
                editable: false,
                label: id.clone(),
                key: id,
            });
        }
        let title = format!("chats of {}", self.character.name);
        self.picker = Some((PickKind::Chats, Picker::new(title, items)));
    }

    pub(super) async fn picker_action(&mut self, act: PickAction) {
        let Some(kind) = self.picker.as_ref().map(|(k, _)| *k) else {
            return;
        };
        match act {
            PickAction::None => {}
            PickAction::Cancel => {
                self.picker = None;
                // Nothing is open behind the start-up window: closing it quits.
                if self.startup_pick && kind == PickKind::Characters {
                    self.quit().await;
                }
            }
            PickAction::Choose(key) => {
                self.picker = None;
                match kind {
                    PickKind::Characters => {
                        if key != self.chat.avatar || self.startup_pick {
                            self.leave(Exit::Character(key)).await;
                        }
                    }
                    PickKind::Personas => {
                        if key == NEW_PERSONA {
                            self.open_persona_form(None).await;
                        } else {
                            self.switch_persona(&key).await;
                        }
                    }
                    PickKind::SysPrompts => {
                        if key == NEW_SYSPROMPT {
                            self.open_sysprompt_form(None).await;
                        } else {
                            self.switch_sysprompt(&key).await;
                        }
                    }
                    PickKind::Chats => {
                        if key == NEW_CHAT {
                            self.leave(Exit::NewChat).await;
                        } else if key != self.chat.file {
                            self.leave(Exit::Chat(key)).await;
                        }
                    }
                }
            }
            PickAction::Delete(key) => match kind {
                PickKind::Personas => self.persona_delete(&key).await,
                PickKind::SysPrompts => self.sysprompt_delete(&key).await,
                PickKind::Characters => self.character_delete(&key).await,
                _ => self.picker_delete(&key).await,
            },
            PickAction::Position(key) => {
                if kind == PickKind::Personas {
                    self.picker = None;
                    self.open_persona_pos(key).await;
                }
            }
            PickAction::Edit(key) => {
                if kind == PickKind::Personas {
                    self.picker = None;
                    self.open_persona_form(Some(key)).await;
                } else if kind == PickKind::SysPrompts {
                    self.picker = None;
                    self.open_sysprompt_form(Some(key)).await;
                }
            }
        }
    }

    /// Delete a character chosen in the `/chars` window (the question was
    /// asked by the window): its card and every chat it has. The window stays
    /// open, except when it is the character of this chat: then tarpi quits.
    /// Under `tarpi connect` the files go from the local copy and the sync
    /// passes the delete on (to the server and so to the other devices).
    pub(super) async fn character_delete(&mut self, avatar: &str) {
        let current = avatar == self.chat.avatar && !self.startup_pick;
        // A reply that is being written in a one-shot chat can't be stopped,
        // and the chat can't be closed under it.
        if current && self.is_generating() && self.chat.is_one_shot() {
            if let Some((_, p)) = self.picker.as_mut() {
                p.note = Some((
                    "one-shot chat: wait for the reply to finish first".to_string(),
                    true,
                ));
            }
            return;
        }
        let name = charstore::get(avatar)
            .map(|v| crate::char_display_name(&v))
            .unwrap_or_else(|_| avatar.to_string());
        let chats = chatstore::list(avatar)
            .map(|l| {
                l.iter()
                    .filter(|c| !chat::is_summary_backup(&chat::file_id(c)))
                    .count()
            })
            .unwrap_or(0);
        if current {
            // Stop first: finishing a generation would save the chat again.
            self.quit().await;
        }
        let outcome = match charstore::delete(avatar) {
            Err(e) => Err(format!("could not delete {}: {:#}", name, e)),
            Ok(()) => match chatstore::delete_all(avatar) {
                Ok(()) => Ok(()),
                Err(e) => Err(format!(
                    "deleted the card of {} but not all its chats: {:#}",
                    name, e
                )),
            },
        };
        let what = match chats {
            0 => String::new(),
            1 => " and its 1 chat".to_string(),
            n => format!(" and its {} chats", n),
        };
        let card_gone = charstore::get(avatar).is_err();
        if card_gone {
            crate::connect::report(format!("deleted character {}{}", name, what));
        }
        match (outcome, current) {
            (Ok(()), true) => {
                self.picker = None;
                self.exit_note = Some(format!("Deleted \"{}\"{}.", name, what));
            }
            (Ok(()), false) => {
                if let Some((_, p)) = self.picker.as_mut() {
                    p.remove(avatar);
                    p.note = Some((format!("deleted {}{}", name, what), false));
                }
            }
            (Err(msg), _) => {
                // Not gone: stay in the chat (the card may be half deleted).
                if current && !card_gone {
                    self.should_quit = false;
                }
                if current && card_gone {
                    self.picker = None;
                    self.exit_note = Some(msg.clone());
                } else if let Some((_, p)) = self.picker.as_mut() {
                    if card_gone {
                        p.remove(avatar);
                    }
                    p.note = Some((msg, true));
                }
            }
        }
    }

    /// Delete a chat file chosen in the `/chats` window; the window stays open.
    pub(super) async fn picker_delete(&mut self, key: &str) {
        // The open chat is the only one left: delete it (and its hidden
        // `_before-summary` copies) and go back to the `choose a character` window.
        let only_one = self
            .picker
            .as_ref()
            .map(|(_, p)| p.items.iter().filter(|i| i.deletable).count() == 1)
            .unwrap_or(false);
        if key == self.chat.file && only_one {
            // Stop first: finishing a generation would save the chat again.
            self.picker = None;
            self.exit_to = Some(Exit::Pick);
            self.quit().await;
            let avatar = self.chat.avatar.clone();
            let mut names = vec![key.to_string()];
            if let Ok(chats) = chatstore::list(&avatar) {
                let ids: Vec<String> = chats.iter().map(chat::file_id).collect();
                names.extend(chat::backups_of(key, &ids));
            }
            for name in names {
                let _ = chatstore::delete(&avatar, &name);
            }
            return;
        }
        let note = if key == self.chat.file {
            (
                "this chat is open \u{2014} use /newchat to replace it".to_string(),
                true,
            )
        } else {
            match chatstore::delete(&self.chat.avatar, key) {
                Ok(()) => {
                    if let Some((_, p)) = self.picker.as_mut() {
                        p.remove(key);
                    }
                    (format!("deleted {}", key), false)
                }
                Err(e) => (format!("could not delete {}: {:#}", key, e), true),
            }
        };
        if let Some((_, p)) = self.picker.as_mut() {
            p.note = Some(note);
        }
    }

    /// Keys while the cursor is on a row of choices in the `/note` window:
    /// left / right (or `h` / `l`) change it, digits type a number, up / down
    /// (or `j` / `k`, Tab) move between rows, Esc cancels like in a text field.
    pub(super) async fn on_pick_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let big = if k.modifiers.contains(KeyModifiers::SHIFT) { 10 } else { 1 };
        let mut cancel = false;
        {
            let Some(c) = self.card_modal.as_mut() else {
                return;
            };
            c.note.clear();
            c.note_err = false;
            let was_armed = std::mem::replace(&mut c.discard_armed, false);
            c.follow = true;
            match k.code {
                KeyCode::Esc => {
                    if c.dirty() && !was_armed {
                        c.discard_armed = true;
                        c.note = "unsaved changes: Esc again discards, Ctrl+S saves".to_string();
                        c.note_err = true;
                    } else {
                        cancel = true;
                    }
                }
                KeyCode::Left => c.step_pick(-big),
                KeyCode::Right => c.step_pick(big),
                KeyCode::Char('h') if !ctrl && !alt => c.step_pick(-1),
                KeyCode::Char('l') if !ctrl && !alt => c.step_pick(1),
                KeyCode::Char('H') if !ctrl && !alt => c.step_pick(-10),
                KeyCode::Char('L') if !ctrl && !alt => c.step_pick(10),
                KeyCode::PageUp => c.step_pick(-10),
                KeyCode::PageDown => c.step_pick(10),
                KeyCode::Up | KeyCode::BackTab => c.move_row(-1),
                KeyCode::Down | KeyCode::Tab | KeyCode::Enter => c.move_row(1),
                KeyCode::Char('k') if !ctrl && !alt => c.move_row(-1),
                KeyCode::Char('j') if !ctrl && !alt => c.move_row(1),
                KeyCode::Char(ch) if ch.is_ascii_digit() && !ctrl && !alt => c.pick_digit(ch),
                KeyCode::Backspace => c.pick_backspace(),
                _ => {}
            }
            c.sync();
        }
        if cancel {
            self.cancel_card_modal().await;
        }
    }
}
