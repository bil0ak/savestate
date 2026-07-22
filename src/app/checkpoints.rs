use super::*;

impl Savestate {
    pub fn create(&mut self, label: Option<String>, if_changed: bool) -> Result<String> {
        self.create_with_options(label, if_changed, false)
    }

    pub fn create_with_options(
        &mut self,
        label: Option<String>,
        if_changed: bool,
        include_ignored: bool,
    ) -> Result<String> {
        self.ensure_no_journal()?;
        self.create_internal(CreateRequest {
            label: normalize_explicit_label(label)?,
            if_changed,
            presentation: Presentation::Announce,
            include_ignored,
            recorded_scope: None,
            database_capture: DatabaseCapture::Configured,
            kind: CheckpointKind::Manual,
            retention: RetentionMode::Apply,
        })
        .map(|publication| publication.id)
    }

    pub fn create_filesystem_only(
        &mut self,
        label: Option<String>,
        if_changed: bool,
        include_ignored: bool,
    ) -> Result<String> {
        self.ensure_no_journal()?;
        self.create_internal(CreateRequest {
            label: normalize_explicit_label(label)?,
            if_changed,
            presentation: Presentation::Announce,
            include_ignored,
            recorded_scope: None,
            database_capture: DatabaseCapture::None,
            kind: CheckpointKind::Manual,
            retention: RetentionMode::Apply,
        })
        .map(|publication| publication.id)
    }

    pub(crate) fn create_quiet(
        &mut self,
        label: Option<String>,
        if_changed: bool,
    ) -> Result<String> {
        self.ensure_no_journal()?;
        self.create_internal(CreateRequest {
            label: normalize_explicit_label(label)?,
            if_changed,
            presentation: Presentation::Notices,
            include_ignored: false,
            recorded_scope: None,
            database_capture: DatabaseCapture::None,
            kind: CheckpointKind::Agent,
            retention: RetentionMode::Apply,
        })
        .map(|publication| publication.id)
    }

    pub(super) fn create_internal(
        &mut self,
        request: CreateRequest<'_>,
    ) -> Result<CheckpointPublication> {
        let _lock = self.store.lock()?;
        self.create_internal_locked(request)
    }

    #[allow(clippy::too_many_lines)] // Publication is kept as one auditable lock-bound sequence.
    pub(super) fn create_internal_locked(
        &mut self,
        request: CreateRequest<'_>,
    ) -> Result<CheckpointPublication> {
        let CreateRequest {
            label,
            if_changed,
            presentation,
            include_ignored,
            recorded_scope,
            database_capture,
            kind,
            retention,
        } = request;
        self.ensure_no_journal()?;
        self.store.heal_catalog()?;
        let git_exclude_notice = ensure_git_exclude(&self.root, self.store.path())?;
        let id = self.store.new_checkpoint_id()?;
        let started = Utc::now();
        let sqlite_paths = self.sqlite_paths(database_capture)?;
        let mut capture = filesystem::capture(
            &self.root,
            &self.config,
            &self.store,
            include_ignored,
            recorded_scope,
            &sqlite_paths,
        )?;
        if let Some(notice) = git_exclude_notice {
            capture.notices.insert(0, notice);
        }
        if !matches!(presentation, Presentation::Silent) {
            for notice in &capture.notices {
                ui::warning(format_args!("{notice}"));
            }
        }
        let mut services = Vec::new();
        for candidate in &capture.sqlite {
            services.push(
                SqliteAdapter {
                    candidate: candidate.clone(),
                }
                .snapshot(&self.store)?,
            );
        }
        for postgres in self.postgres_configs(database_capture)? {
            services.push(PostgresAdapter::new(postgres)?.snapshot(&self.store)?);
        }
        let manifest = SnapshotManifest {
            schema_version: SCHEMA_VERSION,
            id: id.clone(),
            created_at: Utc::now(),
            label,
            kind: Some(kind),
            project_root: self.root.clone(),
            platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            engine: capture.engine,
            consistency: "resource-consistent; transactionally-recoverable".into(),
            capture_started_at: started,
            capture_finished_at: Utc::now(),
            roots: capture.roots,
            files: capture.files,
            services,
            capture_scope: capture.scope,
        };
        if if_changed && !self.store.ids()?.is_empty() {
            let previous = self.store.load("latest")?;
            if previous.resolved_kind() == kind && snapshot_content_equal(&previous, &manifest) {
                if matches!(presentation, Presentation::Announce) {
                    ui::line(format_args!(
                        "No state changes; latest checkpoint is {}",
                        ui::checkpoint(previous.id.as_str())
                    ));
                }
                return Ok(CheckpointPublication {
                    id: previous.id,
                    created: false,
                });
            }
        }
        self.store.verify(&manifest)?;
        self.store.publish(&manifest)?;
        let removed = match retention {
            RetentionMode::Apply => self
                .store
                .prune_with_policy(&self.config.retention_policy(), false)?,
            RetentionMode::Defer => Vec::new(),
        };
        if matches!(presentation, Presentation::Announce) {
            ui::success(format_args!(
                "Created {}{}",
                ui::checkpoint(id.as_str()),
                manifest
                    .label
                    .as_ref()
                    .map(|label| format!(" ({label})"))
                    .unwrap_or_default()
            ));
            ui::line(format_args!(
                "Captured {} filesystem entries and {}{} databases via {}",
                manifest.files.len(),
                manifest.services.len(),
                if manifest.services.is_empty() {
                    ""
                } else {
                    " experimental"
                },
                manifest.engine
            ));
            if !removed.is_empty() {
                ui::line(format_args!(
                    "{} Pruned {} old checkpoints",
                    ui::muted("↳"),
                    removed.len()
                ));
            }
        }
        Ok(CheckpointPublication { id, created: true })
    }

    pub(super) fn sqlite_paths(&self, capture: DatabaseCapture<'_>) -> Result<BTreeSet<PathBuf>> {
        if matches!(capture, DatabaseCapture::None) || !self.config.experimental.databases {
            return Ok(BTreeSet::new());
        }
        let configured: BTreeSet<_> = self
            .config
            .sqlite
            .iter()
            .map(|sqlite| sqlite.path.clone())
            .collect();
        match capture {
            DatabaseCapture::None => Ok(BTreeSet::new()),
            DatabaseCapture::Configured => Ok(configured),
            DatabaseCapture::Matching(manifest) | DatabaseCapture::SqliteMatching(manifest) => manifest
                .services
                .iter()
                .filter_map(|service| match service {
                    ServiceArtifact::Sqlite { root_id, path, .. } => {
                        Some(root_path(&manifest.roots, root_id).map(|root| root.join(path)))
                    }
                    ServiceArtifact::Postgres { .. } => None,
                })
                .map(|path| {
                    let path = path?;
                    if !configured.contains(&path) {
                        bail!(
                            "SQLite database {} from the checkpoint is not explicitly configured",
                            path.display()
                        );
                    }
                    Ok(path)
                })
                .collect(),
        }
    }

    pub(super) fn postgres_configs(
        &self,
        capture: DatabaseCapture<'_>,
    ) -> Result<Vec<config::PostgresConfig>> {
        if matches!(capture, DatabaseCapture::None) || !self.config.experimental.databases {
            return Ok(Vec::new());
        }
        match capture {
            DatabaseCapture::None | DatabaseCapture::SqliteMatching(_) => Ok(Vec::new()),
            DatabaseCapture::Configured => Ok(self.config.postgres.clone()),
            DatabaseCapture::Matching(manifest) => manifest
                .services
                .iter()
                .filter_map(|service| match service {
                    ServiceArtifact::Postgres { name, url_env, .. } => {
                        Some((name.as_str(), url_env.as_str()))
                    }
                    ServiceArtifact::Sqlite { .. } => None,
                })
                .map(|(name, url_env)| {
                    self.config
                        .postgres
                        .iter()
                        .find(|entry| entry.name == name && entry.url_env == url_env)
                        .cloned()
                        .with_context(|| format!("PostgreSQL adapter {name} is not configured"))
                })
                .collect(),
        }
    }

    pub fn label(&self, id: &str, label: String) -> Result<()> {
        self.ensure_no_journal()?;
        let label = normalize_label(&label, 200, false)?;
        let _lock = self.store.lock()?;
        self.ensure_no_journal()?;
        let id = self.store.set_label(id, &label)?;
        ui::success(format_args!("Labeled {} ({label})", ui::checkpoint(&id)));
        Ok(())
    }

    pub fn delete(&self, id: &str, yes: bool) -> Result<()> {
        self.ensure_no_journal()?;
        let id = self.store.resolve_id(id)?;
        if !yes {
            confirm_delete(&id)?;
        }
        let _lock = self.store.lock()?;
        self.ensure_no_journal()?;
        ui::line(format_args!(
            "Deleting {} and collecting unreferenced objects...",
            ui::checkpoint(&id)
        ));
        self.store.delete(&id)?;
        ui::success(format_args!("Deleted {}", ui::checkpoint(&id)));
        Ok(())
    }

    pub fn pin(&self, id: &str) -> Result<()> {
        self.ensure_no_journal()?;
        let _lock = self.store.lock()?;
        self.ensure_no_journal()?;
        let id = self.store.pin(id)?;
        ui::success(format_args!("Pinned {}", ui::checkpoint(&id)));
        Ok(())
    }

    pub fn unpin(&self, id: &str) -> Result<()> {
        self.ensure_no_journal()?;
        let _lock = self.store.lock()?;
        self.ensure_no_journal()?;
        let id = self.store.unpin(id)?;
        ui::success(format_args!("Unpinned {}", ui::checkpoint(&id)));
        Ok(())
    }

    pub fn list(&self, json: bool) -> Result<()> {
        let snapshots = self
            .store
            .ids()?
            .into_iter()
            .map(|id| self.store.load(&id))
            .collect::<Result<Vec<_>>>()?;
        if json {
            ui::line(format_args!(
                "{}",
                serde_json::to_string_pretty(&snapshots)?
            ));
        } else if snapshots.is_empty() {
            ui::line(format_args!("{}", ui::muted("No checkpoints.")));
        } else {
            for manifest in snapshots {
                ui::line(format_args!(
                    "{}  {}  {:>8?}  {:>7} files  {:>2} exp db  {}{}",
                    ui::checkpoint(&manifest.id[..12]),
                    manifest.created_at.to_rfc3339(),
                    manifest.resolved_kind(),
                    manifest.files.len(),
                    manifest.services.len(),
                    if self.store.is_pinned(&manifest.id)? {
                        "📌 "
                    } else {
                        ""
                    },
                    manifest.label.as_deref().unwrap_or("")
                ));
            }
        }
        Ok(())
    }

    pub fn verify(&self, id: Option<&str>) -> Result<()> {
        let manifest = self.store.load(id.unwrap_or("latest"))?;
        self.store.verify(&manifest)?;
        for artifact in &manifest.services {
            match artifact {
                ServiceArtifact::Sqlite { root_id, path, .. } => {
                    let root = root_path(&manifest.roots, root_id)?;
                    SqliteAdapter::from_artifact(root, root_id, path)
                        .verify(artifact, &self.store)?;
                }
                ServiceArtifact::Postgres { url_env, name, .. } => {
                    if let Some(config) = self
                        .config
                        .postgres
                        .iter()
                        .find(|entry| &entry.name == name && &entry.url_env == url_env)
                    {
                        PostgresAdapter::new(config.clone())?.verify(artifact, &self.store)?;
                    } else {
                        bail!("PostgreSQL adapter {name} is not configured");
                    }
                }
            }
        }
        ui::success(format_args!(
            "Verified {}",
            ui::checkpoint(manifest.id.as_str())
        ));
        Ok(())
    }

    pub fn diff(&mut self, id: Option<&str>, to: &str, json: bool) -> Result<()> {
        let from = self.store.load(id.unwrap_or("latest"))?;
        let to_manifest = if to == "current" {
            self.capture_ephemeral(Some(&from.capture_scope), DatabaseCapture::Matching(&from))?
        } else {
            self.store.load(to)?
        };
        let files = filesystem::diff_entries(&from.files, &to_manifest.files);
        let services = diff_services(&from.services, &to_manifest.services);
        #[allow(clippy::items_after_statements)]
        #[derive(Serialize)]
        struct Report<'a> {
            files: &'a filesystem::DiffReport,
            services: &'a Vec<ServiceDiff>,
        }
        if json {
            ui::line(format_args!(
                "{}",
                serde_json::to_string_pretty(&Report {
                    files: &files,
                    services: &services
                })?
            ));
        } else {
            ui::field(
                "Filesystem",
                format_args!(
                    "{} {} {}",
                    ui::added(format!("+{}", files.added.len())),
                    ui::removed(format!("-{}", files.removed.len())),
                    ui::modified(format!("~{}", files.modified.len()))
                ),
            );
            for value in files
                .added
                .iter()
                .map(|v| ("+", v))
                .chain(files.removed.iter().map(|v| ("-", v)))
                .chain(files.modified.iter().map(|v| ("~", v)))
            {
                let path = match value.0 {
                    "+" => ui::added(format!("{} {}", value.0, value.1)),
                    "-" => ui::removed(format!("{} {}", value.0, value.1)),
                    _ => ui::modified(format!("{} {}", value.0, value.1)),
                };
                ui::line(format_args!("  {path}"));
            }
            for service in services {
                ui::line(format_args!(
                    "experimental {}: {}",
                    service.name, service.summary
                ));
                for item in &service.schema_added {
                    ui::line(format_args!("  + {item}"));
                }
                for item in &service.schema_removed {
                    ui::line(format_args!("  - {item}"));
                }
                for row in &service.estimated_row_changes {
                    ui::line(format_args!(
                        "  ~ {} rows (estimated): {} -> {}",
                        row.table,
                        display_estimate(row.before),
                        display_estimate(row.after)
                    ));
                }
            }
        }
        Ok(())
    }

    pub(super) fn capture_ephemeral(
        &self,
        scope: Option<&CaptureScope>,
        database_capture: DatabaseCapture<'_>,
    ) -> Result<SnapshotManifest> {
        let temporary = tempfile::tempdir()?;
        let store = Store::open(temporary.path().join("store"))?;
        let sqlite_paths = self.sqlite_paths(database_capture)?;
        let capture = filesystem::capture(
            &self.root,
            &self.config,
            &store,
            false,
            scope,
            &sqlite_paths,
        )?;
        let mut services = Vec::new();
        for candidate in &capture.sqlite {
            services.push(
                SqliteAdapter {
                    candidate: candidate.clone(),
                }
                .snapshot(&store)?,
            );
        }
        for postgres in self.postgres_configs(database_capture)? {
            services.push(PostgresAdapter::new(postgres)?.snapshot(&store)?);
        }
        Ok(SnapshotManifest {
            schema_version: SCHEMA_VERSION,
            id: "current".into(),
            created_at: Utc::now(),
            label: None,
            kind: Some(CheckpointKind::Manual),
            project_root: self.root.clone(),
            platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            engine: capture.engine,
            consistency: "live-comparison".into(),
            capture_started_at: Utc::now(),
            capture_finished_at: Utc::now(),
            roots: capture.roots,
            files: capture.files,
            services,
            capture_scope: capture.scope,
        })
    }
}
