//! The wire format between `tarpi serve` and `tarpi connect`: one persistent
//! TCP connection per device that carries everything (file sync, token counts,
//! the log lines, the "a file changed" notes and the liveness beat).
//!
//! **Greeting.** Both sides send [`MAGIC`] first. A peer that answers with
//! anything else (a browser, an older tarpi that still spoke HTTP) is refused,
//! and says so in words.
//!
//! **Frames.** After the greeting the stream is a sequence of frames:
//!
//! ```text
//! kind: u8 | length: u32 (big endian) | payload
//! ```
//!
//! - `0` BEAT: no payload. Each side sends one every [`BEAT`] when it has
//!   nothing else to say; *any* frame counts as a sign of life, so a long
//!   upload does not look like a dead link.
//! - `1` MORE / `2` LAST: a piece of a message (at most [`CHUNK`] bytes). A
//!   message is cut into pieces so a beat can go out between two of them.
//!
//! **Messages.** The pieces of one message, put together:
//!
//! ```text
//! id: u32 | head length: u32 | head (JSON) | body (raw bytes)
//! ```
//!
//! The client numbers its requests from 1 and the server answers with the same
//! `id` (so several requests can be in flight at once). `id` 0 is a note that
//! needs no answer: a log line from the device, or the server telling the
//! device that a shared file changed. The head holds the small things
//! (`"op"`, `"path"`, ...), the body the bytes of a file or the file list.
//!
//! Replies carry `"ok": true`, or `"ok": false` with a HTTP-like `"code"` (401
//! no or wrong token, 404, 409 the file changed in the meantime, ...) and an
//! `"error"` text.
//!
//! The link is plain TCP: not encrypted, so use it on a network you trust (or
//! through an SSH tunnel).

use anyhow::{bail, Context, Result};
use futures_util::FutureExt;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch, Notify};
use tokio::time::{Interval, MissedTickBehavior};

/// What both sides say first.
pub const MAGIC: &[u8] = b"TARPI-SYNC 2\r\n\r\n";
/// How often an otherwise quiet side sends a beat.
pub const BEAT: Duration = Duration::from_millis(50);
/// How long a link may be silent while a request is waiting for its answer
/// or a message is half received. A big file over wifi can stall for longer
/// than the quiet-link limit (a lost packet, a busy access point) without the
/// link being dead; a closed or reset connection is still noticed at once.
const BUSY_SILENCE: Duration = Duration::from_secs(5);
/// Largest piece of a message in one frame.
const CHUNK: usize = 16 * 1024;
/// Largest file that is shared.
pub const MAX_FILE: usize = 256 * 1024 * 1024;
/// Largest head of a message.
const MAX_HEAD: usize = 1024 * 1024;
/// Largest message (a file plus its head).
const MAX_MESSAGE: usize = MAX_FILE + MAX_HEAD + 8;

const K_BEAT: u8 = 0;
const K_MORE: u8 = 1;
const K_LAST: u8 = 2;

/// One request, reply or note (see the module text).
#[derive(Debug, Clone)]
pub struct Message {
    pub id: u32,
    pub head: Value,
    pub body: Vec<u8>,
}

impl Message {
    pub fn new(id: u32, head: Value, body: Vec<u8>) -> Message {
        Message { id, head, body }
    }

    /// A successful reply; the fields of `extra` (a JSON object) are added to the head.
    pub fn ok(id: u32, extra: Value) -> Message {
        let mut head = json!({ "ok": true });
        if let (Some(h), Value::Object(e)) = (head.as_object_mut(), extra) {
            h.extend(e);
        }
        Message { id, head, body: Vec::new() }
    }

    /// A refused request.
    pub fn fail(id: u32, code: u16, msg: impl Into<String>) -> Message {
        Message {
            id,
            head: json!({ "ok": false, "code": code, "error": msg.into() }),
            body: Vec::new(),
        }
    }

    pub fn with_body(mut self, body: Vec<u8>) -> Message {
        self.body = body;
        self
    }

    pub fn is_ok(&self) -> bool {
        self.head.get("ok").and_then(|v| v.as_bool()).unwrap_or(false)
    }

    pub fn code(&self) -> u16 {
        self.head.get("code").and_then(|v| v.as_u64()).unwrap_or(0) as u16
    }

    pub fn error(&self) -> &str {
        self.str("error").unwrap_or("")
    }

    /// The `"op"` of a request.
    pub fn op(&self) -> &str {
        self.str("op").unwrap_or("")
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        self.head.get(key).and_then(|v| v.as_str())
    }
}

fn encode(m: &Message) -> Vec<u8> {
    let head = serde_json::to_vec(&m.head).unwrap_or_default();
    let mut v = Vec::with_capacity(8 + head.len() + m.body.len());
    v.extend_from_slice(&m.id.to_be_bytes());
    v.extend_from_slice(&(head.len() as u32).to_be_bytes());
    v.extend_from_slice(&head);
    v.extend_from_slice(&m.body);
    v
}

fn decode(mut buf: Vec<u8>) -> Option<Message> {
    if buf.len() < 8 {
        return None;
    }
    let id = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let hl = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
    if hl > MAX_HEAD || buf.len() < 8 + hl {
        return None;
    }
    let body = buf.split_off(8 + hl);
    let head = serde_json::from_slice(&buf[8..]).ok()?;
    Some(Message { id, head, body })
}

// ---------------------------------------------------------------- greeting

/// The device's side of the greeting: say it, and check the server says it back.
pub async fn client_handshake(sock: &mut TcpStream) -> Result<()> {
    let _ = sock.set_nodelay(true);
    sock.write_all(MAGIC).await.context("sending the greeting")?;
    let mut got = vec![0u8; MAGIC.len()];
    match tokio::time::timeout(Duration::from_secs(10), sock.read_exact(&mut got)).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => bail!("the server hung up on the greeting (is it tarpi serve?): {}", e),
        Err(_) => bail!(
            "the server did not answer the greeting (is it tarpi serve? an older one that speaks HTTP?)"
        ),
    }
    if got.starts_with(b"HTTP/") {
        bail!("this server speaks the old HTTP protocol of tarpi: update `tarpi serve` to the same version");
    }
    if got != MAGIC {
        bail!("that is not a tarpi serve of this version");
    }
    Ok(())
}

/// The server's side: read the greeting and answer it. `false` when the peer
/// is not a `tarpi connect` (it is told so and should be dropped).
pub async fn server_handshake(sock: &mut TcpStream) -> bool {
    let _ = sock.set_nodelay(true);
    let mut got = vec![0u8; MAGIC.len()];
    let read = tokio::time::timeout(Duration::from_secs(10), sock.read_exact(&mut got)).await;
    if !matches!(read, Ok(Ok(_))) || got != MAGIC {
        let _ = sock
            .write_all(b"tarpi serve speaks the tarpi sync protocol, not HTTP: use `tarpi connect`\r\n")
            .await;
        return false;
    }
    sock.write_all(MAGIC).await.is_ok()
}

// -------------------------------------------------------------------- link

enum Out {
    Msg(Message),
    Close,
}

/// Where messages are sent to the other side. Cheap to clone.
#[derive(Clone)]
pub struct Sender {
    tx: mpsc::UnboundedSender<Out>,
    done: watch::Receiver<bool>,
    busy: Arc<AtomicUsize>,
}

/// Held while a request waits for its answer: the link is then given the
/// longer [`BUSY_SILENCE`] before it counts as dead.
pub struct Busy(Arc<AtomicUsize>);

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Sender {
    /// Queue a message. `false` when the link is gone.
    pub fn send(&self, m: Message) -> bool {
        self.tx.send(Out::Msg(m)).is_ok()
    }

    /// Mark the link busy until the returned guard is dropped.
    pub fn busy(&self) -> Busy {
        self.busy.fetch_add(1, Ordering::Relaxed);
        Busy(self.busy.clone())
    }

    /// Send what is queued, then close the connection. Waits at most `wait`.
    pub async fn close(&self, wait: Duration) {
        let _ = self.tx.send(Out::Close);
        let mut done = self.done.clone();
        let _ = tokio::time::timeout(wait, done.wait_for(|d| *d)).await;
    }
}

/// Start a connection whose greeting is done: messages sent through the
/// [`Sender`] go out, and what the other side sends arrives in the receiver
/// (which returns `None` once the link is gone: closed, reset, or silent for
/// longer than `silence`). Both ends send a beat every [`BEAT`] while quiet.
pub fn start(sock: TcpStream, silence: Duration) -> (Sender, mpsc::UnboundedReceiver<Message>) {
    let (rd, wr) = sock.into_split();
    let (out_tx, out_rx) = mpsc::unbounded_channel();
    let (in_tx, in_rx) = mpsc::unbounded_channel();
    let (done_tx, done_rx) = watch::channel(false);
    let stop = Arc::new(Notify::new());
    let busy = Arc::new(AtomicUsize::new(0));
    tokio::spawn(write_loop(wr, out_rx, stop.clone(), done_tx));
    tokio::spawn(read_loop(rd, in_tx, silence, busy.clone(), stop));
    (Sender { tx: out_tx, done: done_rx, busy }, in_rx)
}

async fn write_beat(wr: &mut OwnedWriteHalf) -> std::io::Result<()> {
    wr.write_all(&[K_BEAT, 0, 0, 0, 0]).await
}

/// One message as frames, with a beat in between whenever one is due.
async fn send_message(wr: &mut OwnedWriteHalf, tick: &mut Interval, m: &Message) -> std::io::Result<()> {
    let payload = encode(m);
    let pieces = payload.len().div_ceil(CHUNK);
    for (i, piece) in payload.chunks(CHUNK).enumerate() {
        let kind = if i + 1 == pieces { K_LAST } else { K_MORE };
        let mut frame = Vec::with_capacity(5 + piece.len());
        frame.push(kind);
        frame.extend_from_slice(&(piece.len() as u32).to_be_bytes());
        frame.extend_from_slice(piece);
        wr.write_all(&frame).await?;
        if tick.tick().now_or_never().is_some() {
            write_beat(wr).await?;
        }
    }
    Ok(())
}

async fn write_loop(
    mut wr: OwnedWriteHalf,
    mut rx: mpsc::UnboundedReceiver<Out>,
    stop: Arc<Notify>,
    done: watch::Sender<bool>,
) {
    let mut tick = tokio::time::interval(BEAT);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            _ = stop.notified() => break,
            out = rx.recv() => match out {
                Some(Out::Msg(m)) => {
                    tokio::select! {
                        r = send_message(&mut wr, &mut tick, &m) => {
                            if r.is_err() {
                                break;
                            }
                        }
                        _ = stop.notified() => break,
                    }
                }
                Some(Out::Close) | None => break,
            },
            _ = tick.tick() => {
                if write_beat(&mut wr).await.is_err() {
                    break;
                }
            }
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), wr.shutdown()).await;
    let _ = done.send(true);
}

async fn read_loop(
    mut rd: OwnedReadHalf,
    tx: mpsc::UnboundedSender<Message>,
    silence: Duration,
    busy: Arc<AtomicUsize>,
    stop: Arc<Notify>,
) {
    let patient = silence.max(BUSY_SILENCE);
    let mut buf: Vec<u8> = Vec::new();
    loop {
        if tx.is_closed() {
            break;
        }
        let limit = if !buf.is_empty() || busy.load(Ordering::Relaxed) > 0 {
            patient
        } else {
            silence
        };
        let mut h = [0u8; 5];
        if !matches!(tokio::time::timeout(limit, rd.read_exact(&mut h)).await, Ok(Ok(_))) {
            break;
        }
        let kind = h[0];
        let len = u32::from_be_bytes([h[1], h[2], h[3], h[4]]) as usize;
        match kind {
            K_BEAT => {
                if len != 0 {
                    break;
                }
            }
            K_MORE | K_LAST => {
                if len == 0 || len > CHUNK || buf.len() + len > MAX_MESSAGE {
                    break;
                }
                let at = buf.len();
                buf.resize(at + len, 0);
                if !matches!(tokio::time::timeout(patient, rd.read_exact(&mut buf[at..])).await, Ok(Ok(_))) {
                    break;
                }
                if kind == K_LAST {
                    match decode(std::mem::take(&mut buf)) {
                        Some(m) => {
                            if tx.send(m).is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
            _ => break,
        }
    }
    // Whatever ended it, the writer has nothing left to write to.
    stop.notify_one();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn messages_survive_encoding() {
        let m = Message::new(7, json!({ "op": "put", "path": "chats/a.jsonl" }), vec![1, 2, 3, 0, 255]);
        let back = decode(encode(&m)).unwrap();
        assert_eq!(back.id, 7);
        assert_eq!(back.op(), "put");
        assert_eq!(back.str("path"), Some("chats/a.jsonl"));
        assert_eq!(back.body, vec![1, 2, 3, 0, 255]);
    }

    #[test]
    fn broken_messages_are_refused() {
        assert!(decode(vec![0, 0, 0]).is_none());
        // Head length longer than the message.
        assert!(decode(vec![0, 0, 0, 1, 0, 0, 0, 9, b'{', b'}']).is_none());
        // Head that is not JSON.
        assert!(decode(vec![0, 0, 0, 1, 0, 0, 0, 2, b'x', b'y']).is_none());
    }

    #[test]
    fn replies_say_what_they_are() {
        let ok = Message::ok(3, json!({ "hash": "abc" }));
        assert!(ok.is_ok());
        assert_eq!(ok.str("hash"), Some("abc"));
        let no = Message::fail(3, 409, "changed");
        assert!(!no.is_ok());
        assert_eq!(no.code(), 409);
        assert_eq!(no.error(), "changed");
    }

    async fn pair() -> (TcpStream, TcpStream) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let (c, s) = tokio::join!(TcpStream::connect(addr), l.accept());
        (c.unwrap(), s.unwrap().0)
    }

    #[tokio::test]
    async fn a_big_message_crosses_and_the_link_closes() {
        let (mut c, mut s) = pair().await;
        let (hc, hs) = tokio::join!(client_handshake(&mut c), async { server_handshake(&mut s).await });
        hc.unwrap();
        assert!(hs);
        let (ctx, mut crx) = start(c, Duration::from_secs(2));
        let (stx, mut srx) = start(s, Duration::from_secs(2));

        let body: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
        assert!(ctx.send(Message::new(1, json!({ "op": "put" }), body.clone())));
        let got = srx.recv().await.unwrap();
        assert_eq!(got.id, 1);
        assert_eq!(got.body, body);

        assert!(stx.send(Message::ok(1, json!({}))));
        assert!(crx.recv().await.unwrap().is_ok());

        // A quiet link stays up on beats alone.
        tokio::time::sleep(Duration::from_millis(300)).await;

        ctx.close(Duration::from_secs(2)).await;
        assert!(srx.recv().await.is_none());
    }

    #[tokio::test]
    async fn a_silent_peer_is_noticed() {
        let (mut c, s) = pair().await;
        // `s` is kept open but never speaks (no beats).
        let _keep = s;
        let _ = c.set_nodelay(true);
        let (_tx, mut rx) = start(c, Duration::from_millis(200));
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn http_is_refused_with_an_explanation() {
        let (mut c, mut s) = pair().await;
        c.write_all(b"GET /v1/ping HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        assert!(!server_handshake(&mut s).await);
        let mut got = vec![0u8; 64];
        let n = c.read(&mut got).await.unwrap();
        assert!(String::from_utf8_lossy(&got[..n]).contains("tarpi sync protocol"));
    }
}
