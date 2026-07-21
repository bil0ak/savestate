use super::*;

pub fn materialize_root(
    entries: &[FileEntry],
    root_id: &str,
    destination: &Path,
    store: &Store,
) -> Result<()> {
    if path_exists(destination)? {
        bail!(
            "refusing to overwrite existing restore staging path {}",
            destination.display()
        );
    }
    let selected: Vec<_> = entries
        .iter()
        .filter(|entry| entry.root_id == root_id)
        .collect();
    let root_is_file = selected
        .iter()
        .any(|entry| entry.path.as_os_str().is_empty());
    if !root_is_file {
        fs::create_dir_all(destination)?;
    }
    for entry in selected
        .iter()
        .filter(|entry| entry.kind == EntryKind::Directory)
    {
        fs::create_dir_all(destination.join(&entry.path))?;
    }
    let mut links: HashMap<String, PathBuf> = HashMap::new();
    for entry in selected
        .iter()
        .filter(|entry| entry.kind != EntryKind::Directory)
    {
        let target = if entry.path.as_os_str().is_empty() {
            destination.to_path_buf()
        } else {
            destination.join(&entry.path)
        };
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        match entry.kind {
            EntryKind::File => {
                if let Some(group) = &entry.hardlink_group {
                    if let Some(first) = links.get(group) {
                        fs::hard_link(first, &target)?;
                    } else if let Some(hash) = &entry.object_hash {
                        platform::clone_or_copy(&store.object_path(hash), &target)?;
                        links.insert(group.clone(), target.clone());
                    } else {
                        fs::File::create(&target)?;
                        links.insert(group.clone(), target.clone());
                    }
                } else if let Some(hash) = &entry.object_hash {
                    platform::clone_or_copy(&store.object_path(hash), &target)?;
                } else {
                    fs::File::create(&target)?;
                }
            }
            EntryKind::Symlink => {
                let link_target = entry
                    .symlink_target
                    .as_ref()
                    .context("symlink target missing")?;
                create_symlink(
                    link_target,
                    &target,
                    symlink_target_is_directory(&selected, entry, link_target, &target),
                )?;
            }
            EntryKind::Directory => bail!("directory reached non-directory materialization pass"),
        }
        apply_metadata(&target, entry)?;
    }
    for entry in selected
        .iter()
        .filter(|entry| entry.kind == EntryKind::Directory)
        .rev()
    {
        apply_metadata(&destination.join(&entry.path), entry)?;
    }
    Ok(())
}

/// Reapply captured metadata after ignored paths or databases have been attached.
/// Those operations legitimately change parent directory timestamps even though
/// the checkpoint-owned tree is otherwise correct.
pub fn reapply_manifest_metadata(entries: &[FileEntry], roots: &[RootManifest]) -> Result<()> {
    for entry in entries.iter().rev() {
        let root = roots
            .iter()
            .find(|root| root.id == entry.root_id)
            .with_context(|| format!("unknown root {}", entry.root_id))?;
        let path = root.path.join(&entry.path);
        if path_exists(&path)? {
            apply_metadata(&path, entry)?;
        }
    }
    Ok(())
}

pub fn sync_manifest_data(entries: &[FileEntry], roots: &[RootManifest]) -> Result<()> {
    let mut regular_files = BTreeSet::new();
    let mut directories = BTreeSet::new();
    for entry in entries {
        let root = roots
            .iter()
            .find(|root| root.id == entry.root_id)
            .with_context(|| format!("unknown root {}", entry.root_id))?;
        let path = root.path.join(&entry.path);
        match entry.kind {
            EntryKind::File => {
                regular_files.insert(path);
            }
            EntryKind::Directory => {
                directories.insert(path);
            }
            EntryKind::Symlink => {}
        }
    }
    for path in regular_files {
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            bail!(
                "restored file changed type before durability sync: {}",
                path.display()
            );
        }
        platform::sync_file(&path)
            .with_context(|| format!("sync restored file: {}", path.display()))?;
    }
    #[cfg(unix)]
    {
        for root in roots {
            let metadata = fs::symlink_metadata(&root.path)?;
            if metadata.file_type().is_symlink() {
                bail!(
                    "restored filesystem root became a symlink: {}",
                    root.path.display()
                );
            }
            if metadata.is_dir() {
                directories.insert(root.path.clone());
            } else if metadata.is_file()
                && let Some(parent) = root.path.parent()
            {
                directories.insert(parent.to_path_buf());
            } else if !metadata.is_file() {
                bail!(
                    "restored filesystem root has an unsupported type: {}",
                    root.path.display()
                );
            }
        }
        let mut directories = directories.into_iter().collect::<Vec<_>>();
        directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        for path in directories {
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                bail!(
                    "restored directory changed type before durability sync: {}",
                    path.display()
                );
            }
            fs::File::open(&path)
                .with_context(|| {
                    format!("open restored directory for durability: {}", path.display())
                })?
                .sync_all()
                .with_context(|| format!("sync restored directory: {}", path.display()))?;
        }
    }
    #[cfg(not(unix))]
    let _ = directories;
    Ok(())
}

pub(super) fn apply_metadata(path: &Path, entry: &FileEntry) -> Result<()> {
    let time = filetime::FileTime::from_unix_time(entry.modified_secs, entry.modified_nanos);
    if entry.kind == EntryKind::Symlink {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::symlink_metadata(path)?;
            if metadata.uid() != entry.uid || metadata.gid() != entry.gid {
                platform::set_symlink_ownership(path, entry.uid, entry.gid)
                    .with_context(|| format!("restore symlink ownership for {}", path.display()))?;
            }
        }
        filetime::set_symlink_file_times(path, time, time)?;
    } else {
        // Everything below resolves symlinks, so the path must still be the
        // kind of node the manifest describes before any of it runs.
        let metadata = fs::symlink_metadata(path)?;
        let kind_matches = match entry.kind {
            EntryKind::File => metadata.is_file(),
            EntryKind::Directory => metadata.is_dir(),
            EntryKind::Symlink => false,
        };
        if metadata.file_type().is_symlink() || !kind_matches {
            bail!(
                "path changed type while metadata was being applied: {}",
                path.display()
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            use std::os::unix::fs::PermissionsExt;
            if metadata.uid() != entry.uid || metadata.gid() != entry.gid {
                platform::set_ownership(path, entry.uid, entry.gid)
                    .with_context(|| format!("restore ownership for {}", path.display()))?;
            }
            // Writing user.* attributes requires write permission, so xattrs
            // land before the recorded (possibly read-only) mode. Attribute
            // names keep their raw bytes; only manifest-key comparison needs
            // UTF-8.
            for name in xattr::list(path)? {
                if !name
                    .to_str()
                    .is_some_and(|name| entry.xattrs.contains_key(name))
                {
                    xattr::remove(path, &name)?;
                }
            }
            for (name, value) in &entry.xattrs {
                xattr::set(path, name, &BASE64.decode(value)?)?;
            }
            fs::set_permissions(path, fs::Permissions::from_mode(entry.mode & 0o7777))?;
        }
        filetime::set_file_mtime(path, time)?;
    }
    Ok(())
}

/// Decides whether a symlink's recorded target refers to a directory, which
/// Windows encodes in the link itself. The manifest is the authority — the
/// target usually does not exist yet while the staging tree is built; when
/// the resolved path is outside the manifest, the filesystem is consulted
/// relative to the link's own location.
pub(super) fn symlink_target_is_directory(
    selected: &[&FileEntry],
    entry: &FileEntry,
    link_target: &Path,
    link_location: &Path,
) -> bool {
    use std::path::Component;
    let resolved = if link_target.is_absolute() {
        None
    } else {
        let mut components = entry
            .path
            .parent()
            .map(|parent| parent.components().collect::<Vec<_>>())
            .unwrap_or_default();
        let mut valid = true;
        for component in link_target.components() {
            match component {
                Component::Normal(part) => components.push(Component::Normal(part)),
                Component::ParentDir => {
                    if components.pop().is_none() {
                        valid = false;
                        break;
                    }
                }
                Component::CurDir => {}
                _ => {
                    valid = false;
                    break;
                }
            }
        }
        valid.then(|| components.iter().collect::<PathBuf>())
    };
    if let Some(resolved) = resolved
        && let Some(found) = selected.iter().find(|candidate| candidate.path == resolved)
    {
        return found.kind == EntryKind::Directory;
    }
    link_location
        .parent()
        .is_some_and(|parent| parent.join(link_target).is_dir())
}

#[cfg(unix)]
pub(super) fn create_symlink(source: &Path, destination: &Path, _directory: bool) -> Result<()> {
    std::os::unix::fs::symlink(source, destination).map_err(Into::into)
}
#[cfg(windows)]
pub(super) fn create_symlink(source: &Path, destination: &Path, directory: bool) -> Result<()> {
    if directory {
        std::os::windows::fs::symlink_dir(source, destination)?;
    } else {
        std::os::windows::fs::symlink_file(source, destination)?;
    }
    Ok(())
}

/// Appends a suffix to a path's final component without round-tripping
/// through UTF-8, so non-UTF-8 filenames keep their exact bytes.
pub(super) fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

pub fn remove_path(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn path_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            for ancestor in path.ancestors().skip(1) {
                match fs::symlink_metadata(ancestor) {
                    Ok(metadata) => {
                        if metadata.file_type().is_symlink() {
                            bail!("path descends through a symlink: {}", ancestor.display());
                        }
                        return Ok(false);
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                        ) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}
