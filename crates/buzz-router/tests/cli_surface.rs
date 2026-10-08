//! Task 1.11 (RED): the command surface of the `buzz-router` binary.
//!
//! Covers acceptance criterion 1 and requirement 62.2 against brief section 13 and design 12.1:
//!
//! - `--help` lists exactly the 14 subcommands `run`, `status`, `stop`, `resume`, `cancel`,
//!   `post`, `pass`, `eta`, `wakes`, `capture`, `route`, `keys`, `roster` and `service`
//!   (`all_fourteen_subcommands_and_no_others_are_listed`). clap adds its own `help` entry to the
//!   list, which is not a command of ours, so it is left out of the comparison.
//! - The three commands with their own subcommands list them: `keys set|check`, `roster check`,
//!   `service install|uninstall|status` (`nested_subcommands_are_listed`).
//! - Each subcommand's help lists the flags design 12.1 gives it
//!   (`each_subcommand_help_lists_its_flags`).
//! - There is no shadow or dry-run mode (R62.2): no help text names one
//!   (`no_help_text_offers_a_shadow_or_dry_run_mode`).
//! - The global flags `--config-dir` and `--data-dir`, and their `BUZZ_ROUTER_CONFIG_DIR` and
//!   `BUZZ_ROUTER_DATA_DIR` variables, are hidden (`the_global_dir_flags_are_hidden_from_help`).
//!   That they work is tested where they have an effect, in `cli_roster_check.rs`.
//!
//! The tests read clap's default help layout: a `Commands:` heading, then one entry per
//! subcommand, indented two spaces, with the description after the name. They need nothing but
//! the standard library.

#![allow(
    clippy::expect_used,
    reason = "helpers in a test file fail the test by panicking; clippy.toml exempts only #[test] functions"
)]

use std::collections::BTreeSet;
use std::process::{Command, Stdio};

/// The 14 subcommands of brief section 13.
const SUBCOMMANDS: [&str; 14] = [
    "run", "status", "stop", "resume", "cancel", "post", "pass", "eta", "wakes", "capture",
    "route", "keys", "roster", "service",
];

/// The commands that have subcommands of their own, with those subcommands (design 12.1).
const NESTED: [(&str, &[&str]); 3] = [
    ("keys", &["set", "check"]),
    ("roster", &["check"]),
    ("service", &["install", "uninstall", "status"]),
];

/// A word that clap adds to every list of subcommands. It is not one of ours.
const CLAP_HELP_ENTRY: &str = "help";

/// The flags design 12.1 gives each command, as the words that follow it on the command line.
const FLAGS: [(&[&str], &[&str]); 9] = [
    (&["status"], &["--json"]),
    (&["stop"], &["--bot"]),
    (&["resume"], &["--bot"]),
    (&["cancel"], &["--bot"]),
    (&["post"], &["--text", "--text-file"]),
    (&["eta"], &["--text"]),
    (&["wakes"], &["--bot", "--state"]),
    (&["capture"], &["--channel", "--since"]),
    (&["route"], &["--replay", "--roster"]),
];

/// The help text of `buzz-router <words> --help`, asserting that it exits 0.
fn help(words: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_buzz-router"))
        .args(words)
        .arg("--help")
        .stdin(Stdio::null())
        .output()
        .expect("the binary starts");
    assert_eq!(
        output.status.code(),
        Some(0),
        "`buzz-router {} --help` should exit 0, stderr: {}",
        words.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The names in the `Commands:` section of a clap help text, without clap's own `help` entry.
fn listed_commands(help_text: &str) -> BTreeSet<String> {
    help_text
        .lines()
        .skip_while(|line| line.trim_end() != "Commands:")
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter(|line| line.starts_with("  ") && !line.starts_with("   "))
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| *name != CLAP_HELP_ENTRY)
        .map(str::to_owned)
        .collect()
}

/// The names as a set, for comparing with [`listed_commands`].
fn names(words: &[&str]) -> BTreeSet<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

/// Whether `help_text` lists `flag` as a flag of its own: `--text` is not found in `--text-file`.
fn lists_flag(help_text: &str, flag: &str) -> bool {
    help_text.match_indices(flag).any(|(start, _)| {
        help_text[start + flag.len()..]
            .chars()
            .next()
            .is_none_or(|next| !(next.is_ascii_alphanumeric() || next == '-' || next == '_'))
    })
}

#[test]
fn all_fourteen_subcommands_and_no_others_are_listed() {
    let text = help(&[]);

    assert_eq!(
        listed_commands(&text),
        names(&SUBCOMMANDS),
        "the Commands section of `buzz-router --help` was:\n{text}"
    );
}

#[test]
fn nested_subcommands_are_listed() {
    for (command, expected) in NESTED {
        let text = help(&[command]);

        assert_eq!(
            listed_commands(&text),
            names(expected),
            "the Commands section of `buzz-router {command} --help` was:\n{text}"
        );
    }
}

#[test]
fn each_subcommand_help_lists_its_flags() {
    let mut missing = Vec::new();
    for (words, flags) in FLAGS {
        let text = help(words);
        for flag in flags {
            if !lists_flag(&text, flag) {
                missing.push(format!(
                    "`buzz-router {} --help` lacks {flag}",
                    words.join(" ")
                ));
            }
        }
    }
    let keys_set = help(&["keys", "set"]);
    if !lists_flag(&keys_set, "--bot") {
        missing.push("`buzz-router keys set --help` lacks --bot".to_owned());
    }

    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

#[test]
fn no_help_text_offers_a_shadow_or_dry_run_mode() {
    let top = help(&[]);
    assert!(
        top.contains("route"),
        "the top-level help should name the `route` command, so an empty text cannot pass:\n{top}"
    );
    let mut texts = vec![("buzz-router".to_owned(), top)];
    for command in SUBCOMMANDS {
        texts.push((format!("buzz-router {command}"), help(&[command])));
    }
    for (command, subcommands) in NESTED {
        for subcommand in subcommands {
            texts.push((
                format!("buzz-router {command} {subcommand}"),
                help(&[command, subcommand]),
            ));
        }
    }

    let offenders: Vec<String> = texts
        .into_iter()
        .filter(|(_, text)| {
            let lower = text.to_ascii_lowercase();
            ["shadow", "dry-run", "dry run", "dry_run", "dryrun"]
                .iter()
                .any(|word| lower.contains(word))
        })
        .map(|(command, _)| command)
        .collect();

    assert!(
        offenders.is_empty(),
        "these help texts name a shadow or dry-run mode: {offenders:?}"
    );
}

#[test]
fn the_global_dir_flags_are_hidden_from_help() {
    let commands: [&[&str]; 3] = [&[], &["roster", "check"], &["route"]];
    let mut offenders = Vec::new();
    for words in commands {
        let text = help(words);
        assert!(
            text.contains("--help"),
            "`buzz-router {} --help` should list at least --help, so an empty text cannot pass:\n{text}",
            words.join(" ")
        );
        for hidden in [
            "--config-dir",
            "--data-dir",
            "BUZZ_ROUTER_CONFIG_DIR",
            "BUZZ_ROUTER_DATA_DIR",
        ] {
            if text.contains(hidden) {
                offenders.push(format!(
                    "`buzz-router {} --help` shows {hidden}",
                    words.join(" ")
                ));
            }
        }
    }

    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
}
