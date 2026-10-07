//! The command adapter: runs a local agent as a killable process group (design 7.1, R37).
//!
//! Each wake gets `data_dir/wakes/<id>/` holding `payload.json`, `prompt.txt` (file mode) and
//! `pid`, all private to the operator and deleted when the run ends (DD-21). The child runs in
//! its own process group (Unix) or Job Object (Windows), so cancelling kills everything it
//! started. Its stdout is captured up to 64 KiB and drained past that, and its stderr goes to the
//! log line by line.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use command_group::AsyncCommandGroup;
use futures_util::future::BoxFuture;
use router_core::config::{AdapterConfig, PromptMode, ReplyMode};
use router_core::payload::WakePayload;
use router_core::prompt::{reason_text, render, render_context, PromptVars, BUILT_IN_TEMPLATE};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

use super::{Adapter, AdapterEvent, WakeContext};

/// At most this much stdout is kept (R37.6).
const STDOUT_CAP: usize = 65_536;
/// How long to wait for the stdout pipe to close after the child exits, in case a descendant
/// still holds it.
const STDOUT_GRACE: Duration = Duration::from_secs(2);
/// The variable no agent may receive (R37.3, R59.4).
const PRIVATE_KEY_VAR: &str = "BUZZ_PRIVATE_KEY";

/// Why a command could not be run.
#[derive(Debug, thiserror::Error)]
enum CommandError {
    /// The bot is not configured with a command adapter.
    #[error("the bot's adapter is not a command")]
    NotCommand,
    /// The configured command is empty.
    #[error("the configured command is empty")]
    EmptyCommand,
    /// A file or process operation failed.
    #[error("{what}: {source}")]
    Io {
        what: &'static str,
        source: std::io::Error,
    },
    /// The payload could not be serialised.
    #[error("cannot serialise the payload: {0}")]
    Payload(#[from] serde_json::Error),
}

fn io(what: &'static str) -> impl FnOnce(std::io::Error) -> CommandError {
    move |source| CommandError::Io { what, source }
}

/// Runs each wake's configured command (design 7.1).
#[derive(Debug, Clone)]
pub struct CommandAdapter {
    data_dir: PathBuf,
    api_url: String,
}

impl CommandAdapter {
    /// An adapter keeping wake files under `data_dir/wakes/` and pointing agents at the loopback
    /// API base `api_url`.
    pub fn new(data_dir: PathBuf, api_url: String) -> Self {
        Self { data_dir, api_url }
    }
}

impl Adapter for CommandAdapter {
    fn run(
        &self,
        ctx: WakeContext,
        payload: WakePayload,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, AdapterEvent> {
        let dir = self.data_dir.join("wakes").join(ctx.wake_id.to_string());
        let api_url = self.api_url.clone();
        Box::pin(async move {
            let event = run_command(&dir, &api_url, &ctx, &payload, cancel)
                .await
                .unwrap_or_else(|error| AdapterEvent::Failed(error.to_string()));
            if let Err(error) = std::fs::remove_dir_all(&dir) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(%error, wake_id = %ctx.wake_id, "cannot delete the wake directory");
                }
            }
            event
        })
    }
}

/// The command's settings, from [`AdapterConfig::Command`].
struct Settings<'a> {
    command: &'a [String],
    cwd: &'a str,
    env: &'a BTreeMap<String, String>,
    prompt_mode: PromptMode,
    reply_mode: ReplyMode,
    prompt_template: Option<&'a Path>,
}

impl<'a> Settings<'a> {
    fn of(config: &'a AdapterConfig) -> Result<Self, CommandError> {
        match config {
            AdapterConfig::Command {
                command,
                cwd,
                env,
                prompt_mode,
                reply_mode,
                prompt_template,
            } => Ok(Self {
                command,
                cwd,
                env,
                prompt_mode: *prompt_mode,
                reply_mode: *reply_mode,
                prompt_template: prompt_template.as_deref(),
            }),
            AdapterConfig::Webhook { .. } => Err(CommandError::NotCommand),
        }
    }
}

/// Design 7.1, steps 1 to 7.
async fn run_command(
    dir: &Path,
    api_url: &str,
    ctx: &WakeContext,
    payload: &WakePayload,
    cancel: CancellationToken,
) -> Result<AdapterEvent, CommandError> {
    let settings = Settings::of(&ctx.adapter)?;
    let (program, args) = settings
        .command
        .split_first()
        .ok_or(CommandError::EmptyCommand)?;
    std::fs::create_dir_all(dir).map_err(io("cannot create the wake directory"))?;
    let payload_path = dir.join("payload.json");
    write_private(&payload_path, &serde_json::to_vec(payload)?)
        .map_err(io("cannot write the payload file"))?;
    let prompt = render_prompt(settings.prompt_template, ctx, payload)?;

    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .envs(settings.env)
        .env("BUZZ_ROUTER_URL", api_url)
        .env("BUZZ_ROUTER_WAKE_TOKEN", &ctx.token)
        .env("BUZZ_ROUTER_PAYLOAD", &payload_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !settings.cwd.is_empty() {
        command.current_dir(expand_home(settings.cwd));
    }
    match settings.prompt_mode {
        PromptMode::Stdin => {
            command.stdin(Stdio::piped());
        }
        PromptMode::File => {
            let prompt_path = dir.join("prompt.txt");
            write_private(&prompt_path, prompt.as_bytes())
                .map_err(io("cannot write the prompt file"))?;
            command
                .env("BUZZ_ROUTER_PROMPT_FILE", prompt_path)
                .stdin(Stdio::null());
        }
    }
    command.env_remove(PRIVATE_KEY_VAR);

    let mut child = command
        .group_spawn()
        .map_err(io("cannot start the command"))?;
    if let Some(pid) = child.id() {
        if let Err(error) = write_private(&dir.join("pid"), format!("{pid}\n").as_bytes()) {
            if let Err(error) = child.kill().await {
                tracing::warn!(%error, wake_id = %ctx.wake_id, "cannot kill the command's process group");
            }
            return Err(io("cannot write the pid file")(error));
        }
    }
    let inner = child.inner();
    if let Some(mut stdin) = inner.stdin.take() {
        tokio::spawn(async move {
            // A child that never reads its stdin is not an error.
            let _ = stdin.write_all(prompt.as_bytes()).await;
        });
    }
    let stdout = inner
        .stdout
        .take()
        .map(|out| tokio::spawn(read_capped(out)));
    let stderr = inner
        .stderr
        .take()
        .map(|err| tokio::spawn(log_lines(err, ctx.bot.to_string(), ctx.wake_id)));

    let exited = tokio::select! {
        status = child.wait() => Some(status),
        () = cancel.cancelled() => None,
    };
    let Some(status) = exited else {
        if let Err(error) = child.kill().await {
            tracing::warn!(%error, wake_id = %ctx.wake_id, "cannot kill the command's process group");
        }
        return Ok(AdapterEvent::Failed("cancelled".to_owned()));
    };
    let status = status.map_err(io("cannot wait for the command"))?;
    let captured = match stdout {
        Some(task) => tokio::time::timeout(STDOUT_GRACE, task)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default(),
        None => String::new(),
    };
    if let Some(task) = stderr {
        let _ = tokio::time::timeout(STDOUT_GRACE, task).await;
    }
    let stdout = match settings.reply_mode {
        ReplyMode::Stdout => Some(captured.trim().to_owned()),
        ReplyMode::Api => None,
    };
    Ok(AdapterEvent::Exited {
        code: status.code(),
        stdout,
    })
}

/// The prompt from `prompt_template` (read at wake time) or the built-in template (R37.4).
fn render_prompt(
    template: Option<&Path>,
    ctx: &WakeContext,
    payload: &WakePayload,
) -> Result<String, CommandError> {
    let custom = match template {
        Some(path) => Some(
            std::fs::read_to_string(expand_home(&path.to_string_lossy()))
                .map_err(io("cannot read the prompt template"))?,
        ),
        None => None,
    };
    let vars = PromptVars {
        bot: payload.bot.clone(),
        channel: payload.channel.name.clone(),
        reason_text: reason_text(payload.reason, &ctx.reason_author),
        turn: payload
            .turns_per_round
            .saturating_sub(payload.turns_left_after_this),
        turns_per_round: payload.turns_per_round,
        context: render_context(&payload.context),
    };
    Ok(render(
        custom.as_deref().unwrap_or(BUILT_IN_TEMPLATE),
        &vars,
    ))
}

/// Reads all of `out`, keeping the first [`STDOUT_CAP`] bytes and discarding the rest, so the
/// child never blocks on a full pipe (R37.6).
async fn read_capped(mut out: impl AsyncRead + Unpin) -> String {
    let mut kept = Vec::new();
    let mut chunk = [0_u8; 8_192];
    loop {
        match out.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = STDOUT_CAP.saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..n.min(room)]);
            }
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
}

/// Forwards each stderr line to the log (R37.10).
async fn log_lines(err: impl AsyncRead + Unpin, bot: String, wake_id: uuid::Uuid) {
    let mut lines = BufReader::new(err).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::info!(bot = %bot, wake_id = %wake_id, line = %line, "agent stderr");
    }
}

/// `path` with a leading `~` replaced by the operator's home directory (DD-19).
fn expand_home(path: &str) -> PathBuf {
    let rest = if path == "~" {
        Some("")
    } else {
        path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
    };
    match (rest, directories::BaseDirs::new()) {
        (Some(rest), Some(dirs)) => dirs.home_dir().join(rest),
        _ => PathBuf::from(path),
    }
}

/// Creates `path` with `bytes`, readable only by the operator on Unix (DD-21).
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_leading_tilde_is_expanded() {
        let home = directories::BaseDirs::new()
            .unwrap()
            .home_dir()
            .to_path_buf();
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("~/work"), home.join("work"));
        assert_eq!(expand_home("/srv/~x"), PathBuf::from("/srv/~x"));
        assert_eq!(expand_home("~other"), PathBuf::from("~other"));
    }
}
