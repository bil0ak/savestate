use super::*;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct DiffReport {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub modified: Vec<String>,
}

pub fn diff_entries(from: &[FileEntry], to: &[FileEntry]) -> DiffReport {
    let a: BTreeMap<_, _> = from
        .iter()
        .map(|entry| (key(entry), signature(entry)))
        .collect();
    let b: BTreeMap<_, _> = to
        .iter()
        .map(|entry| (key(entry), signature(entry)))
        .collect();
    let added = b
        .keys()
        .filter(|key| !a.contains_key(*key))
        .cloned()
        .collect();
    let removed = a
        .keys()
        .filter(|key| !b.contains_key(*key))
        .cloned()
        .collect();
    let modified = a
        .iter()
        .filter_map(|(key, value)| {
            b.get(key)
                .filter(|other| *other != value)
                .map(|_| key.clone())
        })
        .collect();
    DiffReport {
        added,
        removed,
        modified,
    }
}

pub(super) fn key(entry: &FileEntry) -> String {
    format!("{}:{}", entry.root_id, entry.path.display())
}
pub(super) fn signature(entry: &FileEntry) -> String {
    // Symlink modes are umask noise (notably on macOS) and are never
    // restored, so they stay out of content signatures.
    let mode = match entry.kind {
        EntryKind::Symlink => 0,
        _ => entry.mode,
    };
    // Physical length is part of the restore contract only for files copied
    // byte-for-byte from a content object. Service-managed files such as
    // SQLite databases are restored logically and verified by their adapter;
    // directory and symlink lengths are filesystem implementation details.
    let size = entry.object_hash.as_ref().map_or(0, |_| entry.size);
    format!(
        "{:?}:{}:{}:{}:{}:{}:{}:{}:{:?}:{:?}:{}",
        entry.kind,
        size,
        mode,
        entry.uid,
        entry.gid,
        entry.modified_secs,
        entry.modified_nanos,
        entry.object_hash.as_deref().unwrap_or(""),
        entry.symlink_target,
        entry.xattrs,
        entry.hardlink_group.is_some()
    )
}

pub fn entries_equivalent(left: &[FileEntry], right: &[FileEntry]) -> bool {
    let diff = diff_entries(left, right);
    diff.added.is_empty()
        && diff.removed.is_empty()
        && diff.modified.is_empty()
        && hardlink_partitions(left) == hardlink_partitions(right)
}

pub(super) fn hardlink_partitions(entries: &[FileEntry]) -> BTreeSet<Vec<(String, PathBuf)>> {
    let mut groups = BTreeMap::<&str, Vec<(String, PathBuf)>>::new();
    for entry in entries {
        if let Some(group) = entry.hardlink_group.as_deref() {
            groups
                .entry(group)
                .or_default()
                .push((entry.root_id.clone(), entry.path.clone()));
        }
    }
    groups
        .into_values()
        .map(|mut paths| {
            paths.sort();
            paths
        })
        .collect()
}
