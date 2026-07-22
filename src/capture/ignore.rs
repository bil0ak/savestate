use super::*;

pub(crate) struct StoredIgnore {
    matchers: Vec<(usize, PathBuf, Gitignore)>,
}

impl StoredIgnore {
    pub(crate) fn new(root: &Path, root_id: &str, scope: &CaptureScope) -> Result<Self> {
        let mut matchers = Vec::new();
        for source in scope
            .git_ignore_sources
            .iter()
            .filter(|source| source.root_id == root_id)
        {
            let (priority, base) = match source.kind.as_str() {
                "global_exclude" => (0, root.to_path_buf()),
                "info_exclude" => (1, root.to_path_buf()),
                _ => {
                    let relative_parent = source.path.parent().unwrap_or(Path::new(""));
                    (
                        2 + relative_parent.components().count(),
                        root.join(relative_parent),
                    )
                }
            };
            let mut builder = GitignoreBuilder::new(&base);
            for line in source.contents.lines() {
                builder
                    .add_line(Some(source.path.clone()), line)
                    .with_context(|| {
                        format!("parse recorded ignore rule from {}", source.path.display())
                    })?;
            }
            matchers.push((priority, base, builder.build()?));
        }
        matchers.sort_by_key(|(priority, _, _)| *priority);
        Ok(Self { matchers })
    }

    pub(crate) fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        let mut ignored = false;
        for (_, base, matcher) in &self.matchers {
            if !path.starts_with(base) {
                continue;
            }
            let matched = matcher.matched_path_or_any_parents(path, is_dir);
            if matched.is_ignore() {
                ignored = true;
            } else if matched.is_whitelist() {
                ignored = false;
            }
        }
        ignored
    }
}

/// Discover ignored state from an already-detached swap entry. This closes the
/// race between restore preview/revalidation and the atomic live-to-old rename:
/// anything created before that rename is examined in the detached tree before
/// the target is installed.
pub(crate) fn ignored_boundaries_in_detached(
    root_id: &str,
    live_root: &Path,
    detached_entry: &Path,
    entry_relative: &Path,
    scope: &CaptureScope,
) -> Result<Vec<PathBuf>> {
    if !path_exists(detached_entry)? {
        return Ok(Vec::new());
    }
    let excludes = globs(&scope.exclude)?;
    let stored = (scope.respect_gitignore && !scope.include_ignored)
        .then(|| StoredIgnore::new(live_root, root_id, scope))
        .transpose()?;
    let ignored = |relative: &Path, is_dir: bool| {
        excludes.is_match(relative)
            || scope.ignored_boundaries.iter().any(|boundary| {
                boundary.root_id == root_id
                    && (relative == boundary.path || relative.starts_with(&boundary.path))
            })
            || stored
                .as_ref()
                .is_some_and(|stored| stored.is_ignored(&live_root.join(relative), is_dir))
    };
    let entry_metadata = fs::symlink_metadata(detached_entry)?;
    if !entry_relative.as_os_str().is_empty()
        && ignored(
            entry_relative,
            entry_metadata.is_dir() && !entry_metadata.file_type().is_symlink(),
        )
    {
        return Ok(vec![entry_relative.to_path_buf()]);
    }
    if !entry_metadata.is_dir() || entry_metadata.file_type().is_symlink() {
        return Ok(Vec::new());
    }
    let mut boundaries = Vec::new();
    let mut walker = WalkDir::new(detached_entry).follow_links(false).into_iter();
    while let Some(entry) = walker.next() {
        let entry = entry?;
        if entry.path() == detached_entry {
            continue;
        }
        let suffix = entry.path().strip_prefix(detached_entry)?;
        let relative = if entry_relative.as_os_str().is_empty() {
            suffix.to_path_buf()
        } else {
            entry_relative.join(suffix)
        };
        if ignored(&relative, entry.file_type().is_dir()) {
            boundaries.push(relative);
            if entry.file_type().is_dir() {
                walker.skip_current_dir();
            }
        }
    }
    boundaries.sort();
    boundaries.dedup();
    Ok(boundaries)
}

pub(crate) fn recorded_scope_ignores(
    root_id: &str,
    live_root: &Path,
    relative: &Path,
    is_directory: bool,
    scope: &CaptureScope,
) -> Result<bool> {
    if globs(&scope.exclude)?.is_match(relative) {
        return Ok(true);
    }
    if scope.explicitly_included_paths.iter().any(|included| {
        included.root_id == root_id
            && (relative == included.path || relative.starts_with(&included.path))
    }) {
        return Ok(false);
    }
    if scope.ignored_boundaries.iter().any(|boundary| {
        boundary.root_id == root_id
            && (relative == boundary.path || relative.starts_with(&boundary.path))
    }) {
        return Ok(true);
    }
    if !scope.respect_gitignore || scope.include_ignored {
        return Ok(false);
    }
    Ok(StoredIgnore::new(live_root, root_id, scope)?
        .is_ignored(&live_root.join(relative), is_directory))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum GitRepositoryStatus {
    Available,
    NotRepository,
    Unavailable(String),
}

impl GitRepositoryStatus {
    pub(crate) fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    pub(crate) fn fallback_notice(&self, has_gitignore: bool) -> Option<String> {
        match self {
            Self::NotRepository if has_gitignore => Some(
                ".gitignore found, but its rules are ignored because this is not a Git repository; capturing the full non-Git scope"
                    .to_owned(),
            ),
            Self::Unavailable(reason) => Some(format!(
                "Git-aware capture is unavailable ({reason}); capturing the full filesystem scope"
            )),
            Self::Available | Self::NotRepository => None,
        }
    }

    pub(crate) fn unavailable_reason(&self) -> Option<&str> {
        match self {
            Self::Unavailable(reason) => Some(reason),
            Self::Available | Self::NotRepository => None,
        }
    }
}

pub(crate) fn git_repository_status(root: &Path) -> GitRepositoryStatus {
    let marker_exists = match fs::symlink_metadata(root.join(".git")) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return GitRepositoryStatus::Unavailable(format!(
                "the repository marker could not be inspected: {error}"
            ));
        }
    };
    match Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
    {
        Ok(output) if output.status.success() && output.stdout.starts_with(b"true") => {
            GitRepositoryStatus::Available
        }
        Ok(output) if marker_exists => GitRepositoryStatus::Unavailable(git_error_reason(
            &output.stderr,
            "Git could not inspect the repository",
        )),
        Err(error) if marker_exists => {
            GitRepositoryStatus::Unavailable(format!("Git could not be executed: {error}"))
        }
        Ok(_) | Err(_) => GitRepositoryStatus::NotRepository,
    }
}

pub(super) fn git_error_reason(stderr: &[u8], fallback: &str) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let message = stderr.lines().find(|line| !line.trim().is_empty());
    message.map_or_else(|| fallback.to_owned(), |line| line.trim().to_owned())
}

pub(super) fn git_paths(root: &Path, args: &[&str]) -> Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .with_context(|| format!("Git-aware capture requires Git for {}", root.display()))?;
    if !output.status.success() {
        bail!(
            "Git-aware capture failed in {}: {}",
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .map(path_from_git_bytes)
        .collect())
}

pub(super) fn effective_global_excludes(root: &Path) -> Result<Option<PathBuf>> {
    let output = Command::new("git")
        .current_dir(root)
        .args(["config", "--path", "core.excludesFile"])
        .output()?;
    if !output.status.success() {
        return Ok(None);
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if path.is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(path);
    Ok(Some(if path.is_absolute() {
        path
    } else {
        root.join(path)
    }))
}

#[cfg(unix)]
pub(super) fn path_from_git_bytes(value: &[u8]) -> PathBuf {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    PathBuf::from(OsString::from_vec(value.to_vec()))
}

#[cfg(not(unix))]
pub(super) fn path_from_git_bytes(value: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(value).into_owned())
}

pub(super) fn clean_git_directory_path(path: &Path) -> &Path {
    path
}

pub(super) fn split_ignored_boundary(
    base: &Path,
    boundary: &Path,
    included: &BTreeSet<PathBuf>,
    output: &mut Vec<PathBuf>,
) -> Result<()> {
    let live = base.join(boundary);
    if !path_exists(&live)? {
        return Ok(());
    }
    let contains_include = included
        .iter()
        .any(|path| path == boundary || path.starts_with(boundary));
    if !contains_include {
        output.push(boundary.to_path_buf());
        return Ok(());
    }
    if fs::symlink_metadata(&live)?.is_dir() {
        for child in fs::read_dir(&live)? {
            let child = child?;
            split_ignored_boundary(base, &boundary.join(child.file_name()), included, output)?;
        }
    }
    Ok(())
}

pub(super) fn git_ignore_sources(spec: &RootManifest) -> Result<Vec<GitIgnoreSource>> {
    let mut paths = Vec::new();
    let mut builder = WalkBuilder::new(&spec.path);
    builder
        .follow_links(false)
        .hidden(false)
        .ignore(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .parents(true);
    for entry in builder.build() {
        let entry = entry?;
        if entry.file_type().is_some_and(|kind| kind.is_file()) && entry.file_name() == ".gitignore"
        {
            paths.push((entry.path().to_path_buf(), "gitignore"));
        }
    }
    for (argument, kind) in [("info/exclude", "info_exclude"), ("", "global_exclude")] {
        let output = if argument.is_empty() {
            Command::new("git")
                .current_dir(&spec.path)
                .args(["config", "--path", "core.excludesFile"])
                .output()?
        } else {
            Command::new("git")
                .current_dir(&spec.path)
                .args(["rev-parse", "--git-path", argument])
                .output()?
        };
        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if !value.is_empty() {
                let path = PathBuf::from(value);
                let path = if path.is_absolute() {
                    path
                } else {
                    spec.path.join(path)
                };
                if path.is_file() {
                    paths.push((path, kind));
                }
            }
        }
    }
    paths.sort_by(|a, b| a.0.cmp(&b.0));
    paths.dedup_by(|a, b| a.0 == b.0);
    paths
        .into_iter()
        .map(|(path, kind)| {
            Ok(GitIgnoreSource {
                root_id: spec.id.clone(),
                path: path
                    .strip_prefix(&spec.path)
                    .map(Path::to_path_buf)
                    .unwrap_or(path.clone()),
                kind: kind.into(),
                contents: fs::read_to_string(path)?,
            })
        })
        .collect()
}
