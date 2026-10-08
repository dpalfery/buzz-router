//! Path-aware reading of a parsed TOML table.
//!
//! `toml::de::Error` carries no public key path and serde stops at the first failure, so neither
//! can report every problem in a file or say where each one is. [`TableReader`] reads one table
//! at a time instead. Each key is decoded on its own, a failure is recorded as a
//! [`ConfigIssue`] at that key's path, and reading goes on. Keys that nothing read are unknown
//! keys, which is how `deny_unknown_fields` (DD-19) is enforced here.

use std::collections::BTreeSet;

use serde::de::DeserializeOwned;
use toml::{Table, Value};

use super::ConfigIssue;

/// The issues found so far while reading and validating one file.
#[derive(Debug, Default)]
pub(super) struct Issues(Vec<ConfigIssue>);

impl Issues {
    /// Records an issue at `path`.
    pub(super) fn add(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.0.push(ConfigIssue {
            path: path.into(),
            message: message.into(),
        });
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn into_vec(self) -> Vec<ConfigIssue> {
        self.0
    }
}

/// The path of `key` inside the table at `prefix` (`""` is the file's root).
pub(super) fn key_path(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_owned()
    } else {
        format!("{prefix}.{key}")
    }
}

/// The path of element `index` of the array at `prefix`.
pub(super) fn index_path(prefix: &str, index: usize) -> String {
    format!("{prefix}[{index}]")
}

/// Whether a missing key is an issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Presence {
    Required,
    Optional,
}

/// Reads the keys of one TOML table and remembers which ones it was asked for.
pub(super) struct TableReader<'a> {
    table: &'a Table,
    path: String,
    read: BTreeSet<String>,
}

impl<'a> TableReader<'a> {
    /// A reader for `table`, which sits at `path` in the file (`""` for the root).
    pub(super) fn new(table: &'a Table, path: impl Into<String>) -> Self {
        Self {
            table,
            path: path.into(),
            read: BTreeSet::new(),
        }
    }

    /// The path of `key` in this table.
    pub(super) fn path_of(&self, key: &str) -> String {
        key_path(&self.path, key)
    }

    /// Looks `key` up and marks it as read.
    fn lookup(&mut self, key: &str, presence: Presence, issues: &mut Issues) -> Option<&'a Value> {
        self.read.insert(key.to_owned());
        let value = self.table.get(key);
        if value.is_none() && presence == Presence::Required {
            issues.add(self.path_of(key), "required key is missing");
        }
        value
    }

    /// Decodes a required key. A missing key or a value of the wrong type or outside the allowed
    /// set is an issue at the key's path, and the result is `None`.
    pub(super) fn required<T: DeserializeOwned>(
        &mut self,
        key: &str,
        issues: &mut Issues,
    ) -> Option<T> {
        let value = self.lookup(key, Presence::Required, issues)?;
        decode(value, self.path_of(key), issues)
    }

    /// Decodes an optional key. `None` means the key is absent, or present and wrong (an issue).
    pub(super) fn optional<T: DeserializeOwned>(
        &mut self,
        key: &str,
        issues: &mut Issues,
    ) -> Option<T> {
        let value = self.lookup(key, Presence::Optional, issues)?;
        decode(value, self.path_of(key), issues)
    }

    /// Decodes a required array element by element, so a bad element is an issue at
    /// `key[index]`. Returns the good elements with their positions.
    pub(super) fn required_list<T: DeserializeOwned>(
        &mut self,
        key: &str,
        issues: &mut Issues,
    ) -> Option<Vec<(usize, T)>> {
        let value = self.lookup(key, Presence::Required, issues)?;
        decode_list(value, self.path_of(key), issues)
    }

    /// Like [`TableReader::required_list`], for a key that may be omitted.
    pub(super) fn optional_list<T: DeserializeOwned>(
        &mut self,
        key: &str,
        issues: &mut Issues,
    ) -> Option<Vec<(usize, T)>> {
        let value = self.lookup(key, Presence::Optional, issues)?;
        decode_list(value, self.path_of(key), issues)
    }

    /// A required sub-table.
    pub(super) fn required_table(&mut self, key: &str, issues: &mut Issues) -> Option<&'a Table> {
        let value = self.lookup(key, Presence::Required, issues)?;
        as_table(value, self.path_of(key), issues)
    }

    /// A sub-table that may be omitted.
    pub(super) fn optional_table(&mut self, key: &str, issues: &mut Issues) -> Option<&'a Table> {
        let value = self.lookup(key, Presence::Optional, issues)?;
        as_table(value, self.path_of(key), issues)
    }

    /// A required array of tables. An element that is not a table is an issue and reads as `None`,
    /// so the other elements keep their positions.
    pub(super) fn required_tables(
        &mut self,
        key: &str,
        issues: &mut Issues,
    ) -> Option<Vec<Option<&'a Table>>> {
        let value = self.lookup(key, Presence::Required, issues)?;
        as_tables(value, self.path_of(key), issues)
    }

    /// An array of tables that may be omitted.
    pub(super) fn optional_tables(
        &mut self,
        key: &str,
        issues: &mut Issues,
    ) -> Option<Vec<Option<&'a Table>>> {
        let value = self.lookup(key, Presence::Optional, issues)?;
        as_tables(value, self.path_of(key), issues)
    }

    /// Reports every key that was never read as an unknown key.
    pub(super) fn finish(self, issues: &mut Issues) {
        for key in self.table.keys() {
            if !self.read.contains(key) {
                issues.add(key_path(&self.path, key), "unknown key");
            }
        }
    }
}

fn decode<T: DeserializeOwned>(value: &Value, path: String, issues: &mut Issues) -> Option<T> {
    match value.clone().try_into::<T>() {
        Ok(decoded) => Some(decoded),
        Err(error) => {
            issues.add(path, error.message());
            None
        }
    }
}

fn decode_list<T: DeserializeOwned>(
    value: &Value,
    path: String,
    issues: &mut Issues,
) -> Option<Vec<(usize, T)>> {
    let Value::Array(items) = value else {
        issues.add(path, "expected an array");
        return None;
    };
    Some(
        items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                decode(item, index_path(&path, index), issues).map(|decoded| (index, decoded))
            })
            .collect(),
    )
}

fn as_table<'a>(value: &'a Value, path: String, issues: &mut Issues) -> Option<&'a Table> {
    match value {
        Value::Table(table) => Some(table),
        _ => {
            issues.add(path, "expected a table");
            None
        }
    }
}

fn as_tables<'a>(
    value: &'a Value,
    path: String,
    issues: &mut Issues,
) -> Option<Vec<Option<&'a Table>>> {
    let Value::Array(items) = value else {
        issues.add(path, "expected an array of tables");
        return None;
    };
    Some(
        items
            .iter()
            .enumerate()
            .map(|(index, item)| as_table(item, index_path(&path, index), issues))
            .collect(),
    )
}
