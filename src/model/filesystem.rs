use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureScope {
    /// False means full-scope capture. This default preserves v1 semantics.
    #[serde(default)]
    pub respect_gitignore: bool,
    #[serde(default)]
    pub include_ignored: bool,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub explicitly_included_paths: Vec<ScopedPath>,
    #[serde(default)]
    pub git_ignore_sources: Vec<GitIgnoreSource>,
    #[serde(default)]
    pub ignored_boundaries: Vec<ScopedPath>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitIgnoreSource {
    pub root_id: String,
    pub path: PathBuf,
    pub kind: String,
    pub contents: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ScopedPath {
    pub root_id: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootManifest {
    pub id: String,
    pub path: PathBuf,
    pub repository: bool,
    #[serde(default)]
    pub identity: Option<RootIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootIdentity {
    pub canonical_path: PathBuf,
    pub kind: String,
    #[serde(default)]
    pub device: Option<u64>,
    #[serde(default)]
    pub file_id: Option<u64>,
    #[serde(default)]
    pub birth_time_secs: Option<i64>,
    #[serde(default)]
    pub birth_time_nanos: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    pub root_id: String,
    pub path: PathBuf,
    pub kind: EntryKind,
    pub size: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub modified_secs: i64,
    pub modified_nanos: u32,
    pub object_hash: Option<String>,
    pub symlink_target: Option<PathBuf>,
    pub hardlink_group: Option<String>,
    pub xattrs: BTreeMap<String, String>,
}
