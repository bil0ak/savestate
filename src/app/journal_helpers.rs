use super::*;

pub(super) fn journal_transaction_root(journal: &RestoreJournal, parent: &Path) -> Result<PathBuf> {
    let candidate = if !journal.preservation_root.as_os_str().is_empty() {
        if journal.preservation_root.file_name() != Some(std::ffi::OsStr::new("preserved")) {
            bail!("restore journal has an invalid preservation directory");
        }
        journal
            .preservation_root
            .parent()
            .context("restore journal preservation directory has no parent")?
            .to_path_buf()
    } else if journal.version >= 3 {
        parent.join(&journal.target_id)
    } else {
        journal
            .swaps
            .iter()
            .find_map(|swap| {
                [&swap.staged, &swap.old].into_iter().find_map(|path| {
                    let relative = path.strip_prefix(parent).ok()?;
                    let first = relative.components().next()?;
                    Some(parent.join(first.as_os_str()))
                })
            })
            .unwrap_or_else(|| parent.join(&journal.target_id))
    };
    let relative = candidate
        .strip_prefix(parent)
        .context("restore transaction is outside the project transaction directory")?;
    if relative.components().count() != 1
        || !matches!(
            relative.components().next(),
            Some(std::path::Component::Normal(_))
        )
    {
        bail!("restore journal has an unsafe transaction directory");
    }
    Ok(candidate)
}

pub(super) fn journal_swap_root<'a>(
    roots: &'a [RootManifest],
    swap: &SwapRecord,
) -> Result<(&'a RootManifest, PathBuf)> {
    if !swap.root_id.is_empty() {
        let root = roots
            .iter()
            .find(|root| root.id == swap.root_id)
            .context("restore journal swap references an unknown root")?;
        if !swap.root_path.as_os_str().is_empty() && swap.root_path != root.path {
            bail!("restore journal swap root path is inconsistent");
        }
        return Ok((root, swap.entry_relative.clone()));
    }
    roots
        .iter()
        .find_map(|root| {
            if root.repository {
                swap.live
                    .strip_prefix(&root.path)
                    .ok()
                    .filter(|relative| !relative.as_os_str().is_empty())
                    .map(|relative| (root, relative.to_path_buf()))
            } else {
                (swap.live == root.path).then(|| (root, PathBuf::new()))
            }
        })
        .context("restore journal swap live path is outside configured roots")
}

pub(super) fn validate_journal_relative(path: &Path, require_nonempty: bool) -> Result<()> {
    if (require_nonempty && path.as_os_str().is_empty())
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        bail!(
            "restore journal contains unsafe relative path {}",
            path.display()
        );
    }
    Ok(())
}

pub(super) fn validate_directory_chain(base: &Path, directory: &Path) -> Result<()> {
    let relative = directory
        .strip_prefix(base)
        .context("restore path ancestry is outside its validated root")?;
    let mut current = base.to_path_buf();
    for component in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(component) = component {
            if !matches!(component, std::path::Component::Normal(_)) {
                bail!("restore path ancestry contains an unsafe component");
            }
            current.push(component.as_os_str());
        }
        if !filesystem::path_exists(&current)? {
            return Ok(());
        }
        let metadata = fs::symlink_metadata(&current)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            bail!(
                "restore path ancestry is not a real directory: {}",
                current.display()
            );
        }
    }
    Ok(())
}

pub(super) fn expected_database_swap(
    artifact: &ServiceArtifact,
    checkpoint_id: &str,
    operation_id: Option<&str>,
) -> Result<DatabaseSwap> {
    let ServiceArtifact::Postgres {
        name,
        url_env,
        database,
        target_identity: Some(identity),
        ..
    } = artifact
    else {
        bail!("PostgreSQL recovery requires a recorded target identity");
    };
    let operation_id = operation_id.context("PostgreSQL journal has no operation identity")?;
    if !PostgresAdapter::valid_operation_id(operation_id) {
        bail!("PostgreSQL journal has an invalid operation identity");
    }
    let (temporary_database, old_database) =
        PostgresAdapter::swap_names(checkpoint_id, name, operation_id);
    Ok(DatabaseSwap {
        name: name.clone(),
        url_env: url_env.clone(),
        database: database.clone(),
        temporary_database,
        old_database,
        system_identifier: Some(identity.system_identifier.clone()),
        operation_id: Some(operation_id.into()),
        original_database_oid: None,
        temporary_database_oid: None,
        temporary_owned: false,
        cutover_owned: false,
    })
}

pub(super) fn root_path<'a>(roots: &'a [RootManifest], id: &str) -> Result<&'a Path> {
    roots
        .iter()
        .find(|root| root.id == id)
        .map(|root| root.path.as_path())
        .with_context(|| format!("missing root {id}"))
}

pub(super) fn savestate_on_path() -> bool {
    Command::new("savestate")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub(super) fn init_git_discovery(root: &Path) -> (bool, Vec<PathBuf>, Vec<String>) {
    let status = capture::git_repository_status(root);
    if !status.is_available() {
        let warnings = status
            .unavailable_reason()
            .map(|reason| {
                vec![format!(
                    "Git integration is unavailable ({reason}); continuing without Git-specific setup"
                )]
            })
            .unwrap_or_default();
        return (false, Vec::new(), warnings);
    }
    match detect_ignored_includes(root) {
        Ok(includes) => (true, includes, Vec::new()),
        Err(error) => (
            true,
            Vec::new(),
            vec![format!(
                "Git could not detect ignored initialization candidates ({error}); continuing without automatic includes"
            )],
        ),
    }
}

pub(super) fn codex_trust_recorded(hooks: &Path) -> Result<Option<bool>> {
    let codex_home = if let Some(path) = std::env::var_os("CODEX_HOME") {
        PathBuf::from(path)
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".codex")
    } else if let Some(home) = std::env::var_os("USERPROFILE") {
        PathBuf::from(home).join(".codex")
    } else {
        return Ok(None);
    };
    let config = codex_home.join("config.toml");
    if !config.is_file() {
        return Ok(None);
    }
    let source = fs::read_to_string(config)?;
    let hooks = hooks.canonicalize().unwrap_or_else(|_| hooks.to_path_buf());
    let hooks = hooks.to_string_lossy();
    Ok(Some(
        ["session_start:0:0", "user_prompt_submit:0:0", "stop:0:0"]
            .iter()
            .all(|event| source.contains(&format!("{hooks}:{event}"))),
    ))
}

enum OptionalGit<T> {
    Available(T),
    Unavailable(String),
}

pub(super) fn ensure_git_exclude(root: &Path, store_root: &Path) -> Result<Option<String>> {
    let status = capture::git_repository_status(root);
    if !status.is_available() {
        return Ok(status.unavailable_reason().map(|reason| {
            format!(
                "Git integration is unavailable ({reason}); continuing without updating Git excludes"
            )
        }));
    }
    let patterns = match git_exclude_patterns(root, store_root)? {
        OptionalGit::Available(patterns) => patterns,
        OptionalGit::Unavailable(warning) => return Ok(Some(warning)),
    };
    let path = match git_exclude_path(root) {
        OptionalGit::Available(path) => path,
        OptionalGit::Unavailable(warning) => return Ok(Some(warning)),
    };
    Ok(update_git_exclude(&path, &patterns).err())
}

fn git_exclude_patterns(root: &Path, store_root: &Path) -> Result<OptionalGit<Vec<String>>> {
    let mut patterns = vec!["/.savestate-transaction/".to_owned()];
    if let Ok(relative) = store_root.strip_prefix(root) {
        if relative.as_os_str().is_empty() {
            bail!("snapshot store cannot be the project root");
        }
        let relative = relative.to_string_lossy().replace('\\', "/");
        let tracked = match Command::new("git")
            .current_dir(root)
            .args(["ls-files", "--error-unmatch", "--"])
            .arg(relative.as_str())
            .output()
        {
            Ok(output) => output,
            Err(error) => {
                return Ok(OptionalGit::Unavailable(format!(
                    "Git could not check the snapshot store ({error}); continuing without updating Git excludes"
                )));
            }
        };
        if tracked.status.success() {
            bail!("snapshot store {relative} is tracked by Git; untrack it before continuing");
        }
        if tracked.status.code() != Some(1) {
            return Ok(OptionalGit::Unavailable(format!(
                "Git could not check the snapshot store ({}); continuing without updating Git excludes",
                git_command_failure(&tracked.stderr)
            )));
        }
        patterns.push(format!("/{}/", relative.trim_matches('/')));
    }
    Ok(OptionalGit::Available(patterns))
}

fn git_exclude_path(root: &Path) -> OptionalGit<PathBuf> {
    let output = match Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--git-path", "info/exclude"])
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            return OptionalGit::Unavailable(format!(
                "Git could not locate its exclude file ({error}); continuing without updating Git excludes"
            ));
        }
    };
    if !output.status.success() {
        return OptionalGit::Unavailable(format!(
            "Git could not locate its exclude file ({}); continuing without updating Git excludes",
            git_command_failure(&output.stderr)
        ));
    }
    let raw = match String::from_utf8(output.stdout) {
        Ok(raw) => raw.trim().to_owned(),
        Err(error) => {
            return OptionalGit::Unavailable(format!(
                "Git returned an invalid exclude path ({error}); continuing without updating Git excludes"
            ));
        }
    };
    let path = PathBuf::from(raw);
    OptionalGit::Available(if path.is_absolute() {
        path
    } else {
        root.join(path)
    })
}

fn update_git_exclude(path: &Path, patterns: &[String]) -> std::result::Result<(), String> {
    let mut source = if path.exists() {
        match fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) => {
                return Err(format!(
                    "Git's exclude file could not be read ({error}); continuing without updating it"
                ));
            }
        }
    } else {
        String::new()
    };
    let mut changed = false;
    for pattern in patterns {
        if !source.lines().any(|line| line.trim() == pattern.as_str()) {
            if !source.ends_with('\n') && !source.is_empty() {
                source.push('\n');
            }
            source.push_str(pattern);
            source.push('\n');
            changed = true;
        }
    }
    if changed {
        if let Err(error) = store::atomic_write(path, source.as_bytes()) {
            return Err(format!(
                "Git's exclude file could not be updated ({error}); continuing without updating it"
            ));
        }
    }
    Ok(())
}

fn git_command_failure(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .find(|line| !line.trim().is_empty())
        .map_or_else(
            || "Git command failed".to_owned(),
            |line| line.trim().to_owned(),
        )
}

pub(super) fn detect_ignored_includes(root: &Path) -> Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .current_dir(root)
        .args([
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
            "--",
            ".env",
            ".env.*",
            "**/.env",
            "**/.env.*",
            "*.db",
            "*.sqlite",
            "*.sqlite3",
            "**/*.db",
            "**/*.sqlite",
            "**/*.sqlite3",
        ])
        .output()
        .context("detect ignored environment and SQLite files")?;
    if !output.status.success() {
        bail!(
            "could not detect ignored state: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let mut includes = Vec::new();
    for value in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|v| !v.is_empty())
    {
        let relative = PathBuf::from(
            String::from_utf8(value.to_vec())
                .context("ignored initialization candidate path is not valid UTF-8")?,
        );
        let name = relative
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name == ".env" || name.starts_with(".env.") {
            includes.push(relative);
            continue;
        }
        let path = root.join(&relative);
        let mut header = [0u8; 16];
        if fs::File::open(&path)
            .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut header))
            .is_ok()
            && &header == b"SQLite format 3\0"
        {
            includes.push(relative);
        }
    }
    includes.sort();
    includes.dedup();
    Ok(includes)
}

#[allow(clippy::unnecessary_wraps)] // Callers share a fallible detection pipeline.
pub(super) fn has_sqlite_header(path: &Path) -> Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    let mut header = [0u8; 16];
    Ok(fs::File::open(path)
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut header))
        .is_ok()
        && &header == b"SQLite format 3\0")
}

pub(super) fn env_file_has_key(path: &Path, key: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    Ok(fs::read_to_string(path)?.lines().any(|line| {
        line.trim_start()
            .strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    }))
}
