//! Messages a running generation sends back to the UI.

pub enum GenEvent {
    Chunk(String),
    /// Token counts the backend reported for this request.
    Usage(crate::conn::Usage),
    Done,
    Error(String),
}

/// `(generation id, event)` so stale events from an aborted request can be ignored.
pub type GenMsg = (u64, GenEvent);
