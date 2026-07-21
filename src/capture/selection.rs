use super::*;

pub fn roots(root: &Path, config: &Config) -> Result<Vec<RootManifest>> {
    let mut roots = vec![RootManifest {
        id: "repo".into(),
        path: root.to_path_buf(),
        repository: true,
        identity: Some(root_identity(root)?),
    }];
    for (index, path) in config.external_paths.iter().enumerate() {
        if !path.exists() {
            bail!("external path {} does not exist", path.display());
        }
        roots.push(RootManifest {
            id: format!("external-{index}"),
            path: path.clone(),
            repository: false,
            identity: Some(root_identity(path)?),
        });
    }
    Ok(roots)
}

pub(super) fn root_identity(path: &Path) -> Result<RootIdentity> {
    let metadata = fs::symlink_metadata(path)?;
    let kind = if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else if metadata.file_type().is_symlink() {
        "symlink"
    } else {
        "special"
    };
    #[cfg(unix)]
    let (device, file_id) = {
        use std::os::unix::fs::MetadataExt;
        (Some(metadata.dev()), Some(metadata.ino()))
    };
    #[cfg(not(unix))]
    let device = None;
    #[cfg(not(unix))]
    let file_id = None;
    #[cfg(target_os = "linux")]
    let birth_time = platform::birth_time(path)?;
    #[cfg(not(target_os = "linux"))]
    let birth_time = metadata.created().ok().map(system_time_parts).transpose()?;
    Ok(RootIdentity {
        canonical_path: path.canonicalize()?,
        kind: kind.into(),
        device,
        file_id,
        birth_time_secs: birth_time.map(|(seconds, _)| seconds),
        birth_time_nanos: birth_time.map(|(_, nanos)| nanos),
    })
}

#[cfg(not(target_os = "linux"))]
fn system_time_parts(value: std::time::SystemTime) -> Result<(i64, u32)> {
    match value.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => Ok((
            i64::try_from(duration.as_secs()).context("filesystem birth time is too large")?,
            duration.subsec_nanos(),
        )),
        Err(error) => {
            let duration = error.duration();
            let seconds =
                i64::try_from(duration.as_secs()).context("filesystem birth time is too small")?;
            if duration.subsec_nanos() == 0 {
                Ok((-seconds, 0))
            } else {
                Ok((
                    seconds
                        .checked_add(1)
                        .and_then(i64::checked_neg)
                        .context("filesystem birth time is too small")?,
                    1_000_000_000 - duration.subsec_nanos(),
                ))
            }
        }
    }
}

pub(super) struct RootSelection {
    pub(super) paths: Vec<(PathBuf, PathBuf)>,
    pub(super) ignored: Vec<PathBuf>,
    pub(super) sources: Vec<GitIgnoreSource>,
    pub(super) explicitly_included: Vec<PathBuf>,
    pub(super) gitignore_rules_inactive: bool,
}

pub(super) struct SelectionPolicy<'a> {
    pub(super) includes: &'a GlobSet,
    pub(super) excludes: &'a GlobSet,
    pub(super) store: &'a Path,
    pub(super) respect_gitignore: bool,
    pub(super) include_patterns: &'a [String],
    pub(super) exclude_patterns: &'a [String],
    pub(super) recorded_scope: Option<&'a CaptureScope>,
}

#[allow(clippy::too_many_lines, clippy::if_not_else)] // Git and explicit-scope precedence is order-sensitive.
pub(super) fn paths_for_root(
    spec: &RootManifest,
    policy: &SelectionPolicy<'_>,
) -> Result<RootSelection> {
    let includes = policy.includes;
    let excludes = policy.excludes;
    let store = policy.store;
    let respect_gitignore = policy.respect_gitignore;
    let include_patterns = policy.include_patterns;
    let exclude_patterns = policy.exclude_patterns;
    let recorded_scope = policy.recorded_scope;
    if spec.path.is_file() || fs::symlink_metadata(&spec.path)?.file_type().is_symlink() {
        return Ok(RootSelection {
            paths: vec![(PathBuf::new(), spec.path.clone())],
            ignored: Vec::new(),
            sources: Vec::new(),
            explicitly_included: Vec::new(),
            gitignore_rules_inactive: false,
        });
    }
    let base = spec.path.clone();
    let git = spec.repository && is_git_worktree(&base)?;
    let exact_git_scope = respect_gitignore && git;
    let stored_ignore = recorded_scope
        .filter(|_| exact_git_scope)
        .map(|scope| StoredIgnore::new(&base, &spec.id, scope))
        .transpose()?;
    let mut selected = BTreeSet::new();
    let mut tracked_paths = BTreeSet::new();
    if exact_git_scope {
        if let Some(stored) = &stored_ignore {
            for entry in WalkDir::new(&base)
                .follow_links(false)
                .into_iter()
                .filter_entry(|entry| {
                    if entry.path() == base {
                        return true;
                    }
                    let Ok(relative) = entry.path().strip_prefix(&base) else {
                        return false;
                    };
                    safe_path(relative, entry.path(), store)
                        && !stored.is_ignored(entry.path(), entry.file_type().is_dir())
                })
            {
                let entry = entry?;
                if entry.path() != base {
                    selected.insert(entry.path().strip_prefix(&base)?.to_path_buf());
                }
            }
        } else {
            let mut builder = WalkBuilder::new(&base);
            builder
                .follow_links(false)
                .hidden(false)
                .ignore(false)
                .git_ignore(true)
                .git_exclude(true)
                .git_global(true)
                .parents(true);
            builder.current_dir(&base);
            if let Some(path) = effective_global_excludes(&base)?
                && let Some(error) = builder.add_ignore(path)
            {
                return Err(error).context("load Git global excludes");
            }
            for entry in builder.build() {
                let entry = entry?;
                if entry.path() == base {
                    continue;
                }
                let relative = entry.path().strip_prefix(&base)?.to_path_buf();
                if safe_path(&relative, entry.path(), store) {
                    selected.insert(relative);
                }
            }
        }
        for relative in git_paths(&base, &["ls-files", "--cached", "-z"])? {
            let live = base.join(&relative);
            if path_exists(&live)? && safe_path(&relative, &live, store) {
                tracked_paths.insert(relative.clone());
                insert_with_parents(&mut selected, &relative, &base, store);
            }
        }
    } else {
        for entry in WalkDir::new(&base)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                entry.path() == base
                    || entry
                        .path()
                        .strip_prefix(&base)
                        .ok()
                        .is_some_and(|relative| safe_path(relative, entry.path(), store))
            })
        {
            let entry = entry?;
            if entry.path() != base {
                selected.insert(entry.path().strip_prefix(&base)?.to_path_buf());
            }
        }
    }
    let mut explicitly_included = BTreeSet::new();
    if let Some(scope) = recorded_scope {
        explicitly_included.extend(
            scope
                .explicitly_included_paths
                .iter()
                .filter(|path| path.root_id == spec.id)
                .map(|path| path.path.clone()),
        );
    }
    if exact_git_scope && !include_patterns.is_empty() {
        for pattern in include_patterns {
            if !contains_glob_meta(pattern) {
                let relative = PathBuf::from(pattern);
                if allowed(&relative, &base.join(&relative), excludes, store) {
                    // Keep the intended hole in an ignored boundary even when the
                    // explicitly included path is currently absent. Restore must
                    // still be able to recreate it from the checkpoint.
                    explicitly_included.insert(relative.clone());
                }
                let live = base.join(pattern);
                if path_exists(&live)? {
                    for entry in WalkDir::new(&live).follow_links(false) {
                        let entry = entry?;
                        let relative = entry.path().strip_prefix(&base)?.to_path_buf();
                        if allowed(&relative, entry.path(), excludes, store) {
                            explicitly_included.insert(relative.clone());
                            insert_with_parents(&mut selected, &relative, &base, store);
                        }
                    }
                }
            } else {
                for entry in WalkDir::new(&base).follow_links(false) {
                    let entry = entry?;
                    if entry.path() == base {
                        continue;
                    }
                    let relative = entry.path().strip_prefix(&base)?.to_path_buf();
                    if allowed(&relative, entry.path(), excludes, store)
                        && matches_or_descends_from(includes, &relative)
                    {
                        explicitly_included.insert(relative.clone());
                        insert_with_parents(&mut selected, &relative, &base, store);
                    }
                }
            }
        }
    }
    let mut ignored = if exact_git_scope && stored_ignore.is_none() {
        git_paths(
            &base,
            &[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
                "-z",
            ],
        )?
    } else {
        Vec::new()
    };
    if let Some(stored) = &stored_ignore {
        let mut walker = WalkDir::new(&base).follow_links(false).into_iter();
        while let Some(entry) = walker.next() {
            let entry = entry?;
            if entry.path() == base {
                continue;
            }
            let relative = entry.path().strip_prefix(&base)?.to_path_buf();
            if !safe_path(&relative, entry.path(), store) {
                if entry.file_type().is_dir() {
                    walker.skip_current_dir();
                }
                continue;
            }
            if stored.is_ignored(entry.path(), entry.file_type().is_dir()) {
                if !ignored
                    .iter()
                    .any(|ancestor| relative.starts_with(ancestor))
                {
                    ignored.push(relative.clone());
                }
                if entry.file_type().is_dir() {
                    walker.skip_current_dir();
                }
            }
        }
    }
    ignored.retain(|relative| {
        let relative = clean_git_directory_path(relative);
        safe_path(relative, &base.join(relative), store)
    });
    let mut preservation =
        explicit_excluded_boundaries(&base, exclude_patterns, excludes, store, &selected)?;
    for boundary in &preservation {
        selected.retain(|path| !path.starts_with(boundary));
        tracked_paths.retain(|path| !path.starts_with(boundary));
    }
    let mut captured_holes = explicitly_included.clone();
    captured_holes.extend(tracked_paths);
    for boundary in ignored {
        let boundary = clean_git_directory_path(&boundary).to_path_buf();
        selected.retain(|path| {
            !path.starts_with(&boundary)
                || captured_holes.contains(path)
                || captured_holes
                    .iter()
                    .any(|included| included.starts_with(path))
        });
        split_ignored_boundary(&base, &boundary, &captured_holes, &mut preservation)?;
    }
    if let Some(scope) = recorded_scope {
        for item in scope
            .ignored_boundaries
            .iter()
            .filter(|item| item.root_id == spec.id)
        {
            let boundary = clean_git_directory_path(&item.path).to_path_buf();
            if path_exists(&base.join(&boundary))? {
                selected.retain(|path| {
                    !path.starts_with(&boundary)
                        || captured_holes.contains(path)
                        || captured_holes
                            .iter()
                            .any(|included| included.starts_with(path))
                });
                split_ignored_boundary(&base, &boundary, &captured_holes, &mut preservation)?;
            }
        }
    }
    preservation.sort();
    preservation.dedup();
    let gitignore_rules_inactive = recorded_scope.is_none()
        && respect_gitignore
        && spec.repository
        && !git
        && selected.iter().any(|relative| {
            relative
                .file_name()
                .is_some_and(|name| name == ".gitignore")
                && base.join(relative).is_file()
        });
    let paths = selected
        .into_iter()
        .map(|relative| (relative.clone(), base.join(relative)))
        .collect();
    let sources = if exact_git_scope {
        git_ignore_sources(spec)?
    } else {
        Vec::new()
    };
    Ok(RootSelection {
        paths,
        ignored: preservation,
        sources,
        explicitly_included: explicitly_included.into_iter().collect(),
        gitignore_rules_inactive,
    })
}

pub(super) fn allowed(relative: &Path, path: &Path, excludes: &GlobSet, store: &Path) -> bool {
    safe_path(relative, path, store) && !excludes.is_match(relative)
}

pub(super) fn safe_path(relative: &Path, path: &Path, store: &Path) -> bool {
    if path.starts_with(store) {
        return false;
    }
    if relative.components().next().is_some_and(|part| {
        part.as_os_str() == ".git"
            || part.as_os_str() == ".savestate"
            || part.as_os_str() == ".savestate-transaction"
    }) {
        return false;
    }
    true
}

pub(super) fn globs(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern)?);
    }
    Ok(builder.build()?)
}

pub(super) fn insert_with_parents(
    selected: &mut BTreeSet<PathBuf>,
    relative: &Path,
    base: &Path,
    store: &Path,
) {
    let mut current = Some(relative);
    while let Some(path) = current.filter(|path| !path.as_os_str().is_empty()) {
        if safe_path(path, &base.join(path), store) {
            selected.insert(path.to_path_buf());
        }
        current = path.parent();
    }
}

pub(super) fn matches_or_descends_from(globs: &GlobSet, path: &Path) -> bool {
    if globs.is_match(path) {
        return true;
    }
    let mut ancestor = path.parent();
    while let Some(value) = ancestor.filter(|value| !value.as_os_str().is_empty()) {
        if globs.is_match(value) {
            return true;
        }
        ancestor = value.parent();
    }
    false
}

pub(super) fn contains_glob_meta(pattern: &str) -> bool {
    pattern
        .chars()
        .any(|character| matches!(character, '*' | '?' | '[' | ']' | '{' | '}'))
}

pub(super) fn explicit_excluded_boundaries(
    base: &Path,
    patterns: &[String],
    excludes: &GlobSet,
    store: &Path,
    selected: &BTreeSet<PathBuf>,
) -> Result<Vec<PathBuf>> {
    let mut boundaries = Vec::new();
    for pattern in patterns
        .iter()
        .filter(|pattern| !contains_glob_meta(pattern))
    {
        let relative = PathBuf::from(pattern);
        let live = base.join(&relative);
        if path_exists(&live)? && safe_path(&relative, &live, store) {
            boundaries.push(relative);
        }
    }
    for relative in selected {
        if excludes.is_match(relative) {
            boundaries.push(relative.clone());
        }
    }
    boundaries.sort();
    boundaries.dedup();
    let mut maximal = Vec::new();
    for boundary in boundaries {
        if !maximal
            .iter()
            .any(|ancestor: &PathBuf| boundary.starts_with(ancestor))
        {
            maximal.push(boundary);
        }
    }
    Ok(maximal)
}
