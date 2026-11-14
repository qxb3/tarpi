//! Turning the chat into the lines the UI draws.

use super::*;

impl App {
    pub fn build_lines(&mut self, width: usize) -> Vec<Line<'static>> {
        let mut out: Vec<Line<'static>> = Vec::new();
        let mut map: Vec<(usize, usize, usize)> = Vec::new();
        let hide_last = self
            .pending
            .as_ref()
            .map(|p| p.replace_last)
            .unwrap_or(false);
        // While continuing, the last message is shown with the new text appended.
        let cont: Option<(String, String)> = self
            .pending
            .as_ref()
            .and_then(|p| p.continue_from.clone().map(|o| (o, p.text.clone())));
        let editing_idx = self.editing.as_ref().map(|e| e.idx);
        let count = self.chat.messages.len();
        for (idx, m) in self.chat.messages.iter().enumerate() {
            if hide_last && idx + 1 == count {
                break;
            }
            if chat::is_system(m) {
                continue;
            }
            let is_user = chat::is_user(m);
            let swipe = if is_user { None } else { chat::swipe_pos(m) };
            let continuing = idx + 1 == count && cont.is_some();
            // {{user}} / {{char}} / <USER> are shown as the names (the stored
            // text is left alone, so editing still shows what was written).
            let text = match &cont {
                Some((orig, chunk)) if continuing => prompt::subst_display(
                    &prose::close_for_display(&chat::join_continuation(orig, chunk)),
                    &self.user_name,
                    &self.character.name,
                ),
                _ => prompt::subst_display(chat::mes(m), &self.user_name, &self.character.name),
            };
            let start = out.len();
            let branch = if idx == 0 { chat::branch_pos(m) } else { None };
            out.extend(ui::message_lines(
                &chat::name(m),
                is_user,
                &text,
                width,
                swipe,
                branch,
                continuing,
                editing_idx == Some(idx),
                self.msg_sep,
            ));
            map.push((idx, start, out.len()));
        }
        let mut last_in_map = true;
        // A summary is not shown while it is written: it appears whole, as the
        // summary branch, when it is finished (the status row says it is working).
        if let Some(p) = self.pending.as_ref().filter(|p| !p.summary) {
            if p.continue_from.is_none() {
                last_in_map = false;
                let shown = prose::close_for_display(&p.text);
                let text = if p.text.is_empty() {
                    "\u{2026}"
                } else {
                    shown.as_str()
                };
                out.extend(ui::message_lines(
                    self.character.name.as_str(),
                    false,
                    text,
                    width,
                    None,
                    None,
                    true,
                    false,
                    self.msg_sep,
                ));
            }
        }
        // No separator line under the very last message.
        if out.pop().is_some() && last_in_map {
            if let Some(l) = map.last_mut() {
                l.2 = l.2.saturating_sub(1);
            }
        }
        self.line_map = map;
        out
    }

    /// `(current, total, next_generates)` of the last reply, or `None` when it
    /// cannot be swiped right now.
    pub fn swipe_bar_info(&self) -> Option<(usize, usize, bool)> {
        if self.is_generating() || self.is_editing() || self.chat.is_one_shot() {
            return None;
        }
        let m = self.chat.messages.last()?;
        if chat::is_user(m) || chat::is_system(m) {
            return None;
        }
        let (cur, total) = chat::swipe_pos(m).unwrap_or((0, 1));
        let len = self.chat.messages.len();
        if total > 1 || len >= 2 {
            Some((cur, total, cur + 1 >= total && len >= 2))
        } else {
            None
        }
    }
}
