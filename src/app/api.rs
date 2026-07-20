use chrono::{DateTime, Utc};
use thiserror::Error;

use super::*;
use crate::model::{CheckpointId, CheckpointSelector};

/// Selects whether a manual checkpoint includes configured experimental databases.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DatabaseSelection {
    /// Capture configured databases when database support is enabled.
    #[default]
    Configured,
    /// Capture filesystem state only.
    FilesystemOnly,
}

/// Options for creating a manual checkpoint through the Rust facade.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CreateOptions {
    /// Optional human-readable checkpoint label.
    pub label: Option<String>,
    /// Reuse the latest equivalent manual checkpoint.
    pub if_changed: bool,
    /// Include Git-ignored paths for this checkpoint only.
    pub include_ignored: bool,
    /// Controls experimental database capture.
    pub databases: DatabaseSelection,
}

/// Result of a checkpoint creation request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateOutcome {
    /// The canonical checkpoint identifier.
    pub checkpoint_id: CheckpointId,
    /// Whether this call published a new checkpoint.
    pub created: bool,
}

/// Stable metadata suitable for presenting a checkpoint list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CheckpointSummary {
    /// Canonical checkpoint identifier.
    pub id: String,
    /// Creation time recorded in the manifest.
    pub created_at: DateTime<Utc>,
    /// Optional human-readable label.
    pub label: Option<String>,
    /// Checkpoint retention class.
    pub kind: CheckpointKind,
    /// Number of captured filesystem entries.
    pub filesystem_entries: usize,
    /// Number of captured experimental database resources.
    pub database_resources: usize,
    /// Whether automatic retention is prohibited for this checkpoint.
    pub pinned: bool,
}

/// Structured project and store status for non-CLI callers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusReport {
    /// Canonical project root.
    pub project_root: PathBuf,
    /// Active store root.
    pub store_root: PathBuf,
    /// Logical bytes held by the store.
    pub logical_store_bytes: u64,
    /// Allocated bytes held by the store.
    pub allocated_store_bytes: u64,
    /// Number of published checkpoints.
    pub checkpoint_count: usize,
    /// Latest checkpoint, when one exists.
    pub latest: Option<CheckpointSummary>,
    /// Number of paths in the selected live scope.
    pub selected_paths: usize,
    /// Logical bytes in the selected live scope.
    pub selected_bytes: u64,
    /// Whether an interrupted restore blocks mutation.
    pub recovery_pending: bool,
}

/// The right-hand side of a checkpoint diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffTarget {
    /// Compare with a fresh capture of current state.
    Current,
    /// Compare with another persisted checkpoint.
    Checkpoint(CheckpointSelector),
}

/// Structured filesystem and database differences.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StateDiffReport {
    /// Filesystem entry differences.
    pub filesystem: filesystem::DiffReport,
    /// Human-readable experimental database change summaries.
    pub database_changes: Vec<String>,
}

/// Recovery operation authorized by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Roll live state back using the recovery checkpoint.
    Rollback,
    /// Resume the interrupted restore toward its original target.
    Resume,
}

/// Coding-agent integration operation authorized by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegrationAction {
    /// Install or update Savestate-owned hooks.
    Install,
    /// Remove only Savestate-owned hooks.
    Remove,
}

/// A restore preview tied to the exact state observed during planning.
///
/// The private transaction data prevents callers from manufacturing a plan.
/// [`Savestate::apply_restore`] refreshes it under the mutation lock and rejects
/// the operation if project or database state changed.
pub struct RestorePlan {
    /// Canonical target checkpoint identifier.
    pub checkpoint_id: String,
    /// Filesystem changes observed during planning.
    pub filesystem: filesystem::DiffReport,
    /// Human-readable summaries of experimental database changes.
    pub database_changes: Vec<String>,
    prepared: PreparedRestore,
}

/// Caller-actionable failures exposed by the narrow Rust facade.
#[derive(Debug, Error)]
pub enum SavestateError {
    /// The requested checkpoint did not exist.
    #[error("checkpoint was not found: {0}")]
    MissingCheckpoint(String),
    /// A checkpoint prefix selected more than one checkpoint.
    #[error("checkpoint selector is ambiguous: {0}")]
    AmbiguousCheckpoint(String),
    /// A source changed repeatedly while it was being captured.
    #[error("capture could not obtain stable source data: {0}")]
    UnstableCapture(String),
    /// An interrupted restore must be resolved before another mutation.
    #[error("restore recovery is pending: {0}")]
    PendingRecovery(String),
    /// State changed after the restore preview was prepared.
    #[error("restore plan changed before mutation; prepare a new plan")]
    ChangedRestorePlan,
    /// An external database tool failed its safety preflight.
    #[error("external tool preflight failed: {0}")]
    ExternalToolPreflight(String),
    /// An operation failed without a more specific public classification.
    #[error(transparent)]
    Operation(#[from] anyhow::Error),
}

impl Savestate {
    /// Creates a manual checkpoint without writing terminal output.
    ///
    /// # Errors
    ///
    /// Returns a typed error when capture is unstable, recovery is pending, or
    /// persistence and external database operations fail.
    pub fn create_checkpoint(
        &mut self,
        options: CreateOptions,
    ) -> Result<CreateOutcome, SavestateError> {
        self.ensure_no_journal().map_err(classify_error)?;
        let existing = self.store.ids().map_err(classify_error)?;
        let database_capture = match options.databases {
            DatabaseSelection::Configured => DatabaseCapture::Configured,
            DatabaseSelection::FilesystemOnly => DatabaseCapture::None,
        };
        let id = self
            .create_internal(CreateRequest {
                label: normalize_explicit_label(options.label).map_err(classify_error)?,
                if_changed: options.if_changed,
                presentation: Presentation::Silent,
                include_ignored: options.include_ignored,
                recorded_scope: None,
                database_capture,
                kind: CheckpointKind::Manual,
                retention: RetentionMode::Apply,
            })
            .map_err(classify_error)?;
        let checkpoint_id = CheckpointId::new(id).map_err(|error| {
            SavestateError::Operation(
                anyhow::Error::new(error).context("invalid stored checkpoint ID"),
            )
        })?;
        let created = !existing.iter().any(|value| value == checkpoint_id.as_str());
        Ok(CreateOutcome {
            checkpoint_id,
            created,
        })
    }

    /// Returns checkpoint metadata without rendering it.
    ///
    /// # Errors
    ///
    /// Returns an error if the catalog or a referenced manifest is invalid.
    pub fn checkpoints(&self) -> Result<Vec<CheckpointSummary>, SavestateError> {
        self.store
            .ids()
            .map_err(classify_error)?
            .into_iter()
            .map(|id| {
                let manifest = self.store.load(&id).map_err(classify_error)?;
                Ok(CheckpointSummary {
                    id: manifest.id.clone(),
                    created_at: manifest.created_at,
                    label: manifest.label.clone(),
                    kind: manifest.resolved_kind(),
                    filesystem_entries: manifest.files.len(),
                    database_resources: manifest.services.len(),
                    pinned: self.store.is_pinned(&manifest.id).map_err(classify_error)?,
                })
            })
            .collect()
    }

    /// Returns project, store, capture-scope, and recovery status without rendering.
    ///
    /// # Errors
    ///
    /// Returns an error if the catalog, latest manifest, configuration-derived
    /// scope, or restore journal cannot be validated.
    pub fn status_report(&self) -> Result<StatusReport, SavestateError> {
        self.store.validate_catalog().map_err(classify_error)?;
        let (logical_store_bytes, allocated_store_bytes) =
            self.store.size().map_err(classify_error)?;
        let checkpoints = self.checkpoints()?;
        let sqlite_paths = self
            .sqlite_paths(DatabaseCapture::Configured)
            .map_err(classify_error)?;
        let discovery = filesystem::discover(&self.root, &self.config, false, None, &sqlite_paths)
            .map_err(classify_error)?;
        let recovery_pending = self.store.journal().map_err(classify_error)?.is_some();
        Ok(StatusReport {
            project_root: self.root.clone(),
            store_root: self.store.path().to_path_buf(),
            logical_store_bytes,
            allocated_store_bytes,
            checkpoint_count: checkpoints.len(),
            latest: checkpoints.into_iter().next(),
            selected_paths: discovery.logical_paths,
            selected_bytes: discovery.logical_bytes,
            recovery_pending,
        })
    }

    /// Compares a checkpoint with current state or another checkpoint.
    ///
    /// # Errors
    ///
    /// Returns an error when a selector cannot be resolved or when current
    /// state and configured databases cannot be captured consistently.
    pub fn diff_report(
        &self,
        from: &CheckpointSelector,
        to: &DiffTarget,
    ) -> Result<StateDiffReport, SavestateError> {
        let from = self
            .store
            .load(selector_text(from))
            .map_err(classify_error)?;
        let target = match to {
            DiffTarget::Current => self
                .capture_ephemeral(Some(&from.capture_scope), DatabaseCapture::Matching(&from))
                .map_err(classify_error)?,
            DiffTarget::Checkpoint(selector) => self
                .store
                .load(selector_text(selector))
                .map_err(classify_error)?,
        };
        let services = diff_services(&from.services, &target.services);
        Ok(StateDiffReport {
            filesystem: filesystem::diff_entries(&from.files, &target.files),
            database_changes: services
                .into_iter()
                .map(|service| format!("{}: {}", service.name, service.summary))
                .collect(),
        })
    }

    /// Prepares and verifies a restore without mutating live state.
    ///
    /// # Errors
    ///
    /// Returns an error for missing or ambiguous checkpoints, invalid targets,
    /// unstable current-state capture, or failed database preflight.
    pub fn plan_restore(
        &self,
        selector: &CheckpointSelector,
    ) -> Result<RestorePlan, SavestateError> {
        let prepared = self
            .prepare_restore(selector_text(selector))
            .map_err(classify_error)?;
        Ok(RestorePlan {
            checkpoint_id: prepared.target.id.clone(),
            filesystem: prepared.files.clone(),
            database_changes: prepared
                .services
                .iter()
                .map(|service| format!("{}: {}", service.name, service.summary))
                .collect(),
            prepared,
        })
    }

    /// Applies a previously prepared restore plan.
    ///
    /// The plan is always refreshed under the exclusive store lock immediately
    /// before mutation. A stale preview is rejected.
    ///
    /// # Errors
    ///
    /// Returns [`SavestateError::ChangedRestorePlan`] if live state changed, or
    /// another typed error if validation, recovery, or mutation fails.
    pub fn apply_restore(&mut self, plan: RestorePlan) -> Result<(), SavestateError> {
        self.apply_restore_plan(plan.prepared)
            .map_err(classify_error)
    }
}

fn selector_text(selector: &CheckpointSelector) -> &str {
    selector.as_str()
}

fn classify_error(error: anyhow::Error) -> SavestateError {
    let message = format!("{error:#}");
    if let Some(lookup) = error.downcast_ref::<store::CheckpointLookupError>() {
        return match lookup {
            store::CheckpointLookupError::NoCheckpoints
            | store::CheckpointLookupError::Missing(_) => {
                SavestateError::MissingCheckpoint(message)
            }
            store::CheckpointLookupError::Ambiguous(_) => {
                SavestateError::AmbiguousCheckpoint(message)
            }
        };
    }
    if message.contains("changed while being captured")
        || message.contains("kept changing during capture")
    {
        SavestateError::UnstableCapture(message)
    } else if message.contains("interrupted restore") || message.contains("run `savestate recover`")
    {
        SavestateError::PendingRecovery(message)
    } else if message.contains("state changed before the restore lock") {
        SavestateError::ChangedRestorePlan
    } else if message.contains("pg_") || message.contains("PostgreSQL") {
        SavestateError::ExternalToolPreflight(message)
    } else {
        SavestateError::Operation(error)
    }
}
