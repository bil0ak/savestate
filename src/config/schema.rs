use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub store: PathBuf,
    pub keep_last: usize,
    pub retention: RetentionConfig,
    pub respect_gitignore: bool,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub limits: LimitsConfig,
    pub experimental: ExperimentalConfig,
    pub external_paths: Vec<PathBuf>,
    pub sqlite: Vec<SqliteConfig>,
    pub postgres: Vec<PostgresConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RetentionConfig {
    pub manual: ManualRetention,
    pub agent: usize,
    pub recovery: usize,
    pub run: usize,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            manual: ManualRetention::Keep,
            agent: 20,
            recovery: 10,
            run: 10,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManualRetention {
    #[default]
    Keep,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LimitsConfig {
    pub warn_files: usize,
    pub warn_size: String,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            warn_files: 100_000,
            warn_size: "2GiB".into(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExperimentalConfig {
    pub databases: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SqliteConfig {
    pub path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PostgresConfig {
    pub name: String,
    pub url_env: String,
    #[serde(default = "default_maintenance_db")]
    pub maintenance_db: String,
    /// Snapshotting is read-only; destructive restore requires a separate opt-in.
    #[serde(default)]
    pub allow_restore: bool,
    /// Remote TCP targets require both restore flags and explicit confirmation.
    #[serde(default)]
    pub allow_remote_restore: bool,
}

fn default_maintenance_db() -> String {
    "postgres".into()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            store: PathBuf::from(".savestate"),
            keep_last: 20,
            retention: RetentionConfig::default(),
            respect_gitignore: true,
            include: Vec::new(),
            exclude: Vec::new(),
            limits: LimitsConfig::default(),
            experimental: ExperimentalConfig::default(),
            external_paths: Vec::new(),
            sqlite: Vec::new(),
            postgres: Vec::new(),
        }
    }
}
