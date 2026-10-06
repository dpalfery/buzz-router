//! The command line (design sections 12 and 14, requirements 2.13 to 2.15, 55.2 to 55.4 and 57).
//!
//! [`main`] parses the command line with clap and hands the command to its handler. Every
//! handler returns `Result<(), CliError>`. On `Err`, `main` writes one JSON line to stderr, nothing
//! to stdout, and exits with the code of the error's kind.
//!
//! Only `roster check` and `route --replay` are built so far. The other commands are in the tree,
//! with their flags, and fail with `CliError::other("not implemented")` until their tasks.

mod replay;
mod roster;

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::error::ErrorKind as ClapErrorKind;
use clap::{ArgGroup, Args, Parser, Subcommand};
use serde::Serialize;

use crate::logging;
use crate::paths::Dirs;

/// What went wrong, as far as the caller's script is concerned (requirement 57.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The input is wrong: a usage error, or a file that is missing or invalid.
    BadInput,
    /// The relay or the daemon could not be reached.
    Network,
    /// Authentication failed.
    Auth,
    /// Anything else.
    Other,
}

impl ErrorKind {
    /// The process exit code: 1, 2, 3 or 4.
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::BadInput => 1,
            Self::Network => 2,
            Self::Auth => 3,
            Self::Other => 4,
        }
    }

    /// The `error` value of the JSON error line.
    pub const fn category(self) -> &'static str {
        match self {
            Self::BadInput => "user_error",
            Self::Network => "network_error",
            Self::Auth => "auth_error",
            Self::Other => "error",
        }
    }

    /// Whether trying again could work: only for a network error.
    pub const fn retryable(self) -> bool {
        matches!(self, Self::Network)
    }
}

/// A failed command: its kind and a message for the operator.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct CliError {
    /// What kind of failure it is. It fixes the exit code and the category.
    pub kind: ErrorKind,
    /// What went wrong, in words.
    pub message: String,
}

impl CliError {
    /// An error of `kind` with `message`.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// An [`ErrorKind::BadInput`] error.
    pub fn bad_input(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::BadInput, message)
    }

    /// An [`ErrorKind::Network`] error.
    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Network, message)
    }

    /// An [`ErrorKind::Auth`] error.
    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Auth, message)
    }

    /// An [`ErrorKind::Other`] error.
    pub fn other(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Other, message)
    }

    /// The error as the line `main` prints on stderr, without the newline:
    /// `{"error":<category>,"message":<message>,"retryable":<bool>}`. JSON escaping keeps it on
    /// one line whatever the message holds.
    fn json_line(&self) -> String {
        /// What `json_line` writes. The field order is the key order.
        #[derive(Serialize)]
        struct Line<'a> {
            error: &'a str,
            message: &'a str,
            retryable: bool,
        }

        let line = Line {
            error: self.kind.category(),
            message: &self.message,
            retryable: self.kind.retryable(),
        };
        // Two strings and a bool always serialise, so the fallback is only for completeness.
        serde_json::to_string(&line).unwrap_or_else(|_| {
            String::from(
                r#"{"error":"error","message":"cannot encode the error","retryable":false}"#,
            )
        })
    }
}

/// The error of a command whose task has not been built yet.
fn not_implemented() -> CliError {
    CliError::other("not implemented")
}

/// The error for a failed write to standard output.
fn write_error(error: io::Error) -> CliError {
    CliError::other(format!("cannot write to standard output: {error}"))
}

/// The router's command line (brief section 13).
#[derive(Debug, Parser)]
#[command(
    name = "buzz-router",
    about = "Route Buzz messages to local agent bots"
)]
struct Cli {
    /// Replaces the config directory. For tests and end-to-end runs (DD-11).
    #[arg(
        long,
        global = true,
        hide = true,
        env = "BUZZ_ROUTER_CONFIG_DIR",
        value_name = "DIR"
    )]
    config_dir: Option<PathBuf>,

    /// Replaces the data directory. For tests and end-to-end runs (DD-11).
    #[arg(
        long,
        global = true,
        hide = true,
        env = "BUZZ_ROUTER_DATA_DIR",
        value_name = "DIR"
    )]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

/// The 14 commands of brief section 13, with the flags of design 12.1.
#[derive(Debug, Subcommand)]
enum Command {
    /// Run the daemon, which is what the service runs
    Run,
    /// Show the bots, halts, wakes, budgets and roster hash of the running router
    Status(StatusArgs),
    /// Halt wakes, for every bot or for the named bots
    Stop(BotsArgs),
    /// Lift a halt, for every bot or for the named bots
    Resume(BotsArgs),
    /// Cancel running wakes, for every bot or for the named bots
    Cancel(BotsArgs),
    /// Post a message as the bot that was woken (for agents)
    Post(PostArgs),
    /// Tell the router the woken bot has nothing to say (for agents)
    Pass,
    /// Tell the router how long the woken bot expects to take (for agents)
    Eta(EtaArgs),
    /// List the wakes recorded in the database
    Wakes(WakesArgs),
    /// Write a channel's recent events to standard output as JSON Lines
    Capture(CaptureArgs),
    /// Replay captured events through the routing rules and print every decision
    Route(RouteArgs),
    /// Manage the bots' signing keys
    Keys {
        #[command(subcommand)]
        command: KeysCommand,
    },
    /// Work with the roster file
    Roster {
        #[command(subcommand)]
        command: RosterCommand,
    },
    /// Install or remove the per-user service
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
}

/// The flags of `status`.
#[derive(Debug, Args)]
struct StatusArgs {
    /// Print the status as JSON
    #[arg(long)]
    json: bool,
}

/// The flags of `stop`, `resume` and `cancel`.
#[derive(Debug, Args)]
struct BotsArgs {
    /// Apply to this bot only. Repeat the flag to name more bots
    #[arg(long = "bot", value_name = "NAME")]
    bots: Vec<String>,
}

/// The flags of `post`: the text comes from exactly one of the two.
#[derive(Debug, Args)]
#[command(group(ArgGroup::new("source").required(true).args(["text", "text_file"])))]
struct PostArgs {
    /// The text to post
    #[arg(long, value_name = "TEXT")]
    text: Option<String>,
    /// Read the text from this file, or from standard input when it is `-`
    #[arg(long, value_name = "PATH")]
    text_file: Option<PathBuf>,
}

/// The flags of `eta`.
#[derive(Debug, Args)]
struct EtaArgs {
    /// The estimate, such as "about 10 minutes"
    #[arg(long, value_name = "TEXT")]
    text: String,
}

/// The flags of `wakes`.
#[derive(Debug, Args)]
struct WakesArgs {
    /// Only this bot's wakes
    #[arg(long, value_name = "NAME")]
    bot: Option<String>,
    /// Only wakes in this state
    #[arg(long, value_name = "STATE")]
    state: Option<String>,
}

/// The flags of `capture`.
#[derive(Debug, Args)]
struct CaptureArgs {
    /// The channel to capture
    #[arg(long, value_name = "ID")]
    channel: String,
    /// How far back to capture: a number and one of m, h or d, such as 7d
    #[arg(long, value_name = "DUR")]
    since: String,
}

/// The flags of `route`.
#[derive(Debug, Args)]
struct RouteArgs {
    /// A JSON Lines file of signed events, as `capture` writes it
    #[arg(long, value_name = "FILE")]
    replay: PathBuf,
    /// The roster to route with, instead of the configured one
    #[arg(long, value_name = "PATH")]
    roster: Option<PathBuf>,
}

/// The subcommands of `keys`.
#[derive(Debug, Subcommand)]
enum KeysCommand {
    /// Read a bot's nsec from standard input into the OS keychain
    Set {
        /// The bot whose key it is
        #[arg(long, value_name = "NAME")]
        bot: String,
    },
    /// Check that every local bot's key can be loaded
    Check,
}

/// The subcommands of `roster`.
#[derive(Debug, Subcommand)]
enum RosterCommand {
    /// Validate the roster and print the SHA-256 of its file
    Check,
}

/// The subcommands of `service`.
#[derive(Debug, Subcommand)]
enum ServiceCommand {
    /// Write and load the service definition
    Install,
    /// Unload and remove the service definition
    Uninstall,
    /// Report whether the service is installed and running
    Status,
}

/// Runs the CLI and returns the process exit code.
///
/// A failure is written to stderr as one JSON line and gives the exit code of its kind
/// (requirement 57). Help goes to stdout with exit code 0.
pub fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Straight to stderr, never through the log. If stderr is gone there is nowhere
            // left to report that.
            let _ = writeln!(io::stderr(), "{}", error.json_line());
            ExitCode::from(error.kind.exit_code())
        }
    }
}

/// Parses the command line, resolves the directories and runs the command.
fn run() -> Result<(), CliError> {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => return usage_outcome(&error),
    };
    logging::init();
    let dirs = Dirs::resolve(cli.config_dir, cli.data_dir)
        .map_err(|error| CliError::other(error.to_string()))?;
    dispatch(cli.command, &dirs)
}

/// What a clap parse error means: help and version are printed to stdout and succeed, and every
/// other error is bad input.
fn usage_outcome(error: &clap::Error) -> Result<(), CliError> {
    match error.kind() {
        ClapErrorKind::DisplayHelp | ClapErrorKind::DisplayVersion => {
            // Printing help can fail only on a closed stdout, and there is nothing to report to.
            let _ = error.print();
            Ok(())
        }
        _ => Err(CliError::bad_input(error.to_string().trim_end())),
    }
}

/// Runs `command`.
fn dispatch(command: Command, dirs: &Dirs) -> Result<(), CliError> {
    match command {
        Command::Roster {
            command: RosterCommand::Check,
        } => roster::check(dirs),
        Command::Route(args) => replay::run(dirs, &args.replay, args.roster.as_deref()),
        Command::Run
        | Command::Status(_)
        | Command::Stop(_)
        | Command::Resume(_)
        | Command::Cancel(_)
        | Command::Post(_)
        | Command::Pass
        | Command::Eta(_)
        | Command::Wakes(_)
        | Command::Capture(_)
        | Command::Keys { .. }
        | Command::Service { .. } => Err(not_implemented()),
    }
}

#[cfg(test)]
mod tests {
    //! Task 1.11 (RED): the exit-code mapping and the JSON error line (design section 14,
    //! acceptance criterion 2, requirement 57), against the shapes of the architect's decision D1.
    //!
    //! | `ErrorKind` | `exit_code()` | `category()` | `retryable()` |
    //! |---|---|---|---|
    //! | `BadInput` | 1 | `user_error` | no |
    //! | `Network` | 2 | `network_error` | yes |
    //! | `Auth` | 3 | `auth_error` | no |
    //! | `Other` | 4 | `error` | no |
    //!
    //! `CliError::json_line` is private and a child module can call it. It returns the stderr line
    //! `{"error":<category>,"message":<message>,"retryable":<bool>}`: three keys, in that order,
    //! compact, on one line and with no trailing newline (`main` adds the newline). `Network` and
    //! `Auth` cannot be reached through the binary before tasks 3.9 and 3.10, so this module is
    //! where all four rows are pinned.
    //!
    //! Run with `cargo test -p buzz-router --lib cli::tests`.

    use serde_json::Value;

    use super::{CliError, ErrorKind};

    /// Each kind with its exit code, its category and whether it is retryable (decision D1).
    const ROWS: [(ErrorKind, u8, &str, bool); 4] = [
        (ErrorKind::BadInput, 1, "user_error", false),
        (ErrorKind::Network, 2, "network_error", true),
        (ErrorKind::Auth, 3, "auth_error", false),
        (ErrorKind::Other, 4, "error", false),
    ];

    #[test]
    fn each_error_kind_maps_to_its_exit_code() {
        for (kind, exit_code, _, _) in ROWS {
            assert_eq!(kind.exit_code(), exit_code, "exit code of {kind:?}");
        }
    }

    #[test]
    fn each_error_kind_maps_to_its_category() {
        for (kind, _, category, _) in ROWS {
            assert_eq!(kind.category(), category, "category of {kind:?}");
        }
    }

    #[test]
    fn only_a_network_error_is_retryable() {
        for (kind, _, _, retryable) in ROWS {
            assert_eq!(kind.retryable(), retryable, "retryable of {kind:?}");
        }
    }

    #[test]
    fn the_json_line_is_a_compact_object_with_the_three_keys_in_order() {
        for (kind, _, category, retryable) in ROWS {
            let line = CliError::new(kind, "relay is down").json_line();

            assert_eq!(
                line,
                format!(
                    r#"{{"error":"{category}","message":"relay is down","retryable":{retryable}}}"#
                ),
                "json_line of {kind:?}"
            );
        }
    }

    #[test]
    fn the_json_line_has_exactly_the_three_keys_with_the_documented_types() {
        let line = CliError::network("relay is down").json_line();

        let value: Value = serde_json::from_str(&line).expect("the line is JSON");
        let fields = value.as_object().expect("the line is a JSON object");

        let mut keys: Vec<&str> = fields.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["error", "message", "retryable"]);
        assert_eq!(fields["error"], "network_error");
        assert_eq!(fields["message"], "relay is down");
        assert_eq!(fields["retryable"], Value::Bool(true));
    }

    #[test]
    fn the_json_line_escapes_the_message_and_stays_on_one_line() {
        let message = "bad \"input\"\nsecond line\\ and a\ttab\r\n";

        let line = CliError::bad_input(message).json_line();

        assert!(
            !line.contains('\n') && !line.contains('\r'),
            "the line must not break: {line:?}"
        );
        let value: Value = serde_json::from_str(&line).expect("the line is JSON");
        let fields = value.as_object().expect("the line is a JSON object");
        assert_eq!(fields["message"], message);
        assert_eq!(fields.len(), 3);
    }

    #[test]
    fn new_keeps_the_kind_and_the_message() {
        for (kind, _, _, _) in ROWS {
            let from_str = CliError::new(kind, "text");
            let from_string = CliError::new(kind, String::from("text"));

            assert_eq!(from_str.kind, kind);
            assert_eq!(from_str.message, "text");
            assert_eq!(from_str, from_string);
        }
    }

    #[test]
    fn each_named_constructor_builds_its_kind() {
        let built = [
            (CliError::bad_input("m"), ErrorKind::BadInput),
            (CliError::network("m"), ErrorKind::Network),
            (CliError::auth("m"), ErrorKind::Auth),
            (CliError::other("m"), ErrorKind::Other),
        ];

        for (error, kind) in built {
            assert_eq!(
                error,
                CliError::new(kind, "m"),
                "the constructor of {kind:?}"
            );
        }
    }

    #[test]
    fn the_display_of_an_error_is_its_message() {
        assert_eq!(
            CliError::other("not implemented").to_string(),
            "not implemented"
        );
    }
}
