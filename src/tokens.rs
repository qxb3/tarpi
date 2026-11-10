//! Token count of the whole prompt for the status row.
//!
//! The exact count is asked from the API connection's own tokenizer
//! (`conn::count_tokens`) in the background. The text being counted is the
//! complete prompt: system prompt, character card, persona, examples,
//! instruct formatting and every message. Until the answer arrives (or when
//! nothing can count, e.g. there is no connection), the characters /
//! `chars_per_token` estimate stands in.

use std::collections::{hash_map::DefaultHasher, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// After a failed request, wait this long before asking the server again.
const RETRY_AFTER: Duration = Duration::from_secs(20);
/// Remember at most this many counted prompts.
const MAX_CACHED: usize = 8;

/// Which backend should tokenize.
#[derive(Clone)]
pub struct Backend {
    pub api_type: String,
    pub server: String,
    /// Model name of the selected Text Completion type (part of the cache key).
    pub model: String,
    /// The standalone connection (`/api`): its own tokenizer is asked
    /// directly.
    pub conn: Option<crate::conn::Connection>,
}

impl Backend {
    fn key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.api_type.hash(&mut h);
        self.server.hash(&mut h);
        self.model.hash(&mut h);
        if let Some(c) = &self.conn {
            c.name.hash(&mut h);
            c.api_key.hash(&mut h);
        }
        h.finish()
    }
}

/// Count of a text.
pub struct Totals {
    pub tokens: usize,
    /// `false` while it is still an estimate.
    pub exact: bool,
}

#[derive(Default)]
struct State {
    /// (backend, text) hash -> token count.
    counts: HashMap<u64, usize>,
    /// The newest text waiting to be counted (older ones are not worth it).
    pending: Option<(u64, String, Backend)>,
    /// A worker task is running.
    working: bool,
    /// Backend that failed last, and when.
    failed: Option<(u64, Instant)>,
    /// Why the last request failed, until `take_error` hands it out.
    last_error: Option<String>,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn text_key(backend: u64, text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    backend.hash(&mut h);
    text.hash(&mut h);
    h.finish()
}

#[derive(Clone, Default)]
pub struct Counter {
    state: Arc<Mutex<State>>,
}

impl Counter {
    /// Token count of `text`: exact when the backend has already counted it,
    /// otherwise an estimate (and the text is queued to be counted when
    /// there is a `backend` to ask). Never waits for the network.
    pub fn total(&self, backend: Option<&Backend>, text: &str, cpt: f32) -> Totals {
        let cpt = if cpt < 1.0 { 3.2 } else { cpt };
        let estimate = (text.chars().count() as f32 / cpt).ceil() as usize;
        if text.is_empty() {
            return Totals {
                tokens: 0,
                exact: true,
            };
        }
        let Some(b) = backend else {
            return Totals {
                tokens: estimate,
                exact: false,
            };
        };
        let key = b.key();
        let h = text_key(key, text);
        let mut st = lock(&self.state);
        if let Some(&n) = st.counts.get(&h) {
            return Totals {
                tokens: n,
                exact: true,
            };
        }
        let failed = matches!(st.failed, Some((k, at)) if k == key && at.elapsed() < RETRY_AFTER);
        if failed {
            return Totals {
                tokens: estimate,
                exact: false,
            };
        }
        // Only the newest text matters: it replaces one still waiting.
        if st.pending.as_ref().map(|p| p.0) != Some(h) {
            st.pending = Some((h, text.to_string(), b.clone()));
        }
        let start = !st.working;
        if start {
            st.working = true;
        }
        drop(st);
        if start {
            self.spawn_worker();
        }
        Totals {
            tokens: estimate,
            exact: false,
        }
    }

    /// The reason for the latest failure, once (so it can be shown once).
    pub fn take_error(&self) -> Option<String> {
        lock(&self.state).last_error.take()
    }

    /// One task counts the waiting text, then any newer one, then stops.
    fn spawn_worker(&self) {
        let state = self.state.clone();
        tokio::spawn(async move {
            loop {
                let job = {
                    let mut st = lock(&state);
                    let job = st.pending.take();
                    if job.is_none() {
                        st.working = false;
                    }
                    job
                };
                let Some((h, text, b)) = job else { break };
                let res = match &b.conn {
                    Some(c) => crate::conn::count_tokens(c, &text).await,
                    None => Err(anyhow::anyhow!("no API connection")),
                };
                let mut st = lock(&state);
                match res {
                    Ok(n) => {
                        if st.counts.len() >= MAX_CACHED {
                            st.counts.clear();
                        }
                        st.counts.insert(h, n);
                        st.failed = None;
                    }
                    Err(e) => {
                        // Server down, or no tokenizer for this backend: give
                        // up for a while and keep showing the estimate.
                        st.last_error = Some(format!("{:#}", e));
                        st.pending = None;
                        st.failed = Some((b.key(), Instant::now()));
                        st.working = false;
                        break;
                    }
                }
            }
        });
    }
}
