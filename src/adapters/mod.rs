pub mod postgres;
pub mod sqlite;

use std::path::Path;

use anyhow::Result;

use crate::{model::ServiceArtifact, store::Store};

pub(crate) trait Adapter {
    fn preflight(&self) -> Result<()>;
    fn snapshot(&self, store: &Store) -> Result<ServiceArtifact>;
    fn verify(&self, artifact: &ServiceArtifact, store: &Store) -> Result<()>;
    fn restore(&self, artifact: &ServiceArtifact, store: &Store) -> Result<()>;
}

pub(crate) fn require_object(artifact: &ServiceArtifact, store: &Store) -> Result<()> {
    store.verify_object(artifact.object_hash())
}

pub(crate) fn executable(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub(crate) fn object_to_temp(
    store: &Store,
    hash: &str,
    directory: &Path,
) -> Result<tempfile::NamedTempFile> {
    let file = tempfile::NamedTempFile::new_in(directory)?;
    std::fs::copy(store.object_path(hash), file.path())?;
    Ok(file)
}
