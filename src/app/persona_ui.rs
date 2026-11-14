//! The `/persona` window, its position window and its form.

use super::*;

impl App {
    /// `/persona`: choose who you are in this chat, make a new persona (the
    /// first row), edit one (`e`) or delete one (`d`).
    pub(super) async fn open_persona_picker(&mut self) {
        let active = persona::current().id;
        let mut items = vec![PickItem {
            key: NEW_PERSONA.to_string(),
            label: "+ new persona".to_string(),
            detail: String::new(),
            current: false,
            deletable: false,
            editable: false,
        }];
        items.extend(persona::list().into_iter().map(|p| PickItem {
            label: p.label(),
            detail: persona::brief(&p.description, 80),
            current: p.id == active,
            deletable: true,
            editable: true,
            key: p.id,
        }));
        let mut picker = Picker::new("persona", items);
        picker.position_key = true;
        self.picker = Some((PickKind::Personas, picker));
    }

    /// The `/persona` window again, with the cursor on persona `id`.
    pub(super) async fn open_persona_picker_at(&mut self, id: &str) {
        self.open_persona_picker().await;
        if let Some((_, p)) = self.picker.as_mut() {
            if let Some(i) = p.items.iter().position(|it| it.key == id) {
                p.sel = i;
            }
        }
    }

    /// `p` in the `/persona` window: the window for where the description of
    /// persona `id` goes into the prompt.
    pub(super) async fn open_persona_pos(&mut self, id: String) {
        match persona::list().into_iter().find(|p| p.id == id) {
            Some(p) => {
                self.pos_edit = Some(PosEdit::new(p.id.clone(), p.label(), p.position, p.depth, p.role));
            }
            None => {
                self.set_status("that persona no longer exists", true);
                self.open_persona_picker().await;
            }
        }
    }

    /// Enter / Esc in the position window: save to the persona's file (and use
    /// it from the next reply on), or leave it as it was. Either way the
    /// `/persona` window comes back. A failed save keeps the window open.
    pub(super) async fn pos_action(&mut self, act: PosAction) {
        match act {
            PosAction::None => {}
            PosAction::Cancel => {
                if let Some(e) = self.pos_edit.take() {
                    self.open_persona_picker_at(&e.id).await;
                }
            }
            PosAction::Save => {
                let Some(e) = self.pos_edit.as_ref() else {
                    return;
                };
                let (id, position, depth, role) = (e.id.clone(), e.position, e.depth, e.role);
                match persona::set_injection(&id, position, depth, role) {
                    Ok(p) => {
                        // The active persona may just have been changed.
                        let settings = self.settings.clone();
                        self.apply_settings(settings);
                        self.pos_edit = None;
                        self.open_persona_picker_at(&id).await;
                        let msg = format!(
                            "{}: {}",
                            p.name,
                            crate::posedit::position_note(p.position, p.depth, p.role)
                        );
                        if let Some((_, pk)) = self.picker.as_mut() {
                            pk.note = Some((msg, false));
                        }
                    }
                    Err(err) => {
                        if let Some(e) = self.pos_edit.as_mut() {
                            e.note = Some(format!("could not save: {:#}", err));
                        }
                    }
                }
            }
        }
    }

    /// Open the persona form for `id`, or an empty one for a new persona.
    pub(super) async fn open_persona_form(&mut self, id: Option<String>) {
        let (name, title, description) = match &id {
            None => (String::new(), String::new(), String::new()),
            Some(a) => match persona::list().into_iter().find(|p| p.id == *a) {
                Some(p) => (p.name, p.title, p.description),
                None => {
                    self.set_status("that persona no longer exists", true);
                    return;
                }
            },
        };
        self.card_modal = Some(CardModal::persona(&name, &title, &description, &self.input));
        self.persona_edit = Some(PersonaEdit { avatar: id });
    }

    /// Ctrl+S in the persona form: create or update the persona, then show the
    /// persona window again. On a failure the form stays open.
    pub(super) async fn save_persona_form(&mut self, close: bool) {
        let (name, title, description) = match self.card_modal.as_mut() {
            Some(m) => {
                m.sync();
                (m.value("name"), m.value("title"), m.value("description"))
            }
            None => return,
        };
        if name.is_empty() {
            self.card_note("give the persona a name", true);
            return;
        }
        let existing = self.persona_edit.as_ref().and_then(|e| e.avatar.clone());
        let (saved, msg) = match persona::put(existing.as_deref(), &name, &title, &description) {
            Ok(p) if existing.is_some() => (p, format!("saved persona {}", name)),
            Ok(p) => (p, format!("created persona {}", name)),
            Err(e) => {
                self.card_note(format!("could not save the persona: {:#}", e), true);
                return;
            }
        };
        // The active persona may just have been edited.
        let settings = self.settings.clone();
        self.apply_settings(settings);
        if !close {
            // Stay in the form. A new persona now exists: further saves
            // update it instead of making another one.
            self.persona_edit = Some(PersonaEdit {
                avatar: Some(saved.id.clone()),
            });
            if let Some(m) = self.card_modal.as_mut() {
                for k in ["name", "title", "description"] {
                    m.mark_saved(k);
                }
            }
            self.card_note(msg.clone(), false);
            self.set_status(msg, false);
            return;
        }
        self.card_modal = None;
        self.persona_edit = None;
        self.open_persona_picker().await;
        if let Some((_, p)) = self.picker.as_mut() {
            p.note = Some((msg, false));
        }
    }

    /// Delete a persona chosen in the `/persona` window; the window stays open.
    pub(super) async fn persona_delete(&mut self, key: &str) {
        let note = if persona::current().id == key {
            (
                "that persona is in use \u{2014} switch to another one first".to_string(),
                true,
            )
        } else {
            match persona::remove(key) {
                Ok(()) => {
                    if let Some((_, p)) = self.picker.as_mut() {
                        p.remove(key);
                    }
                    ("deleted the persona".to_string(), false)
                }
                Err(e) => (format!("could not delete the persona: {:#}", e), true),
            }
        };
        if let Some((_, p)) = self.picker.as_mut() {
            p.note = Some(note);
        }
    }

    /// Make `id` the active persona: remembered in tarpi's persona folder and
    /// used by this chat from the next message on.
    pub(super) async fn switch_persona(&mut self, id: &str) {
        if persona::current().id == id {
            return;
        }
        let p = match persona::activate(id) {
            Ok(p) => p,
            Err(e) => {
                self.set_status(format!("could not switch the persona: {:#}", e), true);
                return;
            }
        };
        let settings = self.settings.clone();
        self.apply_settings(settings);
        self.set_status(format!("now chatting as {}", p.label()), false);
    }
}
