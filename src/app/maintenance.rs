use super::*;

impl Savestate {
    /// Creates a checkpoint, runs a child command, and returns its process exit code.
    ///
    /// Signals are mapped to the conventional `128 + signal` code on Unix. The
    /// library never terminates the caller's process.
    pub fn run(&mut self, command: Vec<String>) -> Result<i32> {
        self.ensure_no_journal()?;
        let (program, args) = command.split_first().context("missing command")?;
        let program_label = Path::new(program)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("command");
        self.create_internal(CreateRequest {
            label: normalize_explicit_label(Some(format!("before:{program_label}")))?,
            if_changed: true,
            presentation: Presentation::Announce,
            include_ignored: false,
            recorded_scope: None,
            database_capture: DatabaseCapture::Configured,
            kind: CheckpointKind::Run,
            retention: RetentionMode::Apply,
        })?;
        let status = Command::new(program).args(args).status()?;
        #[cfg(unix)]
        let fallback = {
            use std::os::unix::process::ExitStatusExt;
            status.signal().map_or(1, |signal| 128 + signal)
        };
        #[cfg(not(unix))]
        let fallback = 1;
        Ok(status.code().unwrap_or(fallback))
    }

    pub fn integrate(&self, agent: integrations::Agent, remove: bool) -> Result<()> {
        let path = integrations::configure(&self.root, agent, remove)?;
        let name = match agent {
            integrations::Agent::Claude => "Claude",
            integrations::Agent::Codex => "Codex",
        };
        ui::success(format_args!(
            "{name} integration {}",
            if remove { "removed" } else { "installed" }
        ));
        ui::field("Configuration", format_args!("{}", path.display()));
        if !remove {
            ui::line(format_args!(""));
            ui::heading("Next");
            match agent {
                integrations::Agent::Codex => {
                    ui::line(format_args!("  1. Open this project in Codex"));
                    ui::line(format_args!("  2. Review and trust the Savestate hooks"));
                    ui::line(format_args!("  3. Start a new task"));
                    ui::line(format_args!(
                        "  4. Run {} after the first completed turn",
                        ui::command("savestate doctor")
                    ));
                }
                integrations::Agent::Claude => {
                    ui::line(format_args!("  1. Open this project in Claude Code"));
                    ui::line(format_args!("  2. Start a new session"));
                    ui::line(format_args!("  3. Complete a changed turn"));
                    ui::line(format_args!(
                        "  4. Run {} after the first completed turn",
                        ui::command("savestate doctor")
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn status(&self) -> Result<()> {
        self.store.validate_catalog()?;
        let (logical, allocated) = self.store.size()?;
        let sqlite_paths = self.sqlite_paths(DatabaseCapture::Configured)?;
        let discovery = filesystem::discover(&self.root, &self.config, false, None, &sqlite_paths)?;
        ui::heading("Savestate status");
        ui::field("Project", format_args!("{}", self.root.display()));
        ui::field("Store", format_args!("{}", self.store.path().display()));
        ui::field(
            "Store size",
            format_args!(
                "{} logical, {} allocated",
                human_bytes(logical),
                human_bytes(allocated)
            ),
        );
        let ids = self.store.ids()?;
        ui::field("Checkpoints", format_args!("{}", ids.len()));
        if let Some(latest) = ids.first() {
            let manifest = self.store.load(latest)?;
            ui::field(
                "Latest checkpoint",
                format_args!(
                    "{}{}",
                    ui::checkpoint(&manifest.id[..12]),
                    manifest
                        .label
                        .as_ref()
                        .map(|label| format!(" ({label})"))
                        .unwrap_or_default()
                ),
            );
        } else {
            ui::field("Latest checkpoint", format_args!("{}", ui::muted("none")));
        }
        ui::field(
            "Snapshot policy",
            format_args!(
                "Git ignore {} ({} explicit includes, {} explicit excludes)",
                if discovery
                    .git_fallback_notices
                    .iter()
                    .any(|notice| { notice.contains("because this is not a Git repository") })
                {
                    "inactive (.gitignore ignored: not a Git repository)"
                } else if !discovery.git_fallback_notices.is_empty() {
                    "inactive (Git unavailable; full filesystem scope)"
                } else if discovery.scope.respect_gitignore {
                    "enabled"
                } else {
                    "disabled"
                },
                discovery.scope.include.len(),
                discovery.scope.exclude.len()
            ),
        );
        ui::field(
            "Selected scope",
            format_args!(
                "{} paths, {} logical",
                discovery.logical_paths,
                human_bytes(discovery.logical_bytes)
            ),
        );
        ui::field(
            "Retention",
            format_args!(
                "manual kept; agent {}, recovery {}, run {}",
                self.config.retention.agent,
                self.config.retention.recovery,
                self.config.retention.run
            ),
        );
        ui::field(
            "Experimental databases",
            format_args!(
                "{} ({} SQLite, {} PostgreSQL configured)",
                if self.config.experimental.databases {
                    "enabled for manual checkpoints"
                } else {
                    "disabled"
                },
                self.config.sqlite.len(),
                self.config.postgres.len()
            ),
        );
        let journal_pending = self.store.journal()?.is_some();
        if journal_pending {
            ui::notice("Restore journal", format_args!("requires recovery"));
        } else {
            ui::ok("Restore journal", format_args!("clean"));
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // The diagnostic order mirrors the stable CLI report.
    pub fn doctor(&self) -> Result<()> {
        let mut failed = false;
        ui::heading("Savestate doctor");
        let marker = self.root.join(".savestate.toml");
        if marker.is_file() {
            ui::ok(
                "Project configuration",
                format_args!("ok ({})", marker.display()),
            );
        } else {
            failed = true;
            ui::failed(
                "Project configuration",
                format_args!(
                    "missing; run {} in {}",
                    ui::command("savestate init"),
                    self.root.display()
                ),
            );
        }

        match tempfile::NamedTempFile::new_in(self.store.path()) {
            Ok(file) => {
                drop(file);
                ui::ok(
                    "Filesystem store",
                    format_args!("writable ({})", self.store.path().display()),
                );
            }
            Err(error) => {
                failed = true;
                ui::failed("Filesystem store", format_args!("not writable ({error})"));
            }
        }

        match self
            .store
            .validate_catalog()
            .and_then(|()| self.store.ids())
            .and_then(|ids| {
                ids.first()
                    .map(|id| self.store.load(id))
                    .transpose()?
                    .map(|manifest| self.store.verify(&manifest))
                    .transpose()
            }) {
            Ok(Some(())) => ui::ok("Latest checkpoint", format_args!("integrity verified")),
            Ok(None) => ui::notice("Latest checkpoint", format_args!("none created yet")),
            Err(error) => {
                failed = true;
                ui::failed("Latest checkpoint", format_args!("{error:#}"));
            }
        }

        match self.store.journal()? {
            Some(journal) => match self.validate_restore_journal(&journal) {
                Ok(()) => {
                    failed = true;
                    ui::failed(
                        "Restore journal",
                        format_args!(
                            "valid interrupted {} transaction; run `savestate recover`",
                            journal.phase.as_str()
                        ),
                    );
                }
                Err(error) => {
                    failed = true;
                    ui::failed("Restore journal", format_args!("invalid: {error:#}"));
                }
            },
            None => ui::ok("Restore journal", format_args!("clean")),
        }

        let codex = integrations::is_configured(&self.root, integrations::Agent::Codex)?;
        let claude = integrations::is_configured(&self.root, integrations::Agent::Claude)?;
        if codex {
            ui::ok(
                "Codex integration",
                format_args!(
                    "installed ({})",
                    integrations::configuration_path(&self.root, integrations::Agent::Codex)
                        .display()
                ),
            );
        } else {
            ui::field(
                "Codex integration",
                format_args!("{}", ui::muted("not installed (optional)")),
            );
        }
        if claude {
            ui::ok(
                "Claude integration",
                format_args!(
                    "installed ({})",
                    integrations::configuration_path(&self.root, integrations::Agent::Claude)
                        .display()
                ),
            );
        } else {
            ui::field(
                "Claude integration",
                format_args!("{}", ui::muted("not installed (optional)")),
            );
        }

        if codex || claude {
            if savestate_on_path() {
                ui::ok(
                    "Agent command",
                    format_args!("{} is available on PATH", ui::command("`savestate`")),
                );
            } else {
                failed = true;
                ui::failed(
                    "Agent command",
                    format_args!(
                        "{} is not available on PATH; reinstall it before using hooks",
                        ui::command("`savestate`")
                    ),
                );
            }
        }

        if codex {
            let hooks = integrations::configuration_path(&self.root, integrations::Agent::Codex);
            match codex_trust_recorded(&hooks)? {
                Some(true) => ui::ok(
                    "Codex hook trust",
                    format_args!("recorded for all Savestate hooks"),
                ),
                Some(false) => {
                    failed = true;
                    ui::failed(
                        "Codex hook trust",
                        format_args!(
                            "not recorded; open Codex, trust the hooks, then start a new task"
                        ),
                    );
                }
                None => ui::notice(
                    "Codex hook trust",
                    format_args!(
                        "could not inspect Codex user configuration; verify the hooks in Codex"
                    ),
                ),
            }
        }

        if codex || claude {
            if self.has_automatic_checkpoint()? {
                ui::ok("Automatic checkpoints", format_args!("detected"));
            } else {
                ui::notice(
                    "Automatic checkpoints",
                    format_args!("none detected yet; complete a changed turn in a new agent task"),
                );
            }
            match integrations::hook_health(self)? {
                Some(health) if health.success => ui::ok(
                    "Hook health",
                    format_args!("last {} succeeded at {}", health.event, health.updated_at),
                ),
                Some(health) => {
                    failed = true;
                    ui::failed(
                        "Hook health",
                        format_args!(
                            "last {} failed at {}: {}",
                            health.event, health.updated_at, health.detail
                        ),
                    );
                }
                None => ui::notice(
                    "Hook health",
                    format_args!("no hook execution has been recorded yet"),
                ),
            }
        }

        if self.config.experimental.databases {
            let roots = filesystem::roots(&self.root, &self.config)?;
            for sqlite in &self.config.sqlite {
                let root = roots
                    .iter()
                    .find(|root| sqlite.path == root.path || sqlite.path.starts_with(&root.path))
                    .context("configured SQLite path is outside filesystem roots")?;
                let relative = sqlite.path.strip_prefix(&root.path)?;
                let adapter = SqliteAdapter::from_artifact(&root.path, &root.id, relative);
                match adapter.preflight() {
                    Ok(()) => ui::ok(
                        &format!("Experimental SQLite {}", sqlite.path.display()),
                        format_args!("quick_check and restore topology ok"),
                    ),
                    Err(error) => {
                        failed = true;
                        ui::failed(
                            &format!("Experimental SQLite {}", sqlite.path.display()),
                            format_args!("{error:#}"),
                        );
                    }
                }
            }
            for config in self.config.postgres.clone() {
                match PostgresAdapter::new(config.clone()).and_then(|adapter| {
                    adapter.preflight()?;
                    if config.allow_restore {
                        adapter.restore_preflight()?;
                    }
                    Ok(())
                }) {
                    Ok(()) => ui::ok(
                        &format!("Experimental PostgreSQL {}", config.name),
                        format_args!(
                            "preflight ok; restore {}; remote restore {}",
                            if config.allow_restore {
                                "enabled"
                            } else {
                                "disabled"
                            },
                            if config.allow_remote_restore {
                                "enabled"
                            } else {
                                "disabled"
                            }
                        ),
                    ),
                    Err(error) => {
                        failed = true;
                        ui::failed(
                            &format!("Experimental PostgreSQL {}", config.name),
                            format_args!("{error:#}"),
                        );
                    }
                }
            }
        } else if !self.config.sqlite.is_empty() || !self.config.postgres.is_empty() {
            ui::field(
                "Experimental database preflight",
                format_args!("{}", ui::muted("skipped (databases are disabled)")),
            );
        }
        if cfg!(windows) {
            ui::notice(
                "Windows support is experimental",
                format_args!("locked files and Unix metadata are limited"),
            );
        }
        if failed {
            bail!("one or more checks failed");
        }
        ui::success(format_args!("Doctor: all required checks passed"));
        Ok(())
    }

    pub(super) fn has_automatic_checkpoint(&self) -> Result<bool> {
        for id in self.store.ids()? {
            let manifest = self.store.load(&id)?;
            if manifest.resolved_kind() == CheckpointKind::Agent {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn prune(&self, dry_run: bool) -> Result<()> {
        self.ensure_no_journal()?;
        let _lock = self.store.lock()?;
        self.ensure_no_journal()?;
        let removed = self
            .store
            .prune_with_policy(&self.config.retention_policy(), dry_run)?;
        for id in &removed {
            if dry_run {
                ui::line(format_args!("Would prune {}", ui::checkpoint(id.as_str())));
            } else {
                ui::success(format_args!("Pruned {}", ui::checkpoint(id.as_str())));
            }
        }
        if removed.is_empty() {
            ui::line(format_args!("{}", ui::muted("Nothing to prune.")));
        }
        Ok(())
    }

    pub(super) fn ensure_no_journal(&self) -> Result<()> {
        if self.store.journal()?.is_some() {
            bail!("an interrupted restore requires `savestate recover --rollback` or `--resume`");
        }
        Ok(())
    }

    pub(super) fn validate_target(&self, target: &SnapshotManifest) -> Result<()> {
        if target.project_root != self.root {
            bail!(
                "checkpoint belongs to {}, not {}",
                target.project_root.display(),
                self.root.display()
            );
        }
        let current_roots = filesystem::roots(&self.root, &self.config)?;
        if current_roots
            .iter()
            .map(|root| (&root.id, &root.path))
            .collect::<Vec<_>>()
            != target
                .roots
                .iter()
                .map(|root| (&root.id, &root.path))
                .collect::<Vec<_>>()
        {
            bail!("configured filesystem roots differ from the checkpoint");
        }
        for recorded in &target.roots {
            let Some(expected) = &recorded.identity else {
                if !recorded.repository {
                    bail!(
                        "legacy checkpoint has no stable identity for external root {}; restore is refused",
                        recorded.id
                    );
                }
                continue;
            };
            if !recorded.repository
                && expected.file_id.is_none()
                && expected.birth_time_secs.is_none()
            {
                bail!(
                    "checkpoint has no stable file identity for external root {}; restore is refused",
                    recorded.id
                );
            }
            let current = current_roots
                .iter()
                .find(|root| root.id == recorded.id)
                .and_then(|root| root.identity.as_ref())
                .context("current filesystem root has no identity")?;
            if expected.canonical_path != current.canonical_path
                || expected.kind != current.kind
                || expected.device.is_some() && expected.device != current.device
                || expected.file_id.is_some() && expected.file_id != current.file_id
                || expected.birth_time_secs.is_some()
                    && (expected.birth_time_secs != current.birth_time_secs
                        || expected.birth_time_nanos != current.birth_time_nanos)
            {
                bail!(
                    "filesystem root {} changed identity: expected {} on device {:?} with file ID {:?} and birth time {:?}.{:?}, found {} on device {:?} with file ID {:?} and birth time {:?}.{:?}",
                    recorded.id,
                    expected.canonical_path.display(),
                    expected.device,
                    expected.file_id,
                    expected.birth_time_secs,
                    expected.birth_time_nanos,
                    current.canonical_path.display(),
                    current.device,
                    current.file_id,
                    current.birth_time_secs,
                    current.birth_time_nanos
                );
            }
        }
        if !target.services.is_empty() {
            if !self.config.experimental.databases {
                bail!(
                    "checkpoint contains experimental databases; set `[experimental] databases = true` before restoring it"
                );
            }
            self.sqlite_paths(DatabaseCapture::Matching(target))?;
            self.postgres_configs(DatabaseCapture::Matching(target))?;
        }
        Ok(())
    }
}
