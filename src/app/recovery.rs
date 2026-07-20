use super::*;

impl Savestate {
    pub fn recover(&mut self, rollback: bool, resume: bool) -> Result<()> {
        let _lock = self.store.lock()?;
        let Some(mut journal) = self.store.journal()? else {
            ui::line(format_args!("{}", ui::muted("No interrupted restore.")));
            return Ok(());
        };
        self.validate_restore_journal(&journal)?;
        if !rollback && !resume {
            bail!("choose --rollback or --resume");
        }
        if journal.phase == JournalPhase::Complete {
            if rollback {
                bail!(
                    "restore {} was already verified and committed; use `savestate recover --resume` to finish cleanup",
                    journal.target_id
                );
            }
            self.finalize_database_swaps(&journal)?;
            restore::cleanup(&journal)?;
            self.remove_transaction_directories(&journal)?;
            self.store.clear_journal()?;
            ui::success(format_args!("Completed restore cleanup"));
            return Ok(());
        }
        self.rollback_database_swaps(&journal)?;
        restore::rollback_filesystem(&self.store, &mut journal)?;
        let recovery = self.store.load(&journal.recovery_id)?;
        filesystem::reapply_manifest_metadata(&recovery.files, &recovery.roots)?;
        filesystem::sync_manifest_data(&recovery.files, &recovery.roots)?;
        self.verify_live_state(&recovery, &journal)?;
        restore::cleanup(&journal)?;
        self.remove_transaction_directories(&journal)?;
        self.store.clear_journal()?;
        if resume {
            let target = self.store.load(&journal.target_id)?;
            self.store.verify(&target)?;
            self.validate_target(&target)?;
            let recovery_id = self.create_internal_locked(CreateRequest {
                label: Some(format!("pre-resume:{}", target.id)),
                if_changed: false,
                presentation: Presentation::Announce,
                include_ignored: false,
                recorded_scope: Some(&target.capture_scope),
                database_capture: DatabaseCapture::Matching(&target),
                kind: CheckpointKind::Recovery,
                retention: RetentionMode::Defer,
            })?;
            let current = self.capture_ephemeral(
                Some(&target.capture_scope),
                DatabaseCapture::Matching(&target),
            )?;
            self.execute_restore(
                target,
                recovery_id,
                current.capture_scope.ignored_boundaries,
                current.files,
            )
        } else {
            ui::success(format_args!(
                "Rolled back interrupted restore to {}",
                ui::checkpoint(recovery.id.as_str())
            ));
            Ok(())
        }
    }

    #[allow(clippy::too_many_lines)] // All recovery ownership checks remain visibly fail-closed.
    pub(super) fn validate_restore_journal(&self, journal: &RestoreJournal) -> Result<()> {
        if !(1..=5).contains(&journal.version) {
            bail!("unsupported restore journal version {}", journal.version);
        }
        let target = self.store.load(&journal.target_id)?;
        let recovery = self.store.load(&journal.recovery_id)?;
        if target.id != journal.target_id || recovery.id != journal.recovery_id {
            bail!("restore journal checkpoint IDs must be complete, exact ULIDs");
        }
        self.validate_journal_roots(&target)?;
        self.validate_journal_roots(&recovery)?;
        if target
            .roots
            .iter()
            .map(|root| (&root.id, &root.path))
            .ne(recovery.roots.iter().map(|root| (&root.id, &root.path)))
        {
            bail!("restore journal checkpoints describe different filesystem roots");
        }

        let transaction_parent = self.root.join(".savestate-transaction");
        let transaction = journal_transaction_root(journal, &transaction_parent)?;
        if journal.version >= 3 && transaction != transaction_parent.join(&journal.target_id) {
            bail!("restore journal uses an unexpected transaction directory");
        }
        let expected_preservation_root = transaction.join("preserved");
        if !journal.preservation_root.as_os_str().is_empty()
            && journal.preservation_root != expected_preservation_root
        {
            bail!("restore journal has an unsafe preservation directory");
        }
        validate_directory_chain(&self.root, &transaction_parent)?;
        validate_directory_chain(&transaction_parent, &transaction)?;

        let mut live_paths = BTreeSet::new();
        let mut staging_paths = BTreeSet::new();
        for swap in &journal.swaps {
            let (root, relative) = journal_swap_root(&target.roots, swap)?;
            validate_journal_relative(&relative, root.repository)?;
            let expected_live = root.path.join(&relative);
            if swap.live != expected_live {
                bail!("restore journal live path is outside its filesystem root");
            }
            if root.repository
                && (swap.live == self.store.path()
                    || swap.live.starts_with(self.store.path())
                    || self.store.path().starts_with(&swap.live)
                    || swap.live.starts_with(self.root.join(".git"))
                    || swap
                        .live
                        .starts_with(self.root.join(".savestate-transaction")))
            {
                bail!("restore journal swap overlaps a mandatory safety exclusion");
            }
            let (expected_staged, expected_old) = if root.repository {
                if journal.version >= 3 {
                    (
                        transaction.join("new").join(&root.id).join(&relative),
                        transaction.join("old/repo").join(&relative),
                    )
                } else {
                    (
                        transaction.join("new").join(&relative),
                        transaction.join("old").join(&relative),
                    )
                }
            } else {
                let parent = root.path.parent().context("external root has no parent")?;
                let name = root
                    .path
                    .file_name()
                    .context("external root has no filename")?
                    .to_string_lossy();
                let suffix = journal.target_id.chars().take(10).collect::<String>();
                (
                    parent.join(format!(".{name}.savestate-new-{suffix}")),
                    parent.join(format!(".{name}.savestate-old-{suffix}")),
                )
            };
            if swap.staged != expected_staged || swap.old != expected_old {
                bail!("restore journal contains an unexpected staging or rollback path");
            }
            if root.repository {
                validate_directory_chain(&root.path, swap.live.parent().unwrap_or(&root.path))?;
                validate_directory_chain(
                    &transaction,
                    swap.staged.parent().context("staging path has no parent")?,
                )?;
                validate_directory_chain(
                    &transaction,
                    swap.old.parent().context("rollback path has no parent")?,
                )?;
            } else {
                let parent = root.path.parent().context("external root has no parent")?;
                validate_directory_chain(parent, parent)?;
            }
            if !live_paths.insert(&swap.live)
                || !staging_paths.insert(&swap.staged)
                || !staging_paths.insert(&swap.old)
            {
                bail!("restore journal contains duplicate filesystem paths");
            }
            if journal.version >= 3 {
                let target_owns = if root.repository {
                    target.files.iter().any(|entry| {
                        entry.root_id == root.id
                            && (entry.path == relative || entry.path.starts_with(&relative))
                    })
                } else {
                    true
                };
                if swap.target_existed != target_owns {
                    bail!("restore journal target-existence flag is inconsistent");
                }
                let recovery_owns = recovery.files.iter().any(|entry| {
                    entry.root_id == root.id
                        && (entry.path == relative || entry.path.starts_with(&relative))
                });
                if !swap.live_existed && recovery_owns {
                    bail!("restore journal could delete checkpoint-owned recovery state");
                }
            }
        }

        let mut preserved_live = BTreeSet::new();
        for record in &journal.preserved_paths {
            let (root, relative) = target
                .roots
                .iter()
                .find_map(|root| {
                    record
                        .live
                        .strip_prefix(&root.path)
                        .ok()
                        .filter(|relative| !relative.as_os_str().is_empty())
                        .map(|relative| (root, relative.to_path_buf()))
                })
                .context("preserved path is outside configured filesystem roots")?;
            validate_journal_relative(&relative, false)?;
            if root.repository
                && (record.live.starts_with(self.root.join(".git"))
                    || record.live.starts_with(self.store.path())
                    || record.live.starts_with(&transaction_parent))
            {
                bail!("restore journal attempts to preserve a mandatory safety exclusion");
            }
            let expected_held = expected_preservation_root.join(&root.id).join(&relative);
            if record.held != expected_held || !preserved_live.insert(&record.live) {
                bail!("restore journal contains an unsafe or duplicate preserved path");
            }
            let source = if filesystem::path_exists(&record.held)? {
                &record.held
            } else if filesystem::path_exists(&record.live)? {
                &record.live
            } else {
                bail!("preserved path is missing from both live and held locations");
            };
            if source == &record.held {
                validate_directory_chain(
                    &transaction,
                    source.parent().context("held path has no parent")?,
                )?;
            } else {
                validate_directory_chain(
                    &root.path,
                    source
                        .parent()
                        .context("live preserved path has no parent")?,
                )?;
            }
            let metadata = fs::symlink_metadata(source)?;
            if !filesystem::recorded_scope_ignores(
                &root.id,
                &root.path,
                &relative,
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                &target.capture_scope,
            )? {
                bail!("restore journal attempts to preserve a checkpoint-owned path");
            }
        }

        let postgres = target
            .services
            .iter()
            .filter_map(|artifact| match artifact {
                ServiceArtifact::Postgres { name, url_env, .. } => {
                    Some(((name.as_str(), url_env.as_str()), artifact))
                }
                ServiceArtifact::Sqlite { .. } => None,
            })
            .collect::<BTreeMap<_, _>>();
        if journal.version < 5 && !journal.database_swaps.is_empty() {
            bail!(
                "legacy PostgreSQL journal has no database OID ownership records; automatic destructive recovery is refused"
            );
        }
        if postgres.len() != journal.database_swaps.len() {
            bail!("restore journal PostgreSQL resources do not match the checkpoint");
        }
        for swap in &journal.database_swaps {
            let artifact = postgres
                .get(&(swap.name.as_str(), swap.url_env.as_str()))
                .context("restore journal references an unknown PostgreSQL resource")?;
            let expected =
                expected_database_swap(artifact, &journal.target_id, swap.operation_id.as_deref())?;
            if swap.database != expected.database
                || swap.temporary_database != expected.temporary_database
                || swap.old_database != expected.old_database
                || swap.system_identifier != expected.system_identifier
            {
                bail!("restore journal PostgreSQL identity is inconsistent");
            }
            if swap.cutover_owned && !swap.temporary_owned {
                bail!("restore journal PostgreSQL ownership state is inconsistent");
            }
            if swap.original_database_oid.is_none()
                || swap.temporary_owned != swap.temporary_database_oid.is_some()
                || swap.original_database_oid == swap.temporary_database_oid
                || swap.original_database_oid == Some(0)
                || swap.temporary_database_oid == Some(0)
            {
                bail!("restore journal PostgreSQL OID ownership state is inconsistent");
            }
            if journal.phase == JournalPhase::Prepared && swap.cutover_owned {
                bail!("prepared restore journal contains a PostgreSQL cutover intent");
            }
            if journal.phase == JournalPhase::Complete
                && (!swap.temporary_owned || !swap.cutover_owned)
            {
                bail!("completed restore journal has incomplete PostgreSQL ownership state");
            }
        }
        Ok(())
    }

    pub(super) fn validate_journal_roots(&self, manifest: &SnapshotManifest) -> Result<()> {
        if manifest.project_root != self.root {
            bail!("restore journal checkpoint belongs to another project");
        }
        let expected = std::iter::once(("repo".to_owned(), self.root.clone(), true))
            .chain(
                self.config
                    .external_paths
                    .iter()
                    .enumerate()
                    .map(|(index, path)| (format!("external-{index}"), path.clone(), false)),
            )
            .collect::<Vec<_>>();
        if manifest.roots.len() != expected.len()
            || manifest
                .roots
                .iter()
                .zip(expected)
                .any(|(root, (id, path, repository))| {
                    root.id != id || root.path != path || root.repository != repository
                })
        {
            bail!("restore journal checkpoint roots differ from project configuration");
        }
        Ok(())
    }

    pub(super) fn remove_transaction_directories(&self, journal: &RestoreJournal) -> Result<()> {
        let parent = self.root.join(".savestate-transaction");
        let transaction = journal_transaction_root(journal, &parent)?;
        if filesystem::path_exists(&transaction)? {
            filesystem::remove_path(&transaction)?;
        }
        if filesystem::path_exists(&parent)? {
            fs::remove_dir(&parent).with_context(|| {
                format!(
                    "restore transaction directory is not empty: {}",
                    parent.display()
                )
            })?;
        }
        Ok(())
    }
}
