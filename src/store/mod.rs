use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use fs4::fs_std::FileExt;
use tempfile::NamedTempFile;
use thiserror::Error;
use ulid::Ulid;

use crate::{
    model::{
        CheckpointKind, EntryKind, MIN_SCHEMA_VERSION, RestoreJournal, RetentionPolicy,
        SCHEMA_VERSION, ServiceArtifact, SnapshotManifest, validate_scope_patterns,
    },
    platform,
};

mod atomic;
mod catalog;
mod journal;
mod layout;
mod object_types;
mod objects;
mod retention;
mod validation;

pub use atomic::atomic_write;
pub(crate) use object_types::ObjectWrite;
pub use object_types::hash_file;

use atomic::*;
use object_types::*;
use validation::*;

pub struct Store {
    root: PathBuf,
}

#[derive(Debug, Error)]
pub(crate) enum CheckpointLookupError {
    #[error("no checkpoints exist")]
    NoCheckpoints,
    #[error("snapshot {0:?} not found")]
    Missing(String),
    #[error("snapshot prefix {0:?} is ambiguous")]
    Ambiguous(String),
}
