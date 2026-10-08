//! Task 4.3: the status document behind `GET /v1/status` (design 12.2, DD-12, R2.16, R20.4,
//! R21.4, R48.3, R51.1, R52.2).

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use buzz_router::api::{loopback_router, ApiState};
use buzz_router::core::{ApiRequest, CoreHandle, StatusSources};
use buzz_router::ingest::Source;
use http_body_util::BodyExt;
use router_core::config::{parse_roster, roster_hash, Limits};
use router_core::ids::BotName;
use router_core::route::{Control, Scope};
use serde_json::Value;
use support::{
    base_secs, channel, keys, roster_toml, spawn_test_core_with, top_level, FakeAdapter, Step,
    TestCoreOptions,
};
use tower::ServiceExt;

const ADMIN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn bot(name: &str) -> BotName {
    BotName::new(name).unwrap()
}

struct Status {
    core: CoreHandle,
    adapter: FakeAdapter,
    router: Router,
    roster_hash: String,
}

impl Status {
    /// A core whose bots run `script`, with `limits`, where C's key did not load and only A's
    /// relay is connected.
    fn start(script: Vec<Step>, limits: &str) -> Self {
        let roster_text = roster_toml(limits);
        let roster_hash = roster_hash(roster_text.as_bytes());
        let connected: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(|| true);
        let adapter = FakeAdapter::new(script);
        let (core, _relay, _store) = spawn_test_core_with(TestCoreOptions {
            adapter: adapter.clone(),
            limits: limits.to_owned(),
            status: StatusSources {
                roster_hash: roster_hash.clone(),
                unavailable: BTreeSet::from([bot("C")]),
                connected: BTreeMap::from([(bot("A"), connected)]),
            },
            ..TestCoreOptions::default()
        });
        let roster = parse_roster(&roster_text).unwrap();
        let router = loopback_router(ApiState::new(core.clone(), ADMIN.to_owned(), &roster));
        Self {
            core,
            adapter,
            router,
            roster_hash,
        }
    }

    async fn ingest(&self, author: &str, text: &str, age_secs: u64, source: Source) {
        let event = top_level(&keys(author), channel(), text, base_secs() - age_secs);
        self.core.ingest(bot("A"), event, source);
        self.core.flush().await;
    }

    async fn get(&self, token: Option<&str>) -> (StatusCode, Value) {
        let mut request = Request::builder().method("GET").uri("/v1/status");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = self
            .router
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn document(&self) -> Value {
        let (status, body) = self.get(Some(ADMIN)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }
}

fn bot_entry<'a>(document: &'a Value, name: &str) -> &'a Value {
    document["bots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn the_status_needs_the_admin_token() {
    let status = Status::start(vec![Step::Exit(0)], "");

    let (code, body) = status.get(None).await;

    assert_eq!(code, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "unauthorized");
}

#[tokio::test(start_paused = true)]
async fn the_document_has_every_section_and_each_bot_its_fields() {
    let status = Status::start(vec![Step::Hang], "");
    status.ingest("owner", "@A hi", 0, Source::Live).await;
    let wake_id = status.adapter.dispatches()[0].wake_id;

    let document = status.document().await;

    let keys: BTreeSet<&str> = document
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from([
            "version",
            "roster_hash",
            "bots",
            "halts",
            "missed",
            "unmanaged_posts"
        ])
    );
    assert_eq!(document["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(document["roster_hash"], status.roster_hash);
    assert_eq!(document["halts"], serde_json::json!([]));
    assert_eq!(document["missed"], serde_json::json!([]));
    assert_eq!(document["unmanaged_posts"], 0);
    let names: Vec<&str> = document["bots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["A", "B", "C"]);

    let limits = Limits::default();
    let a = bot_entry(&document, "A");
    assert_eq!(
        *a,
        serde_json::json!({
            "name": "A", "available": true, "connected": true, "halted": false,
            "running": [wake_id.to_string()], "queued": 0,
            "budget": {
                "hour": {"used": 1, "limit": limits.wakes_per_hour},
                "day": {"used": 1, "limit": limits.wakes_per_day},
                "suppressed_since_start": 0
            }
        })
    );
    assert_eq!(bot_entry(&document, "B")["connected"], false);
    assert_eq!(bot_entry(&document, "C")["available"], false);
}

#[tokio::test(start_paused = true)]
async fn the_document_shows_halts_queued_wakes_missed_unmanaged_posts_and_suppressions() {
    let status = Status::start(vec![Step::Hang], "wakes_per_hour = 1");
    status.ingest("owner", "@B one", 0, Source::Live).await;
    status.ingest("carol", "@B two", 0, Source::Live).await;
    status.ingest("owner", "@B three", 0, Source::Live).await;
    status
        .ingest("owner", "@A old", 25 * 3_600, Source::Backfill)
        .await;
    status
        .ingest("C", "a post C's agent made by hand", 0, Source::Live)
        .await;
    status
        .core
        .api(ApiRequest::Control(Control::Stop(Scope::Bots(
            BTreeSet::from([bot("A")]),
        ))))
        .await;

    let document = status.document().await;

    assert_eq!(document["halts"], serde_json::json!(["A"]));
    assert_eq!(bot_entry(&document, "A")["halted"], true);
    let b = bot_entry(&document, "B");
    assert_eq!(b["halted"], false);
    assert_eq!(b["running"].as_array().unwrap().len(), 1);
    assert_eq!(b["queued"], 1);
    assert_eq!(b["budget"]["suppressed_since_start"], 1);
    let missed = document["missed"].as_array().unwrap();
    assert_eq!(missed.len(), 1);
    assert_eq!(missed[0]["channel_id"], channel().to_string());
    assert_eq!(
        missed[0]["created_at"],
        i64::try_from(base_secs() - 25 * 3_600).unwrap()
    );
    assert_eq!(missed[0]["event_id"].as_str().unwrap().len(), 64);
    assert_eq!(document["unmanaged_posts"], 1);
}
