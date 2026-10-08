//! `keys set` and `keys check` (design section 11, requirements 54.1 to 54.4, assumption A16).
//!
//! [`set`] and [`check`] use whatever credential store is the process default, so tests can run
//! them against `keyring_core`'s mock store. The command line goes through [`run_set`] and
//! [`run_check`], which install the OS keychain first.

use std::fs;
use std::io::{self, BufRead, Write};

use nostr::nips::nip19::FromBech32;
use router_core::config::parse_router;
use router_core::ids::{BotName, Pubkey};

use super::roster::{load as load_roster, roster_path};
use super::{write_error, CliError};
use crate::keys::{load_key, store_keychain_secret, use_native_store, KeySource};
use crate::paths::Dirs;

/// The router configuration file, in the config directory.
const ROUTER_TOML: &str = "router.toml";

/// Reads one line from `input` and stores it as `bot`'s keychain entry. The line must be a
/// bech32 `nsec`; anything else is bad input and nothing is stored. Loads no configuration.
pub fn set(bot: &BotName, input: &mut dyn BufRead) -> Result<(), CliError> {
    let nsec = read_nsec(input)?;
    store_keychain_secret(bot, &nsec).map_err(|error| CliError::key(error.to_string()))
}

/// `keys set --bot N` from the command line: validates the nsec on stdin before the OS keychain
/// is touched.
pub(super) fn run_set(bot: &str) -> Result<(), CliError> {
    let bot = BotName::new(bot).map_err(|error| CliError::bad_input(error.to_string()))?;
    let nsec = read_nsec(&mut io::stdin().lock())?;
    use_native_store().map_err(|error| CliError::key(error.to_string()))?;
    store_keychain_secret(&bot, &nsec).map_err(|error| CliError::key(error.to_string()))
}

/// The trimmed first line of `input`, if it is a valid bech32 secret key. The error never
/// repeats the input.
fn read_nsec(input: &mut dyn BufRead) -> Result<String, CliError> {
    let mut line = String::new();
    input
        .read_line(&mut line)
        .map_err(|error| CliError::bad_input(format!("cannot read standard input: {error}")))?;
    let nsec = line.trim();
    nostr::SecretKey::from_bech32(nsec)
        .map_err(|_| CliError::bad_input("standard input is not a valid bech32 nsec"))?;
    Ok(nsec.to_owned())
}

/// Checks that every `router.toml` bot's key loads and matches its roster `pubkey`. Writes one
/// line per bot to `out`. Any failure, including an unreadable configuration, is a
/// [`super::ErrorKind::Key`] error.
pub fn check(dirs: &Dirs, out: &mut dyn Write) -> Result<(), CliError> {
    check_with(dirs, out, &|| {})
}

/// `keys check` from the command line, against the OS keychain.
pub(super) fn run_check(dirs: &Dirs) -> Result<(), CliError> {
    // A machine without a usable keychain can still check file keys. Keychain bots then fail
    // one by one, each reporting that no store is available.
    check_with(dirs, &mut io::stdout().lock(), &|| {
        let _ = use_native_store();
    })
}

/// [`check`], calling `before_keychain` once the configuration has loaded and before the first
/// keychain read, only when some bot uses the keychain.
fn check_with(
    dirs: &Dirs,
    out: &mut dyn Write,
    before_keychain: &dyn Fn(),
) -> Result<(), CliError> {
    let key_error = |error: CliError| CliError::key(error.message);
    let (roster, _) =
        load_roster(&roster_path(&dirs.config_dir).map_err(key_error)?).map_err(key_error)?;
    let router_toml = dirs.config_dir.join(ROUTER_TOML);
    let text = fs::read_to_string(&router_toml).map_err(|error| {
        CliError::key(format!("cannot read {}: {error}", router_toml.display()))
    })?;
    let config = parse_router(&text, &roster)
        .map_err(|errors| CliError::key(format!("invalid {}: {errors}", router_toml.display())))?;

    if config
        .bots
        .values()
        .any(|bot| bot.key == router_core::config::KeySource::Keychain)
    {
        before_keychain();
    }
    let mut failed = 0_usize;
    for (name, bot) in &config.bots {
        let source = KeySource::from_config(&bot.key, &dirs.config_dir);
        let outcome = match (load_key(&source, name), roster.bots.get(name)) {
            (Err(error), _) => Err(error.to_string()),
            (Ok(_), None) => Err(String::from("not in the roster")),
            (Ok(keys), Some(entry)) if Pubkey::from_nostr(&keys.public_key()) != entry.pubkey => {
                Err(String::from("the key does not match the roster pubkey"))
            }
            (Ok(_), Some(_)) => Ok(()),
        };
        match outcome {
            Ok(()) => writeln!(out, "{name}: ok"),
            Err(reason) => {
                failed += 1;
                writeln!(out, "{name}: {reason}")
            }
        }
        .map_err(write_error)?;
    }
    if failed == 0 {
        Ok(())
    } else {
        Err(CliError::key(format!(
            "{failed} of {} bot keys failed the check",
            config.bots.len()
        )))
    }
}
