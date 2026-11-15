//! The wire format between `tarpi serve` and `tarpi connect`: one persistent
//! TCP connection per device that carries everything (file sync, token counts,
//! the log lines, the "a file changed" notes and the liveness beat).
//!
//! **Greeting.** Both sides send a magic line first: [`MAGIC`] for a plain link,
//! [`MAGIC_SECURE`] for an encrypted one (`sync: encrypted: true`, the default).
//! A peer that answers with anything else (a browser, an older tarpi that
//! still spoke HTTP) is refused, and says so in words. Two tarpis that are set
//! differently (one encrypted, one not) tell each other so instead of failing
//! silently.
//!
//! **Encryption.** After the secure greeting each side sends a fresh X25519
//! public key (32 bytes). The shared secret and the hash of both public keys
//! (the *binding*) give one ChaCha20-Poly1305 key per direction. From then on
//! every frame is sealed: `length: u32 | ciphertext of (kind | payload) + tag`,
//! with a counter as the nonce, so a frame that is changed, dropped, repeated
//! or reordered breaks the link. Keys live for one connection only. The key
//! exchange alone does not say *who* is at the other end: that is the token
//! (see [`proof`]), which is checked against the binding, so someone sitting
//! in the middle can neither read the traffic nor pass the token check. With
//! `serve_trust_all` there is no token, so the link is protected against
//! eavesdropping but not against someone who actively plays the server.
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
//! With `encrypted: false` the link is plain TCP, not encrypted: use it on a
//! network you trust (or through an SSH tunnel).

use anyhow::{anyhow, bail, Context, Result};
use futures_util::FutureExt;
use ring::{aead, agreement, digest, hmac, rand::SystemRandom};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch, Notify};
use tokio::time::{Interval, MissedTickBehavior};

/// What both sides say first on a plain (not encrypted) link.
pub const MAGIC: &[u8] = b"TARPI-SYNC 2\r\n\r\n";
/// The same for an encrypted link. Same length as [`MAGIC`], so a server can
/// read either and tell which one the device speaks.
pub const MAGIC_SECURE: &[u8] = b"TARPI-SYNC 3\r\n\r\n";
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

/// Bytes the authentication tag adds to a sealed frame.
const TAG: usize = 16;

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

// ------------------------------------------------------------- encryption

/// The keys of one encrypted connection.
pub struct Keys {
    send: [u8; 32],
    recv: [u8; 32],
    binding: [u8; 32],
}

/// What the greeting settled: a plain link, or an encrypted one.
pub enum Link {
    Plain,
    Secure(Keys),
}

impl Link {
    /// The hash of both public keys of an encrypted link (what [`proof`] is
    /// bound to); `None` for a plain link.
    pub fn binding(&self) -> Option<[u8; 32]> {
        match self {
            Link::Plain => None,
            Link::Secure(k) => Some(k.binding),
        }
    }
}

fn hmac32(key: &[u8], data: &[u8]) -> [u8; 32] {
    let tag = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data);
    let mut out = [0u8; 32];
    out.copy_from_slice(tag.as_ref());
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// What a side shows to prove it knows the token without sending it: an
/// HMAC of the link's binding. `role` is `"client"` or `"server"`, so one
/// side's proof cannot be played back as the other's. Only an encrypted link
/// has a binding.
pub fn proof(token: &str, binding: &[u8; 32], role: &str) -> String {
    let mut data = binding.to_vec();
    data.extend_from_slice(role.as_bytes());
    hex(&hmac32(token.as_bytes(), &data))
}

/// A fresh key pair: the private half and the 32 public bytes to send.
fn new_key_pair() -> Result<(agreement::EphemeralPrivateKey, [u8; 32])> {
    let rng = SystemRandom::new();
    let private = agreement::EphemeralPrivateKey::generate(&agreement::X25519, &rng)
        .map_err(|_| anyhow!("could not make an encryption key"))?;
    let public = private
        .compute_public_key()
        .map_err(|_| anyhow!("could not make an encryption key"))?;
    let mut out = [0u8; 32];
    out.copy_from_slice(public.as_ref());
    Ok((private, out))
}

/// Finish the key exchange. `client` says which side we are.
fn derive_keys(
    private: agreement::EphemeralPrivateKey,
    mine: &[u8; 32],
    theirs: &[u8; 32],
    client: bool,
) -> Result<Keys> {
    let (client_pub, server_pub) = if client { (mine, theirs) } else { (theirs, mine) };
    let mut transcript = b"tarpi-sync-3".to_vec();
    transcript.extend_from_slice(client_pub);
    transcript.extend_from_slice(server_pub);
    let mut binding = [0u8; 32];
    binding.copy_from_slice(digest::digest(&digest::SHA256, &transcript).as_ref());

    let shared = agreement::agree_ephemeral(
        private,
        &agreement::UnparsedPublicKey::new(&agreement::X25519, &theirs[..]),
        |secret| secret.to_vec(),
    )
    .map_err(|_| anyhow!("the key exchange failed"))?;

    let prk = hmac32(&binding, &shared);
    let c2s = hmac32(&prk, b"tarpi-sync client to server");
    let s2c = hmac32(&prk, b"tarpi-sync server to client");
    let (send, recv) = if client { (c2s, s2c) } else { (s2c, c2s) };
    Ok(Keys { send, recv, binding })
}

fn nonce(n: u64) -> aead::Nonce {
    let mut b = [0u8; 12];
    b[4..].copy_from_slice(&n.to_be_bytes());
    aead::Nonce::assume_unique_for_key(b)
}

fn aead_key(key: &[u8; 32]) -> aead::LessSafeKey {
    aead::LessSafeKey::new(aead::UnboundKey::new(&aead::CHACHA20_POLY1305, key).expect("a 32 byte key"))
}

/// Seals the frames we send (one key, a counter for the nonce).
struct Sealer {
    key: aead::LessSafeKey,
    n: u64,
}

impl Sealer {
    fn new(key: &[u8; 32]) -> Sealer {
        Sealer { key: aead_key(key), n: 0 }
    }

    /// Encrypt `buf` in place and append the tag.
    fn seal(&mut self, buf: &mut Vec<u8>) -> bool {
        let nonce = nonce(self.n);
        self.n += 1;
        self.key.seal_in_place_append_tag(nonce, aead::Aad::empty(), buf).is_ok()
    }
}

/// Opens the frames we receive.
struct Opener {
    key: aead::LessSafeKey,
    n: u64,
}

impl Opener {
    fn new(key: &[u8; 32]) -> Opener {
        Opener { key: aead_key(key), n: 0 }
    }

    /// Decrypt `buf` in place (the tag is cut off). `false`: it was not made
    /// with our key, or it was changed, dropped, repeated or reordered.
    fn open(&mut self, buf: &mut Vec<u8>) -> bool {
        let nonce = nonce(self.n);
        self.n += 1;
        let len = match self.key.open_in_place(nonce, aead::Aad::empty(), buf.as_mut_slice()) {
            Ok(plain) => plain.len(),
            Err(_) => return false,
        };
        buf.truncate(len);
        true
    }
}

// ---------------------------------------------------------------- greeting

/// The two ends of a link disagree about `sync: encrypted` (one has it on, the
/// other off). It is the error of the device's greeting in that case, so
/// `tarpi connect` can say what to change instead of a vague "did not answer
/// like tarpi serve" (see [`EncryptionMismatch::explain`]).
#[derive(Debug)]
pub struct EncryptionMismatch {
    /// This device wants an encrypted link (the server does not); `false`: the
    /// device wants a plain one and the server insists on encryption.
    pub client_encrypted: bool,
}

impl EncryptionMismatch {
    /// What is wrong and what to change, for the device's user. `server` names
    /// the machine (`host:port`).
    pub fn explain(&self, server: &str) -> String {
        if self.client_encrypted {
            format!(
                "encryption settings do not match: this device has `encrypted: true` (the default) under \
                 `sync:`, but the tarpi serve at {} runs with `encrypted: false`.\n\
                 Make both the same, either:\n  \
                 - set `encrypted: true` (or leave the line out) under `sync:` in the config.yaml of the \
                 machine that runs `tarpi serve`, and restart it (recommended), or\n  \
                 - set `encrypted: false` under `sync:` in this device's config.yaml (then the link is NOT \
                 encrypted: only do that on a network you trust).",
                server
            )
        } else {
            format!(
                "encryption settings do not match: this device has `encrypted: false` under `sync:`, but \
                 the tarpi serve at {} runs with `encrypted: true` (the default).\n\
                 Make both the same, either:\n  \
                 - set `encrypted: true` (or remove the line) under `sync:` in this device's config.yaml \
                 (recommended), or\n  \
                 - set `encrypted: false` under `sync:` in the config.yaml of the machine that runs \
                 `tarpi serve`, and restart it (then the link is NOT encrypted: only do that on a network \
                 you trust).",
                server
            )
        }
    }
}

impl std::fmt::Display for EncryptionMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.explain("the server"))
    }
}

impl std::error::Error for EncryptionMismatch {}

/// Why the server turned a connection away during the greeting.
#[derive(Debug, PartialEq)]
pub enum Refusal {
    /// The peer sent something that is not the tarpi greeting (a browser, a
    /// scanner, an older tarpi that spoke HTTP).
    NotTarpi,
    /// A tarpi that is set the other way round about `sync: encrypted`.
    Mismatch {
        /// The device wanted an encrypted link (this server does not run one).
        client_encrypted: bool,
    },
    /// The greeting broke off: the peer went away, stalled, or the key
    /// exchange failed.
    Failed,
}

impl Refusal {
    /// A line for `tarpi serve`'s log (without time and address). `None` when
    /// there is nothing worth saying: a peer that merely connected and left,
    /// as a port check does.
    pub fn describe(&self) -> Option<String> {
        match self {
            Refusal::Failed => None,
            Refusal::NotTarpi => Some(
                "refused: not a tarpi device (it did not send the tarpi greeting)".to_string(),
            ),
            Refusal::Mismatch { client_encrypted: true } => Some(
                "refused: encryption mismatch: the device asked for an encrypted link, but this server \
                 runs with `sync: encrypted: false`. Set the same on both sides: `encrypted: true` here \
                 (then restart tarpi serve, recommended) or `encrypted: false` on the device"
                    .to_string(),
            ),
            Refusal::Mismatch { client_encrypted: false } => Some(
                "refused: encryption mismatch: the device asked for an unencrypted link, but this server \
                 runs with `sync: encrypted: true`. Set the same on both sides: `encrypted: true` on the \
                 device (recommended) or `encrypted: false` here (then restart tarpi serve)"
                    .to_string(),
            ),
        }
    }
}

async fn read_exact_within(sock: &mut TcpStream, buf: &mut [u8]) -> bool {
    matches!(tokio::time::timeout(Duration::from_secs(10), sock.read_exact(buf)).await, Ok(Ok(_)))
}

/// The device's side of the greeting: say it, and check the server says it
/// back. With `secure` the keys are exchanged too.
pub async fn client_handshake(sock: &mut TcpStream, secure: bool) -> Result<Link> {
    let _ = sock.set_nodelay(true);
    let own = if secure { MAGIC_SECURE } else { MAGIC };
    let mut hello = own.to_vec();
    let keys = if secure {
        let (private, public) = new_key_pair()?;
        hello.extend_from_slice(&public);
        Some((private, public))
    } else {
        None
    };
    sock.write_all(&hello).await.context("sending the greeting")?;
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
    if got != own {
        if got == MAGIC && secure {
            return Err(anyhow::Error::new(EncryptionMismatch { client_encrypted: true }));
        }
        if got == MAGIC_SECURE && !secure {
            return Err(anyhow::Error::new(EncryptionMismatch { client_encrypted: false }));
        }
        bail!("that is not a tarpi serve of this version");
    }
    let Some((private, public)) = keys else {
        return Ok(Link::Plain);
    };
    let mut theirs = [0u8; 32];
    if !read_exact_within(sock, &mut theirs).await {
        bail!("the server did not finish the key exchange");
    }
    Ok(Link::Secure(derive_keys(private, &public, &theirs, true)?))
}

/// The server's side: read the greeting and answer it. `Err` when the peer
/// is not a `tarpi connect` that speaks the same way (it is told so and
/// should be dropped); the [`Refusal`] says why, so `tarpi serve` can log it.
pub async fn server_handshake(sock: &mut TcpStream, secure: bool) -> std::result::Result<Link, Refusal> {
    let _ = sock.set_nodelay(true);
    let own = if secure { MAGIC_SECURE } else { MAGIC };
    let not_http: &[u8] = b"tarpi serve speaks the tarpi sync protocol, not HTTP: use `tarpi connect`\r\n";
    let mut got = vec![0u8; MAGIC.len()];
    if !read_exact_within(sock, &mut got).await {
        let _ = sock.write_all(not_http).await;
        return Err(Refusal::Failed);
    }
    if got != MAGIC && got != MAGIC_SECURE {
        let _ = sock.write_all(not_http).await;
        return Err(Refusal::NotTarpi);
    }
    if got != own {
        // A tarpi set the other way: answer with our own greeting, which is
        // how it learns the difference, and hang up.
        let client_encrypted = got == MAGIC_SECURE;
        if client_encrypted {
            // An encrypted device sends its key right behind the greeting. Read
            // it before hanging up: closing a connection with unread data makes
            // the system reset it, and the reset can wipe out our answer before
            // the device has read it (it would only see "hung up").
            let mut key = [0u8; 32];
            let _ = tokio::time::timeout(Duration::from_secs(1), sock.read_exact(&mut key)).await;
        }
        let _ = sock.write_all(own).await;
        let _ = sock.shutdown().await;
        return Err(Refusal::Mismatch { client_encrypted });
    }
    if !secure {
        return if sock.write_all(MAGIC).await.is_ok() { Ok(Link::Plain) } else { Err(Refusal::Failed) };
    }
    let mut theirs = [0u8; 32];
    if !read_exact_within(sock, &mut theirs).await {
        return Err(Refusal::Failed);
    }
    let (private, public) = new_key_pair().map_err(|_| Refusal::Failed)?;
    let mut reply = MAGIC_SECURE.to_vec();
    reply.extend_from_slice(&public);
    sock.write_all(&reply).await.map_err(|_| Refusal::Failed)?;
    derive_keys(private, &public, &theirs, false)
        .map(Link::Secure)
        .map_err(|_| Refusal::Failed)
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
pub fn start(sock: TcpStream, silence: Duration, link: Link) -> (Sender, mpsc::UnboundedReceiver<Message>) {
    let (rd, wr) = sock.into_split();
    let (seal, open) = match &link {
        Link::Plain => (None, None),
        Link::Secure(k) => (Some(Sealer::new(&k.send)), Some(Opener::new(&k.recv))),
    };
    let (out_tx, out_rx) = mpsc::unbounded_channel();
    let (in_tx, in_rx) = mpsc::unbounded_channel();
    let (done_tx, done_rx) = watch::channel(false);
    let stop = Arc::new(Notify::new());
    let busy = Arc::new(AtomicUsize::new(0));
    tokio::spawn(write_loop(Tx { wr, seal }, out_rx, stop.clone(), done_tx));
    tokio::spawn(read_loop(Rx { rd, open }, in_tx, silence, busy.clone(), stop));
    (Sender { tx: out_tx, done: done_rx, busy }, in_rx)
}

/// The sending half: frames go out sealed when the link is encrypted.
struct Tx {
    wr: OwnedWriteHalf,
    seal: Option<Sealer>,
}

impl Tx {
    async fn frame(&mut self, kind: u8, payload: &[u8]) -> std::io::Result<()> {
        let out = match &mut self.seal {
            None => {
                let mut v = Vec::with_capacity(5 + payload.len());
                v.push(kind);
                v.extend_from_slice(&(payload.len() as u32).to_be_bytes());
                v.extend_from_slice(payload);
                v
            }
            Some(seal) => {
                let mut p = Vec::with_capacity(1 + payload.len() + TAG);
                p.push(kind);
                p.extend_from_slice(payload);
                if !seal.seal(&mut p) {
                    return Err(std::io::Error::new(std::io::ErrorKind::Other, "encryption failed"));
                }
                let mut v = Vec::with_capacity(4 + p.len());
                v.extend_from_slice(&(p.len() as u32).to_be_bytes());
                v.extend_from_slice(&p);
                v
            }
        };
        self.wr.write_all(&out).await
    }
}

async fn write_beat(tx: &mut Tx) -> std::io::Result<()> {
    tx.frame(K_BEAT, &[]).await
}

/// One message as frames, with a beat in between whenever one is due.
async fn send_message(tx: &mut Tx, tick: &mut Interval, m: &Message) -> std::io::Result<()> {
    let payload = encode(m);
    let pieces = payload.len().div_ceil(CHUNK);
    for (i, piece) in payload.chunks(CHUNK).enumerate() {
        let kind = if i + 1 == pieces { K_LAST } else { K_MORE };
        tx.frame(kind, piece).await?;
        if tick.tick().now_or_never().is_some() {
            write_beat(tx).await?;
        }
    }
    Ok(())
}

async fn write_loop(
    mut tx: Tx,
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
                        r = send_message(&mut tx, &mut tick, &m) => {
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
                if write_beat(&mut tx).await.is_err() {
                    break;
                }
            }
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), tx.wr.shutdown()).await;
    let _ = done.send(true);
}

/// The receiving half: frames are opened when the link is encrypted.
struct Rx {
    rd: OwnedReadHalf,
    open: Option<Opener>,
}

/// Is this a frame we know, with a payload of a sensible size?
fn frame_ok(kind: u8, len: usize) -> bool {
    match kind {
        K_BEAT => len == 0,
        K_MORE | K_LAST => (1..=CHUNK).contains(&len),
        _ => false,
    }
}

impl Rx {
    /// The next frame's kind and payload. `limit` is how long the frame may
    /// take to begin, `patient` how long the rest of it may take. `None`: the
    /// link is dead or the frame is wrong (for an encrypted link, also one
    /// that does not open).
    async fn frame(&mut self, limit: Duration, patient: Duration) -> Option<(u8, Vec<u8>)> {
        match &mut self.open {
            None => {
                let mut h = [0u8; 5];
                if !matches!(tokio::time::timeout(limit, self.rd.read_exact(&mut h)).await, Ok(Ok(_))) {
                    return None;
                }
                let kind = h[0];
                let len = u32::from_be_bytes([h[1], h[2], h[3], h[4]]) as usize;
                if !frame_ok(kind, len) {
                    return None;
                }
                let mut payload = vec![0u8; len];
                if len > 0
                    && !matches!(tokio::time::timeout(patient, self.rd.read_exact(&mut payload)).await, Ok(Ok(_)))
                {
                    return None;
                }
                Some((kind, payload))
            }
            Some(open) => {
                let mut h = [0u8; 4];
                if !matches!(tokio::time::timeout(limit, self.rd.read_exact(&mut h)).await, Ok(Ok(_))) {
                    return None;
                }
                let len = u32::from_be_bytes(h) as usize;
                if len < 1 + TAG || len > 1 + CHUNK + TAG {
                    return None;
                }
                let mut sealed = vec![0u8; len];
                if !matches!(tokio::time::timeout(patient, self.rd.read_exact(&mut sealed)).await, Ok(Ok(_))) {
                    return None;
                }
                if !open.open(&mut sealed) || sealed.is_empty() {
                    return None;
                }
                let kind = sealed[0];
                let payload = sealed.split_off(1);
                if !frame_ok(kind, payload.len()) {
                    return None;
                }
                Some((kind, payload))
            }
        }
    }
}

async fn read_loop(
    mut rx: Rx,
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
        let Some((kind, payload)) = rx.frame(limit, patient).await else {
            break;
        };
        if kind == K_BEAT {
            continue;
        }
        if buf.len() + payload.len() > MAX_MESSAGE {
            break;
        }
        buf.extend_from_slice(&payload);
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

    async fn linked(secure: bool) -> ((Sender, mpsc::UnboundedReceiver<Message>), (Sender, mpsc::UnboundedReceiver<Message>), Option<[u8; 32]>, Option<[u8; 32]>) {
        let (mut c, mut s) = pair().await;
        let (hc, hs) = tokio::join!(client_handshake(&mut c, secure), server_handshake(&mut s, secure));
        let (lc, ls) = (hc.unwrap(), hs.unwrap());
        let (bc, bs) = (lc.binding(), ls.binding());
        (start(c, Duration::from_secs(2), lc), start(s, Duration::from_secs(2), ls), bc, bs)
    }

    async fn big_message_crosses(secure: bool) {
        let ((ctx, mut crx), (stx, mut srx), bc, bs) = linked(secure).await;
        assert_eq!(bc.is_some(), secure);
        assert_eq!(bc, bs);

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
    async fn a_big_message_crosses_and_the_link_closes() {
        big_message_crosses(false).await;
    }

    #[tokio::test]
    async fn a_big_message_crosses_an_encrypted_link() {
        big_message_crosses(true).await;
    }

    #[tokio::test]
    async fn encrypted_traffic_is_not_readable_on_the_wire() {
        // A man in the middle that copies what the client sends.
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let (c, s) = tokio::join!(TcpStream::connect(addr), l.accept());
        let (mut c, mut s) = (c.unwrap(), s.unwrap().0);
        let (hc, hs) = tokio::join!(client_handshake(&mut c, true), server_handshake(&mut s, true));
        let (lc, _ls) = (hc.unwrap(), hs.unwrap());
        let (ctx, _crx) = start(c, Duration::from_secs(2), lc);
        assert!(ctx.send(Message::new(1, json!({ "op": "secret-op-name" }), b"secret body".to_vec())));
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut seen = vec![0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(1), s.read(&mut seen)).await.unwrap().unwrap();
        let text = String::from_utf8_lossy(&seen[..n]).to_string();
        assert!(n > 0);
        assert!(!text.contains("secret"));
    }

    #[tokio::test]
    async fn a_frame_not_sealed_with_the_key_ends_the_link() {
        let (mut c, mut s) = pair().await;
        let (hc, hs) = tokio::join!(client_handshake(&mut c, true), server_handshake(&mut s, true));
        hc.unwrap();
        let (_stx, mut srx) = start(s, Duration::from_secs(2), hs.unwrap());
        // The right shape (length, then bytes), but not made with the key.
        let mut frame = (20u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&[7u8; 20]);
        c.write_all(&frame).await.unwrap();
        assert!(srx.recv().await.is_none());
    }

    #[tokio::test]
    async fn encrypted_and_plain_peers_tell_each_other_apart() {
        // Encrypted client, plain server.
        let (mut c, mut s) = pair().await;
        let (hc, hs) = tokio::join!(client_handshake(&mut c, true), server_handshake(&mut s, false));
        assert_eq!(hs.err(), Some(Refusal::Mismatch { client_encrypted: true }));
        let e = hc.err().unwrap();
        let m = e.downcast_ref::<EncryptionMismatch>().expect("a mismatch, not a vague failure");
        assert!(m.client_encrypted);
        let text = m.explain("10.0.0.5:7878");
        assert!(text.contains("10.0.0.5:7878"));
        assert!(text.contains("this device has `encrypted: true`"));
        assert!(text.contains("runs with `encrypted: false`"));
        // Plain client, encrypted server.
        let (mut c, mut s) = pair().await;
        let (hc, hs) = tokio::join!(client_handshake(&mut c, false), server_handshake(&mut s, true));
        assert_eq!(hs.err(), Some(Refusal::Mismatch { client_encrypted: false }));
        let e = hc.err().unwrap();
        let m = e.downcast_ref::<EncryptionMismatch>().expect("a mismatch, not a vague failure");
        assert!(!m.client_encrypted);
        let text = m.explain("10.0.0.5:7878");
        assert!(text.contains("this device has `encrypted: false`"));
        assert!(text.contains("runs with `encrypted: true`"));
    }

    #[tokio::test]
    async fn a_mismatch_is_logged_on_the_server_and_a_stray_peer_is_not_a_mismatch() {
        let said = |r: Refusal| r.describe().unwrap_or_default();
        let a = said(Refusal::Mismatch { client_encrypted: true });
        assert!(a.contains("encryption mismatch") && a.contains("`sync: encrypted: false`"));
        let b = said(Refusal::Mismatch { client_encrypted: false });
        assert!(b.contains("encryption mismatch") && b.contains("`sync: encrypted: true`"));
        assert!(said(Refusal::NotTarpi).contains("not a tarpi"));
        // A peer that connects and leaves (a port check) is not worth a line.
        assert!(Refusal::Failed.describe().is_none());

        // HTTP on the port is "not tarpi", not a mismatch.
        let (mut c, mut s) = pair().await;
        c.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        assert_eq!(server_handshake(&mut s, true).await.err(), Some(Refusal::NotTarpi));
        // Connecting and closing without a word is `Failed`.
        let (c, mut s) = pair().await;
        drop(c);
        assert_eq!(server_handshake(&mut s, true).await.err(), Some(Refusal::Failed));
    }

    #[test]
    fn proofs_depend_on_the_token_the_link_and_the_role() {
        let b1 = [1u8; 32];
        let b2 = [2u8; 32];
        let p = proof("token", &b1, "client");
        assert_eq!(p, proof("token", &b1, "client"));
        assert_ne!(p, proof("other", &b1, "client"));
        assert_ne!(p, proof("token", &b2, "client"));
        assert_ne!(p, proof("token", &b1, "server"));
        assert_eq!(p.len(), 64);
    }

    #[tokio::test]
    async fn a_silent_peer_is_noticed() {
        let (mut c, s) = pair().await;
        // `s` is kept open but never speaks (no beats).
        let _keep = s;
        let _ = c.set_nodelay(true);
        let (_tx, mut rx) = start(c, Duration::from_millis(200), Link::Plain);
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn http_is_refused_with_an_explanation() {
        let (mut c, mut s) = pair().await;
        c.write_all(b"GET /v1/ping HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        assert!(server_handshake(&mut s, false).await.is_err());
        let mut got = vec![0u8; 64];
        let n = c.read(&mut got).await.unwrap();
        assert!(String::from_utf8_lossy(&got[..n]).contains("tarpi sync protocol"));
    }
}
