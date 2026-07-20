use super::*;

impl Store {
    pub fn pin(&self, requested: &str) -> Result<String> {
        self.heal_catalog()?;
        let id = self.resolve_id(requested)?;
        let path = self.root.join("pins").join(&id);
        ensure_absent_or_regular_file(&path, "checkpoint pin")?;
        atomic_write(&path, b"pinned\n")?;
        Ok(id)
    }

    pub fn unpin(&self, requested: &str) -> Result<String> {
        self.heal_catalog()?;
        let id = self.resolve_id(requested)?;
        let path = self.root.join("pins").join(&id);
        if path_exists(&path)? {
            ensure_regular_file(&path, "checkpoint pin")?;
            fs::remove_file(path)?;
            sync_directory(&self.root.join("pins"))?;
        }
        Ok(id)
    }

    pub fn is_pinned(&self, id: &str) -> Result<bool> {
        let path = self.root.join("pins").join(id);
        if !path_exists(&path)? {
            return Ok(false);
        }
        ensure_regular_file(&path, "checkpoint pin")?;
        Ok(true)
    }

    pub fn prune_with_policy(
        &self,
        policy: &RetentionPolicy,
        dry_run: bool,
    ) -> Result<Vec<String>> {
        self.heal_catalog()?;
        let mut by_kind = std::collections::BTreeMap::<CheckpointKind, Vec<String>>::new();
        for id in self.ids()? {
            if self.is_pinned(&id)? {
                continue;
            }
            let manifest = self.load(&id)?;
            by_kind
                .entry(manifest.resolved_kind())
                .or_default()
                .push(id);
        }
        let mut removed = Vec::new();
        for (kind, ids) in by_kind {
            let keep = match kind {
                CheckpointKind::Manual => continue,
                CheckpointKind::Agent => policy.agent,
                CheckpointKind::Recovery => policy.recovery,
                CheckpointKind::Run => policy.run,
            };
            removed.extend(ids.into_iter().skip(keep));
        }
        removed.sort();
        if dry_run {
            return Ok(removed);
        }
        for id in &removed {
            self.move_head_before_removal(id)?;
            self.retire_checkpoint(id)
                .with_context(|| format!("prune checkpoint {id}"))?;
        }
        self.gc_objects()?;
        Ok(removed)
    }

    pub fn prune(&self, keep: usize, dry_run: bool) -> Result<Vec<String>> {
        let policy = RetentionPolicy {
            agent: keep,
            recovery: keep,
            run: keep,
        };
        self.prune_with_policy(&policy, dry_run)
    }

    pub(super) fn gc_objects(&self) -> Result<()> {
        let mut live = BTreeSet::new();
        for id in self.ids()? {
            let manifest = self.load(&id)?;
            for file in manifest.files {
                if let Some(hash) = file.object_hash {
                    live.insert(hash);
                }
            }
            for service in manifest.services {
                live.insert(service.object_hash().to_owned());
            }
        }
        for prefix in fs::read_dir(self.root.join("objects"))? {
            let prefix = prefix?;
            let prefix_name = prefix.file_name().to_string_lossy().into_owned();
            if prefix_name.starts_with('.') {
                continue;
            }
            if prefix_name.len() != 2 || !prefix_name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                bail!("object store contains invalid prefix directory {prefix_name:?}");
            }
            ensure_real_directory_existing(&prefix.path(), "object prefix")?;
            for object in fs::read_dir(prefix.path())? {
                let object = object?;
                let name = object.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    // Staging debris from a crashed capture; the exclusive
                    // lock guarantees no live writer owns it.
                    ensure_regular_file(&object.path(), "object staging file")?;
                    fs::remove_file(object.path())?;
                    continue;
                }
                if !valid_hash(&name) || !name.starts_with(&prefix_name) {
                    bail!("object store contains invalid object {name:?}");
                }
                ensure_regular_file(&object.path(), "content object")?;
                if !live.contains(&name) {
                    fs::remove_file(object.path())?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn move_head_before_removal(&self, id: &str) -> Result<()> {
        let head = self.root.join("HEAD");
        let ids = self.ids()?;
        let current_head = self.head_id()?;
        if let Some(current) = &current_head
            && !ids.contains(current)
        {
            bail!("store HEAD references missing checkpoint {current}");
        }
        if current_head.is_none_or(|current| current != id) {
            return Ok(());
        }
        if let Some(next) = ids.into_iter().find(|candidate| candidate != id) {
            atomic_write(&head, next.as_bytes())?;
        } else if path_exists(&head)? {
            fs::remove_file(&head)?;
            sync_directory(&self.root)?;
        }
        Ok(())
    }

    pub(super) fn retire_checkpoint(&self, id: &str) -> Result<()> {
        if !valid_checkpoint_id(id) {
            bail!("invalid checkpoint ID {id:?}");
        }
        let snapshots = self.root.join("snapshots");
        let source = snapshots.join(id);
        let retiring = snapshots.join(format!(".{id}.deleting"));
        if path_exists(&retiring)? {
            bail!(
                "checkpoint retirement path already exists: {}",
                retiring.display()
            );
        }
        // Metadata goes first so a crash can only strand a label-less
        // checkpoint, never an orphan label or pin.
        for directory in ["labels", "pins"] {
            let path = self.root.join(directory).join(id);
            if path_exists(&path)? {
                ensure_regular_file(&path, "checkpoint metadata")?;
                fs::remove_file(path)?;
                sync_directory(&self.root.join(directory))?;
            }
        }
        fs::rename(&source, &retiring)?;
        sync_directory(&snapshots)?;
        fs::remove_dir_all(retiring)?;
        sync_directory(&snapshots)
    }

    pub(super) fn head_id(&self) -> Result<Option<String>> {
        let path = self.root.join("HEAD");
        if !path_exists(&path)? {
            return Ok(None);
        }
        ensure_regular_file(&path, "store HEAD")?;
        let id = fs::read_to_string(&path)?;
        let id = id.trim();
        if !valid_checkpoint_id(id) {
            bail!("store HEAD contains invalid checkpoint ID {id:?}");
        }
        Ok(Some(id.to_owned()))
    }
}
