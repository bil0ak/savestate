use super::*;

impl Savestate {
    pub fn restore(&mut self, id: Option<&str>, dry_run: bool, yes: bool) -> Result<()> {
        self.ensure_no_journal()?;
        if id.is_none() && !yes && interactive::TerminalRestorePrompter::is_available() {
            let mut prompts = interactive::TerminalRestorePrompter::new();
            return self.restore_interactive_with(dry_run, &mut prompts);
        }

        let mut plan = self.prepare_restore(id.unwrap_or("latest"))?;
        loop {
            self.render_restore_preview(&plan);
            if dry_run {
                ui::line(format_args!("{}", ui::muted("Dry run: no changes made.")));
                return Ok(());
            }
            if !yes && !confirm_restore()? {
                print_restore_cancelled();
                return Ok(());
            }
            confirm_remote_targets(&plan.remote_confirmations)?;
            let refreshed = self.prepare_restore(&plan.target.id)?;
            if plan.equivalent(&refreshed) {
                return self.apply_restore_plan(refreshed);
            }
            ui::warning(format_args!(
                "Project or database state changed while the preview was open; review the refreshed preview"
            ));
            if yes {
                bail!(
                    "restore scope changed after preview; no changes were made; rerun the command"
                );
            }
            plan = refreshed;
        }
    }

    pub(crate) fn restore_interactive_with(
        &mut self,
        dry_run: bool,
        prompts: &mut dyn interactive::RestorePrompter,
    ) -> Result<()> {
        let columns = prompts.columns();
        let choices = self.restore_choices(columns)?;
        if choices.is_empty() {
            bail!("no checkpoints exist; run `savestate create` first");
        }

        loop {
            let Some(selection) = prompts.choose_checkpoint(&choices)? else {
                print_restore_cancelled();
                return Ok(());
            };
            let choice = choices
                .get(selection)
                .context("checkpoint picker returned an invalid selection")?;
            let plan = self.prepare_restore(&choice.id)?;
            self.render_restore_preview(&plan);
            if dry_run {
                ui::line(format_args!("{}", ui::muted("Dry run: no changes made.")));
                return Ok(());
            }

            let summary = interactive::RestoreSummary {
                added: plan.files.added.len(),
                removed: plan.files.removed.len(),
                modified: plan.files.modified.len(),
            };
            match prompts.choose_action(summary)? {
                interactive::RestoreDecision::Restore => {
                    confirm_remote_targets(&plan.remote_confirmations)?;
                    let refreshed = self.prepare_restore(&plan.target.id)?;
                    if plan.equivalent(&refreshed) {
                        return self.apply_restore_plan(refreshed);
                    }
                    ui::warning(format_args!(
                        "Project or database state changed while the preview was open; review the refreshed preview"
                    ));
                }
                interactive::RestoreDecision::Back => {}
                interactive::RestoreDecision::Cancel => {
                    print_restore_cancelled();
                    return Ok(());
                }
            }
        }
    }

    pub(crate) fn restore_choices(
        &self,
        columns: usize,
    ) -> Result<Vec<interactive::CheckpointChoice>> {
        self.store
            .ids()?
            .into_iter()
            .map(|id| {
                let manifest = self.store.load(&id)?;
                Ok(interactive::CheckpointChoice::from_manifest(
                    &manifest, columns,
                ))
            })
            .collect()
    }

    pub(super) fn prepare_restore(&self, id: &str) -> Result<PreparedRestore> {
        let target = self.store.load(id)?;
        self.store.verify(&target)?;
        self.validate_target(&target)?;
        let current = self.capture_ephemeral(
            Some(&target.capture_scope),
            DatabaseCapture::Matching(&target),
        )?;
        let mut remote_confirmations = Vec::new();
        for artifact in &target.services {
            if let ServiceArtifact::Postgres { name, url_env, .. } = artifact {
                let adapter = self.postgres_for(name, url_env)?;
                adapter.ensure_restore_allowed(artifact)?;
                if let Some(token) = adapter.remote_confirmation_token(artifact)? {
                    remote_confirmations.push(token);
                }
            }
        }
        remote_confirmations.sort();
        remote_confirmations.dedup();
        Ok(PreparedRestore {
            files: filesystem::diff_entries(&current.files, &target.files),
            services: diff_services(&current.services, &target.services),
            current_owned: current.files,
            preserved: current.capture_scope.ignored_boundaries,
            remote_confirmations,
            target,
        })
    }

    #[allow(clippy::unused_self)] // Preview rendering remains grouped with restore orchestration.
    pub(super) fn render_restore_preview(&self, plan: &PreparedRestore) {
        ui::field(
            "Restore",
            format_args!(
                "{} — {} {} {} filesystem paths; {} experimental database resources",
                ui::checkpoint(plan.target.id.as_str()),
                ui::added(format!("+{}", plan.files.added.len())),
                ui::removed(format!("-{}", plan.files.removed.len())),
                ui::modified(format!("~{}", plan.files.modified.len())),
                plan.target.services.len()
            ),
        );
        for root in plan.target.roots.iter().filter(|root| !root.repository) {
            let prefix = format!("{}:", root.id);
            let changed = plan
                .files
                .added
                .iter()
                .chain(&plan.files.removed)
                .chain(&plan.files.modified)
                .any(|path| path.starts_with(&prefix));
            if changed
                || plan.target.services.iter().any(
                    |service| matches!(service, ServiceArtifact::Sqlite { root_id, .. } if root_id == &root.id),
                )
            {
                ui::field(
                    "External root",
                    format_args!("{} → {}", root.id, root.path.display()),
                );
            }
        }
        for artifact in &plan.target.services {
            match artifact {
                ServiceArtifact::Sqlite { root_id, path, .. } => {
                    if let Ok(root) = root_path(&plan.target.roots, root_id) {
                        ui::field(
                            "SQLite target",
                            format_args!("{}", root.join(path).display()),
                        );
                    }
                }
                ServiceArtifact::Postgres {
                    name,
                    database,
                    target_identity: Some(identity),
                    ..
                } => ui::field(
                    "PostgreSQL target",
                    format_args!(
                        "{}: {} at {}:{} (cluster {})",
                        name, database, identity.host, identity.port, identity.system_identifier
                    ),
                ),
                ServiceArtifact::Postgres { .. } => {}
            }
        }
        render_restore_path_samples(&plan.files, &plan.target.id);
        for service in &plan.services {
            ui::line(format_args!("{}: {}", service.name, service.summary));
        }
    }

    pub(super) fn apply_restore_plan(&mut self, plan: PreparedRestore) -> Result<()> {
        let _lock = self.store.lock()?;
        self.ensure_no_journal()?;
        let refreshed = self.prepare_restore(&plan.target.id)?;
        if !plan.equivalent(&refreshed) {
            bail!("state changed before the restore lock was acquired; preview again");
        }
        self.ensure_transaction_namespace_available(&refreshed.target.id)?;
        let recovery = self.create_internal_locked(CreateRequest {
            label: Some(format!("pre-restore:{}", plan.target.id)),
            if_changed: false,
            presentation: Presentation::Announce,
            include_ignored: false,
            recorded_scope: Some(&plan.target.capture_scope),
            database_capture: DatabaseCapture::Matching(&plan.target),
            kind: CheckpointKind::Recovery,
            retention: RetentionMode::Defer,
        })?;
        self.execute_restore(
            refreshed.target,
            recovery,
            refreshed.preserved,
            refreshed.current_owned,
        )
    }
}
