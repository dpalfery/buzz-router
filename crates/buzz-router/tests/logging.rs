//! Task 4.4: logging file output, rate limiting and redaction (design 14,
//! R4.4, R20.4, R21.4, R51.2, R59.4).

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use buzz_router::ingest::Source;
use buzz_router::keys::{load_key, KeySource};
use buzz_router::logging::{
    file_writer, filter_from_env, log_dir, prune_old_logs, redact, LogLimiter,
};
use chrono::{TimeZone, Utc};
use nostr::nips::nip19::ToBech32;
use support::{
    base_secs, channel, keys, spawn_test_core_with, top_level, FakeAdapter, Step, TestCoreOptions,
};

fn bot(name: &str) -> router_core::ids::BotName {
    router_core::ids::BotName::new(name).unwrap()
}

/// A `tracing` writer that appends to a shared buffer.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn captured_subscriber(captured: Captured) -> tracing::subscriber::DefaultGuard {
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::set_default(subscriber)
}

fn logs_of(captured: &Captured) -> String {
    String::from_utf8(captured.0.lock().unwrap().clone()).unwrap()
}

#[test]
fn limiter_allows_once_per_key() {
    let mut limiter = LogLimiter::new();
    assert!(limiter.allow_once("drift:abc"));
    assert!(!limiter.allow_once("drift:abc"));
    assert!(limiter.allow_once("drift:def"));
    assert!(!limiter.allow_once("drift:def"));
}

#[test]
fn limiter_allows_once_per_key_per_hour() {
    let mut limiter = LogLimiter::new();
    let start = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let plus_59min = start + chrono::Duration::minutes(59);
    let plus_61min = start + chrono::Duration::minutes(61);

    assert!(limiter.allow_hourly("unmanaged:A", start));
    assert!(!limiter.allow_hourly("unmanaged:A", plus_59min));
    assert!(limiter.allow_hourly("unmanaged:A", plus_61min));
    // A different key is independent.
    assert!(limiter.allow_hourly("unmanaged:B", plus_59min));
}

#[test]
fn file_writer_creates_a_json_log_under_data_dir_logs() {
    let data_dir = tempfile::tempdir().unwrap().keep();
    assert_eq!(log_dir(&data_dir), data_dir.join("logs"));

    let (writer, guard) = file_writer(&data_dir).unwrap();
    {
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_writer(writer)
            .finish();
        let _local = tracing::subscriber::set_default(subscriber);
        tracing::info!(bot = "A", wake_id = "wake-1", "wake dispatched");
    }
    drop(guard);

    let mut lines = Vec::new();
    for entry in std::fs::read_dir(log_dir(&data_dir)).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            lines.extend(
                std::fs::read_to_string(&path)
                    .unwrap()
                    .lines()
                    .map(str::to_owned),
            );
        }
    }
    assert!(
        lines.iter().any(|line| {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            let fields = value.get("fields");
            fields
                .and_then(|fields| fields.get("message"))
                .and_then(|message| message.as_str())
                == Some("wake dispatched")
                && fields
                    .and_then(|fields| fields.get("bot"))
                    .and_then(|bot| bot.as_str())
                    == Some("A")
        }),
        "{lines:?}"
    );
}

#[test]
fn prune_old_logs_keeps_the_newest_fourteen_files() {
    let data_dir = tempfile::tempdir().unwrap().keep();
    let dir = log_dir(&data_dir);
    std::fs::create_dir_all(&dir).unwrap();
    for day in 1..=16 {
        std::fs::write(dir.join(format!("buzz-router.log.2026-09-{day:02}")), "x").unwrap();
    }
    let removed = prune_old_logs(&dir, 14).unwrap();
    assert_eq!(removed, 2);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names.len(), 14);
    assert!(!names.contains(&"buzz-router.log.2026-09-01".to_owned()));
    assert!(!names.contains(&"buzz-router.log.2026-09-02".to_owned()));
}

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn buzz_router_log_debug_enables_debug_output() {
    let _locked = ENV_LOCK.lock().unwrap();

    std::env::set_var("BUZZ_ROUTER_LOG", "debug");
    let captured = Captured::default();
    {
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(filter_from_env())
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        let _local = tracing::subscriber::set_default(subscriber);
        tracing::debug!("debug-line-visible");
    }
    assert!(logs_of(&captured).contains("debug-line-visible"));

    std::env::remove_var("BUZZ_ROUTER_LOG");
    let captured = Captured::default();
    {
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(filter_from_env())
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        let _local = tracing::subscriber::set_default(subscriber);
        tracing::debug!("debug-line-hidden");
        tracing::info!("info-line-visible");
    }
    let logs = logs_of(&captured);
    assert!(!logs.contains("debug-line-hidden"), "{logs}");
    assert!(logs.contains("info-line-visible"), "{logs}");
}

#[test]
fn redact_replaces_every_secret() {
    let token = "wake-token-abc123";
    let nsec = "nsec1secretvalue";
    let admin = "admintoken456";
    let webhook = "webhook-secret-789";
    let message = format!("token {token} nsec {nsec} admin {admin} hook {webhook} end");
    let redacted = redact(&message, &[token, nsec, admin, webhook]);
    assert!(!redacted.contains(token));
    assert!(!redacted.contains(nsec));
    assert!(!redacted.contains(admin));
    assert!(!redacted.contains(webhook));
    assert!(redacted.contains("[redacted]"));
    // Empty secrets change nothing and never match everything.
    assert_eq!(redact("plain line", &[""]), "plain line");
    assert_eq!(redact("plain line", &[]), "plain line");
}

#[tokio::test(start_paused = true)]
async fn logs_never_contain_secrets() {
    let captured = Captured::default();
    let _subscriber = captured_subscriber(captured.clone());

    // A FakeAdapter wake: the dispatch hands the adapter a wake token.
    let adapter = FakeAdapter::new(vec![Step::Exit(0)]);
    let (core, _relay, _store) = spawn_test_core_with(TestCoreOptions {
        adapter: adapter.clone(),
        ..TestCoreOptions::default()
    });
    let ask = top_level(&keys("owner"), channel(), "@A hi", base_secs());
    core.ingest(bot("A"), ask, Source::Live);
    core.flush().await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    let token = adapter.dispatches().pop().unwrap().token;

    // A run with a file key: loading it must not log the secret.
    let dir = tempfile::tempdir().unwrap().keep();
    let nsec = keys("owner").secret_key().to_bech32().unwrap();
    let key_file = dir.join("owner.key");
    std::fs::write(&key_file, format!("{nsec}\n")).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(
        &key_file,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();
    load_key(&KeySource::File(key_file), &bot("owner")).unwrap();

    // An admin-token-shaped secret that passes through redaction.
    let admin_token = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    tracing::info!(
        "{}",
        redact(&format!("using token {admin_token}"), &[admin_token])
    );

    let logs = logs_of(&captured);
    assert!(!logs.contains(&token), "{logs}");
    assert!(!logs.contains(&nsec), "{logs}");
    assert!(!logs.contains(admin_token), "{logs}");
}
