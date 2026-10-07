//! Wake payload serialisation (task 3.1; requirements 40.1–40.4, 40.6, 40.7, design section 5.7).

use chrono::{TimeZone, Utc};
use router_core::payload::{
    format_deadline, turns_left_after_this, ApiRef, ChannelRef, ContextMessage, WakePayload,
};
use router_core::route::Reason;
use router_core::thread::RoundMode;
use serde_json::{json, Value};
use uuid::Uuid;

#[allow(
    clippy::expect_used,
    reason = "a test helper fails the test by panicking; clippy.toml exempts only #[test] functions"
)]
fn payload() -> WakePayload {
    let deadline = Utc
        .with_ymd_and_hms(2026, 10, 5, 3, 20, 0)
        .single()
        .expect("valid timestamp");
    WakePayload {
        wake_id: Uuid::parse_str("6f1c2a52-7d8e-4b0a-9c3e-2f4d5a6b7c8d").expect("uuid"),
        token: "ab".repeat(32),
        bot: "A".to_owned(),
        channel: ChannelRef {
            id: "0b5e8f6a-1c2d-4e3f-8a9b-0c1d2e3f4a5b".to_owned(),
            name: "work".to_owned(),
        },
        thread_root_id: "1".repeat(64),
        reply_parent_id: "2".repeat(64),
        reason: Reason::ReplyTarget,
        round_mode: RoundMode::Discussion,
        turns_left_after_this: turns_left_after_this(4, 3),
        turns_per_round: 4,
        deadline: format_deadline(deadline),
        triggers: vec!["3".repeat(64), "4".repeat(64)],
        context: vec![ContextMessage {
            id: "3".repeat(64),
            author: "David".to_owned(),
            class: "owner".to_owned(),
            created_at: "2026-10-05T03:00:00Z".to_owned(),
            text: "hello".to_owned(),
            new: true,
        }],
        api: ApiRef::new("http://127.0.0.1:47821"),
    }
}

#[test]
fn payload_json_has_the_brief_keys_and_values() {
    let value = serde_json::to_value(payload()).expect("serialises");
    let expected = json!({
        "wake_id": "6f1c2a52-7d8e-4b0a-9c3e-2f4d5a6b7c8d",
        "token": "ab".repeat(32),
        "bot": "A",
        "channel": {"id": "0b5e8f6a-1c2d-4e3f-8a9b-0c1d2e3f4a5b", "name": "work"},
        "thread_root_id": "1".repeat(64),
        "reply_parent_id": "2".repeat(64),
        "reason": "reply_target",
        "round_mode": "discussion",
        "turns_left_after_this": 1,
        "turns_per_round": 4,
        "deadline": "2026-10-05T03:20:00Z",
        "triggers": ["3".repeat(64), "4".repeat(64)],
        "context": [{
            "id": "3".repeat(64),
            "author": "David",
            "class": "owner",
            "created_at": "2026-10-05T03:00:00Z",
            "text": "hello",
            "new": true
        }],
        "api": {
            "url": "http://127.0.0.1:47821",
            "post": "/v1/post",
            "pass": "/v1/pass",
            "eta": "/v1/eta"
        }
    });
    assert_eq!(value, expected);
}

#[test]
fn direct_round_mode_serialises_lowercase() {
    let mut payload = payload();
    payload.round_mode = RoundMode::Direct;
    payload.reason = Reason::Participant;
    let value = serde_json::to_value(payload).expect("serialises");
    assert_eq!(value["round_mode"], Value::from("direct"));
    assert_eq!(value["reason"], Value::from("participant"));
}

#[test]
fn turns_left_is_limit_minus_turns_used_after_this_wake() {
    assert_eq!(turns_left_after_this(4, 1), 3);
    assert_eq!(turns_left_after_this(4, 4), 0);
    assert_eq!(turns_left_after_this(4, 5), 0);
}

#[test]
fn deadline_is_rfc3339_seconds_with_z_suffix() {
    let at = Utc
        .timestamp_opt(1_791_170_400, 999_000_000)
        .single()
        .expect("valid timestamp");
    let text = format_deadline(at);
    assert!(text.ends_with('Z'), "{text}");
    assert!(!text.contains('.'), "{text}");
    assert_eq!(text.len(), "2026-10-05T03:20:00Z".len(), "{text}");
}
