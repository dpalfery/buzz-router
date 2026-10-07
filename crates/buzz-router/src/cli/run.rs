//! `run`: the daemon (design 6.2 steps 1 to 5 and 7, DD-23, DD-24, R1.15, R52.1, R62.6).
//!
//! One process serves every local bot. Startup installs the rustls ring provider and logging,
//! loads and validates the configuration (any error is a JSON line and exit 1, before anything
//! listens), opens the store, ensures `admin.token`, loads each bot's key, then starts the core,
//! the API listeners and one synced relay connection per bot whose key loaded. A bot whose key
//! does not load is reported unavailable and not served. The daemon exits 0 on ctrl-c or SIGTERM.

use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use router_core::config::{
    parse_router, roster_hash, KeySource as ConfigKeySource, Roster, RouterConfig,
};
use router_core::ids::{BotName, Pubkey};
use tokio::sync::mpsc;

use super::roster::{load as load_roster, roster_path};
use super::CliError;
use crate::api::{admin_token, loopback_router, tailnet_router, ApiState};
use crate::clock::SystemClock;
use crate::core::{
    select_adapters, spawn_core, ConnectedProbe, CoreDeps, CoreHandle, StatusSources,
};
use crate::keys::{load_key, use_native_store, KeySource};
use crate::logging;
use crate::paths::Dirs;
use crate::relay::conn::{spawn_synced_connection, ConnParams, Connection, Delivered, SyncParams};
use crate::relay::rest::RestClient;
use crate::relay::{RelayError, RelayPort};
use crate::store::{self, Store};

/// The router configuration file, in the config directory.
const ROUTER_TOML: &str = "router.toml";
/// How long spawned tasks get to finish after a shutdown signal.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// Runs the daemon until ctrl-c or SIGTERM.
pub(super) fn run(dirs: &Dirs) -> Result<(), CliError> {
    // A provider installed already (only possible in-process) is just as good.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let _log_guard = match logging::init_with_data_dir(&dirs.data_dir) {
        Ok(guard) => Some(guard),
        Err(error) => {
            logging::init();
            tracing::warn!(%error, "cannot open the log file; logging to stderr only");
            None
        }
    };
    let loaded = load_config(dirs)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::other(format!("cannot start the async runtime: {error}")))?;
    let result = runtime.block_on(serve(dirs, loaded));
    runtime.shutdown_timeout(SHUTDOWN_GRACE);
    result
}

/// The validated configuration.
struct Loaded {
    roster: Roster,
    /// The hex SHA-256 of the roster file, as `roster check` prints it.
    roster_hash: String,
    config: RouterConfig,
}

/// Loads and validates the roster and `router.toml` (design 6.2 step 2).
fn load_config(dirs: &Dirs) -> Result<Loaded, CliError> {
    let (roster, roster_text) = load_roster(&roster_path(&dirs.config_dir)?)?;
    let path = dirs.config_dir.join(ROUTER_TOML);
    let text = fs::read_to_string(&path)
        .map_err(|error| CliError::bad_input(format!("cannot read {}: {error}", path.display())))?;
    let config = parse_router(&text, &roster)
        .map_err(|errors| CliError::bad_input(format!("invalid {}: {errors}", path.display())))?;
    Ok(Loaded {
        roster,
        roster_hash: roster_hash(roster_text.as_bytes()),
        config,
    })
}

/// Design 6.2 steps 3, 4 and 7, then waits for the shutdown signal.
async fn serve(dirs: &Dirs, loaded: Loaded) -> Result<(), CliError> {
    let Loaded {
        roster,
        roster_hash,
        config,
    } = loaded;
    fs::create_dir_all(&dirs.data_dir).map_err(|error| {
        CliError::other(format!(
            "cannot create the data directory {}: {error}",
            dirs.data_dir.display()
        ))
    })?;
    let db_path = dirs.data_dir.join(store::FILE_NAME);
    let open = |result: Result<Store, store::StoreError>| {
        result
            .map_err(|error| CliError::other(format!("cannot open {}: {error}", db_path.display())))
    };
    let core_store = open(Store::open(&db_path))?;
    let ingest_store = open(Store::open_read_only(&db_path))?;
    let token = admin_token::ensure(&dirs.data_dir)
        .map_err(|error| CliError::other(format!("cannot create the admin token: {error}")))?;

    let keys = load_keys(&config, &roster, &dirs.config_dir);
    match core_store.halts().list() {
        Ok(halts) => {
            for halt in halts {
                tracing::info!(scope = ?halt.scope, "a halt is in force");
            }
        }
        Err(error) => tracing::warn!(%error, "cannot read the halts"),
    }
    let mut served = config.clone();
    served.bots.retain(|name, _| keys.contains_key(name));

    let http = reqwest::Client::new();
    let relays: BTreeMap<BotName, Arc<BotRelay>> = served
        .bots
        .iter()
        .filter_map(|(name, bot)| {
            let keys = keys.get(name)?.clone();
            let rest = RestClient::new(&served.relay_url, keys, auth_tag(bot));
            Some((name.clone(), Arc::new(BotRelay::new(rest))))
        })
        .collect();
    let core = spawn_core(CoreDeps {
        store: core_store,
        ingest_store,
        roster: roster.clone(),
        config: served.clone(),
        clock: Arc::new(SystemClock),
        relays: relays
            .iter()
            .map(|(name, relay)| (name.clone(), relay.clone() as Arc<dyn RelayPort>))
            .collect(),
        keys: keys.clone(),
        memberships: BTreeMap::new(),
        adapters: select_adapters(&served, &dirs.data_dir, &http),
        data_dir: Some(dirs.data_dir.clone()),
        status: StatusSources {
            roster_hash,
            unavailable: config
                .bots
                .keys()
                .filter(|name| !served.bots.contains_key(*name))
                .cloned()
                .collect(),
            connected: relays
                .iter()
                .map(|(name, relay)| {
                    let relay = relay.clone();
                    let probe: ConnectedProbe = Arc::new(move || {
                        relay.connection.get().is_some_and(Connection::is_connected)
                    });
                    (name.clone(), probe)
                })
                .collect(),
        },
    });

    let state = ApiState::new(core.clone(), token, &roster);
    listen(config.api_bind, loopback_router(state.clone())).await?;
    if let Some(bind) = config.tailnet_bind {
        listen(bind, tailnet_router(state)).await?;
    }

    for (name, relay) in &relays {
        let (Some(bot), Some(keys)) = (served.bots.get(name), keys.get(name)) else {
            continue;
        };
        let connection = connect(
            &served.relay_url,
            name,
            keys.clone(),
            auth_tag(bot),
            open(Store::open_read_only(&db_path))?,
            &core,
        );
        let _ = relay.connection.set(connection);
    }
    tracing::info!(bots = relays.len(), "buzz-router is running");

    shutdown_signal().await;
    tracing::info!("shutting down");
    core.shutdown();
    Ok(())
}

/// Each configured bot's keys, leaving out (and reporting unavailable) every bot whose key does
/// not load or does not match its roster pubkey (DD-23).
fn load_keys(
    config: &RouterConfig,
    roster: &Roster,
    config_dir: &Path,
) -> BTreeMap<BotName, nostr::Keys> {
    if config
        .bots
        .values()
        .any(|bot| bot.key == ConfigKeySource::Keychain)
    {
        if let Err(error) = use_native_store() {
            tracing::warn!(%error, "no usable OS keychain; keychain bots are unavailable");
        }
    }
    config
        .bots
        .iter()
        .filter_map(|(name, bot)| {
            let source = KeySource::from_config(&bot.key, config_dir);
            let loaded = load_key(&source, name).map_err(|error| error.to_string());
            let checked = loaded.and_then(|keys| match roster.bots.get(name) {
                Some(entry) if Pubkey::from_nostr(&keys.public_key()) == entry.pubkey => Ok(keys),
                Some(_) => Err("the key does not match the roster pubkey".to_owned()),
                None => Err("the bot is not in the roster".to_owned()),
            });
            match checked {
                Ok(keys) => Some((name.clone(), keys)),
                Err(error) => {
                    tracing::warn!(bot = %name, %error, "bot {name} is unavailable: its key did not load");
                    None
                }
            }
        })
        .collect()
}

/// The configured NIP-OA tag as the JSON array the relay expects.
fn auth_tag(bot: &router_core::config::RouterBot) -> Option<String> {
    bot.auth_tag
        .as_ref()
        .and_then(|tag| serde_json::to_string(tag.as_slice()).ok())
}

/// Binds `address` and serves `router` on it in the background.
async fn listen(address: std::net::SocketAddr, router: axum::Router) -> Result<(), CliError> {
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| CliError::other(format!("cannot listen on {address}: {error}")))?;
    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, router).await {
            tracing::warn!(%error, %address, "the API listener stopped");
        }
    });
    Ok(())
}

/// Starts `bot`'s synced relay connection and forwards what it delivers to the core.
fn connect(
    relay_url: &str,
    bot: &BotName,
    keys: nostr::Keys,
    auth_tag: Option<String>,
    store: Store,
    core: &CoreHandle,
) -> Connection {
    let (sink, mut delivered) = mpsc::unbounded_channel::<Delivered>();
    let forward_core = core.clone();
    let forward_bot = bot.clone();
    tokio::spawn(async move {
        while let Some(event) = delivered.recv().await {
            forward_core.ingest(forward_bot.clone(), event.event, event.source);
        }
    });
    let rest = RestClient::new(relay_url, keys.clone(), auth_tag.clone());
    spawn_synced_connection(SyncParams {
        conn: ConnParams::new(relay_url.to_owned(), keys, auth_tag),
        rest,
        store,
        bot: bot.clone(),
        sink,
        core: core.clone(),
        membership_tap: None,
    })
}

/// Resolves on ctrl-c, or on SIGTERM on Unix.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let ctrl_c = async {
            if tokio::signal::ctrl_c().await.is_err() {
                std::future::pending::<()>().await;
            }
        };
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    () = ctrl_c => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(error) => {
                tracing::warn!(%error, "cannot listen for SIGTERM; only ctrl-c stops the daemon");
                ctrl_c.await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "cannot listen for ctrl-c");
            std::future::pending::<()>().await;
        }
    }
}

/// A bot's [`RelayPort`]: publishes over its WebSocket connection, falling back once to REST
/// (DD-7), and queries over REST. The connection is set once it has been started, after the core
/// it reports to.
struct BotRelay {
    connection: OnceLock<Connection>,
    rest: RestClient,
}

impl BotRelay {
    fn new(rest: RestClient) -> Self {
        Self {
            connection: OnceLock::new(),
            rest,
        }
    }
}

impl RelayPort for BotRelay {
    fn publish(
        &self,
        event: nostr::Event,
    ) -> Pin<Box<dyn Future<Output = Result<(), RelayError>> + Send + '_>> {
        Box::pin(async move {
            let socket = match self.connection.get() {
                Some(connection) => connection.publish(event.clone()).await,
                None => Err(RelayError::Transport(
                    "the relay connection has not started".to_owned(),
                )),
            };
            match socket {
                Ok(()) => Ok(()),
                // Ephemeral events such as typing indicators go over the WebSocket only.
                Err(error) if event.kind.is_ephemeral() => Err(error),
                Err(error) => {
                    tracing::debug!(%error, "WebSocket publish failed; trying REST");
                    self.rest.submit_event(&event).await
                }
            }
        })
    }

    fn query(
        &self,
        filters: Vec<nostr::Filter>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<nostr::Event>, RelayError>> + Send + '_>> {
        Box::pin(self.rest.query(filters))
    }
}
