//! The `/system` window and its form.

use super::*;

impl App {
    /// `/system`: choose the system prompt, make a new one (the first row),
    /// edit one (`e`) or delete one (`d`).
    pub(super) async fn open_sysprompt_picker(&mut self) {
        let active = sysprompt::current().map(|p| p.id);
        let mut items = vec![PickItem {
            key: NEW_SYSPROMPT.to_string(),
            label: "+ new system prompt".to_string(),
            detail: String::new(),
            current: false,
            deletable: false,
            editable: false,
        }];
        let list = sysprompt::list();
        let none_yet = list.is_empty();
        items.extend(list.into_iter().map(|p| PickItem {
            label: p.name.clone(),
            detail: persona::brief(&p.content, 80),
            current: active.as_ref() == Some(&p.id),
            deletable: true,
            editable: true,
            key: p.id,
        }));
        let mut picker = Picker::new("system prompt", items);
        if none_yet {
            picker.note = Some((
                "none yet: make one with + new, or run tarpi import-st".to_string(),
                false,
            ));
        }
        self.picker = Some((PickKind::SysPrompts, picker));
    }

    /// Open the system prompt form for `id`, or an empty one for a new prompt.
    pub(super) async fn open_sysprompt_form(&mut self, id: Option<String>) {
        let (name, content, post_history) = match &id {
            None => (String::new(), String::new(), String::new()),
            Some(i) => match sysprompt::list().into_iter().find(|p| p.id == *i) {
                Some(p) => (p.name, p.content, p.post_history),
                None => {
                    self.set_status("that system prompt no longer exists", true);
                    return;
                }
            },
        };
        self.card_modal = Some(CardModal::sysprompt(
            &name,
            &content,
            &post_history,
            &self.input,
        ));
        self.sys_edit = Some(SysEdit { id });
    }

    /// Ctrl+S in the system prompt form: create or update the prompt, then
    /// show the `/system` window again. On a failure the form stays open.
    pub(super) async fn save_sysprompt_form(&mut self, close: bool) {
        let (name, content, post_history) = match self.card_modal.as_mut() {
            Some(m) => {
                m.sync();
                (m.value("name"), m.value("content"), m.value("post_history"))
            }
            None => return,
        };
        if name.is_empty() {
            self.card_note("give the system prompt a name", true);
            return;
        }
        let existing = self.sys_edit.as_ref().and_then(|e| e.id.clone());
        let (saved, msg) = match sysprompt::put(existing.as_deref(), &name, &content, &post_history)
        {
            Ok(p) if existing.is_some() => (p, format!("saved system prompt {}", name)),
            Ok(p) => (p, format!("created system prompt {}", name)),
            Err(e) => {
                self.card_note(format!("could not save the system prompt: {:#}", e), true);
                return;
            }
        };
        // The prompt in use may just have been edited.
        let settings = self.settings.clone();
        self.apply_settings(settings);
        if !close {
            // Stay in the form; a new prompt now exists, so later saves update it.
            self.sys_edit = Some(SysEdit {
                id: Some(saved.id.clone()),
            });
            if let Some(m) = self.card_modal.as_mut() {
                for k in ["name", "content", "post_history"] {
                    m.mark_saved(k);
                }
            }
            self.card_note(msg.clone(), false);
            self.set_status(msg, false);
            return;
        }
        self.card_modal = None;
        self.sys_edit = None;
        self.open_sysprompt_picker().await;
        if let Some((_, p)) = self.picker.as_mut() {
            p.note = Some((msg, false));
        }
    }

    /// Delete a system prompt chosen in the `/system` window; the window stays
    /// open. The one in use can't be deleted.
    pub(super) async fn sysprompt_delete(&mut self, key: &str) {
        let in_use = sysprompt::current().map(|p| p.id == key).unwrap_or(false);
        let note = if in_use {
            (
                "that system prompt is in use \u{2014} switch to another one first".to_string(),
                true,
            )
        } else {
            match sysprompt::remove(key) {
                Ok(()) => {
                    if let Some((_, p)) = self.picker.as_mut() {
                        p.remove(key);
                    }
                    ("deleted the system prompt".to_string(), false)
                }
                Err(e) => (format!("could not delete the system prompt: {:#}", e), true),
            }
        };
        if let Some((_, p)) = self.picker.as_mut() {
            p.note = Some(note);
        }
    }

    /// Make `id` the system prompt in use: remembered in tarpi's
    /// `system-prompt.jsonl` and used by this chat from the next message on.
    pub(super) async fn switch_sysprompt(&mut self, id: &str) {
        if sysprompt::current().map(|p| p.id == id).unwrap_or(false) {
            return;
        }
        let p = match sysprompt::activate(id) {
            Ok(p) => p,
            Err(e) => {
                self.set_status(format!("could not switch the system prompt: {:#}", e), true);
                return;
            }
        };
        let settings = self.settings.clone();
        self.apply_settings(settings);
        self.set_status(format!("system prompt: {}", p.name), false);
    }
}
