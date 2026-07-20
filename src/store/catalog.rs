use super::*;

impl Store {
    pub fn publish(&self, manifest: &SnapshotManifest) -> Result<()> {
        self.heal_catalog()?;
        validate_manifest(manifest)?;
        if manifest.schema_version != SCHEMA_VERSION {
            bail!("new checkpoints must use manifest schema {SCHEMA_VERSION}");
        }
        if self
            .ids()?
            .first()
            .is_some_and(|latest| manifest.id <= *latest)
        {
            bail!("new checkpoint ID must sort after every published checkpoint");
        }
        self.verify(manifest)?;
        let snapshots = self.root.join("snapshots");
        let stage = snapshots.join(format!(".{}.tmp", manifest.id));
        if path_exists(&stage)? {
            bail!(
                "checkpoint staging path already exists: {}",
                stage.display()
            );
        }
        let destination = snapshots.join(&manifest.id);
        if path_exists(&destination)? {
            bail!("checkpoint {} already exists", manifest.id);
        }
        fs::create_dir(&stage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&stage, fs::Permissions::from_mode(0o700))?;
        }
        let bytes = serde_json::to_vec_pretty(manifest)?;
        atomic_write(&stage.join("manifest.json"), &bytes)?;
        atomic_write(
            &stage.join("manifest.blake3"),
            blake3::hash(&bytes).to_hex().as_bytes(),
        )?;
        atomic_write(&stage.join("complete"), b"ok\n")?;
        fs::rename(&stage, destination)?;
        sync_directory(&snapshots)?;
        atomic_write(&self.root.join("HEAD"), manifest.id.as_bytes())?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Result<SnapshotManifest> {
        let id = self.resolve_id(id)?;
        let dir = self.root.join("snapshots").join(&id);
        ensure_real_directory_existing(&dir, "snapshot")?;
        let complete = dir.join("complete");
        if !path_exists(&complete)? {
            bail!("snapshot {id} is incomplete");
        }
        ensure_regular_file(&complete, "snapshot completion marker")?;
        let manifest_path = dir.join("manifest.json");
        let checksum_path = dir.join("manifest.blake3");
        ensure_regular_file(&manifest_path, "snapshot manifest")?;
        ensure_regular_file(&checksum_path, "snapshot manifest checksum")?;
        let bytes = fs::read(manifest_path)?;
        let expected = fs::read_to_string(checksum_path)?;
        if blake3::hash(&bytes).to_hex().as_str() != expected.trim() {
            bail!("snapshot {id} manifest checksum mismatch");
        }
        let mut manifest: SnapshotManifest = serde_json::from_slice(&bytes)?;
        if manifest.id != id {
            bail!(
                "snapshot directory {id} contains manifest for {}",
                manifest.id
            );
        }
        validate_manifest(&manifest)?;
        // Legacy manifests derive their kind from the label they were written
        // with; pinning it here keeps a later relabel from changing how
        // retention classifies the checkpoint.
        manifest.kind = Some(manifest.resolved_kind());
        let label = self.root.join("labels").join(&id);
        if path_exists(&label)? {
            ensure_regular_file(&label, "checkpoint label")?;
            let value = fs::read_to_string(label)?;
            validate_label(&value)?;
            manifest.label = Some(value);
        }
        Ok(manifest)
    }

    pub fn resolve_id(&self, requested: &str) -> Result<String> {
        let requested = if requested == "latest" {
            let head = self.root.join("HEAD");
            if !path_exists(&head)? {
                if self.ids()?.is_empty() {
                    return Err(CheckpointLookupError::NoCheckpoints.into());
                }
                bail!("snapshot store has checkpoints but no HEAD");
            }
            ensure_regular_file(&head, "store HEAD")?;
            fs::read_to_string(head)?
        } else {
            requested.to_owned()
        };
        let requested = requested.trim();
        if requested.is_empty() {
            bail!("checkpoint ID cannot be empty");
        }
        let matches: Vec<_> = self
            .ids()?
            .into_iter()
            .filter(|id| id.starts_with(requested))
            .collect();
        match matches.as_slice() {
            [id] => Ok(id.clone()),
            [] => Err(CheckpointLookupError::Missing(requested.to_owned()).into()),
            _ => Err(CheckpointLookupError::Ambiguous(requested.to_owned()).into()),
        }
    }

    pub fn ids(&self) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for entry in fs::read_dir(self.root.join("snapshots"))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if !valid_checkpoint_id(&name) {
                bail!(
                    "snapshot store contains a foreign entry {name:?}; remove {} to continue",
                    entry.path().display()
                );
            }
            ensure_real_directory_existing(&entry.path(), "snapshot")?;
            ensure_regular_file(&entry.path().join("complete"), "snapshot completion marker")
                .with_context(|| format!("snapshot {name} is incomplete or corrupt"))?;
            ids.push(name);
        }
        ids.sort();
        ids.reverse();
        Ok(ids)
    }

    pub fn set_label(&self, requested: &str, label: &str) -> Result<String> {
        self.heal_catalog()?;
        let id = self.resolve_id(requested)?;
        validate_label(label)?;
        let path = self.root.join("labels").join(&id);
        ensure_absent_or_regular_file(&path, "checkpoint label")?;
        atomic_write(&path, label.as_bytes())?;
        Ok(id)
    }

    pub fn delete(&self, requested: &str) -> Result<String> {
        self.heal_catalog()?;
        let id = self.resolve_id(requested)?;
        let remaining = self
            .ids()?
            .into_iter()
            .filter(|candidate| candidate != &id)
            .collect::<Vec<_>>();
        let head = self.root.join("HEAD");
        let current_head = self.head_id()?;
        if let Some(current) = &current_head
            && current != &id
            && !remaining.contains(current)
        {
            bail!("store HEAD references missing checkpoint {current}");
        }
        if current_head.is_some_and(|current| current == id) {
            if let Some(next) = remaining.first() {
                atomic_write(&head, next.as_bytes())?;
            } else if path_exists(&head)? {
                fs::remove_file(&head)?;
                sync_directory(&self.root)?;
            }
        }

        self.retire_checkpoint(&id)?;
        self.gc_objects()?;
        Ok(id)
    }

    pub fn size(&self) -> Result<(u64, u64)> {
        let mut logical = 0u64;
        let mut allocated = 0u64;
        for entry in walkdir::WalkDir::new(&self.root).follow_links(false) {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_file() {
                logical = logical.saturating_add(metadata.len());
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    allocated = allocated.saturating_add(metadata.blocks().saturating_mul(512));
                }
                #[cfg(not(unix))]
                {
                    allocated = allocated.saturating_add(metadata.len());
                }
            }
        }
        Ok((logical, allocated))
    }

    pub fn verify(&self, manifest: &SnapshotManifest) -> Result<()> {
        validate_manifest(manifest)?;
        for entry in &manifest.files {
            if let Some(hash) = &entry.object_hash {
                self.verify_object(hash)?;
                let size = fs::metadata(self.object_path(hash))?.len();
                if size != entry.size {
                    bail!(
                        "object {hash} has size {size}, but {} records {}",
                        entry.path.display(),
                        entry.size
                    );
                }
            }
        }
        for service in &manifest.services {
            self.verify_object(service.object_hash())?;
        }
        Ok(())
    }

    pub fn heal_catalog(&self) -> Result<()> {
        let ids = self.ids()?;
        let head_path = self.root.join("HEAD");
        match ids.first() {
            Some(newest) => {
                let head_matches = path_exists(&head_path)?
                    && ensure_regular_file(&head_path, "store HEAD").is_ok()
                    && fs::read_to_string(&head_path)?.trim() == newest;
                if !head_matches {
                    atomic_write(&head_path, newest.as_bytes())?;
                }
            }
            None => {
                if path_exists(&head_path)? {
                    ensure_regular_file(&head_path, "store HEAD")?;
                    fs::remove_file(&head_path)?;
                    sync_directory(&self.root)?;
                }
            }
        }
        let known = ids.into_iter().collect::<BTreeSet<_>>();
        for directory in ["labels", "pins"] {
            let parent = self.root.join(directory);
            for entry in fs::read_dir(&parent)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                if valid_checkpoint_id(&name) && !known.contains(&name) {
                    ensure_regular_file(&entry.path(), "checkpoint metadata")?;
                    fs::remove_file(entry.path())?;
                    sync_directory(&parent)?;
                }
            }
        }
        let snapshots = self.root.join("snapshots");
        for entry in fs::read_dir(&snapshots)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            // Control-file suffixes are protocol tokens, not user-facing extensions.
            #[allow(clippy::case_sensitive_file_extension_comparisons)]
            if name.starts_with('.') && (name.ends_with(".deleting") || name.ends_with(".tmp")) {
                ensure_real_directory_existing(&entry.path(), "stale snapshot staging")?;
                fs::remove_dir_all(entry.path())?;
                sync_directory(&snapshots)?;
            }
        }
        self.validate_catalog()
    }

    pub fn validate_catalog(&self) -> Result<()> {
        let ids = self.ids()?;
        for id in &ids {
            self.load(id)?;
        }
        let head = self.head_id()?;
        match (ids.first(), head.as_deref()) {
            (None, None) => {}
            (Some(newest), Some(head)) if newest == head => {}
            (None, Some(head)) => bail!("store HEAD references missing checkpoint {head}"),
            (Some(_), None) => bail!("snapshot store has checkpoints but no HEAD"),
            (Some(newest), Some(head)) => {
                bail!("store HEAD references {head}, but newest checkpoint is {newest}")
            }
        }
        let known = ids.into_iter().collect::<BTreeSet<_>>();
        for (directory, description) in [("labels", "label"), ("pins", "pin")] {
            for entry in fs::read_dir(self.root.join(directory))? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                if !valid_checkpoint_id(&name) || !known.contains(&name) {
                    bail!("snapshot store contains an orphan or invalid checkpoint {description}");
                }
                ensure_regular_file(&entry.path(), &format!("checkpoint {description}"))?;
            }
        }
        Ok(())
    }
}
