use super::*;

#[allow(clippy::too_many_lines)] // Stable capture retries and manifest assembly form one operation.
pub fn capture(
    root: &Path,
    config: &Config,
    store: &Store,
    include_ignored: bool,
    recorded_scope: Option<&CaptureScope>,
    enabled_sqlite: &BTreeSet<PathBuf>,
) -> Result<Capture> {
    let discovery = discover(
        root,
        config,
        include_ignored,
        recorded_scope,
        enabled_sqlite,
    )?;
    let mut notices = Vec::new();
    notices.extend(discovery.git_fallback_notices.iter().cloned());
    if let Some(notice) = scope_notice(config, discovery.logical_paths, discovery.logical_bytes)? {
        notices.push(notice);
    }
    let sqlite_paths: BTreeSet<_> = discovery
        .sqlite
        .iter()
        .map(|candidate| candidate.live_path.clone())
        .collect();
    let sqlite_sidecars: BTreeSet<_> = sqlite_paths
        .iter()
        .flat_map(|path| {
            [
                sibling_with_suffix(path, "-wal"),
                sibling_with_suffix(path, "-shm"),
                sibling_with_suffix(path, "-journal"),
            ]
        })
        .collect();

    let mut files = Vec::new();
    let mut engines = BTreeSet::new();
    for spec in &discovery.roots {
        let paths = discovery
            .paths
            .get(&spec.id)
            .context("captured root traversal is missing")?;
        for (relative, live) in paths {
            if sqlite_sidecars.contains(live) {
                continue;
            }
            let metadata = fs::symlink_metadata(live)?;
            let kind = if metadata.file_type().is_symlink() {
                EntryKind::Symlink
            } else if metadata.is_dir() {
                EntryKind::Directory
            } else if metadata.is_file() {
                EntryKind::File
            } else {
                bail!("unsupported special file {}", live.display());
            };
            let is_sqlite = sqlite_paths.contains(live);
            let (object_hash, engine) = if kind == EntryKind::File && !is_sqlite {
                let mut result = None;
                for _ in 0..3 {
                    let before = stable_fingerprint(live)?;
                    let stored = match store.put_object_retryable(live, None)? {
                        crate::store::ObjectWrite::Stored(stored) => stored,
                        crate::store::ObjectWrite::SourceChanged => continue,
                    };
                    let after = stable_fingerprint(live)?;
                    if before == after && crate::store::hash_file(live)? == stored.0 {
                        result = Some(stored);
                        break;
                    }
                }
                let (hash, engine) = result
                    .with_context(|| format!("{} kept changing during capture", live.display()))?;
                engines.insert(engine.clone());
                (Some(hash), Some(engine))
            } else {
                (None, None)
            };
            if let Some(engine) = engine {
                engines.insert(engine);
            }
            files.push(file_entry(
                &spec.id,
                relative.clone(),
                live,
                &metadata,
                kind,
                object_hash,
            )?);
        }
    }
    files.sort_by(|a, b| (&a.root_id, &a.path).cmp(&(&b.root_id, &b.path)));
    validate_captured_hardlinks(&files)?;
    let second = discover(
        root,
        config,
        include_ignored,
        recorded_scope,
        enabled_sqlite,
    )?;
    validate_stable_tree(&second, &sqlite_sidecars, &files)?;
    let engine = if engines.is_empty() {
        "metadata_only".into()
    } else {
        engines.into_iter().collect::<Vec<_>>().join("+")
    };
    Ok(Capture {
        roots: discovery.roots,
        files,
        sqlite: discovery.sqlite,
        engine,
        scope: discovery.scope,
        logical_paths: discovery.logical_paths,
        logical_bytes: discovery.logical_bytes,
        notices,
    })
}

pub(super) fn validate_stable_tree(
    discovery: &Discovery,
    sqlite_sidecars: &BTreeSet<PathBuf>,
    captured: &[FileEntry],
) -> Result<()> {
    let mut current = Vec::new();
    for spec in &discovery.roots {
        let paths = discovery
            .paths
            .get(&spec.id)
            .context("discovered root is missing")?;
        for (relative, live) in paths {
            if sqlite_sidecars.contains(live) {
                continue;
            }
            let metadata = fs::symlink_metadata(live)?;
            let kind = if metadata.file_type().is_symlink() {
                EntryKind::Symlink
            } else if metadata.is_dir() {
                EntryKind::Directory
            } else if metadata.is_file() {
                EntryKind::File
            } else {
                bail!("unsupported special file {}", live.display());
            };
            current.push(file_entry(
                &spec.id,
                relative.clone(),
                live,
                &metadata,
                kind,
                None,
            )?);
        }
    }
    current.sort_by(|a, b| (&a.root_id, &a.path).cmp(&(&b.root_id, &b.path)));
    let mut expected = captured.to_vec();
    for entry in &mut expected {
        entry.object_hash = None;
    }
    if current != expected {
        bail!("filesystem namespace or metadata changed during capture; retry the checkpoint");
    }
    Ok(())
}

pub(super) fn scope_notice(config: &Config, paths: usize, bytes: u64) -> Result<Option<String>> {
    let size_limit = config.warn_size_bytes()?;
    if paths > config.limits.warn_files || bytes > size_limit {
        return Ok(Some(format!(
            "checkpoint scope contains {paths} paths ({bytes} logical bytes), above the configured warning threshold; continuing"
        )));
    }
    Ok(None)
}

pub(super) fn sqlite_header(path: &Path) -> Result<bool> {
    let mut file = fs::File::open(path)?;
    let mut header = [0u8; 16];
    let read = file.read(&mut header)?;
    Ok(read == 16 && &header == b"SQLite format 3\0")
}

pub(super) fn stable_fingerprint(path: &Path) -> Result<(u64, i64, u32)> {
    let metadata = fs::metadata(path)?;
    let (seconds, nanos) = timestamp_parts(metadata.modified()?)?;
    Ok((metadata.len(), seconds, nanos))
}

pub(super) fn file_entry(
    root_id: &str,
    path: PathBuf,
    live: &Path,
    metadata: &fs::Metadata,
    kind: EntryKind,
    object_hash: Option<String>,
) -> Result<FileEntry> {
    let (modified_secs, modified_nanos) = timestamp_parts(metadata.modified()?)?;
    #[cfg(unix)]
    let (mode, uid, gid, hardlink_group) = {
        use std::os::unix::fs::MetadataExt;
        let group = (metadata.is_file() && metadata.nlink() > 1)
            .then(|| format!("{}:{}:{}", metadata.dev(), metadata.ino(), metadata.nlink()));
        (metadata.mode(), metadata.uid(), metadata.gid(), group)
    };
    #[cfg(not(unix))]
    let (mode, uid, gid, hardlink_group) = (
        if metadata.permissions().readonly() {
            0o444
        } else {
            0o666
        },
        0,
        0,
        None,
    );
    let symlink_target = (kind == EntryKind::Symlink)
        .then(|| fs::read_link(live))
        .transpose()?;
    #[cfg(unix)]
    let xattrs = {
        let mut xattrs = BTreeMap::new();
        if kind != EntryKind::Symlink {
            for name in xattr::list(live)
                .with_context(|| format!("list extended attributes for {}", live.display()))?
            {
                if let Some(value) = xattr::get(live, &name)? {
                    let name = name.into_string().map_err(|_| {
                        anyhow::anyhow!(
                            "extended attribute name is not valid UTF-8 for {}",
                            live.display()
                        )
                    })?;
                    xattrs.insert(name, BASE64.encode(value));
                }
            }
        }
        xattrs
    };
    #[cfg(not(unix))]
    let xattrs = BTreeMap::new();
    Ok(FileEntry {
        root_id: root_id.into(),
        path,
        kind,
        size: metadata.len(),
        mode,
        uid,
        gid,
        modified_secs,
        modified_nanos,
        object_hash,
        symlink_target,
        hardlink_group,
        xattrs,
    })
}

pub(super) fn timestamp_parts(value: std::time::SystemTime) -> Result<(i64, u32)> {
    match value.duration_since(UNIX_EPOCH) {
        Ok(duration) => Ok((
            i64::try_from(duration.as_secs()).context("filesystem timestamp is too large")?,
            duration.subsec_nanos(),
        )),
        Err(error) => {
            let duration = error.duration();
            let seconds =
                i64::try_from(duration.as_secs()).context("filesystem timestamp is too small")?;
            if duration.subsec_nanos() == 0 {
                Ok((-seconds, 0))
            } else {
                Ok((
                    seconds
                        .checked_add(1)
                        .and_then(i64::checked_neg)
                        .context("filesystem timestamp is too small")?,
                    1_000_000_000 - duration.subsec_nanos(),
                ))
            }
        }
    }
}

pub(super) fn validate_captured_hardlinks(entries: &[FileEntry]) -> Result<()> {
    let mut groups = BTreeMap::<&str, Vec<&FileEntry>>::new();
    for entry in entries {
        if let Some(group) = entry.hardlink_group.as_deref() {
            groups.entry(group).or_default().push(entry);
        }
    }
    for (group, members) in groups {
        let expected = group
            .rsplit_once(':')
            .and_then(|(_, count)| count.parse::<usize>().ok())
            .context("captured hardlink group has no link count")?;
        if members.len() != expected {
            bail!(
                "hardlink group containing {} has links outside the selected snapshot scope",
                members[0].path.display()
            );
        }
        if members
            .iter()
            .map(|entry| &entry.root_id)
            .collect::<BTreeSet<_>>()
            .len()
            != 1
        {
            bail!("hardlink group spans filesystem roots and cannot be restored atomically");
        }
    }
    Ok(())
}
