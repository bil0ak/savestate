use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::UNIX_EPOCH,
};

use ::ignore::{
    WalkBuilder,
    gitignore::{Gitignore, GitignoreBuilder},
};
use anyhow::{Context, Result, bail};
#[cfg(unix)]
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use globset::{Glob, GlobSet, GlobSetBuilder};
use walkdir::WalkDir;

use crate::{
    config::Config,
    model::{
        CaptureScope, EntryKind, FileEntry, GitIgnoreSource, RootIdentity, RootManifest, ScopedPath,
    },
    platform,
    store::Store,
};

mod diff;
mod discovery;
mod ignore;
mod materialize;
mod selection;
mod snapshot;

pub use diff::{DiffReport, diff_entries, entries_equivalent};
pub use discovery::discover;
pub use materialize::{
    materialize_root, path_exists, reapply_manifest_metadata, remove_path, sync_manifest_data,
};
pub use selection::roots;
pub use snapshot::capture;

pub(crate) use ignore::{StoredIgnore, ignored_boundaries_in_detached, recorded_scope_ignores};

use ignore::*;
use materialize::sibling_with_suffix;
use selection::*;
use snapshot::*;

#[derive(Clone, Debug)]
pub struct SqliteCandidate {
    pub root_id: String,
    pub relative_path: PathBuf,
    pub live_path: PathBuf,
}

pub struct Capture {
    pub roots: Vec<RootManifest>,
    pub files: Vec<FileEntry>,
    pub sqlite: Vec<SqliteCandidate>,
    pub engine: String,
    pub scope: CaptureScope,
    pub logical_paths: usize,
    pub logical_bytes: u64,
    pub(crate) notices: Vec<String>,
}

pub struct Discovery {
    pub roots: Vec<RootManifest>,
    paths: BTreeMap<String, Vec<(PathBuf, PathBuf)>>,
    pub sqlite: Vec<SqliteCandidate>,
    pub scope: CaptureScope,
    pub gitignore_rules_inactive: bool,
    pub logical_paths: usize,
    pub logical_bytes: u64,
}
