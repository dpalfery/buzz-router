//! The relay REST client (design section 10.4).
//!
//! This mirrors `buzz-acp`'s `RestClient` (`relay.rs:251-580`):
//!
//! - the base URL is [`relay_ws_to_http`] of the relay URL, with `wss` to
//!   `https` and `ws` to `http`;
//! - `POST /query` and `POST /events` carry `Authorization: Nostr <base64>` of
//!   a kind-27235 event pinning the exact URL, the method, the lowercase hex
//!   SHA-256 of the raw body and a nonce, re-signed on each attempt;
//! - `x-auth-tag` is sent if and only if an auth tag is configured;
//! - transient failures (429, 502, 503, 504, timeouts and connect errors)
//!   retry after 500 ms, 1 s and 2 s with ±20% jitter; anything else fails at
//!   once;
//! - [`RestClient::query`] pages with `until` and `before_id` taken from the
//!   oldest event of each full page, until a short page. There is no event cap.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use base64::Engine as _;
use nostr::{EventBuilder, Kind, Tag};
use sha2::{Digest, Sha256};

use super::{RelayError, RelayPort};

/// Base retry delays for transient HTTP failures: 500 ms, 1 s, 2 s.
/// Jitter (±20%) is applied at call time via [`jittered`].
const RETRY_BASE_DELAYS: [Duration; 3] = [
    Duration::from_millis(500),
    Duration::from_millis(1000),
    Duration::from_millis(2000),
];

/// The default page size when the caller's filters set no limit.
const DEFAULT_PAGE_LIMIT: usize = 500;

/// Maps a relay WebSocket URL to its HTTP bridge base URL: `wss` becomes
/// `https`, `ws` becomes `http`, and one trailing `/` is trimmed.
pub fn relay_ws_to_http(relay_url: &str) -> String {
    let mapped = if let Some(rest) = relay_url.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = relay_url.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        relay_url.to_string()
    };
    mapped.trim_end_matches('/').to_string()
}

/// Whether an HTTP status is transient and worth retrying.
fn is_retriable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 429 | 502 | 503 | 504)
}

/// Applies ±20% jitter to a retry delay, exactly as `buzz-acp` does.
///
/// The factor is drawn from the current sub-second nanos over `u32::MAX`, so
/// it lands in [0.8, 0.9) in practice: the delay never exceeds its base.
fn jittered(base: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or_default();
    // factor in [0.8, 1.2).
    let factor = 0.8 + (f64::from(nanos) / f64::from(u32::MAX)) * 0.4;
    base.mul_f64(factor)
}

/// The relay HTTP bridge client: NIP-98, `x-auth-tag`, retries and paging.
#[derive(Debug, Clone)]
pub struct RestClient {
    http: reqwest::Client,
    base_url: String,
    keys: nostr::Keys,
    auth_tag: Option<String>,
}

impl RestClient {
    /// Builds a client for the relay at `relay_url` (the `ws://` form),
    /// signing as `keys`, with an optional NIP-OA auth tag for `x-auth-tag`.
    pub fn new(relay_url: &str, keys: nostr::Keys, auth_tag: Option<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: relay_ws_to_http(relay_url),
            keys,
            auth_tag,
        }
    }

    /// Queries historical events matching any of the filters.
    ///
    /// Full pages are followed with `until` and `before_id` taken from the
    /// oldest event of the page, until a page returns fewer than the limit.
    /// The combined events come back sorted by `(created_at, id)` ascending.
    pub async fn query(
        &self,
        mut filters: Vec<nostr::Filter>,
    ) -> Result<Vec<nostr::Event>, RelayError> {
        let page_limit = filters
            .iter()
            .filter_map(|filter| filter.limit)
            .max()
            .unwrap_or(DEFAULT_PAGE_LIMIT);
        let mut events = Vec::new();
        let mut before_id: Option<String> = None;
        loop {
            let body = self.page_body(&filters, before_id.as_deref())?;
            let response = self.post("/query", &body).await?;
            let page: Vec<nostr::Event> = response
                .json()
                .await
                .map_err(|error| RelayError::Decode(error.to_string()))?;
            let done = page.len() < page_limit;
            if !done {
                let oldest = page
                    .iter()
                    .min_by_key(|event| (event.created_at.as_secs(), event.id.to_hex()));
                match oldest {
                    Some(oldest) => {
                        let until = nostr::Timestamp::from(oldest.created_at.as_secs());
                        for filter in &mut filters {
                            filter.until = Some(until);
                        }
                        before_id = Some(oldest.id.to_hex());
                    }
                    None => break,
                }
            }
            events.extend(page);
            if done {
                break;
            }
        }
        events.sort_by_key(|event| (event.created_at.as_secs(), event.id.to_hex()));
        Ok(events)
    }

    /// Submits one signed event via `POST /events`.
    pub async fn submit_event(&self, event: &nostr::Event) -> Result<(), RelayError> {
        let body = serde_json::to_vec(event)
            .map_err(|error| RelayError::Decode(format!("event serialize error: {error}")))?;
        self.post("/events", &body).await?;
        Ok(())
    }

    /// Serializes the filters for one page, injecting `before_id` once paging
    /// has started. `nostr::Filter` has no `before_id` field, so it goes into
    /// the JSON directly; `until` is set on the filters themselves.
    fn page_body(
        &self,
        filters: &[nostr::Filter],
        before_id: Option<&str>,
    ) -> Result<Vec<u8>, RelayError> {
        let mut value = serde_json::to_value(filters)
            .map_err(|error| RelayError::Decode(format!("filter serialize error: {error}")))?;
        if let (Some(id), Some(items)) = (before_id, value.as_array_mut()) {
            for item in items {
                if let Some(object) = item.as_object_mut() {
                    object.insert(
                        "before_id".to_string(),
                        serde_json::Value::String(id.to_string()),
                    );
                }
            }
        }
        serde_json::to_vec(&value)
            .map_err(|error| RelayError::Decode(format!("filter serialize error: {error}")))
    }

    /// POSTs `body` to `path` with NIP-98 auth and the retry policy.
    /// The auth event is re-signed on each attempt (fresh `created_at`).
    async fn post(&self, path: &str, body: &[u8]) -> Result<reqwest::Response, RelayError> {
        let url = format!("{}{path}", self.base_url);
        let mut attempt = 0;
        loop {
            let authorization = self.nip98_header("POST", &url, body).unwrap_or_default();
            let mut request = self
                .http
                .post(&url)
                .header("Authorization", authorization)
                .header("Content-Type", "application/json")
                .body(body.to_vec());
            if let Some(tag) = &self.auth_tag {
                request = request.header("x-auth-tag", tag);
            }
            match request.send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        return Ok(response);
                    }
                    if is_retriable_status(response.status()) && attempt < RETRY_BASE_DELAYS.len() {
                        let delay = RETRY_BASE_DELAYS[attempt];
                        attempt += 1;
                        tokio::time::sleep(jittered(delay)).await;
                        continue;
                    }
                    return Err(RelayError::Status(response.status().as_u16()));
                }
                Err(error)
                    if (error.is_timeout() || error.is_connect())
                        && attempt < RETRY_BASE_DELAYS.len() =>
                {
                    let delay = RETRY_BASE_DELAYS[attempt];
                    attempt += 1;
                    tokio::time::sleep(jittered(delay)).await;
                }
                Err(error) => return Err(RelayError::Transport(error.to_string())),
            }
        }
    }

    /// The full `Authorization` header value: `Nostr <base64>`.
    fn nip98_header(&self, method: &str, url: &str, body: &[u8]) -> Result<String, RelayError> {
        Ok(format!("Nostr {}", self.sign_nip98(method, url, body)?))
    }

    /// Signs a NIP-98 HTTP auth event (kind 27235) for the method, URL and
    /// body, and returns its JSON base64-encoded. The `payload` tag is the
    /// lowercase hex SHA-256 of the raw body; the nonce keeps rapid-fire
    /// requests with identical bodies distinct.
    fn sign_nip98(&self, method: &str, url: &str, body: &[u8]) -> Result<String, RelayError> {
        let auth_failed = |error: String| RelayError::Auth(error);
        let tags = vec![
            Tag::parse(["u", url]).map_err(|error| auth_failed(format!("tag error: {error}")))?,
            Tag::parse(["method", method])
                .map_err(|error| auth_failed(format!("tag error: {error}")))?,
            Tag::parse(["nonce", &uuid::Uuid::new_v4().to_string()])
                .map_err(|error| auth_failed(format!("tag error: {error}")))?,
            Tag::parse(["payload", &hex::encode(Sha256::digest(body))])
                .map_err(|error| auth_failed(format!("tag error: {error}")))?,
        ];
        let event = EventBuilder::new(Kind::HttpAuth, "")
            .tags(tags)
            .sign_with_keys(&self.keys)
            .map_err(|error| auth_failed(format!("sign error: {error}")))?;
        let event_json = serde_json::to_string(&event)
            .map_err(|error| auth_failed(format!("serialize error: {error}")))?;
        Ok(base64::engine::general_purpose::STANDARD.encode(event_json))
    }
}

impl RelayPort for RestClient {
    fn publish(
        &self,
        event: nostr::Event,
    ) -> Pin<Box<dyn Future<Output = Result<(), RelayError>> + Send + '_>> {
        Box::pin(async move { self.submit_event(&event).await })
    }

    fn query(
        &self,
        filters: Vec<nostr::Filter>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<nostr::Event>, RelayError>> + Send + '_>> {
        Box::pin(async move { RestClient::query(self, filters).await })
    }
}
