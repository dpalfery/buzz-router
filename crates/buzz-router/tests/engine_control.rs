//! Task 3.7: executing stop, resume and cancel from Buzz messages and the admin API (design 6.7,
//! DD-15, R30–R33, A4).

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::collections::BTreeSet;
use std::path::PathBuf;

use buzz_router::core::control::RESUME_EMOJI;
use buzz_router::core::{ApiFailure, ApiRequest, ApiResponse, CoreHandle};
use buzz_router::ingest::Source;
use buzz_router::store::halts::{HaltRow, HaltScope};
use buzz_router::store::wakes::{WakeRow, WakeState};
use buzz_router::store::Store;
use nostr::{Event, Kind, PublicKey};
use router_core::ids::BotName;
use router_core::route::{Control, Decision, Scope, SuppressWhy};
use support::{
    base_secs, channel, emoji, keys, spawn_test_core_with, top_level, FakeAdapter, FakeRelay, Step,
    TestCoreOptions,
};

fn bot(name: &str) -> BotName {
    BotName::new(name).unwrap()
}

fn bots(names: &[&str]) -> Scope {
    Scope::Bots(names.iter().map(|name| bot(name)).collect())
}

struct Engine {
    core: CoreHandle,
    relay: FakeRelay,
    store: Store,
    adapter: FakeAdapter,
    clock_secs: u64,
}

impl Engine {
    fn new(db_path: Option<PathBuf>) -> Self {
        let adapter = FakeAdapter::new(vec![Step::Hang]);
        let (core, relay, store) = spawn_test_core_with(TestCoreOptions {
            adapter: adapter.clone(),
            db_path,
            ..TestCoreOptions::default()
        });
        Self {
            core,
            relay,
            store,
            adapter,
            clock_secs: base_secs(),
        }
    }

    /// The owner posts `text` at top level, received by bot A.
    async fn owner(&mut self, text: &str) -> Event {
        self.clock_secs += 1;
        let event = top_level(&keys("owner"), channel(), text, self.clock_secs);
        self.core.ingest(bot("A"), event.clone(), Source::Live);
        self.core.flush().await;
        event
    }

    async fn admin(&self, control: Control) -> ApiResponse {
        let response = self.core.api(ApiRequest::Control(control)).await;
        self.core.flush().await;
        response
    }

    fn halts(&self) -> Vec<HaltRow> {
        self.store.halts().list().unwrap()
    }

    fn scopes(&self) -> Vec<HaltScope> {
        self.halts().into_iter().map(|row| row.scope).collect()
    }

    fn wake(&self, n: usize) -> WakeRow {
        let id = self.adapter.dispatches()[n].wake_id;
        self.store.wakes().get(&id).unwrap().unwrap()
    }

    fn all_wakes(&self) -> Vec<WakeRow> {
        let mut statement = self
            .store
            .connection()
            .prepare("SELECT id FROM wakes ORDER BY created_at")
            .unwrap();
        let ids: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        ids.iter()
            .map(|id| {
                self.store
                    .wakes()
                    .get(&uuid::Uuid::parse_str(id).unwrap())
                    .unwrap()
                    .unwrap()
            })
            .collect()
    }

    /// The authors of every reaction `emoji` on `target`.
    fn reactors(&self, target: &Event, emoji: &str) -> BTreeSet<PublicKey> {
        self.relay
            .published()
            .iter()
            .filter(|event| {
                event.kind == Kind::Reaction
                    && event.content == emoji
                    && event.tags.event_ids().next() == Some(&target.id)
            })
            .map(|event| event.pubkey)
            .collect()
    }

    async fn last_decisions(&self) -> Vec<Decision> {
        self.core.debug_counters().await.last_decisions
    }
}

fn pubkeys(names: &[&str]) -> BTreeSet<PublicKey> {
    names.iter().map(|name| keys(name).public_key()).collect()
}

fn is_dropped(wake: &WakeRow) -> bool {
    wake.state == WakeState::Killed
        && serde_json::from_str::<serde_json::Value>(wake.outcome.as_deref().unwrap()).unwrap()
            ["dropped"]
            == true
}

#[tokio::test(start_paused = true)]
async fn a_stop_message_halts_all_kills_running_and_drops_queued_wakes() {
    let mut engine = Engine::new(None);
    let running = engine.owner("@A one").await;
    let queued = engine.owner("@A two").await;
    assert_eq!(engine.adapter.dispatches().len(), 1);
    let running_token = engine.adapter.dispatches()[0].token.clone();

    let stop = engine.owner("stop").await;

    let halts = engine.halts();
    assert_eq!(halts.len(), 1);
    assert_eq!(halts[0].scope, HaltScope::All);
    assert_eq!(halts[0].set_by_event, Some(stop.id.to_hex()));

    assert_eq!(engine.adapter.cancelled(), vec![engine.wake(0).id]);
    assert_eq!(engine.wake(0).state, WakeState::Killed);
    assert_eq!(engine.reactors(&running, emoji::STOP), pubkeys(&["A"]));

    let wakes = engine.all_wakes();
    assert_eq!(wakes.len(), 2);
    assert!(is_dropped(&wakes[1]), "{:?}", wakes[1]);
    assert!(engine.reactors(&queued, emoji::STOP).is_empty());

    assert_eq!(
        engine.reactors(&stop, emoji::STOP),
        pubkeys(&["A", "B", "C"])
    );

    engine.owner("@A are you there?").await;
    assert_eq!(
        engine.last_decisions().await,
        vec![Decision::Suppress {
            bot: bot("A"),
            why: SuppressWhy::Halted
        }]
    );
    let response = engine
        .core
        .api(ApiRequest::Post {
            token: running_token,
            text: "late".to_owned(),
        })
        .await;
    assert_eq!(response, ApiResponse::Failed(ApiFailure::Halted));
    assert_eq!(engine.adapter.dispatches().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_stop_naming_a_bot_halts_that_bot_only() {
    let mut engine = Engine::new(None);
    let stop = engine.owner("@A stop").await;

    assert_eq!(engine.scopes(), vec![HaltScope::Bot(bot("A"))]);
    assert_eq!(engine.reactors(&stop, emoji::STOP), pubkeys(&["A"]));

    engine.owner("@B hi").await;
    assert_eq!(engine.adapter.dispatches().len(), 1);
    assert_eq!(engine.adapter.dispatches()[0].bot, bot("B"));
}

#[tokio::test(start_paused = true)]
async fn resume_all_clears_every_halt_and_each_bot_reacts() {
    let mut engine = Engine::new(None);
    engine.owner("stop").await;
    engine.owner("@B stop").await;
    assert_eq!(engine.halts().len(), 2);

    let resume = engine.owner("resume").await;

    assert!(engine.halts().is_empty());
    assert_eq!(
        engine.reactors(&resume, RESUME_EMOJI),
        pubkeys(&["A", "B", "C"])
    );
    engine.owner("@A hi").await;
    assert_eq!(engine.adapter.dispatches().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn resuming_one_bot_after_stop_all_leaves_the_others_halted() {
    let mut engine = Engine::new(None);
    engine.owner("stop").await;

    let resume = engine.owner("@A resume").await;

    assert_eq!(
        engine.scopes(),
        vec![HaltScope::Bot(bot("B")), HaltScope::Bot(bot("C"))]
    );
    assert_eq!(engine.reactors(&resume, RESUME_EMOJI), pubkeys(&["A"]));
    engine.owner("@A @B hi").await;
    assert_eq!(engine.adapter.dispatches().len(), 1);
    assert_eq!(engine.adapter.dispatches()[0].bot, bot("A"));
}

#[tokio::test(start_paused = true)]
async fn cancel_kills_and_drops_without_halting() {
    let mut engine = Engine::new(None);
    let running = engine.owner("@A one").await;
    engine.owner("@A two").await;

    let cancel = engine.owner("@A !cancel").await;

    assert!(engine.halts().is_empty());
    assert_eq!(engine.wake(0).state, WakeState::Killed);
    assert_eq!(engine.reactors(&running, emoji::STOP), pubkeys(&["A"]));
    assert!(is_dropped(&engine.all_wakes()[1]));
    assert_eq!(engine.reactors(&cancel, emoji::STOP), pubkeys(&["A"]));

    engine.owner("@A three").await;
    assert_eq!(engine.adapter.dispatches().len(), 2);
    assert_eq!(engine.wake(1).state, WakeState::Running);
}

#[tokio::test(start_paused = true)]
async fn admin_controls_have_the_same_effects_without_reactions() {
    let mut engine = Engine::new(None);
    let running = engine.owner("@A one").await;
    engine.owner("@A two").await;

    assert_eq!(
        engine.admin(Control::Stop(Scope::All)).await,
        ApiResponse::Done
    );
    let halts = engine.halts();
    assert_eq!(halts.len(), 1);
    assert_eq!(halts[0].scope, HaltScope::All);
    assert_eq!(halts[0].set_by_event.as_deref(), Some("admin-api"));
    assert_eq!(engine.wake(0).state, WakeState::Killed);
    assert!(is_dropped(&engine.all_wakes()[1]));
    assert_eq!(engine.reactors(&running, emoji::STOP), pubkeys(&["A"]));
    assert_eq!(engine.relay.reactions(emoji::STOP).len(), 1);

    assert_eq!(
        engine.admin(Control::Resume(bots(&["A"]))).await,
        ApiResponse::Done
    );
    assert_eq!(
        engine.scopes(),
        vec![HaltScope::Bot(bot("B")), HaltScope::Bot(bot("C"))]
    );
    assert_eq!(
        engine.admin(Control::Resume(Scope::All)).await,
        ApiResponse::Done
    );
    assert!(engine.halts().is_empty());
    assert!(engine.relay.reactions(RESUME_EMOJI).is_empty());

    engine.owner("@A three").await;
    assert_eq!(engine.adapter.dispatches().len(), 2);
    assert_eq!(
        engine.admin(Control::Cancel(bots(&["A"]))).await,
        ApiResponse::Done
    );
    assert!(engine.halts().is_empty());
    assert_eq!(engine.wake(1).state, WakeState::Killed);
    assert_eq!(engine.relay.reactions(emoji::STOP).len(), 2);

    assert_eq!(
        engine.admin(Control::Stop(bots(&["B"]))).await,
        ApiResponse::Done
    );
    let halts = engine.halts();
    assert_eq!(halts[0].scope, HaltScope::Bot(bot("B")));
    assert_eq!(halts[0].set_by_event.as_deref(), Some("admin-api"));
}

#[tokio::test(start_paused = true)]
async fn halts_survive_reopening_the_store() {
    let path = tempfile::tempdir().unwrap().keep().join("state.sqlite3");
    let mut first = Engine::new(Some(path.clone()));
    let stop = first.owner("@A stop").await;
    first.core.abort();

    let reopened = Store::open(&path).unwrap();
    let halts = reopened.halts().list().unwrap();
    assert_eq!(halts.len(), 1);
    assert_eq!(halts[0].scope, HaltScope::Bot(bot("A")));
    assert_eq!(halts[0].set_by_event, Some(stop.id.to_hex()));

    let mut second = Engine::new(Some(path));
    second.clock_secs += 10;
    second.owner("@A hi").await;
    assert_eq!(
        second.last_decisions().await,
        vec![Decision::Suppress {
            bot: bot("A"),
            why: SuppressWhy::Halted
        }]
    );
    assert!(second.adapter.dispatches().is_empty());
}
