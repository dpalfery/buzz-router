//! Where the router keeps its files (design section 4.1, requirements 3.1 to 3.4, DD-11).
//!
//! [`Dirs::resolve`] names the config directory and the data directory with the `directories`
//! crate. It only works out the two paths: it creates nothing and reads nothing.

use std::path::PathBuf;

use directories::BaseDirs;

/// The directory name under the platform's config and data locations.
const APP_DIR: &str = "buzz-router";

/// The router's two directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    /// Holds `roster.toml` and `router.toml`: `~/Library/Application Support/buzz-router` on
    /// macOS, `~/.config/buzz-router` on Linux and `%APPDATA%\buzz-router` on Windows.
    pub config_dir: PathBuf,
    /// Holds the database, `admin.token` and the logs: `~/Library/Application Support/buzz-router`
    /// on macOS, `~/.local/share/buzz-router` on Linux and `%LOCALAPPDATA%\buzz-router` on Windows.
    pub data_dir: PathBuf,
}

/// Why the directories could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathsError {
    /// A default is needed and the platform has no home directory.
    #[error("cannot determine the home directory")]
    NoHomeDir,
}

impl Dirs {
    /// Resolves the config and data directories.
    ///
    /// An override is the directory itself, used exactly as given: it is not joined with
    /// `buzz-router`, made absolute, cleaned or expanded. An empty path counts as no override.
    /// The platform's home directory is looked up only when an override is missing.
    pub fn resolve(
        config_override: Option<PathBuf>,
        data_override: Option<PathBuf>,
    ) -> Result<Self, PathsError> {
        let config_override = config_override.filter(|path| !path.as_os_str().is_empty());
        let data_override = data_override.filter(|path| !path.as_os_str().is_empty());
        let (config_dir, data_dir) = match (config_override, data_override) {
            (Some(config_dir), Some(data_dir)) => (config_dir, data_dir),
            (config_override, data_override) => {
                let base = BaseDirs::new().ok_or(PathsError::NoHomeDir)?;
                (
                    config_override.unwrap_or_else(|| base.config_dir().join(APP_DIR)),
                    data_override.unwrap_or_else(|| base.data_local_dir().join(APP_DIR)),
                )
            }
        };
        Ok(Self {
            config_dir,
            data_dir,
        })
    }
}

#[cfg(test)]
mod tests {
    //! Task 1.11 (RED): config and data directory resolution (design section 4.1, requirements
    //! 3.1 to 3.4, DD-11).
    //!
    //! The interface list and the architect's decision D3 fix `paths::Dirs { config_dir, data_dir }`
    //! (two `PathBuf`s; `Debug`, `Clone`, `PartialEq`, `Eq`) and `Dirs::resolve(config_override:
    //! Option<PathBuf>, data_override: Option<PathBuf>) -> Result<Dirs, PathsError>`. The tests need
    //! nothing from `PathsError` but `Debug`.
    //!
    //! - The default config directory ends with `Library/Application Support/buzz-router` on
    //!   macOS, `.config/buzz-router` on Linux and `AppData\Roaming\buzz-router` on Windows (the
    //!   task's Behaviour bullet, requirements 3.1 to 3.3). One test per OS, gated with
    //!   `cfg(target_os)`.
    //! - The default data directory (design 4.1 and DD-11, beyond the Behaviour bullet) ends with
    //!   `Library/Application Support/buzz-router` on macOS, `.local/share/buzz-router` on Linux
    //!   and `AppData\Local\buzz-router` on Windows.
    //! - An override wins: the directory it names is the directory, used as given and not joined
    //!   with `buzz-router` (the hidden `--config-dir` and `--data-dir` flags replace the whole
    //!   directory, design 4.1). The two overrides are independent.
    //! - Decision D3 adds three rules, each tested: an override is used exactly as given (no
    //!   canonicalising, no `~` expansion), an empty override counts as no override, and `resolve`
    //!   does no file-system I/O.
    //! - Not tested, because a unit test cannot observe it: `NoHomeDir` when the platform has no
    //!   home directory, and that `BaseDirs::new()` is not called when both overrides are given.
    //!
    //! The Linux defaults assume `XDG_CONFIG_HOME` and `XDG_DATA_HOME` are unset or end in
    //! `.config` and `.local/share`, as on a stock runner. The override tests name paths that do
    //! not exist and do not create them.

    use std::path::PathBuf;

    use super::Dirs;

    /// The directories that `resolve` gives when nothing overrides them.
    fn defaults() -> Dirs {
        Dirs::resolve(None, None).expect("the platform has a home directory")
    }

    /// A path that no test creates, so a resolver that touches the file system still returns it.
    fn unused_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join("buzz-router-paths-tests")
            .join(name)
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn default_config_dir_ends_with_library_application_support_buzz_router_on_macos() {
        assert!(
            defaults()
                .config_dir
                .ends_with("Library/Application Support/buzz-router"),
            "config_dir was {:?}",
            defaults().config_dir
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn default_config_dir_ends_with_dot_config_buzz_router_on_linux() {
        assert!(
            defaults().config_dir.ends_with(".config/buzz-router"),
            "config_dir was {:?}",
            defaults().config_dir
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn default_config_dir_ends_with_appdata_roaming_buzz_router_on_windows() {
        assert!(
            defaults()
                .config_dir
                .ends_with(r"AppData\Roaming\buzz-router"),
            "config_dir was {:?}",
            defaults().config_dir
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn default_data_dir_ends_with_library_application_support_buzz_router_on_macos() {
        assert!(
            defaults()
                .data_dir
                .ends_with("Library/Application Support/buzz-router"),
            "data_dir was {:?}",
            defaults().data_dir
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn default_data_dir_ends_with_dot_local_share_buzz_router_on_linux() {
        assert!(
            defaults().data_dir.ends_with(".local/share/buzz-router"),
            "data_dir was {:?}",
            defaults().data_dir
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn default_data_dir_ends_with_appdata_local_buzz_router_on_windows() {
        assert!(
            defaults().data_dir.ends_with(r"AppData\Local\buzz-router"),
            "data_dir was {:?}",
            defaults().data_dir
        );
    }

    #[test]
    fn overrides_win_over_the_defaults_and_are_used_as_given() {
        let config = unused_path("config");
        let data = unused_path("data");

        let dirs = Dirs::resolve(Some(config.clone()), Some(data.clone()))
            .expect("both directories are overridden");

        assert_eq!(dirs.config_dir, config);
        assert_eq!(dirs.data_dir, data);
    }

    #[test]
    fn a_config_override_leaves_the_data_dir_at_its_default() {
        let config = unused_path("config-only");

        let dirs = Dirs::resolve(Some(config.clone()), None).expect("the config dir is overridden");

        assert_eq!(dirs.config_dir, config);
        assert_eq!(dirs.data_dir, defaults().data_dir);
    }

    #[test]
    fn a_data_override_leaves_the_config_dir_at_its_default() {
        let data = unused_path("data-only");

        let dirs = Dirs::resolve(None, Some(data.clone())).expect("the data dir is overridden");

        assert_eq!(dirs.config_dir, defaults().config_dir);
        assert_eq!(dirs.data_dir, data);
    }

    #[test]
    fn an_override_is_used_exactly_as_given() {
        // Each text is changed by one thing the rule rules out: canonicalising or cleaning the path
        // (the `.` and `..` components, the trailing separator), making it absolute (the relative
        // paths) and expanding a leading tilde. The comparison is on the raw text, because `Path`
        // equality ignores a `.` component and a trailing separator.
        let awkward = [
            "relative/./dir/../dir",
            "./here",
            "trailing/slash/",
            "~/buzz-router-data",
            "~/./x/../y",
        ];

        for text in awkward {
            let given = PathBuf::from(text);

            let dirs = Dirs::resolve(Some(given.clone()), Some(given.clone()))
                .expect("both directories are overridden");

            assert_eq!(
                dirs.config_dir.as_os_str(),
                given.as_os_str(),
                "config override {text:?}"
            );
            assert_eq!(
                dirs.data_dir.as_os_str(),
                given.as_os_str(),
                "data override {text:?}"
            );
        }
    }

    #[test]
    fn an_empty_override_counts_as_no_override() {
        let both = Dirs::resolve(Some(PathBuf::new()), Some(PathBuf::new()))
            .expect("empty overrides fall back to the defaults");
        let config_only = Dirs::resolve(Some(PathBuf::new()), Some(unused_path("data-kept")))
            .expect("an empty config override falls back to its default");
        let data_only = Dirs::resolve(Some(unused_path("config-kept")), Some(PathBuf::new()))
            .expect("an empty data override falls back to its default");

        assert_eq!(both, defaults());
        assert_eq!(config_only.config_dir, defaults().config_dir);
        assert_eq!(config_only.data_dir, unused_path("data-kept"));
        assert_eq!(data_only.config_dir, unused_path("config-kept"));
        assert_eq!(data_only.data_dir, defaults().data_dir);
    }

    #[test]
    fn resolve_does_no_file_system_io() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let config = root.path().join("config");
        let data = root.path().join("data");

        let dirs = Dirs::resolve(Some(config.clone()), Some(data.clone()))
            .expect("both directories are overridden");

        assert_eq!(dirs.config_dir, config);
        assert_eq!(dirs.data_dir, data);
        assert!(!config.exists(), "resolve must not create the config dir");
        assert!(!data.exists(), "resolve must not create the data dir");
        assert_eq!(
            std::fs::read_dir(root.path())
                .expect("the temporary directory is readable")
                .count(),
            0,
            "resolve must not create anything"
        );
    }
}
