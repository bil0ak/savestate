use super::*;

#[allow(clippy::too_many_lines)] // Discovery keeps scope accounting in one deterministic pass.
pub fn discover(
    root: &Path,
    config: &Config,
    include_ignored: bool,
    recorded_scope: Option<&CaptureScope>,
    enabled_sqlite: &BTreeSet<PathBuf>,
) -> Result<Discovery> {
    let roots = roots(root, config)?;
    let include_patterns =
        recorded_scope.map_or(config.include.as_slice(), |scope| scope.include.as_slice());
    let exclude_patterns =
        recorded_scope.map_or(config.exclude.as_slice(), |scope| scope.exclude.as_slice());
    let one_off_include_ignored =
        recorded_scope.map_or(include_ignored, |scope| scope.include_ignored);
    let respect_gitignore = recorded_scope
        .map_or(config.respect_gitignore && !include_ignored, |scope| {
            scope.respect_gitignore && !scope.include_ignored
        });
    let includes = globs(include_patterns)?;
    let excludes = globs(exclude_patterns)?;
    let mut sqlite = Vec::new();
    let mut captured_paths = BTreeMap::new();
    let mut ignored_boundaries = Vec::new();
    let mut explicitly_included_paths = Vec::new();
    let mut ignore_sources = Vec::new();
    let mut git_fallback_notices = Vec::new();
    let mut git_ignore_status = GitIgnoreStatus::Disabled;
    let mut logical_paths = 0usize;
    let mut logical_bytes = 0u64;
    let mut found_sqlite = BTreeSet::new();
    let policy = SelectionPolicy {
        includes: &includes,
        excludes: &excludes,
        store: &config.store,
        respect_gitignore,
        include_patterns,
        exclude_patterns,
        recorded_scope,
    };
    for spec in &roots {
        let selection = paths_for_root(spec, &policy)?;
        if spec.repository {
            git_ignore_status = selection.git_ignore_status;
        }
        let mut paths = selection.paths;
        ignored_boundaries.extend(selection.ignored.into_iter().map(|path| ScopedPath {
            root_id: spec.id.clone(),
            path,
        }));
        explicitly_included_paths.extend(selection.explicitly_included.into_iter().map(|path| {
            ScopedPath {
                root_id: spec.id.clone(),
                path,
            }
        }));
        ignore_sources.extend(selection.sources);
        git_fallback_notices.extend(selection.git_fallback_notice);
        let mut omitted_database_paths = BTreeSet::new();
        for (relative, live) in &paths {
            let metadata = fs::symlink_metadata(live)?;
            if !metadata.is_file() && !metadata.is_dir() && !metadata.file_type().is_symlink() {
                bail!("unsupported special file {}", live.display());
            }
            if metadata.is_file() {
                match sqlite_header(live)? {
                    true if enabled_sqlite.contains(live) => {
                        found_sqlite.insert(live.clone());
                        sqlite.push(SqliteCandidate {
                            root_id: spec.id.clone(),
                            relative_path: relative.clone(),
                            live_path: live.clone(),
                        });
                    }
                    true => {
                        omitted_database_paths.insert(relative.clone());
                        for suffix in ["-wal", "-shm", "-journal"] {
                            let sidecar = sibling_with_suffix(live, suffix);
                            if path_exists(&sidecar)?
                                && let Ok(sidecar_relative) = sidecar.strip_prefix(&spec.path)
                            {
                                omitted_database_paths.insert(sidecar_relative.to_path_buf());
                            }
                        }
                    }
                    false if enabled_sqlite.contains(live) => {
                        bail!(
                            "configured SQLite path {} is not a readable plain SQLite database",
                            live.display()
                        );
                    }
                    false => {}
                }
            }
        }
        for relative in &omitted_database_paths {
            ignored_boundaries.push(ScopedPath {
                root_id: spec.id.clone(),
                path: relative.clone(),
            });
        }
        paths.retain(|(relative, _)| !omitted_database_paths.contains(relative));
        for (_, live) in &paths {
            let metadata = fs::symlink_metadata(live)?;
            logical_paths += 1;
            if metadata.is_file() {
                logical_bytes = logical_bytes.saturating_add(metadata.len());
            }
        }
        captured_paths.insert(spec.id.clone(), paths);
    }
    for path in enabled_sqlite {
        if !found_sqlite.contains(path) {
            bail!(
                "configured SQLite database {} was not found in the selected snapshot scope; add its exact path to `include` if Git ignores it",
                path.display()
            );
        }
    }
    ignored_boundaries.sort();
    ignored_boundaries.dedup();
    explicitly_included_paths.sort();
    explicitly_included_paths.dedup();
    git_fallback_notices.sort();
    git_fallback_notices.dedup();
    Ok(Discovery {
        roots,
        paths: captured_paths,
        sqlite,
        scope: CaptureScope {
            respect_gitignore,
            include_ignored: one_off_include_ignored,
            include: include_patterns.to_vec(),
            exclude: exclude_patterns.to_vec(),
            explicitly_included_paths,
            git_ignore_sources: ignore_sources,
            ignored_boundaries,
        },
        git_fallback_notices,
        git_ignore_status,
        logical_paths,
        logical_bytes,
    })
}
