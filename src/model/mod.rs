//! Persistent checkpoint and restore data structures.

mod checkpoint;
mod filesystem;
mod journal;
mod policy;
mod service;
mod value;

pub use checkpoint::{CheckpointKind, SnapshotManifest};
pub use filesystem::{
    CaptureScope, EntryKind, FileEntry, GitIgnoreSource, RootIdentity, RootManifest, ScopedPath,
};
pub use journal::{
    DatabaseSwap, JournalPhase, JournalTransitionError, PreserveRecord, RestoreJournal, SwapRecord,
};
pub use policy::RetentionPolicy;
pub(crate) use policy::validate_scope_patterns;
pub use service::{PostgresTargetIdentity, ServiceArtifact};
pub use value::{
    CheckpointId, CheckpointSelector, ObjectHash, OperationId, RelativePath, RootId, ValueError,
};

pub const SCHEMA_VERSION: u32 = 4;
pub const MIN_SCHEMA_VERSION: u32 = 1;
