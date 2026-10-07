//! The webhook adapter: posts the signed wake payload to a URL (design 7.2, R38, R39).
//!
//! Every request carries `X-Buzz-Router-Signature: sha256=<hex HMAC-SHA256(secret, body)>` and
//! `X-Buzz-Router-Wake: <wake id>`. The secret is read from the environment variable named by
//! `secret_env` at wake time, so it never sits in the config or the store. An async webhook must
//! answer 2xx within 10 s and the agent calls back through the API; a sync webhook answers with
//! the reply itself and is bounded only by the core's deadline.

use std::time::Duration;

use futures_util::future::BoxFuture;
use hmac::{Hmac, KeyInit, Mac};
use reqwest::header::CONTENT_TYPE;
use router_core::config::{AdapterConfig, WebhookMode};
use router_core::payload::WakePayload;
use sha2::Sha256;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{Adapter, AdapterEvent, SyncReply, WakeContext};

/// How long an async webhook has to answer (R38.2, A13).
const ASYNC_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a `cancel_url` call may take (DD-16).
const CANCEL_TIMEOUT: Duration = Duration::from_secs(5);
const SIGNATURE_HEADER: &str = "X-Buzz-Router-Signature";
const WAKE_HEADER: &str = "X-Buzz-Router-Wake";

/// Why a webhook call failed.
#[derive(Debug, thiserror::Error)]
enum WebhookError {
    /// The bot is not configured with a webhook adapter.
    #[error("the bot's adapter is not a webhook")]
    NotWebhook,
    /// The secret variable is unset or not Unicode.
    #[error("the secret variable {0} is not set")]
    MissingSecret(String),
    /// The body could not be serialised.
    #[error("cannot serialise the request body: {0}")]
    Body(#[from] serde_json::Error),
    /// The request failed or timed out.
    #[error("the request failed: {0}")]
    Request(#[from] reqwest::Error),
    /// The request did not finish in time.
    #[error("no answer within {} s", .0.as_secs())]
    Timeout(Duration),
    /// The webhook answered non-2xx.
    #[error("the webhook answered {0}")]
    Status(reqwest::StatusCode),
    /// A sync webhook's body is neither `{"text": s}` nor `{"pass": true}`.
    #[error("the webhook's answer is neither {{\"text\": ...}} nor {{\"pass\": true}}")]
    UnrecognisedReply,
}

/// The webhook's settings, from [`AdapterConfig::Webhook`].
struct Settings<'a> {
    url: &'a str,
    secret_env: &'a str,
    mode: WebhookMode,
    cancel_url: Option<&'a str>,
}

impl<'a> Settings<'a> {
    fn of(config: &'a AdapterConfig) -> Result<Self, WebhookError> {
        match config {
            AdapterConfig::Webhook {
                url,
                secret_env,
                mode,
                cancel_url,
            } => Ok(Self {
                url,
                secret_env,
                mode: *mode,
                cancel_url: cancel_url.as_deref(),
            }),
            AdapterConfig::Command { .. } => Err(WebhookError::NotWebhook),
        }
    }

    fn secret(&self) -> Result<String, WebhookError> {
        std::env::var(self.secret_env)
            .map_err(|_| WebhookError::MissingSecret(self.secret_env.to_owned()))
    }
}

/// Calls each wake's configured webhook (design 7.2).
#[derive(Debug, Clone)]
pub struct WebhookAdapter {
    http: reqwest::Client,
}

impl WebhookAdapter {
    /// An adapter sending through the shared rustls client `http` (R38.1).
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// Best-effort cancel of wake `wake_id` (DD-16): when the bot's webhook has a `cancel_url`,
    /// posts `{"wake_id": ...}` to it, signed like the wake request. Failures are logged.
    pub async fn cancel(&self, config: &AdapterConfig, wake_id: Uuid) {
        let Ok(settings) = Settings::of(config) else {
            return;
        };
        let Some(cancel_url) = settings.cancel_url else {
            return;
        };
        let result = async {
            let secret = settings.secret()?;
            let body = serde_json::to_vec(&serde_json::json!({ "wake_id": wake_id }))?;
            let response = self
                .signed_post(cancel_url, &secret, wake_id, body, Some(CANCEL_TIMEOUT))
                .await?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(WebhookError::Status(response.status()))
            }
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, %wake_id, "the cancel_url call failed");
        }
    }

    /// Posts `body` to `url` with the signature and wake headers, within `limit` if given.
    async fn signed_post(
        &self,
        url: &str,
        secret: &str,
        wake_id: Uuid,
        body: Vec<u8>,
        limit: Option<Duration>,
    ) -> Result<reqwest::Response, WebhookError> {
        let request = self
            .http
            .post(url)
            .header(CONTENT_TYPE, "application/json")
            .header(SIGNATURE_HEADER, signature(secret, &body))
            .header(WAKE_HEADER, wake_id.to_string())
            .body(body)
            .send();
        match limit {
            Some(limit) => tokio::time::timeout(limit, request)
                .await
                .map_err(|_| WebhookError::Timeout(limit))?
                .map_err(WebhookError::from),
            None => Ok(request.await?),
        }
    }

    async fn call(
        &self,
        ctx: &WakeContext,
        payload: &WakePayload,
    ) -> Result<AdapterEvent, WebhookError> {
        let settings = Settings::of(&ctx.adapter)?;
        let secret = settings.secret()?;
        let body = serde_json::to_vec(payload)?;
        match settings.mode {
            WebhookMode::Async => {
                // The timeout covers the whole exchange, so a 2xx whose body never ends still fails.
                let exchange = async {
                    let response = self
                        .signed_post(settings.url, &secret, ctx.wake_id, body, None)
                        .await?;
                    let status = response.status();
                    response.bytes().await?;
                    Ok::<_, WebhookError>(status)
                };
                let status = tokio::time::timeout(ASYNC_TIMEOUT, exchange)
                    .await
                    .map_err(|_| WebhookError::Timeout(ASYNC_TIMEOUT))??;
                if status.is_success() {
                    Ok(AdapterEvent::AsyncAccepted)
                } else {
                    Err(WebhookError::Status(status))
                }
            }
            WebhookMode::Sync => {
                let response = self
                    .signed_post(settings.url, &secret, ctx.wake_id, body, None)
                    .await?;
                let status = response.status();
                if !status.is_success() {
                    return Err(WebhookError::Status(status));
                }
                let reply = sync_reply(&response.bytes().await?)?;
                Ok(AdapterEvent::SyncReply(reply))
            }
        }
    }
}

impl Adapter for WebhookAdapter {
    fn run(
        &self,
        ctx: WakeContext,
        payload: WakePayload,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, AdapterEvent> {
        let adapter = self.clone();
        Box::pin(async move {
            tokio::select! {
                result = adapter.call(&ctx, &payload) => {
                    result.unwrap_or_else(|error| AdapterEvent::Failed(error.to_string()))
                }
                () = cancel.cancelled() => AdapterEvent::Failed("cancelled".to_owned()),
            }
        })
    }

    fn killed(&self, config: &AdapterConfig, wake_id: Uuid) -> BoxFuture<'static, ()> {
        let adapter = self.clone();
        let config = config.clone();
        Box::pin(async move { adapter.cancel(&config, wake_id).await })
    }
}

/// `sha256=<hex HMAC-SHA256(secret, body)>`.
fn signature(secret: &str, body: &[u8]) -> String {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes())
        .unwrap_or_else(|_| unreachable!("HMAC takes keys of any length"));
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// Reads a sync webhook's answer: `{"text": s}` or `{"pass": true}` (R39.2).
fn sync_reply(body: &[u8]) -> Result<SyncReply, WebhookError> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| WebhookError::UnrecognisedReply)?;
    if let Some(text) = value.get("text").and_then(serde_json::Value::as_str) {
        return Ok(SyncReply::Text(text.to_owned()));
    }
    if value.get("pass") == Some(&serde_json::Value::Bool(true)) {
        return Ok(SyncReply::Pass);
    }
    Err(WebhookError::UnrecognisedReply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signature_is_hex_hmac_sha256() {
        // RFC 4231 test case 2.
        assert_eq!(
            signature("Jefe", b"what do ya want for nothing?"),
            "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn sync_replies_are_read() {
        assert_eq!(
            sync_reply(br#"{"text":"hi"}"#).ok(),
            Some(SyncReply::Text("hi".to_owned()))
        );
        assert_eq!(sync_reply(br#"{"pass":true}"#).ok(), Some(SyncReply::Pass));
        assert!(sync_reply(br#"{"pass":false}"#).is_err());
        assert!(sync_reply(br#"{"foo":1}"#).is_err());
        assert!(sync_reply(b"not json").is_err());
    }
}
