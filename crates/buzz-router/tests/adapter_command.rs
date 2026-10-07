//! Task 3.5: the command adapter and the test agent (design 7.1, R37, R40.8, R59.4, DD-19 to
//! DD-21). These run real processes, so they use real time.

#![allow(
    clippy::unwrap_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use buzz_router::adapter::command::CommandAdapter;
use buzz_router::adapter::{Adapter, AdapterEvent, WakeContext};
use buzz_router::core::{ApiRequest, CoreHandle};
use buzz_router::ingest::Source;
use buzz_router::store::wakes::{WakeRow, WakeState};
use buzz_router::store::Store;
use chrono::Utc;
use futures_util::future::BoxFuture;
use router_core::config::{AdapterConfig, Limits, PromptMode, ReplyMode};
use router_core::ids::{BotName, ChannelId, EventId};
use router_core::payload::{ApiRef, ChannelRef, WakePayload};
use router_core::prompt::{reason_text, render, render_context, PromptVars, BUILT_IN_TEMPLATE};
use router_core::route::Reason;
use router_core::thread::RoundMode;
use support::{
    base_secs, channel, keys, spawn_test_core_with, test_agent_path, top_level, TestCoreOptions,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Captured stdout is capped at this many bytes (R37.6).
const STDOUT_CAP: usize = 65_536;

fn agent(args: &[&str]) -> Vec<String> {
    let mut command = vec![test_agent_path().display().to_string()];
    command.extend(args.iter().map(|arg| (*arg).to_owned()));
    command
}

fn command_config(command: Vec<String>, cwd: &Path) -> AdapterConfig {
    AdapterConfig::Command {
        command,
        cwd: cwd.display().to_string(),
        env: BTreeMap::new(),
        prompt_mode: PromptMode::Stdin,
        reply_mode: ReplyMode::Stdout,
        prompt_template: None,
    }
}

fn event_id(n: u8) -> EventId {
    EventId::from_hex(&format!("{n:064x}")).unwrap()
}

fn wake(adapter: AdapterConfig) -> (WakeContext, WakePayload) {
    let wake_id = Uuid::new_v4();
    let token = "ab".repeat(32);
    let limits = Limits::default();
    let ctx = WakeContext {
        wake_id,
        token: token.clone(),
        bot: BotName::new("A").unwrap(),
        adapter,
        channel: ChannelId::from(channel()),
        root: event_id(1),
        reply_parent: event_id(1),
        reason: Reason::Mention,
        reason_author: "Owner".to_owned(),
        mode: RoundMode::Direct,
        turns_left: limits.turns_per_round - 1,
        limits,
        deadline: Utc::now(),
        trigger_ids: vec![event_id(1)],
    };
    let payload = WakePayload {
        wake_id,
        token,
        bot: "A".to_owned(),
        channel: ChannelRef {
            id: channel().to_string(),
            name: "general".to_owned(),
        },
        thread_root_id: event_id(1).to_string(),
        reply_parent_id: event_id(1).to_string(),
        reason: Reason::Mention,
        round_mode: RoundMode::Direct,
        turns_left_after_this: limits.turns_per_round - 1,
        turns_per_round: limits.turns_per_round,
        deadline: "2026-10-05T15:20:00Z".to_owned(),
        triggers: vec![event_id(1).to_string()],
        context: Vec::new(),
        api: ApiRef::new("http://127.0.0.1:47821"),
    };
    (ctx, payload)
}

struct Fixture {
    _dir: tempfile::TempDir,
    data_dir: PathBuf,
    cwd: PathBuf,
    adapter: CommandAdapter,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let cwd = dir.path().join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let adapter = CommandAdapter::new(data_dir.clone(), "http://127.0.0.1:47821".to_owned());
    Fixture {
        _dir: dir,
        data_dir,
        cwd,
        adapter,
    }
}

async fn run(adapter: &CommandAdapter, config: AdapterConfig) -> AdapterEvent {
    let (ctx, payload) = wake(config);
    tokio::time::timeout(
        Duration::from_secs(30),
        adapter.run(ctx, payload, CancellationToken::new()),
    )
    .await
    .unwrap()
}

fn stdout(event: &AdapterEvent) -> String {
    let AdapterEvent::Exited {
        code: Some(0),
        stdout: Some(text),
    } = event
    else {
        unreachable!("expected a clean exit with stdout, got {event:?}");
    };
    text.clone()
}

fn env_lines(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

#[tokio::test]
async fn the_child_gets_the_router_variables_and_its_env_but_never_the_private_key() {
    let fixture = fixture();
    std::env::set_var("BUZZ_PRIVATE_KEY", "inherited-placeholder");
    let mut config = command_config(agent(&["print-env"]), &fixture.cwd);
    if let AdapterConfig::Command { env, .. } = &mut config {
        env.insert("AGENT_FLAVOUR".to_owned(), "plain".to_owned());
        env.insert(
            "BUZZ_PRIVATE_KEY".to_owned(),
            "configured-placeholder".to_owned(),
        );
    }

    let vars = env_lines(&stdout(&run(&fixture.adapter, config).await));

    assert_eq!(vars["BUZZ_ROUTER_URL"], "http://127.0.0.1:47821");
    assert_eq!(vars["BUZZ_ROUTER_WAKE_TOKEN"], "ab".repeat(32));
    assert!(vars["BUZZ_ROUTER_PAYLOAD"].ends_with("payload.json"));
    assert!(Path::new(&vars["BUZZ_ROUTER_PAYLOAD"]).starts_with(fixture.data_dir.join("wakes")));
    assert_eq!(vars["AGENT_FLAVOUR"], "plain");
    assert!(!vars.contains_key("BUZZ_PRIVATE_KEY"), "{vars:?}");
    assert!(!vars.contains_key("BUZZ_ROUTER_PROMPT_FILE"));
}

fn built_in_prompt() -> String {
    let limits = Limits::default();
    render(
        BUILT_IN_TEMPLATE,
        &PromptVars {
            bot: "A".to_owned(),
            channel: "general".to_owned(),
            reason_text: reason_text(Reason::Mention, "Owner"),
            turn: 1,
            turns_per_round: limits.turns_per_round,
            context: render_context(&[]),
        },
    )
}

#[tokio::test]
async fn stdin_mode_delivers_the_rendered_prompt() {
    let fixture = fixture();
    let config = command_config(agent(&["echo", "--delay", "0"]), &fixture.cwd);

    let text = stdout(&run(&fixture.adapter, config).await);

    assert_eq!(text, format!("echo: {}", built_in_prompt()).trim());
}

#[tokio::test]
async fn file_mode_passes_a_readable_prompt_file() {
    let fixture = fixture();
    let mut config = command_config(agent(&["echo", "--delay", "0"]), &fixture.cwd);
    if let AdapterConfig::Command { prompt_mode, .. } = &mut config {
        *prompt_mode = PromptMode::File;
    }

    let text = stdout(&run(&fixture.adapter, config).await);

    assert_eq!(text, format!("echo: {}", built_in_prompt()).trim());
}

#[tokio::test]
async fn a_prompt_template_file_is_used_when_set() {
    let fixture = fixture();
    let template = fixture.cwd.join("prompt.tmpl");
    std::fs::write(
        &template,
        "{bot}|{channel}|{reason_text}|{turn}/{turns_per_round}|{other}",
    )
    .unwrap();
    let mut config = command_config(agent(&["echo", "--delay", "0"]), &fixture.cwd);
    if let AdapterConfig::Command {
        prompt_template, ..
    } = &mut config
    {
        *prompt_template = Some(template);
    }

    let text = stdout(&run(&fixture.adapter, config).await);

    assert_eq!(text, "echo: A|general|David mentioned you|1/4|{other}");
}

#[tokio::test]
async fn large_output_is_drained_and_capped_at_64_kib() {
    let fixture = fixture();
    let config = command_config(agent(&["write-stdout", "--bytes", "204800"]), &fixture.cwd);

    let text = stdout(&run(&fixture.adapter, config).await);

    assert_eq!(text.len(), STDOUT_CAP);
}

#[tokio::test]
async fn api_reply_mode_reports_no_stdout() {
    let fixture = fixture();
    let mut config = command_config(
        agent(&["exit", "--code", "0", "--stdout", "x"]),
        &fixture.cwd,
    );
    if let AdapterConfig::Command { reply_mode, .. } = &mut config {
        *reply_mode = ReplyMode::Api;
    }

    let event = run(&fixture.adapter, config).await;

    assert_eq!(
        event,
        AdapterEvent::Exited {
            code: Some(0),
            stdout: None
        }
    );
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

#[tokio::test]
async fn stderr_lines_are_logged() {
    let fixture = fixture();
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let config = command_config(
        agent(&["stderr", "--text", "agent-says-hello"]),
        &fixture.cwd,
    );

    run(&fixture.adapter, config).await;

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("agent-says-hello"), "{logs}");
}

#[tokio::test]
async fn wake_files_are_private_and_deleted_when_the_wake_ends() {
    let fixture = fixture();
    let mut config = command_config(agent(&["echo", "--delay", "2"]), &fixture.cwd);
    if let AdapterConfig::Command { prompt_mode, .. } = &mut config {
        *prompt_mode = PromptMode::File;
    }
    let (ctx, payload) = wake(config);
    let dir = fixture.data_dir.join("wakes").join(ctx.wake_id.to_string());
    let running = tokio::spawn(fixture.adapter.run(ctx, payload, CancellationToken::new()));

    let started = Instant::now();
    while !dir.join("pid").exists() {
        assert!(started.elapsed() < Duration::from_secs(10), "no pid file");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let payload_json = std::fs::read_to_string(dir.join("payload.json")).unwrap();
    assert!(payload_json.contains("\"wake_id\""));
    assert!(dir.join("prompt.txt").exists());
    let pid = std::fs::read_to_string(dir.join("pid")).unwrap();
    assert!(pid.trim().parse::<u32>().is_ok(), "{pid:?}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for file in ["payload.json", "prompt.txt", "pid"] {
            let mode = std::fs::metadata(dir.join(file))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{file}");
        }
    }

    tokio::time::timeout(Duration::from_secs(30), running)
        .await
        .unwrap()
        .unwrap();
    assert!(!dir.exists(), "the wake directory is deleted");
}

#[tokio::test]
async fn a_tilde_cwd_is_expanded_to_the_home_directory() {
    let fixture = fixture();
    let mut config = command_config(agent(&["print-env"]), &fixture.cwd);
    if let AdapterConfig::Command { cwd, .. } = &mut config {
        *cwd = "~".to_owned();
    }

    let vars = env_lines(&stdout(&run(&fixture.adapter, config).await));

    let home = directories::BaseDirs::new()
        .unwrap()
        .home_dir()
        .to_path_buf();
    assert_eq!(
        PathBuf::from(&vars["CWD"]).canonicalize().unwrap(),
        home.canonicalize().unwrap()
    );
}

/// The `[bots.adapter]` TOML running the test agent with `args` in `cwd`.
fn adapter_toml(args: &[&str], cwd: &Path, reply_mode: &str) -> String {
    let command: Vec<String> = agent(args).iter().map(|arg| format!("'{arg}'")).collect();
    format!(
        "type = \"command\"\ncommand = [{}]\ncwd = '{}'\nenv = {{}}\nprompt_mode = \"stdin\"\nreply_mode = \"{reply_mode}\"\nprompt_template = \"\"\n",
        command.join(", "),
        cwd.display()
    )
}

/// Polls until the only wake has ended.
async fn ended_wake(store: &Store) -> WakeRow {
    let started = Instant::now();
    loop {
        let row: Option<String> = store
            .connection()
            .query_row(
                "SELECT id FROM wakes WHERE state NOT IN ('queued', 'running')",
                [],
                |row| row.get(0),
            )
            .ok();
        if let Some(id) = row {
            return store
                .wakes()
                .get(&Uuid::parse_str(&id).unwrap())
                .unwrap()
                .unwrap();
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the wake never ended"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn outcome(args: &[&str], reply_mode: &str) -> (WakeRow, support::FakeRelay) {
    let fixture = fixture();
    let (core, relay, store) = spawn_test_core_with(TestCoreOptions {
        adapter_toml: adapter_toml(args, &fixture.cwd, reply_mode),
        real_adapter: Some(Arc::new(fixture.adapter.clone())),
        ..TestCoreOptions::default()
    });
    core.ingest(
        BotName::new("A").unwrap(),
        top_level(&keys("owner"), channel(), "@A hi", base_secs()),
        Source::Live,
    );
    let wake = ended_wake(&store).await;
    core.flush().await;
    (wake, relay)
}

#[tokio::test]
async fn no_reply_and_empty_output_pass() {
    let (wake, relay) = outcome(&["exit", "--code", "0", "--stdout", "[no-reply]"], "stdout").await;
    assert_eq!(wake.state, WakeState::Passed);
    assert!(relay.messages("reply").is_empty());

    let (wake, relay) = outcome(&["exit", "--code", "0"], "stdout").await;
    assert_eq!(wake.state, WakeState::Passed);
    assert!(relay.messages("reply").is_empty());
}

#[tokio::test]
async fn exit_three_with_stdout_fails_and_posts_nothing() {
    let (wake, relay) = outcome(&["exit", "--code", "3", "--stdout", "half done"], "stdout").await;
    assert_eq!(wake.state, WakeState::Failed);
    assert!(relay.messages("reply").is_empty());
}

#[tokio::test]
async fn api_mode_exit_zero_without_a_post_passes() {
    let (wake, relay) = outcome(&["exit", "--code", "0", "--stdout", "ignored"], "api").await;
    assert_eq!(wake.state, WakeState::Passed);
    assert!(relay.messages("reply").is_empty());
}

#[tokio::test]
async fn stdout_text_is_posted() {
    let (wake, relay) = outcome(&["exit", "--code", "0", "--stdout", "the answer"], "stdout").await;
    assert_eq!(wake.state, WakeState::Posted);
    let replies = relay.messages("reply");
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].content, "the answer");
}

/// Records when the wrapped adapter's run returns.
struct Timed {
    inner: CommandAdapter,
    ended: Arc<Mutex<Option<Instant>>>,
}

impl Adapter for Timed {
    fn run(
        &self,
        ctx: WakeContext,
        payload: WakePayload,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, AdapterEvent> {
        let run = self.inner.run(ctx, payload, cancel);
        let ended = Arc::clone(&self.ended);
        Box::pin(async move {
            let event = run.await;
            *ended.lock().unwrap() = Some(Instant::now());
            event
        })
    }
}

#[derive(Clone)]
struct Shim {
    core: Arc<Mutex<Option<CoreHandle>>>,
    passed: Arc<Mutex<Option<Instant>>>,
}

/// A stand-in for the task 3.6 HTTP API: forwards `/v1/pass` to the core.
async fn pass(State(shim): State<Shim>, headers: HeaderMap) -> &'static str {
    let token = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_owned();
    *shim.passed.lock().unwrap() = Some(Instant::now());
    let core = shim.core.lock().unwrap().clone().unwrap();
    core.api(ApiRequest::Pass { token }).await;
    "{}"
}

#[tokio::test]
async fn a_process_lingering_after_an_api_pass_is_killed_within_five_seconds() {
    let fixture = fixture();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let shim = Shim {
        core: Arc::default(),
        passed: Arc::default(),
    };
    let ended = Arc::new(Mutex::new(None));
    let (core, _relay, store) = spawn_test_core_with(TestCoreOptions {
        adapter_toml: adapter_toml(&["api-pass-then-sleep"], &fixture.cwd, "api"),
        real_adapter: Some(Arc::new(Timed {
            inner: CommandAdapter::new(fixture.data_dir.clone(), url),
            ended: Arc::clone(&ended),
        })),
        ..TestCoreOptions::default()
    });
    *shim.core.lock().unwrap() = Some(core.clone());
    let app = axum::Router::new()
        .route("/v1/pass", post(pass))
        .with_state(shim.clone());
    tokio::spawn(async move { axum::serve(listener, app).await });

    core.ingest(
        BotName::new("A").unwrap(),
        top_level(&keys("owner"), channel(), "@A hi", base_secs()),
        Source::Live,
    );
    let wake = ended_wake(&store).await;
    assert_eq!(wake.state, WakeState::Passed);

    let started = Instant::now();
    while ended.lock().unwrap().is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "the agent was never killed"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let passed = shim.passed.lock().unwrap().unwrap();
    let lingered = ended.lock().unwrap().unwrap() - passed;
    assert!(lingered <= Duration::from_millis(6_500), "{lingered:?}");
}
