use super::*;

pub(super) fn ensure_real_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                bail!("store path is not a real directory: {}", path.display());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
            ensure_real_directory_existing(path, "store")?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub(super) fn ensure_real_directory_existing(path: &Path, description: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("{description} is not a real directory: {}", path.display());
    }
    Ok(())
}

pub(super) fn ensure_regular_file(path: &Path, description: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("{description} is not a regular file: {}", path.display());
    }
    Ok(())
}

pub(super) fn ensure_absent_or_regular_file(path: &Path, description: &str) -> Result<()> {
    if path_exists(path)? {
        ensure_regular_file(path, description)?;
    }
    Ok(())
}

pub(super) fn path_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn valid_hash(hash: &str) -> bool {
    crate::model::ObjectHash::new(hash).is_ok()
}

pub(super) fn valid_checkpoint_id(id: &str) -> bool {
    crate::model::CheckpointId::new(id).is_ok()
}

#[allow(clippy::too_many_lines)] // Wire-compatibility invariants are audited together.
pub(super) fn validate_manifest(manifest: &SnapshotManifest) -> Result<()> {
    if !(MIN_SCHEMA_VERSION..=SCHEMA_VERSION).contains(&manifest.schema_version) {
        bail!("unsupported manifest schema {}", manifest.schema_version);
    }
    if manifest.schema_version >= 3 && manifest.kind.is_none() {
        bail!("manifest schema 3 or newer requires an explicit checkpoint kind");
    }
    if !valid_checkpoint_id(&manifest.id) {
        bail!("manifest contains invalid checkpoint ID {:?}", manifest.id);
    }
    if !manifest.project_root.is_absolute() {
        bail!("manifest project root must be absolute");
    }
    if manifest.capture_finished_at < manifest.capture_started_at {
        bail!("manifest capture time window is reversed");
    }
    if let Some(label) = &manifest.label {
        validate_label(label)?;
    }
    validate_scope_patterns(&manifest.capture_scope.include, "manifest include")?;
    validate_scope_patterns(&manifest.capture_scope.exclude, "manifest exclude")?;
    let root_ids: BTreeSet<_> = manifest.roots.iter().map(|root| &root.id).collect();
    if root_ids.len() != manifest.roots.len() {
        bail!("manifest contains duplicate filesystem root IDs");
    }
    if manifest.schema_version >= 3 && manifest.roots.iter().any(|root| root.identity.is_none()) {
        bail!("manifest schema 3 or newer requires filesystem root identities");
    }
    if manifest.roots.iter().filter(|root| root.repository).count() != 1 {
        bail!("manifest must contain exactly one repository root");
    }
    for root in &manifest.roots {
        validate_component(&root.id, "filesystem root ID")?;
        if !root.path.is_absolute() {
            bail!("filesystem root {} is not absolute", root.id);
        }
        if root.repository && root.path != manifest.project_root {
            bail!("repository root does not match the manifest project root");
        }
        if let Some(identity) = &root.identity {
            if !identity.canonical_path.is_absolute() || identity.canonical_path != root.path {
                bail!(
                    "filesystem root {} has an inconsistent identity path",
                    root.id
                );
            }
            if !matches!(identity.kind.as_str(), "directory" | "file") {
                bail!(
                    "filesystem root {} has unsupported type {}",
                    root.id,
                    identity.kind
                );
            }
            if root.repository && identity.kind != "directory" {
                bail!("repository root must be a directory");
            }
            if identity.birth_time_secs.is_some() != identity.birth_time_nanos.is_some()
                || identity
                    .birth_time_nanos
                    .is_some_and(|nanos| nanos >= 1_000_000_000)
            {
                bail!("filesystem root {} has an invalid birth time", root.id);
            }
            if manifest.schema_version >= 4
                && (manifest.platform.starts_with("macos-")
                    || manifest.platform.starts_with("linux-"))
                && (identity.device.is_none() || identity.file_id.is_none())
            {
                bail!("manifest schema 4 requires stable Unix filesystem root identities");
            }
        }
    }
    for (index, root) in manifest.roots.iter().enumerate() {
        for other in manifest.roots.iter().skip(index + 1) {
            if root.path == other.path
                || root.path.starts_with(&other.path)
                || other.path.starts_with(&root.path)
            {
                bail!("manifest contains overlapping filesystem roots");
            }
        }
    }
    let mut paths = std::collections::BTreeMap::new();
    for entry in &manifest.files {
        if !root_ids.contains(&entry.root_id) {
            bail!("manifest file references unknown root {}", entry.root_id);
        }
        validate_relative_path(&entry.path)?;
        let root = manifest
            .roots
            .iter()
            .find(|root| root.id == entry.root_id)
            .context("manifest file root disappeared during validation")?;
        if entry.path.as_os_str().is_empty() && root.repository {
            bail!("repository manifest cannot replace the repository root itself");
        }
        if paths
            .insert(
                (entry.root_id.clone(), entry.path.clone()),
                entry.kind.clone(),
            )
            .is_some()
        {
            bail!("manifest contains duplicate path {}", entry.path.display());
        }
        match entry.kind {
            EntryKind::File => {
                if let Some(hash) = &entry.object_hash
                    && !valid_hash(hash)
                {
                    bail!("manifest contains an invalid file object hash");
                }
            }
            EntryKind::Directory | EntryKind::Symlink if entry.object_hash.is_some() => {
                bail!("only regular files may reference content objects");
            }
            EntryKind::Directory | EntryKind::Symlink => {}
        }
        if entry.modified_nanos >= 1_000_000_000 {
            bail!("manifest contains an invalid nanosecond timestamp");
        }
        match entry.kind {
            EntryKind::Symlink if entry.symlink_target.is_none() => {
                bail!("manifest symlink has no target");
            }
            EntryKind::File | EntryKind::Directory if entry.symlink_target.is_some() => {
                bail!("only symlinks may contain a symlink target");
            }
            EntryKind::Directory | EntryKind::Symlink if entry.hardlink_group.is_some() => {
                bail!("only regular files may belong to a hardlink group");
            }
            EntryKind::File | EntryKind::Directory | EntryKind::Symlink => {}
        }
        for (name, value) in &entry.xattrs {
            if name.is_empty() || BASE64.decode(value).is_err() {
                bail!("manifest contains an invalid extended attribute");
            }
        }
    }
    let mut hardlink_groups =
        std::collections::BTreeMap::<&str, Vec<&crate::model::FileEntry>>::new();
    for entry in &manifest.files {
        if let Some(group) = entry.hardlink_group.as_deref() {
            if group.is_empty() {
                bail!("manifest contains an empty hardlink group identity");
            }
            hardlink_groups.entry(group).or_default().push(entry);
        }
    }
    for entries in hardlink_groups.values() {
        if entries.len() < 2 {
            bail!("manifest contains an incomplete hardlink group");
        }
        let first = entries[0];
        if entries.iter().skip(1).any(|entry| {
            entry.root_id != first.root_id
                || entry.kind != EntryKind::File
                || entry.object_hash != first.object_hash
                || entry.size != first.size
                || entry.mode != first.mode
                || entry.uid != first.uid
                || entry.gid != first.gid
                || entry.modified_secs != first.modified_secs
                || entry.modified_nanos != first.modified_nanos
                || entry.xattrs != first.xattrs
        }) {
            bail!("manifest hardlink group members have inconsistent content or metadata");
        }
    }
    for path in &manifest.capture_scope.ignored_boundaries {
        if !root_ids.contains(&path.root_id) {
            bail!("capture scope references unknown root {}", path.root_id);
        }
        validate_relative_path(&path.path)?;
        if path.path.as_os_str().is_empty() {
            bail!("capture scope cannot preserve a filesystem root");
        }
    }
    for path in &manifest.capture_scope.explicitly_included_paths {
        if !root_ids.contains(&path.root_id) {
            bail!(
                "capture scope include references unknown root {}",
                path.root_id
            );
        }
        validate_relative_path(&path.path)?;
    }
    for source in &manifest.capture_scope.git_ignore_sources {
        if !root_ids.contains(&source.root_id) {
            bail!(
                "Git ignore source references unknown root {}",
                source.root_id
            );
        }
        if !matches!(
            source.kind.as_str(),
            "gitignore" | "info_exclude" | "global_exclude"
        ) {
            bail!("Git ignore source has unknown kind {:?}", source.kind);
        }
        if source.kind == "gitignore" {
            validate_relative_path(&source.path)?;
        } else if source.path.as_os_str().is_empty() {
            bail!("Git exclude source path cannot be empty");
        }
    }
    for entry in &manifest.files {
        let mut parent = entry.path.parent();
        while let Some(value) = parent.filter(|value| !value.as_os_str().is_empty()) {
            if paths
                .get(&(entry.root_id.clone(), value.to_path_buf()))
                .is_some_and(|kind| *kind != EntryKind::Directory)
            {
                bail!(
                    "manifest path {} descends through a non-directory",
                    entry.path.display()
                );
            }
            parent = value.parent();
        }
    }
    let mut service_keys = BTreeSet::new();
    let mut postgres_names = BTreeSet::new();
    let mut postgres_environments = BTreeSet::new();
    for service in &manifest.services {
        if !valid_hash(service.object_hash()) {
            bail!("manifest contains an invalid service object hash");
        }
        match service {
            ServiceArtifact::Sqlite { root_id, path, .. } => {
                if !root_ids.contains(root_id) {
                    bail!("SQLite artifact references unknown root {root_id}");
                }
                validate_relative_path(path)?;
                if !paths
                    .get(&(root_id.clone(), path.clone()))
                    .is_some_and(|kind| *kind == EntryKind::File)
                {
                    bail!("SQLite artifact does not reference a captured regular file");
                }
                if !service_keys.insert(format!("sqlite:{root_id}:{}", path.display())) {
                    bail!("manifest contains a duplicate SQLite artifact");
                }
            }
            ServiceArtifact::Postgres {
                name,
                url_env,
                database,
                target_identity,
                ..
            } => {
                if name.trim().is_empty() || url_env.trim().is_empty() || database.trim().is_empty()
                {
                    bail!("PostgreSQL artifact contains an empty resource identity");
                }
                if !postgres_names.insert(name) || !postgres_environments.insert(url_env) {
                    bail!("manifest contains an ambiguous PostgreSQL artifact");
                }
                if let Some(identity) = target_identity
                    && (identity.system_identifier.trim().is_empty()
                        || identity.database != *database
                        || identity.host.trim().is_empty()
                        || identity.user.trim().is_empty()
                        || identity
                            .database_owner
                            .as_deref()
                            .is_some_and(|owner| owner.trim().is_empty())
                        || identity.port == 0
                        || identity.server_major == 0)
                {
                    bail!("PostgreSQL artifact has an inconsistent target identity");
                }
            }
        }
        if manifest.schema_version >= 3
            && matches!(
                service,
                ServiceArtifact::Postgres {
                    target_identity: None,
                    ..
                }
            )
        {
            bail!("manifest schema 3 PostgreSQL artifact has no target identity");
        }
    }
    for entry in &manifest.files {
        if entry.kind == EntryKind::File
            && entry.object_hash.is_none()
            && !manifest.services.iter().any(|service| {
                matches!(service, ServiceArtifact::Sqlite { root_id, path, .. } if root_id == &entry.root_id && path == &entry.path)
            })
        {
            bail!("regular file {} has no content object", entry.path.display());
        }
    }
    Ok(())
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod legacy_manifest_tests {
    use super::*;

    #[test]
    fn schemas_one_through_four_remain_valid() -> Result<()> {
        let root = tempfile::tempdir()?.path().canonicalize()?;
        let fixtures = [
            include_str!("../../tests/fixtures/manifests/schema-1.json"),
            include_str!("../../tests/fixtures/manifests/schema-2.json"),
            include_str!("../../tests/fixtures/manifests/schema-3.json"),
            include_str!("../../tests/fixtures/manifests/schema-4.json"),
        ];

        for (index, fixture) in fixtures.into_iter().enumerate() {
            let mut manifest: SnapshotManifest = serde_json::from_str(fixture)?;
            manifest.project_root.clone_from(&root);
            manifest.roots[0].path.clone_from(&root);
            if let Some(identity) = &mut manifest.roots[0].identity {
                identity.canonical_path.clone_from(&root);
            }
            validate_manifest(&manifest)?;
            assert_eq!(manifest.schema_version, u32::try_from(index + 1)?);
        }
        Ok(())
    }
}

pub(super) fn validate_component(value: &str, description: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("unsafe {description} {value:?}");
    }
    Ok(())
}

pub(super) fn validate_label(label: &str) -> Result<()> {
    if label.is_empty()
        || label.chars().count() > 200
        || label
            .chars()
            .any(|character| character.is_control() || character == '\u{1b}')
    {
        bail!("checkpoint label contains unsafe text");
    }
    Ok(())
}

pub(super) fn validate_relative_path(path: &Path) -> Result<()> {
    if crate::model::RelativePath::new(path).is_err() {
        bail!("unsafe manifest path {}", path.display());
    }
    Ok(())
}
