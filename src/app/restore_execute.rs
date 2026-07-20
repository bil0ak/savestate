use super::*;

impl Savestate {
    #[allow(clippy::too_many_lines)] // Journaled transaction ordering is intentionally linear.
    pub(super) fn execute_restore(
        &mut self,
        target: SnapshotManifest,
        recovery: String,
        preserved: Vec<ScopedPath>,
        current_owned: Vec<FileEntry>,
    ) -> Result<()> {
        self.ensure_transaction_namespace_available(&target.id)?;
        let transaction_parent = self.root.join(".savestate-transaction");
        let txn = transaction_parent.join(&target.id);
        let swaps = self.plan_filesystem_swaps(&target, &txn)?;
        let preserved_paths = restore::plan_preservation(&target, &preserved, &txn)?;
        // Preservation moves ignored state into the transaction directory by
        // rename, which cannot cross filesystems; refuse up front instead of
        // failing mid-commit.
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let transaction_device = fs::metadata(&self.root)?.dev();
            for record in &preserved_paths {
                let Ok(metadata) = fs::symlink_metadata(&record.live) else {
                    continue;
                };
                if metadata.dev() != transaction_device {
                    bail!(
                        "ignored path {} is on a different filesystem than the project root, so its state cannot be preserved through a restore; exclude it or move it before restoring",
                        record.live.display()
                    );
                }
            }
        }
        let database_swaps = self.plan_database_swaps(&target)?;
        let mut journal = RestoreJournal {
            version: 5,
            target_id: target.id.clone(),
            recovery_id: recovery,
            phase: JournalPhase::Prepared,
            commit_started: false,
            swaps,
            database_swaps,
            preserved_paths,
            preservation_root: txn.join("preserved"),
        };
        self.store.write_journal(&journal)?;
        let result = (|| -> Result<()> {
            fs::create_dir_all(txn.join("new"))?;
            fs::create_dir_all(txn.join("old"))?;
            self.materialize_filesystem_swaps(&target, &journal)?;
            for artifact in &target.services {
                if let ServiceArtifact::Postgres { name, url_env, .. } = artifact {
                    let index = journal
                        .database_swaps
                        .iter()
                        .position(|swap| &swap.name == name && &swap.url_env == url_env)
                        .context("PostgreSQL restore journal entry is missing")?;
                    let adapter = self.postgres_for(name, url_env)?;
                    adapter.preflight_staging(
                        artifact,
                        &self.store,
                        &journal.database_swaps[index],
                    )?;
                    let temporary_oid = adapter.create_staged_database(
                        artifact,
                        &self.store,
                        &journal.database_swaps[index],
                    )?;
                    journal.database_swaps[index].temporary_database_oid = Some(temporary_oid);
                    journal.database_swaps[index].temporary_owned = true;
                    self.store.write_journal(&journal)?;
                    adapter.populate_staged(
                        artifact,
                        &self.store,
                        &journal.database_swaps[index],
                    )?;
                }
            }
            journal.transition_to(JournalPhase::Committing)?;
            journal.commit_started = true;
            self.store.write_journal(&journal)?;
            restore::commit_filesystem(&self.store, &mut journal, &target, &current_owned)?;
            for artifact in &target.services {
                match artifact {
                    ServiceArtifact::Sqlite { root_id, path, .. } => {
                        let root = root_path(&target.roots, root_id)?;
                        SqliteAdapter::from_artifact(root, root_id, path)
                            .restore(artifact, &self.store)?;
                    }
                    ServiceArtifact::Postgres { name, url_env, .. } => {
                        let index = journal
                            .database_swaps
                            .iter()
                            .position(|swap| &swap.name == name && &swap.url_env == url_env)
                            .context("PostgreSQL restore journal entry is missing")?;
                        journal.database_swaps[index].cutover_owned = true;
                        self.store.write_journal(&journal)?;
                        self.postgres_for(name, url_env)?
                            .commit_staged(&journal.database_swaps[index])?;
                    }
                }
            }
            filesystem::reapply_manifest_metadata(&target.files, &target.roots)?;
            filesystem::sync_manifest_data(&target.files, &target.roots)?;
            self.verify_live_state(&target, &journal)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                journal.transition_to(JournalPhase::Complete)?;
                self.store.write_journal(&journal)?;
                self.finalize_database_swaps(&journal)?;
                restore::cleanup(&journal)?;
                self.remove_transaction_directories(&journal)?;
                self.store.clear_journal()?;
                ui::success(format_args!(
                    "Restored {}",
                    ui::checkpoint(target.id.as_str())
                ));
                Ok(())
            }
            Err(error) => {
                ui::warning(format_args!(
                    "restore failed; rolling filesystem state back: {error:#}"
                ));
                journal.transition_to(JournalPhase::RollingBack)?;
                self.store.write_journal(&journal)?;
                let mut compensation_errors = Vec::new();
                if let Err(compensation) = self.rollback_database_swaps(&journal) {
                    compensation_errors.push(format!("PostgreSQL: {compensation:#}"));
                }
                if let Err(compensation) = restore::rollback_filesystem(&self.store, &mut journal) {
                    compensation_errors.push(format!("filesystem: {compensation:#}"));
                }
                let recovery = self.store.load(&journal.recovery_id)?;
                if let Err(metadata_error) =
                    filesystem::reapply_manifest_metadata(&recovery.files, &recovery.roots)
                {
                    compensation_errors.push(format!("metadata: {metadata_error:#}"));
                }
                if compensation_errors.is_empty()
                    && let Err(durability) =
                        filesystem::sync_manifest_data(&recovery.files, &recovery.roots)
                {
                    compensation_errors.push(format!("durability: {durability:#}"));
                }
                if compensation_errors.is_empty()
                    && let Err(verification) = self.verify_live_state(&recovery, &journal)
                {
                    compensation_errors.push(format!("verification: {verification:#}"));
                }
                if compensation_errors.is_empty() {
                    restore::cleanup(&journal)?;
                    self.remove_transaction_directories(&journal)?;
                    self.store.clear_journal()?;
                    Err(error
                        .context("restore failed; the verified pre-restore state was reinstated"))
                } else {
                    ui::error(format_args!(
                        "RESTORE INCOMPLETE — run `savestate recover --rollback`\n  {}",
                        compensation_errors.join("\n  ")
                    ));
                    Err(error.context("restore failed and compensation is incomplete; the recovery journal was retained"))
                }
            }
        }
    }

    pub(super) fn verify_live_state(
        &self,
        expected: &SnapshotManifest,
        journal: &RestoreJournal,
    ) -> Result<()> {
        let actual = self.capture_ephemeral(
            Some(&expected.capture_scope),
            DatabaseCapture::SqliteMatching(expected),
        )?;
        let files = filesystem::diff_entries(&expected.files, &actual.files);
        if !filesystem::entries_equivalent(&expected.files, &actual.files) {
            bail!(
                "live filesystem does not match checkpoint {} (+{:?} -{:?} ~{:?})",
                expected.id,
                files.added,
                files.removed,
                files.modified
            );
        }
        for artifact in &expected.services {
            match artifact {
                ServiceArtifact::Sqlite { root_id, path, .. } => {
                    SqliteAdapter::from_artifact(
                        root_path(&expected.roots, root_id)?,
                        root_id,
                        path,
                    )
                    .verify_live_restore(artifact, &self.store)?;
                }
                ServiceArtifact::Postgres { name, url_env, .. } => {
                    self.postgres_for(name, url_env)?
                        .verify_live_restore(artifact)?;
                }
            }
        }
        if !restore::preservation_is_live(journal)? {
            bail!("one or more ignored paths are not attached to the live tree");
        }
        Ok(())
    }

    pub(super) fn plan_database_swaps(
        &self,
        target: &SnapshotManifest,
    ) -> Result<Vec<DatabaseSwap>> {
        target
            .services
            .iter()
            .filter_map(|artifact| match artifact {
                ServiceArtifact::Postgres { name, url_env, .. } => Some(
                    self.postgres_for(name, url_env)
                        .and_then(|adapter| adapter.plan_swap(artifact, &target.id)),
                ),
                ServiceArtifact::Sqlite { .. } => None,
            })
            .collect()
    }

    pub(super) fn ensure_transaction_namespace_available(&self, target_id: &str) -> Result<()> {
        let parent = self.root.join(".savestate-transaction");
        let transaction = parent.join(target_id);
        if filesystem::path_exists(&transaction)? {
            bail!(
                "refusing to overwrite stale restore transaction {}; inspect it and run `savestate recover`",
                transaction.display()
            );
        }
        if filesystem::path_exists(&parent)? {
            let metadata = fs::symlink_metadata(&parent)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                bail!(
                    "restore transaction path is not a real directory: {}",
                    parent.display()
                );
            }
            if fs::read_dir(&parent)?.next().transpose()?.is_some() {
                bail!(
                    "restore transaction directory contains stale or ambiguous state: {}; inspect it before restoring",
                    parent.display()
                );
            }
        }
        Ok(())
    }

    pub(super) fn postgres_for(&self, name: &str, url_env: &str) -> Result<PostgresAdapter> {
        let config = self
            .config
            .postgres
            .iter()
            .find(|entry| entry.name == name && entry.url_env == url_env)
            .with_context(|| format!("PostgreSQL adapter {name} is not configured"))?;
        PostgresAdapter::new(config.clone())
    }

    pub(super) fn rollback_database_swaps(&self, journal: &RestoreJournal) -> Result<()> {
        let mut errors = Vec::new();
        for swap in journal.database_swaps.iter().rev() {
            if let Err(error) = self
                .postgres_for(&swap.name, &swap.url_env)
                .and_then(|adapter| adapter.rollback_staged(swap))
            {
                errors.push(format!("{}: {error:#}", swap.name));
            }
        }
        if !errors.is_empty() {
            bail!(
                "one or more PostgreSQL resources could not roll back:\n  {}",
                errors.join("\n  ")
            );
        }
        Ok(())
    }

    pub(super) fn finalize_database_swaps(&self, journal: &RestoreJournal) -> Result<()> {
        let mut errors = Vec::new();
        for swap in &journal.database_swaps {
            if let Err(error) = self
                .postgres_for(&swap.name, &swap.url_env)
                .and_then(|adapter| adapter.finalize_swap(swap))
            {
                errors.push(format!("{}: {error:#}", swap.name));
            }
        }
        if !errors.is_empty() {
            bail!(
                "one or more PostgreSQL resources could not finish cleanup:\n  {}",
                errors.join("\n  ")
            );
        }
        Ok(())
    }

    pub(super) fn plan_filesystem_swaps(
        &self,
        target: &SnapshotManifest,
        txn: &Path,
    ) -> Result<Vec<SwapRecord>> {
        let mut swaps = Vec::new();
        for root in &target.roots {
            let staged_root = txn.join("new").join(&root.id);
            if root.repository {
                let mut protected = BTreeSet::from([
                    PathBuf::from(".git"),
                    PathBuf::from(".savestate"),
                    PathBuf::from(".savestate-transaction"),
                ]);
                if let Ok(relative) = self.store.path().strip_prefix(&root.path)
                    && !relative.as_os_str().is_empty()
                {
                    protected.insert(relative.to_path_buf());
                }
                self.plan_repository_entry_swaps(
                    target,
                    root,
                    txn,
                    &staged_root,
                    Path::new(""),
                    &protected,
                    &mut swaps,
                )?;
            } else {
                let live = root.path.clone();
                let parent = live.parent().context("external root has no parent")?;
                let name = live
                    .file_name()
                    .context("external root has no filename")?
                    .to_string_lossy();
                let suffix = target.id.chars().take(10).collect::<String>();
                let external_staged = parent.join(format!(".{name}.savestate-new-{suffix}"));
                let external_old = parent.join(format!(".{name}.savestate-old-{suffix}"));
                if filesystem::path_exists(&external_staged)? {
                    bail!(
                        "refusing to overwrite ambiguous external restore staging path {}",
                        external_staged.display()
                    );
                }
                if filesystem::path_exists(&external_old)? {
                    bail!(
                        "refusing to overwrite ambiguous external restore rollback path {}",
                        external_old.display()
                    );
                }
                swaps.push(SwapRecord {
                    root_id: root.id.clone(),
                    root_path: root.path.clone(),
                    entry_relative: PathBuf::new(),
                    staged: external_staged,
                    old: external_old,
                    live_existed: filesystem::path_exists(&live)?,
                    target_existed: true,
                    live,
                    committed: false,
                });
            }
        }
        Ok(swaps)
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::self_only_used_in_recursion)] // Kept as a method with its planning peers.
    pub(super) fn plan_repository_entry_swaps(
        &self,
        target: &SnapshotManifest,
        root: &RootManifest,
        txn: &Path,
        staged_root: &Path,
        relative: &Path,
        protected: &BTreeSet<PathBuf>,
        swaps: &mut Vec<SwapRecord>,
    ) -> Result<()> {
        if protected.contains(relative) {
            return Ok(());
        }
        if protected
            .iter()
            .any(|path| path != relative && path.starts_with(relative))
        {
            let live = root.path.join(relative);
            let mut children = BTreeSet::new();
            if filesystem::path_exists(&live)? {
                let metadata = fs::symlink_metadata(&live)?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    bail!(
                        "mandatory restore exclusion descends through non-directory {}",
                        live.display()
                    );
                }
                for child in fs::read_dir(&live)? {
                    children.insert(child?.file_name());
                }
            }
            for entry in target.files.iter().filter(|entry| entry.root_id == root.id) {
                let Ok(suffix) = entry.path.strip_prefix(relative) else {
                    continue;
                };
                if let Some(component) = suffix.components().next() {
                    children.insert(component.as_os_str().to_owned());
                }
            }
            for child in children {
                self.plan_repository_entry_swaps(
                    target,
                    root,
                    txn,
                    staged_root,
                    &relative.join(child),
                    protected,
                    swaps,
                )?;
            }
            return Ok(());
        }

        let live = root.path.join(relative);
        let target_existed = target.files.iter().any(|entry| {
            entry.root_id == root.id && (entry.path == relative || entry.path.starts_with(relative))
        });
        swaps.push(SwapRecord {
            root_id: root.id.clone(),
            root_path: root.path.clone(),
            entry_relative: relative.to_path_buf(),
            staged: staged_root.join(relative),
            old: txn.join("old/repo").join(relative),
            live_existed: filesystem::path_exists(&live)?,
            target_existed,
            live,
            committed: false,
        });
        Ok(())
    }

    pub(super) fn materialize_filesystem_swaps(
        &self,
        target: &SnapshotManifest,
        journal: &RestoreJournal,
    ) -> Result<()> {
        for root in &target.roots {
            if root.repository {
                let staged_root = self
                    .root
                    .join(".savestate-transaction")
                    .join(&target.id)
                    .join("new")
                    .join(&root.id);
                filesystem::materialize_root(&target.files, &root.id, &staged_root, &self.store)?;
            } else {
                let swap = journal
                    .swaps
                    .iter()
                    .find(|swap| swap.root_id == root.id)
                    .with_context(|| format!("restore swap for root {} is missing", root.id))?;
                filesystem::materialize_root(&target.files, &root.id, &swap.staged, &self.store)?;
            }
        }
        Ok(())
    }
}
