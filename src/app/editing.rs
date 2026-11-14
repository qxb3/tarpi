//! Editing a message in the input box (click, Ctrl+P / Ctrl+O): start, cancel
//! and commit.

use super::*;

impl App {
    /// Ctrl+P (`user` false) / Ctrl+O (`user` true): edit the very last
    /// reply of the character / the very last message of your own.
    pub(super) fn edit_last(&mut self, user: bool) {
        if self.one_shot_blocks("editing") {
            return;
        }
        let found = self
            .chat
            .messages
            .iter()
            .rposition(|m| !chat::is_system(m) && chat::is_user(m) == user);
        match found {
            Some(idx) => self.begin_edit(idx),
            None => self.set_status(
                if user {
                    "no message of yours to edit yet"
                } else {
                    "no reply from the character to edit yet"
                },
                false,
            ),
        }
    }

    /// Load message `idx` into the input box. The unsent draft is stashed and
    /// comes back when the edit is saved or cancelled.
    pub(super) fn begin_edit(&mut self, idx: usize) {
        if self.one_shot_blocks("editing") {
            return;
        }
        if self.is_generating() {
            self.set_status("stop the generation first (Esc) to edit a message", false);
            return;
        }
        let Some(m) = self.chat.messages.get(idx) else {
            return;
        };
        let text = chat::mes(m).to_string();
        let draft = match self.editing.take() {
            Some(e) => e.draft,
            None => (self.input.text.clone(), self.input.cursor),
        };
        self.input.clear();
        self.input.reset_vim();
        self.input.insert_str(&text);
        // Edits open in INSERT mode (qq leaves it for NORMAL mode) with the
        // cursor at the very end of the text.
        self.input.enter_insert();
        self.input.cursor = self.input.len_chars();
        let slot = self.greeting_slot_of(idx);
        self.editing = Some(Editing { idx, draft, slot });
        self.reveal = Some(idx);
    }

    pub(super) fn restore_draft(&mut self, draft: (String, usize)) {
        self.input.clear();
        self.input.insert_str(&draft.0);
        self.input.cursor = draft.1.min(self.input.len_chars());
        self.input.enter_insert();
        self.input.reset_vim();
    }

    pub(super) fn cancel_edit(&mut self) {
        if let Some(e) = self.editing.take() {
            self.restore_draft(e.draft);
            self.set_status("edit cancelled", false);
        }
    }

    pub(super) async fn commit_edit(&mut self) {
        let Some(e) = self.editing.take() else {
            return;
        };
        let new_text = chat::tidy(&self.input.text);
        self.auto_closed = None;
        self.restore_draft(e.draft);
        let changed = match self.chat.messages.get_mut(e.idx) {
            Some(m) if chat::mes(m) != new_text => {
                chat::set_text(m, &new_text);
                true
            }
            _ => false,
        };
        if changed {
            self.save().await;
            self.set_status("message edited", false);
        } else {
            self.set_status("no changes", false);
        }
    }
}
