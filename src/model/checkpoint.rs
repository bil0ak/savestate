use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{CaptureScope, FileEntry, RootManifest, ServiceArtifact};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub schema_version: u32,
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub label: Option<String>,
    #[serde(default)]
    pub kind: Option<CheckpointKind>,
    pub project_root: PathBuf,
    pub platform: String,
    pub engine: String,
    pub consistency: String,
    pub capture_started_at: DateTime<Utc>,
    pub capture_finished_at: DateTime<Utc>,
    pub roots: Vec<RootManifest>,
    pub files: Vec<FileEntry>,
    pub services: Vec<ServiceArtifact>,
    #[serde(default)]
    pub capture_scope: CaptureScope,
}

impl SnapshotManifest {
    pub fn resolved_kind(&self) -> CheckpointKind {
        self.kind
            .unwrap_or_else(|| CheckpointKind::from_legacy_label(self.label.as_deref()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointKind {
    Manual,
    Agent,
    Recovery,
    Run,
}

impl CheckpointKind {
    fn from_legacy_label(label: Option<&str>) -> Self {
        match label.unwrap_or_default() {
            value if value.starts_with("pre-restore:") || value.starts_with("pre-resume:") => {
                Self::Recovery
            }
            value if value.starts_with("before:") => Self::Run,
            value if value.starts_with("Codex ·") || value.starts_with("agent:") => Self::Agent,
            _ => Self::Manual,
        }
    }
}
