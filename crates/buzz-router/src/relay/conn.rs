//! One WebSocket connection task per bot (design sections 10.1 and 10.5,
//! requirement R61.1).
//!
//! [`spawn_connection`] starts a task that dials the relay, answers the
//! `AUTH` challenge ([`auth::build_auth_event`]), reports Up, publishes
//! events with `OK` tracking, answers pings, and redials on the backoff
//! ladder when the socket drops. Messages the relay sends on its own
//! (`EVENT`, `EOSE`, notices) are ignored here; subscription and backfill
//! wire them up in task 2.5.
//!
//! Reconnect follows `buzz-acp`'s `wait_for_reconnect`: the ladder is 1, 2,
//! 4, 8, 16 and 32 s, then 60 s, each with ±20% jitter, resetting after
//! 60 s of stable connection. DNS failures retry on a flat 2 s without
//! consuming a rung.

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio_tungstenite::tungstenite::Message;

use super::auth::build_auth_event;
use super::{jittered, RelayError};

/// How long to wait for the dial (TCP, TLS and WebSocket handshake) itself.
const DIAL_TIMEOUT: Duration = Duration::from_secs(5);
/// How long to wait for the relay's `AUTH` challenge on a fresh socket.
const AUTH_CHALLENGE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long to wait for the `OK` answering our auth event.
const AUTH_OK_TIMEOUT: Duration = Duration::from_secs(10);
/// Flat retry delay for DNS failures, which consume no ladder rung.
const DNS_RETRY_DELAY: Duration = Duration::from_secs(2);
/// A connection stable for at least this long resets the ladder.
const STABLE_RESET_AFTER: Duration = Duration::from_secs(60);

/// The ladder rungs: 1, 2, 4, 8, 16 and 32 s, then 60 s.
const LADDER: [Duration; 7] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(32),
    Duration::from_secs(60),
];

/// The base delay for `rung`, capped at the 60 s rung.
fn base_delay(rung: u32) -> Duration {
    LADDER[rung.min(6) as usize]
}

/// The rung for the next wait: the ladder resets after 60 s of stable
/// connection, otherwise it climbs one rung (capped at the 60 s rung).
fn next_rung(rung: u32, stable_for: Duration) -> u32 {
    if stable_for >= STABLE_RESET_AFTER {
        0
    } else {
        (rung + 1).min(6)
    }
}

/// Whether a dial failure message is a DNS resolution failure, mirroring
/// `buzz-acp`'s classifier. DNS failures retry on a flat delay and consume no
/// ladder rung.
fn is_dns_error(message: &str) -> bool {
    message.contains("nodename nor servname")
        || message.contains("Name or service not known")
        || message.contains("No such host")
        || message.contains("failed to lookup address")
}

/// How to connect and authenticate one bot.
#[derive(Debug, Clone)]
pub struct ConnParams {
    /// The relay URL in `ws://` form.
    pub relay_url: String,
    /// The bot's signing keys (NIP-42 auth and published events).
    pub keys: nostr::Keys,
    /// The NIP-OA auth tag, sent as the `auth` tag when present.
    pub auth_tag: Option<String>,
    /// How long a publish waits for its `OK` before failing.
    pub publish_timeout: Duration,
}

impl ConnParams {
    /// Builds params with the default 10 s publish timeout.
    pub fn new(relay_url: String, keys: nostr::Keys, auth_tag: Option<String>) -> Self {
        Self {
            relay_url,
            keys,
            auth_tag,
            publish_timeout: Duration::from_secs(10),
        }
    }
}

/// A command from [`Connection`] handles to the connection task.
enum ConnCmd {
    /// Sends `event` as `["EVENT", …]`; `reply` resolves on its `OK`.
    /// The event is boxed: it is far larger than the `Cancel` variant.
    Publish {
        event: Box<nostr::Event>,
        reply: oneshot::Sender<Result<(), RelayError>>,
    },
    /// Drops a timed-out publish's pending entry (its `OK` may still arrive).
    Cancel { id: String },
}

#[derive(Debug)]
struct Inner {
    cmd: mpsc::UnboundedSender<ConnCmd>,
    notify_up: Notify,
    up: AtomicBool,
    attempts: AtomicU64,
    publish_timeout: Duration,
}

/// The handle to a connection task: cloneable, `Send + Sync`.
#[derive(Debug, Clone)]
pub struct Connection {
    inner: Arc<Inner>,
}

impl Connection {
    /// Resolves once the connection has completed authentication. This is a
    /// latch: it stays resolved across later reconnects.
    pub async fn wait_up(&self) {
        loop {
            let notified = self.inner.notify_up.notified();
            if self.inner.up.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }

    /// Publishes a signed event, resolving on `["OK", id, true]`.
    ///
    /// A `false` OK fails with the relay's reason; no OK within
    /// `publish_timeout` fails with a timeout.
    pub async fn publish(&self, event: nostr::Event) -> Result<(), RelayError> {
        let id = event.id.to_hex();
        let (reply_tx, reply_rx) = oneshot::channel();
        self.inner
            .cmd
            .send(ConnCmd::Publish {
                event: Box::new(event),
                reply: reply_tx,
            })
            .map_err(|_| RelayError::Transport("the relay connection task is gone".to_string()))?;
        match tokio::time::timeout(self.inner.publish_timeout, reply_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(RelayError::Transport(
                "the relay connection task is gone".to_string(),
            )),
            Err(_) => {
                let _ = self.inner.cmd.send(ConnCmd::Cancel { id });
                Err(RelayError::Transport(format!(
                    "publish timed out after {:?}",
                    self.inner.publish_timeout
                )))
            }
        }
    }

    /// The number of dial attempts so far (for retries, status and tests).
    pub fn attempts(&self) -> u64 {
        self.inner.attempts.load(Ordering::SeqCst)
    }
}

/// Starts the connection task for `params` and returns its handle.
pub fn spawn_connection(params: ConnParams) -> Connection {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let inner = Arc::new(Inner {
        cmd: cmd_tx,
        notify_up: Notify::new(),
        up: AtomicBool::new(false),
        attempts: AtomicU64::new(0),
        publish_timeout: params.publish_timeout,
    });
    let task_inner = inner.clone();
    tokio::spawn(async move { run_loop(params, task_inner, cmd_rx).await });
    Connection { inner }
}

/// How one dial attempt ended.
enum Outcome {
    /// A DNS failure: retry on the flat delay, consuming no rung.
    Dns,
    /// The socket is down: wait out the current ladder rung.
    Down {
        /// How long the connection had been stable (zero if it never
        /// authenticated). A stable 60 s resets the ladder.
        stable_for: Duration,
    },
}

/// Dials forever, serving one connection per attempt.
async fn run_loop(
    params: ConnParams,
    inner: Arc<Inner>,
    mut cmds: mpsc::UnboundedReceiver<ConnCmd>,
) {
    let mut rung = 0;
    loop {
        inner.attempts.fetch_add(1, Ordering::SeqCst);
        match dial_and_serve(&params, &inner, &mut cmds).await {
            None => return,
            Some(Outcome::Dns) => tokio::time::sleep(DNS_RETRY_DELAY).await,
            Some(Outcome::Down { stable_for }) => {
                let delay = base_delay(rung);
                rung = next_rung(rung, stable_for);
                tokio::time::sleep(jittered(delay)).await;
            }
        }
    }
}

/// Dials once and serves the connection until it drops. Returns `None` when
/// every handle is gone and the task should end.
async fn dial_and_serve(
    params: &ConnParams,
    inner: &Inner,
    cmds: &mut mpsc::UnboundedReceiver<ConnCmd>,
) -> Option<Outcome> {
    let connected = match tokio::time::timeout(
        DIAL_TIMEOUT,
        tokio_tungstenite::connect_async(&params.relay_url),
    )
    .await
    {
        Ok(Ok((ws, _))) => ws,
        Ok(Err(error)) if is_dns_error(&error.to_string()) => return Some(Outcome::Dns),
        Ok(Err(_)) | Err(_) => {
            return Some(Outcome::Down {
                stable_for: Duration::ZERO,
            });
        }
    };
    let (mut sink, mut stream) = connected.split();
    let authed_at = match authenticate(params, &mut sink, &mut stream).await {
        Some(at) => at,
        None => {
            return Some(Outcome::Down {
                stable_for: Duration::ZERO,
            });
        }
    };
    inner.up.store(true, Ordering::SeqCst);
    inner.notify_up.notify_waiters();
    let mut pending: HashMap<String, oneshot::Sender<Result<(), RelayError>>> = HashMap::new();
    serve(&mut sink, &mut stream, cmds, &mut pending).await;
    for (_, reply) in pending {
        let _ = reply.send(Err(RelayError::Transport(
            "the relay connection closed before answering".to_string(),
        )));
    }
    Some(Outcome::Down {
        stable_for: authed_at.elapsed(),
    })
}

/// Runs the `AUTH` handshake: waits up to 5 s for the challenge, answers it,
/// and waits for the `OK`. Returns when the connection authenticated.
async fn authenticate<Sink, Stream>(
    params: &ConnParams,
    sink: &mut Sink,
    stream: &mut Stream,
) -> Option<tokio::time::Instant>
where
    Sink: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
    Stream:
        futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let challenge = tokio::time::timeout(AUTH_CHALLENGE_TIMEOUT, wait_for_challenge(stream)).await;
    let challenge = match challenge {
        Ok(Some(challenge)) => challenge,
        Ok(None) | Err(_) => return None,
    };
    let event = build_auth_event(
        &params.keys,
        &params.relay_url,
        &challenge,
        params.auth_tag.as_deref(),
    )
    .ok()?;
    let id = event.id.to_hex();
    let text = serde_json::to_string(&serde_json::json!(["AUTH", event])).ok()?;
    sink.send(Message::Text(text.into())).await.ok()?;
    let ok = tokio::time::timeout(AUTH_OK_TIMEOUT, wait_for_ok(stream, &id)).await;
    match ok {
        Ok(true) => Some(tokio::time::Instant::now()),
        Ok(false) | Err(_) => None,
    }
}

/// Reads until the relay's `["AUTH", challenge]` arrives. Returns `None` when
/// the socket closes first; other messages are ignored.
async fn wait_for_challenge<Stream>(stream: &mut Stream) -> Option<String>
where
    Stream:
        futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match stream.next().await? {
            Ok(Message::Text(text)) => {
                let value: serde_json::Value = serde_json::from_str(&text).ok()?;
                if value.get(0)?.as_str() == Some("AUTH") {
                    return value.get(1)?.as_str().map(str::to_string);
                }
            }
            Ok(_) => {}
            Err(_) => return None,
        }
    }
}

/// Reads until `["OK", id, _]` arrives. Returns whether it is positive.
/// `EVENT` and other traffic are ignored; a closed socket reads as negative.
async fn wait_for_ok<Stream>(stream: &mut Stream, id: &str) -> bool
where
    Stream:
        futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match stream.next().await {
            Some(Ok(Message::Text(text))) => {
                let value: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                if value.get(0).and_then(serde_json::Value::as_str) == Some("OK")
                    && value.get(1).and_then(serde_json::Value::as_str) == Some(id)
                {
                    return value.get(2).and_then(serde_json::Value::as_bool) == Some(true);
                }
            }
            Some(Ok(_)) => {}
            Some(Err(_)) | None => return false,
        }
    }
}

/// Serves an authenticated connection: answers pings, resolves publish
/// `OK`s, and forwards publish commands. Ends when the socket drops.
async fn serve<Sink, Stream>(
    sink: &mut Sink,
    stream: &mut Stream,
    cmds: &mut mpsc::UnboundedReceiver<ConnCmd>,
    pending: &mut HashMap<String, oneshot::Sender<Result<(), RelayError>>>,
) -> ()
where
    Sink: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
    Stream:
        futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        tokio::select! {
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        handle_text(&text, pending);
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if sink.send(Message::Pong(payload)).await.is_err() {
                            return;
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => return,
                }
            }
            cmd = cmds.recv() => {
                match cmd {
                    Some(ConnCmd::Publish { event, reply }) => {
                        let id = event.id.to_hex();
                        let text = match serde_json::to_string(&serde_json::json!(["EVENT", event])) {
                            Ok(text) => text,
                            Err(error) => {
                                let _ = reply.send(Err(RelayError::Decode(format!("event serialize error: {error}"))));
                                continue;
                            }
                        };
                        if sink.send(Message::Text(text.into())).await.is_err() {
                            let _ = reply.send(Err(RelayError::Transport(
                                "the relay connection closed before answering".to_string(),
                            )));
                            return;
                        }
                        pending.insert(id, reply);
                    }
                    Some(ConnCmd::Cancel { id }) => {
                        pending.remove(&id);
                    }
                    None => return,
                }
            }
        }
    }
}

/// Handles one text message on an authenticated connection: only `OK`s matter
/// here. Anything else (live events, notices) waits for task 2.5.
fn handle_text(text: &str, pending: &mut HashMap<String, oneshot::Sender<Result<(), RelayError>>>) {
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(_) => return,
    };
    if value.get(0).and_then(serde_json::Value::as_str) != Some("OK") {
        return;
    }
    let Some(id) = value.get(1).and_then(serde_json::Value::as_str) else {
        return;
    };
    let Some(reply) = pending.remove(id) else {
        return;
    };
    if value.get(2).and_then(serde_json::Value::as_bool) == Some(true) {
        let _ = reply.send(Ok(()));
    } else {
        let reason = value
            .get(3)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("the relay rejected the event");
        let _ = reply.send(Err(RelayError::Rejected(reason.to_string())));
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the pure reconnect helpers (design section 10.5). The
    //! integration behaviour lives in `tests/relay_conn.rs`.

    use std::time::Duration;

    use super::{base_delay, is_dns_error, next_rung};

    #[test]
    fn the_ladder_is_1_2_4_8_16_32_then_60_seconds() {
        let bases: Vec<u64> = (0..8).map(|rung| base_delay(rung).as_secs()).collect();
        assert_eq!(bases, [1, 2, 4, 8, 16, 32, 60, 60]);
    }

    #[test]
    fn the_ladder_resets_after_sixty_seconds_stable() {
        assert_eq!(next_rung(4, Duration::from_secs(60)), 0);
        assert_eq!(next_rung(6, Duration::from_secs(3600)), 0);
    }

    #[test]
    fn short_connections_keep_climbing_the_ladder() {
        assert_eq!(next_rung(0, Duration::ZERO), 1);
        assert_eq!(next_rung(5, Duration::from_secs(59)), 6);
        assert_eq!(next_rung(6, Duration::ZERO), 6);
    }

    #[test]
    fn dns_failures_match_the_known_resolver_messages() {
        assert!(is_dns_error("nodename nor servname provided"));
        assert!(is_dns_error("Name or service not known"));
        assert!(is_dns_error("No such host is known"));
        assert!(is_dns_error("failed to lookup address information"));
        assert!(!is_dns_error("Connection refused"));
    }
}
