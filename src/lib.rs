//! Verified local checkpoints for coding-agent sessions.
//!
//! The crate is organized around a small application facade. Command-line
//! parsing and rendering live in [`cli`], while storage and restore machinery
//! remain implementation details behind [`Savestate`].

mod adapters;
mod app;
#[doc(hidden)]
pub mod capture;
#[doc(hidden)]
pub mod cli;
#[doc(hidden)]
pub mod config;
#[doc(hidden)]
pub mod integrations;
#[doc(hidden)]
pub mod model;
mod platform;
mod restore;
#[doc(hidden)]
pub mod store;
mod terminal_signals;

pub use capture as filesystem;
pub(crate) use cli::output as ui;
pub(crate) use cli::restore_prompt as interactive;

pub use app::{
    CheckpointSummary, CreateOptions, CreateOutcome, DatabaseSelection, DiffTarget,
    IntegrationAction, RecoveryAction, RestorePlan, Savestate, SavestateError, StateDiffReport,
    StatusReport,
};

/// Compatibility alias for the original pre-1.0 facade name.
pub type App = Savestate;
pub use model::{
    CaptureScope, CheckpointId, CheckpointKind, CheckpointSelector, EntryKind, FileEntry,
    GitIgnoreSource, JournalTransitionError, ObjectHash, OperationId, PostgresTargetIdentity,
    RelativePath, RetentionPolicy, RootId, RootIdentity, RootManifest, ScopedPath, ServiceArtifact,
    SnapshotManifest, ValueError,
};

pub(crate) use app::normalize_label;
