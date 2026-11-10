//! Is the API connection alive? Asks the backend in the background, every few
//! seconds, whether it answers (the same probe as `t` in `/api`), so the top
//! bar can show the connection in red while it does not. Never waits for the
//! network: `tick` only starts a probe when one is due, and `down` reads the
//! last answer.

use crate::conn::{self, Connection};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Time between two probes.
const EVERY: Duration = Duration::from_secs(5);

const UNKNOWN: u8 = 0;
const ALIVE: u8 = 1;
const DOWN: u8 = 2;

pub struct Health {
    state: Arc<AtomicU8>,
    busy: Arc<AtomicBool>,
    last: Option<Instant>,
}

impl Default for Health {
    fn default() -> Self {
        Health::new()
    }
}

impl Health {
    pub fn new() -> Health {
        Health {
            state: Arc::new(AtomicU8::new(UNKNOWN)),
            busy: Arc::new(AtomicBool::new(false)),
            last: None,
        }
    }

    /// Forget what is known (the connection changed): the next `tick` probes
    /// at once. A probe still running for the old connection keeps its own
    /// state and cannot change the new one.
    pub fn reset(&mut self) {
        *self = Health::new();
    }

    /// Start a probe of `c` when one is due and none is running.
    pub fn tick(&mut self, c: Option<&Connection>) {
        let Some(c) = c else { return };
        if self.busy.load(Ordering::Relaxed) {
            return;
        }
        if self.last.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        self.last = Some(Instant::now());
        self.busy.store(true, Ordering::Relaxed);
        let (state, busy, c) = (self.state.clone(), self.busy.clone(), c.clone());
        tokio::spawn(async move {
            let ok = conn::probe(&c).await.is_ok();
            state.store(if ok { ALIVE } else { DOWN }, Ordering::Relaxed);
            busy.store(false, Ordering::Relaxed);
        });
    }

    /// The last probe got no good answer. `false` until the first one is back.
    pub fn down(&self) -> bool {
        self.state.load(Ordering::Relaxed) == DOWN
    }
}
