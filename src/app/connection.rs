//! The API connection the replies come from: setting it up, reloading it
//! after the `/api`, `/formatting` and `/lore` windows close, and the settings
//! that go with it.

use super::*;

/// `host:port` of a URL, without the scheme or any path.
pub fn host_of(url: &str) -> String {
    let u = url.trim();
    let u = u.split_once("://").map(|(_, rest)| rest).unwrap_or(u);
    u.split('/').next().unwrap_or("").to_string()
}

/// `(source, model, api type for the title bar)` for the active API.
pub(super) fn backend_info(settings: &Value) -> (String, String, String) {
    match jstr(settings, "main_api").as_str() {
        "textgenerationwebui" => {
            let ty = textgen::api_type(settings);
            let label = ty.clone();
            (ty, String::new(), label)
        }
        other => (
            other.to_string(),
            String::new(),
            format!("{} (unsupported)", other),
        ),
    }
}

impl App {
    /// At start-up: say how to get going when there is no API connection yet.
    pub fn hint_if_no_connection(&mut self) {
        if self.conn.is_none() {
            self.set_status(NO_CONNECTION, false);
        }
    }

    /// The API manager (`/api`) was closed: use whatever it saved right away
    /// (title bar, token counts), not only from the next reply on.
    pub fn api_closed(&mut self, res: anyhow::Result<()>, rows: u16) {
        self.on_resize(rows);
        self.reload_connection();
        match res {
            Err(e) => self.set_status(format!("API settings: {:#}", e), true),
            Ok(()) => match &self.conn {
                Some(c) => {
                    let msg = format!("back in the chat, using API connection '{}'", c.name);
                    self.set_status(msg, false);
                }
                None => self.set_status(NO_CONNECTION, true),
            },
        }
    }

    /// The formatting editor (`/formatting`) was closed: use whatever it saved
    /// right away (prompt token counts), not only from the next reply on.
    pub fn formatting_closed(&mut self, res: anyhow::Result<()>, rows: u16) {
        self.on_resize(rows);
        formatting::normalize(&mut self.settings);
        self.settings_ver += 1;
        match res {
            Err(e) => self.set_status(format!("formatting: {:#}", e), true),
            Ok(()) => self.set_status("back in the chat, using your formatting".to_string(), false),
        }
    }

    /// The lorebook manager (`/lore`) was closed: the books it saved are used
    /// from now on (prompt token counts too), not only from the next reply.
    pub fn lore_closed(&mut self, res: anyhow::Result<()>, rows: u16) {
        self.on_resize(rows);
        self.settings_ver += 1;
        match res {
            Err(e) => self.set_status(format!("lorebooks: {:#}", e), true),
            Ok(()) => self.set_status("back in the chat, using your lorebooks".to_string(), false),
        }
    }

    /// Use `c` from now on (with `None` there is no way to generate).
    /// The title bar then shows the connection's backend and address.
    pub fn set_connection(&mut self, c: Option<conn::Connection>) {
        self.conn = c;
        self.health.reset();
        if let Some(c) = &self.conn {
            conn::overlay(&mut self.settings, c);
            self.server = host_of(&c.url);
        }
        let (source, model, label) = self.backend_labels(&self.settings);
        self.source = source;
        self.model = model;
        self.label = label;
        self.settings_ver += 1;
    }

    /// `(source, model, title-bar label)`: the standalone connection's, or a
    /// reminder that there is none.
    pub(super) fn backend_labels(&self, settings: &Value) -> (String, String, String) {
        if self.conn.is_some() {
            backend_info(settings)
        } else {
            (String::new(), String::new(), "no API connection".to_string())
        }
    }

    /// Re-read `api.yaml` so changes made in `/api` (even in another
    /// terminal) apply to the next reply. An unreadable file keeps the
    /// connection as it was.
    pub(super) fn reload_connection(&mut self) {
        let Ok(store) = conn::Store::load() else {
            return;
        };
        let fresh = store.active_connection();
        let changed = match (&self.conn, &fresh) {
            (None, None) => false,
            (Some(a), Some(b)) => {
                a.name != b.name
                    || a.kind != b.kind
                    || a.url != b.url
                    || a.api_key != b.api_key
                    || a.params != b.params
            }
            _ => true,
        };
        if changed {
            self.set_connection(fresh);
        }
    }

    /// Rebuild the settings the next reply is made from: the base settings
    /// with the current persona, system prompt, formatting and API connection
    /// written over them, so changes made in the windows (or by hand in the
    /// files, even from another terminal) apply to the next reply.
    pub(super) async fn refresh_settings(&mut self) {
        self.reload_connection();
        self.apply_settings(config::base_settings());
    }

    pub(super) fn apply_settings(&mut self, mut settings: Value) {
        persona::normalize(&mut settings);
        sysprompt::normalize(&mut settings);
        formatting::normalize(&mut settings);
        if let Some(c) = &self.conn {
            conn::overlay(&mut settings, c);
        }
        let p = persona::current();
        self.user_name = p.name;
        self.persona_title = p.title;
        if let Some(pu) = settings.get("power_user") {
            self.persona = jstr(pu, "persona_description");
            self.persona_position = ji64(pu, "persona_description_position", 0);
        }
        let (source, model, label) = self.backend_labels(&settings);
        self.source = source;
        self.model = model;
        self.label = label;
        self.settings = settings;
        self.settings_ver += 1;
    }
}
