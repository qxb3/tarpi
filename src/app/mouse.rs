//! Mouse and touch: clicks, taps on messages, the wheel and the finger-drag
//! gesture that swipes a reply.

use super::*;

impl App {
    pub(super) async fn on_mouse(&mut self, m: MouseEvent) {
        if let Some(a) = self.lore_ask.as_mut() {
            let act = a.on_mouse(m);
            self.lore_ask_action(act);
            return;
        }
        if self.image_modal.is_some() {
            if matches!(m.kind, MouseEventKind::Down(_)) {
                self.image_modal = None;
            }
            return;
        }
        if self.card_modal.is_some() {
            self.on_card_mouse(m).await;
            return;
        }
        if let Some(e) = self.pos_edit.as_mut() {
            let act = e.on_mouse(m);
            self.pos_action(act).await;
            return;
        }
        if let Some((_, p)) = self.picker.as_mut() {
            let act = p.on_mouse(m);
            self.picker_action(act).await;
            return;
        }
        if self.prompt_view.is_some() {
            // The wheel scrolls the prompt; a click closes it.
            match m.kind {
                MouseEventKind::ScrollUp => {
                    if let Some(v) = self.prompt_view.as_mut() {
                        v.scroll = v.scroll.saturating_sub(3);
                    }
                }
                MouseEventKind::ScrollDown => {
                    if let Some(v) = self.prompt_view.as_mut() {
                        v.scroll = v.scroll.saturating_add(3);
                    }
                }
                MouseEventKind::Down(MouseButton::Left) => self.prompt_view = None,
                _ => {}
            }
            return;
        }
        match m.kind {
            // The wheel scrolls whatever is under the pointer: the text box
            // when it is over the box, the chat everywhere else.
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if self.show_help {
                    self.help_scroll = if up {
                        self.help_scroll.saturating_sub(3)
                    } else {
                        self.help_scroll.saturating_add(3)
                    };
                    return;
                }
                if self.input_area.contains(Position::new(m.column, m.row)) {
                    let (total, visible) = self.input_rows;
                    let max = total.saturating_sub(visible);
                    let base = self.input_scroll.unwrap_or(self.input_first_row);
                    let next = if up {
                        base.saturating_sub(3)
                    } else {
                        (base + 3).min(max)
                    };
                    self.input_scroll = Some(next.min(max));
                } else {
                    match self.wheel_gesture(m.column, m.row) {
                        WheelAction::Swipe(dir) => self.swipe(dir).await,
                        WheelAction::Ignore => {}
                        WheelAction::Scroll if up => {
                            self.scroll = self.scroll.saturating_add(3)
                        }
                        WheelAction::Scroll => self.scroll = self.scroll.saturating_sub(3),
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some((x0, y0)) = self.press.take() else {
                    return;
                };
                let dx = m.column as i32 - x0 as i32;
                let dy = m.row as i32 - y0 as i32;
                if dx.abs() >= 6 && dx.abs() > dy.abs() * 2 {
                    // Finger / pointer moved left: next swipe; right: previous.
                    self.swipe(if dx < 0 { 1 } else { -1 }).await;
                } else if dx.abs() <= 1 && dy.abs() <= 1 {
                    self.tap_message(x0, y0).await;
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.press = None;
                if self.show_help {
                    self.show_help = false;
                    self.help_scroll = 0;
                    return;
                }
                let pos = Position::new(m.column, m.row);
                if self.spell_menu.is_some() && self.spell_area.contains(pos) {
                    // A click on a suggestion puts it in.
                    if let Some(&(_, i)) = self.spell_rows.iter().find(|(y, _)| *y == m.row) {
                        self.spell_accept(i);
                    }
                    return;
                }
                if self.comp_area.contains(pos) {
                    // A click on a command of the menu fills it in.
                    if let Some(&(_, name)) = self.comp_rows.iter().find(|(y, _)| *y == m.row) {
                        if self.accept_completion(name) {
                            self.submit().await;
                        }
                    } else if let Some(&(_, i)) =
                        self.path_rows.iter().find(|(y, _)| *y == m.row)
                    {
                        // ... and a click on a path of the `/import` menu.
                        self.accept_path(i);
                    }
                    return;
                }
                if self.card_btn.contains(pos) {
                    self.commit_edit_to_card().await;
                } else if self.send_btn.contains(pos) {
                    // Send / Save, or Stop while a reply is streaming.
                    if self.is_generating() {
                        if !self.stop_blocked() {
                            self.cancel_generation().await;
                        }
                    } else {
                        self.submit().await;
                    }
                } else if self.msg_area.contains(pos) {
                    // Not a click yet: it may become a swipe. Decided on release.
                    self.press = Some((m.column, m.row));
                } else if self.input_area.width > 2 && self.input_area.height > 2 {
                    // Click inside the input box (normal or editing): move the cursor there.
                    let a = self.input_area;
                    let inner = Rect::new(a.x + 1, a.y + 1, a.width - 2, a.height - 2);
                    if inner.contains(pos) {
                        let row = self.input_first_row + (m.row - inner.y) as usize;
                        let col = (m.column - inner.x) as usize;
                        if self.tap_move_cursor {
                            self.input.cursor =
                                ui::index_at(&self.input.text, self.input_w, row, col);
                            self.input.cancel_pending();
                            self.input.clamp_normal();
                        }
                        if self.touch && self.tap_open_keyboard && self.mouse && !self.kbd_up {
                            // With mouse reporting off a tap opens the phone
                            // keyboard. It comes back on by itself when the
                            // keyboard is up (see `on_resize`).
                            self.set_mouse(false);
                            self.mouse_back_at = Some(Instant::now() + Duration::from_secs(3));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Swipes on a phone, the hack. Termux drops the sideways part of a finger
    /// drag but still sends a wheel event per row of vertical movement, and
    /// each one carries the finger's *current column*. A sideways swipe with a
    /// slight slant therefore shows up as wheel events whose column jumps a
    /// long way over a row or two, while a normal scroll stays in one column.
    /// Such a burst is a swipe: moving left is the next swipe, right the
    /// previous one. The burst's first event has already scrolled the chat, so
    /// that is undone, and the rest of the burst (including the fling that
    /// follows) is ignored so that one drag flips exactly one swipe.
    pub(super) fn wheel_gesture(&mut self, col: u16, row: u16) -> WheelAction {
        if !self.touch {
            return WheelAction::Scroll;
        }
        let now = Instant::now();
        let gap = Duration::from_millis(350);
        let continues = self
            .wheel_burst
            .as_ref()
            .is_some_and(|b| now.duration_since(b.last) <= gap);
        if !continues {
            self.wheel_burst = Some(WheelBurst {
                last: now,
                x0: col,
                y0: row,
                scroll0: self.scroll,
                fired: false,
            });
            return WheelAction::Scroll;
        }
        let swipeable = self.swipe_bar_info().is_some();
        let Some(b) = self.wheel_burst.as_mut() else {
            return WheelAction::Scroll;
        };
        b.last = now;
        if b.fired {
            return WheelAction::Ignore;
        }
        if !swipeable {
            return WheelAction::Scroll;
        }
        let dx = col as i32 - b.x0 as i32;
        let dy = (row as i32 - b.y0 as i32).abs();
        if dx.abs() >= 10 && dx.abs() >= 4 * dy.max(1) {
            b.fired = true;
            self.scroll = b.scroll0;
            return WheelAction::Swipe(if dx < 0 { 1 } else { -1 });
        }
        WheelAction::Scroll
    }

    /// A click or tap at (`col`, `row`) in the message area: the ‹n/m› counter
    /// on the last reply flips swipes (left half: previous, right half: next);
    /// anywhere else on a message opens it for editing.
    pub(super) async fn tap_message(&mut self, col: u16, row: u16) {
        let line = self.view_start + row.saturating_sub(self.msg_area.y) as usize;
        let Some(&(idx, start, _)) = self
            .line_map
            .iter()
            .find(|(_, s, e)| line >= *s && line < *e)
        else {
            return;
        };
        if line == start && idx == 0 {
            // The `branch n/m` counter of a summarized chat flips the branch.
            let m = &self.chat.messages[0];
            if let Some((cur, total)) = chat::branch_pos(m) {
                let rel = col.saturating_sub(self.msg_area.x) as usize;
                let (from, to) = ui::branch_zone(&chat::name(m), cur, total);
                if rel >= from && rel < to {
                    let mid = (from + to) / 2;
                    self.switch_branch(if rel < mid { -1 } else { 1 }).await;
                    return;
                }
            }
        }
        if line == start && idx + 1 == self.chat.messages.len() {
            let m = &self.chat.messages[idx];
            if !chat::is_user(m) {
                if let Some((cur, total)) = chat::swipe_pos(m) {
                    if total > 1 {
                        let rel = col.saturating_sub(self.msg_area.x) as usize;
                        let branch = if idx == 0 { chat::branch_pos(m) } else { None };
                        let (from, to) = ui::swipe_zone(&chat::name(m), cur, total, branch);
                        if rel >= from && rel < to {
                            let mid = (from + to) / 2;
                            self.swipe(if rel < mid { -1 } else { 1 }).await;
                            return;
                        }
                    }
                }
            }
        }
        self.begin_edit(idx);
    }
}
