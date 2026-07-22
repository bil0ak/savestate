use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::adapters::{Adapter, postgres::PostgresAdapter, sqlite::SqliteAdapter};
use crate::model::{
    CaptureScope, CheckpointKind, DatabaseSwap, FileEntry, JournalPhase, RestoreJournal,
    RootManifest, SCHEMA_VERSION, ScopedPath, ServiceArtifact, SnapshotManifest, SwapRecord,
};
use crate::store::Store;
use crate::{capture, config, filesystem, integrations, interactive, restore, store, ui};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::Serialize;

mod api;
mod checkpoints;
mod init;
mod journal_helpers;
mod maintenance;
mod recovery;
mod restore_execute;
mod restore_flow;
mod service_diff;

use crate::cli::prompts::*;
use journal_helpers::*;
pub(crate) use service_diff::normalize_label;
use service_diff::*;

pub use api::{
    CheckpointSummary, CreateOptions, CreateOutcome, DatabaseSelection, DiffTarget,
    IntegrationAction, RecoveryAction, RestorePlan, SavestateError, StateDiffReport, StatusReport,
};

/// Application facade for checkpoint, restore, and integration operations.
pub struct Savestate {
    pub(crate) root: PathBuf,
    pub(crate) config: config::Config,
    pub(crate) store: Store,
}

#[derive(Clone, Copy)]
enum DatabaseCapture<'a> {
    None,
    Configured,
    Matching(&'a SnapshotManifest),
    SqliteMatching(&'a SnapshotManifest),
}

#[derive(Clone, Copy)]
enum RetentionMode {
    Apply,
    Defer,
}

#[derive(Clone, Copy)]
enum Presentation {
    Silent,
    Notices,
    Announce,
}

struct CreateRequest<'a> {
    label: Option<String>,
    if_changed: bool,
    presentation: Presentation,
    include_ignored: bool,
    /// When set, capture runs under this recorded scope instead of the live
    /// configuration. Recovery checkpoints pass the restore target's scope so
    /// the undo captures exactly what the restore will replace.
    recorded_scope: Option<&'a CaptureScope>,
    database_capture: DatabaseCapture<'a>,
    kind: CheckpointKind,
    retention: RetentionMode,
}

struct CheckpointPublication {
    id: String,
    created: bool,
}

struct PreparedRestore {
    target: SnapshotManifest,
    current_owned: Vec<FileEntry>,
    preserved: Vec<ScopedPath>,
    files: filesystem::DiffReport,
    services: Vec<ServiceDiff>,
    remote_confirmations: Vec<String>,
}

impl PreparedRestore {
    fn equivalent(&self, other: &Self) -> bool {
        self.target.id == other.target.id
            && self.current_owned == other.current_owned
            && self.files == other.files
            && self.services == other.services
            && self.preserved == other.preserved
            && self.remote_confirmations == other.remote_confirmations
    }
}
