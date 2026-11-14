//! The heartbeat and the life of the chat screen: the periodic tick,
//! terminal resizes (the phone keyboard), mouse reporting, leaving, quitting
//! and saving.

use super::*;

impl App {
    pub fn on_tick(&mut self) {
        self.health.tick(self.conn.as_ref());
        if self.stats_at.map_or(true, |t| t.elapsed() >= Duration::from_millis(400)) {
            self.update_stats();
        }
        // No resize came after the tap (the keyboard was already open): give
        // the messages their taps back anyway.
        if let Some(t) = self.mouse_back_at {
            if Instant::now() >= t {
                self.mouse_back_at = None;
                self.set_mouse(true);
            }
        }
        self.collect_import();
        self.promote_lore_ask();
        if let Some((_, _, when)) = &self.status {
            if when.elapsed() >= Duration::from_secs(10) {
                self.status = None;
            }
        }
    }

    /// The terminal changed size. On a phone that is the keyboard opening or
    /// closing: remember which, and as soon as it is open turn taps on
    /// messages back on.
    pub fn on_resize(&mut self, rows: u16) {
        self.max_rows = self.max_rows.max(rows);
        self.kbd_up = self.max_rows > 0 && (rows as u32) * 10 < (self.max_rows as u32) * 8;
        if self.touch && !self.mouse && self.mouse_back_at.take().is_some() {
            self.set_mouse(true);
        }
    }

    /// Turn the terminal's mouse reporting on or off.
    pub(super) fn set_mouse(&mut self, on: bool) {
        self.mouse = on;
        let _ = if on {
            crossterm::execute!(std::io::stdout(), EnableMouseCapture)
        } else {
            crossterm::execute!(std::io::stdout(), DisableMouseCapture)
        };
    }

    /// Close the chat screen and go back to a menu (see `main`).
    pub(super) async fn leave(&mut self, to: Exit) {
        if self.stop_blocked() {
            return;
        }
        self.exit_to = Some(to);
        self.quit().await;
    }

    pub(super) async fn quit(&mut self) {
        if self.stop_blocked() {
            return;
        }
        if self.is_generating() {
            self.cancel_generation().await;
        }
        // A download that is still going has nowhere to land.
        if let Some((job, _)) = self.import_job.take() {
            job.abort();
        }
        self.should_quit = true;
    }

    pub async fn save(&mut self) {
        let lines = self.chat.lines();
        let result = chatstore::save(&self.chat.avatar, &self.chat.file, &lines);
        if let Err(e) = result {
            self.set_status(format!("{:#}", e), true);
        }
    }
}
