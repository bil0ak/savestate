//! Crash-recoverable filesystem restore primitives.
//!
//! Every path that may move is present in the durable journal before the first
//! rename. Operations then reconcile the journal with the filesystem, making
//! both forward execution and rollback safe to repeat after interruption.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::{
    filesystem,
    model::{
        EntryKind, FileEntry, JournalPhase, PreserveRecord, RestoreJournal, RootManifest,
        ScopedPath, SnapshotManifest,
    },
    store::Store,
};

pub(crate) fn plan_preservation(
    target: &SnapshotManifest,
    ignored: &[ScopedPath],
    transaction: &Path,
) -> Result<Vec<PreserveRecord>> {
    let mut paths = BTreeSet::new();
    let mut boundaries = ignored.to_vec();
    boundaries.sort_by(|left, right| {
        left.root_id
            .cmp(&right.root_id)
            .then_with(|| {
                left.path
                    .components()
                    .count()
                    .cmp(&right.path.components().count())
            })
            .then_with(|| left.path.cmp(&right.path))
    });

    for boundary in boundaries {
        let root = target
            .roots
            .iter()
            .find(|root| root.id == boundary.root_id)
            .with_context(|| {
                format!("ignored path references unknown root {}", boundary.root_id)
            })?;
        partition_ignored_boundary(
            target,
            root,
            &root.path,
            Path::new(""),
            &boundary.path,
            &[],
            &mut paths,
        )?;
    }

    paths
        .into_iter()
        .map(|(root_id, relative)| -> Result<PreserveRecord> {
            let root = target
                .roots
                .iter()
                .find(|root| root.id == root_id)
                .context("preservation root disappeared during planning")?;
            Ok(PreserveRecord {
                live: root.path.join(&relative),
                held: transaction.join("preserved").join(root_id).join(relative),
                detached: false,
                reattached: false,
            })
        })
        .collect()
}

fn partition_ignored_boundary(
    target: &SnapshotManifest,
    root: &RootManifest,
    source_entry: &Path,
    source_prefix: &Path,
    relative: &Path,
    current_owned: &[FileEntry],
    output: &mut BTreeSet<(String, PathBuf)>,
) -> Result<()> {
    let source = source_path(source_entry, source_prefix, relative)?;
    if !filesystem::path_exists(&source)? {
        return Ok(());
    }

    if target
        .capture_scope
        .explicitly_included_paths
        .iter()
        .any(|path| {
            path.root_id == root.id && (relative == path.path || relative.starts_with(&path.path))
        })
    {
        return Ok(());
    }

    let metadata = fs::symlink_metadata(&source)
        .with_context(|| format!("inspect ignored path {}", source.display()))?;
    let is_directory = metadata.is_dir() && !metadata.file_type().is_symlink();
    let exact_content = target.files.iter().chain(current_owned).any(|entry| {
        entry.root_id == root.id
            && entry.path == relative
            && !matches!(entry.kind, EntryKind::Directory)
    });
    if exact_content {
        return Ok(());
    }

    let owns_descendant = target.files.iter().chain(current_owned).any(|entry| {
        entry.root_id == root.id
            && ((entry.path == relative && matches!(entry.kind, EntryKind::Directory))
                || (entry.path != relative && entry.path.starts_with(relative)))
    }) || target
        .capture_scope
        .explicitly_included_paths
        .iter()
        .any(|path| path.root_id == root.id && path.path.starts_with(relative));
    if !is_directory || !owns_descendant {
        insert_preservation(output, &root.id, relative);
        return Ok(());
    }

    for child in fs::read_dir(&source).with_context(|| format!("read {}", source.display()))? {
        let child = child?;
        partition_ignored_boundary(
            target,
            root,
            source_entry,
            source_prefix,
            &relative.join(child.file_name()),
            current_owned,
            output,
        )?;
    }
    Ok(())
}

fn insert_preservation(output: &mut BTreeSet<(String, PathBuf)>, root_id: &str, relative: &Path) {
    if output
        .iter()
        .any(|(root, existing)| root == root_id && relative.starts_with(existing))
    {
        return;
    }
    output.retain(|(root, existing)| root != root_id || !existing.starts_with(relative));
    output.insert((root_id.to_owned(), relative.to_path_buf()));
}

fn source_path(source_entry: &Path, source_prefix: &Path, relative: &Path) -> Result<PathBuf> {
    if source_prefix.as_os_str().is_empty() {
        return Ok(source_entry.join(relative));
    }
    let suffix = relative.strip_prefix(source_prefix).with_context(|| {
        format!(
            "path {} is outside swap entry {}",
            relative.display(),
            source_prefix.display()
        )
    })?;
    Ok(source_entry.join(suffix))
}

pub(crate) fn commit_filesystem(
    store: &Store,
    journal: &mut RestoreJournal,
    target: &SnapshotManifest,
    current_owned: &[FileEntry],
) -> Result<()> {
    detach_preserved(store, journal)?;
    commit_swaps(store, journal, target, current_owned)?;
    reattach_preserved(store, journal)
}

fn detach_preserved(store: &Store, journal: &mut RestoreJournal) -> Result<()> {
    for index in 0..journal.preserved_paths.len() {
        let (live, held) = {
            let record = &journal.preserved_paths[index];
            (record.live.clone(), record.held.clone())
        };
        match (
            filesystem::path_exists(&live)?,
            filesystem::path_exists(&held)?,
        ) {
            (true, false) => {
                if let Some(parent) = held.parent() {
                    fs::create_dir_all(parent)?;
                }
                durable_rename(&live, &held)?;
                failpoint(&format!("preserve_detached:{index}"))?;
            }
            (false, true) => {}
            (false, false) => continue,
            (true, true) => bail!(
                "ambiguous preserved path state: both {} and {} exist",
                live.display(),
                held.display()
            ),
        }
        journal.preserved_paths[index].detached = true;
        journal.preserved_paths[index].reattached = false;
        store.write_journal(journal)?;
    }
    Ok(())
}

fn commit_swaps(
    store: &Store,
    journal: &mut RestoreJournal,
    target: &SnapshotManifest,
    current_owned: &[FileEntry],
) -> Result<()> {
    for index in 0..journal.swaps.len() {
        let (live, old, staged) = {
            let swap = &journal.swaps[index];
            (swap.live.clone(), swap.old.clone(), swap.staged.clone())
        };
        if let Some(parent) = old.parent() {
            fs::create_dir_all(parent)?;
        }

        if journal.swaps[index].target_existed && filesystem::path_exists(&staged)? {
            match (
                filesystem::path_exists(&live)?,
                filesystem::path_exists(&old)?,
            ) {
                (true, false) => {
                    durable_rename(&live, &old)?;
                    failpoint(&format!("swap_detached:{index}"))?;
                }
                (false, false | true) => {}
                (true, true) => bail!(
                    "ambiguous swap state: both {} and {} exist before install",
                    live.display(),
                    old.display()
                ),
            }
            capture_late_preservation(store, journal, target, current_owned, index)?;
            if let Some(parent) = live.parent() {
                fs::create_dir_all(parent)?;
            }
            durable_rename(&staged, &live)?;
            failpoint(&format!("target_installed:{index}"))?;
        } else if journal.swaps[index].target_existed {
            if !filesystem::path_exists(&live)? {
                bail!("staged restore entry disappeared: {}", staged.display());
            }
            // Staged no longer exists and live does: the install rename completed
            // before an interruption, regardless of whether old exists.
        } else if !filesystem::path_exists(&old)? && filesystem::path_exists(&live)? {
            durable_rename(&live, &old)?;
            failpoint(&format!("swap_detached:{index}"))?;
            capture_late_preservation(store, journal, target, current_owned, index)?;
        } else if filesystem::path_exists(&old)? {
            capture_late_preservation(store, journal, target, current_owned, index)?;
        }
        journal.swaps[index].committed = true;
        store.write_journal(journal)?;
    }
    Ok(())
}

fn capture_late_preservation(
    store: &Store,
    journal: &mut RestoreJournal,
    target: &SnapshotManifest,
    current_owned: &[FileEntry],
    swap_index: usize,
) -> Result<()> {
    let swap = journal.swaps[swap_index].clone();
    if swap.root_id.is_empty() || !filesystem::path_exists(&swap.old)? {
        return Ok(());
    }
    let root = target
        .roots
        .iter()
        .find(|root| root.id == swap.root_id)
        .context("restore swap references an unknown root")?;
    let boundaries = filesystem::ignored_boundaries_in_detached(
        &swap.root_id,
        &swap.root_path,
        &swap.old,
        &swap.entry_relative,
        &target.capture_scope,
    )?;
    let mut paths = BTreeSet::new();
    for boundary in boundaries {
        partition_ignored_boundary(
            target,
            root,
            &swap.old,
            &swap.entry_relative,
            &boundary,
            current_owned,
            &mut paths,
        )?;
    }
    // A record only owns its path once `detached` is set; a planned record
    // whose path was absent at detach time holds nothing, so a match found in
    // the detached tree must still be preserved through that same record.
    let mut pending = Vec::new();
    let mut appended = Vec::new();
    for (root_id, relative) in paths {
        let live = root.path.join(&relative);
        match journal
            .preserved_paths
            .iter()
            .position(|record| record.live == live)
        {
            Some(index) if journal.preserved_paths[index].detached => {}
            Some(index) => pending.push((relative, index)),
            None => appended.push((
                relative.clone(),
                PreserveRecord {
                    live,
                    held: journal.preservation_root.join(root_id).join(relative),
                    detached: false,
                    reattached: false,
                },
            )),
        }
    }
    if pending.is_empty() && appended.is_empty() {
        return Ok(());
    }
    let start = journal.preserved_paths.len();
    journal
        .preserved_paths
        .extend(appended.iter().map(|(_, record)| record.clone()));
    pending.extend(
        appended
            .into_iter()
            .enumerate()
            .map(|(offset, (relative, _))| (relative, start + offset)),
    );
    store.write_journal(journal)?;

    for (relative, index) in pending {
        let source = source_path(&swap.old, &swap.entry_relative, &relative)?;
        let held = journal.preserved_paths[index].held.clone();
        if !filesystem::path_exists(&source)? {
            continue;
        }
        if let Some(parent) = held.parent() {
            fs::create_dir_all(parent)?;
        }
        durable_rename(&source, &held)?;
        failpoint(&format!("late_preserve_detached:{index}"))?;
        journal.preserved_paths[index].detached = true;
        store.write_journal(journal)?;
    }
    Ok(())
}

fn reattach_preserved(store: &Store, journal: &mut RestoreJournal) -> Result<()> {
    for index in 0..journal.preserved_paths.len() {
        if !journal.preserved_paths[index].detached {
            continue;
        }
        let (live, held) = {
            let record = &journal.preserved_paths[index];
            (record.live.clone(), record.held.clone())
        };
        match (
            filesystem::path_exists(&live)?,
            filesystem::path_exists(&held)?,
        ) {
            (false, true) => {
                if let Some(parent) = live.parent() {
                    fs::create_dir_all(parent)?;
                }
                durable_rename(&held, &live)?;
                failpoint(&format!("preserve_reattached:{index}"))?;
            }
            (true, false) => {}
            (false, false) => bail!("preserved path disappeared: {}", live.display()),
            (true, true) => bail!("restore conflicts with ignored path {}", live.display()),
        }
        journal.preserved_paths[index].reattached = true;
        store.write_journal(journal)?;
    }
    Ok(())
}

pub(crate) fn rollback_filesystem(store: &Store, journal: &mut RestoreJournal) -> Result<()> {
    redetach_reattached(store, journal)?;
    rollback_swaps(journal)?;
    restore_preserved(store, journal)
}

fn redetach_reattached(store: &Store, journal: &mut RestoreJournal) -> Result<()> {
    for index in (0..journal.preserved_paths.len()).rev() {
        let record = &journal.preserved_paths[index];
        if !record.detached {
            continue;
        }
        let live = record.live.clone();
        let held = record.held.clone();
        match (
            filesystem::path_exists(&live)?,
            filesystem::path_exists(&held)?,
        ) {
            (true, false) => {
                if let Some(parent) = held.parent() {
                    fs::create_dir_all(parent)?;
                }
                durable_rename(&live, &held)?;
            }
            (false, true) => {}
            (false, false) => bail!(
                "preserved path disappeared during rollback: {}",
                live.display()
            ),
            (true, true) => bail!("ambiguous preserved rollback state for {}", live.display()),
        }
        journal.preserved_paths[index].reattached = false;
        store.write_journal(journal)?;
    }
    Ok(())
}

fn rollback_swaps(journal: &RestoreJournal) -> Result<()> {
    // `commit_started` survives the transition to `RollingBack`; journals
    // written before the field existed record a mid-commit crash as a durable
    // `Committing` phase instead.
    let installs_possible = journal.commit_started || journal.phase == JournalPhase::Committing;
    for swap in journal.swaps.iter().rev() {
        let old_exists = filesystem::path_exists(&swap.old)?;
        let live_exists = filesystem::path_exists(&swap.live)?;
        if old_exists {
            if live_exists {
                filesystem::remove_path(&swap.live)?;
            }
            if let Some(parent) = swap.live.parent() {
                fs::create_dir_all(parent)?;
            }
            durable_rename(&swap.old, &swap.live)?;
        } else if !swap.live_existed && live_exists {
            // A consumed staged path proves the install rename ran even when
            // the crash landed before the `committed` journal write; content
            // at `live` is then checkpoint data, not user state.
            let installed = installs_possible && !filesystem::path_exists(&swap.staged)?;
            if swap.committed || installed {
                filesystem::remove_path(&swap.live)?;
            }
        }
    }
    Ok(())
}

fn restore_preserved(store: &Store, journal: &mut RestoreJournal) -> Result<()> {
    for index in 0..journal.preserved_paths.len() {
        let record = &journal.preserved_paths[index];
        if !record.detached && !filesystem::path_exists(&record.held)? {
            continue;
        }
        let live = record.live.clone();
        let held = record.held.clone();
        match (
            filesystem::path_exists(&live)?,
            filesystem::path_exists(&held)?,
        ) {
            (false, true) => {
                if let Some(parent) = live.parent() {
                    fs::create_dir_all(parent)?;
                }
                durable_rename(&held, &live)?;
            }
            (true, false) if !record.detached => {}
            (true, false) => bail!(
                "preserved rollback source is missing for {}",
                live.display()
            ),
            (false, false) => bail!(
                "preserved path is missing after rollback: {}",
                live.display()
            ),
            (true, true) => bail!("rollback conflicts with preserved path {}", live.display()),
        }
        journal.preserved_paths[index].reattached = true;
        store.write_journal(journal)?;
    }
    Ok(())
}

pub(crate) fn cleanup(journal: &RestoreJournal) -> Result<()> {
    for swap in &journal.swaps {
        if filesystem::path_exists(&swap.old)? {
            filesystem::remove_path(&swap.old)?;
        }
        if filesystem::path_exists(&swap.staged)? {
            filesystem::remove_path(&swap.staged)?;
        }
    }
    for record in &journal.preserved_paths {
        if filesystem::path_exists(&record.held)? {
            filesystem::remove_path(&record.held)?;
        }
    }
    Ok(())
}

pub(crate) fn preservation_is_live(journal: &RestoreJournal) -> Result<bool> {
    for record in &journal.preserved_paths {
        if record.detached
            && (!filesystem::path_exists(&record.live)? || filesystem::path_exists(&record.held)?)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg_attr(not(any(test, debug_assertions)), allow(unused_variables))]
fn failpoint(name: &str) -> Result<()> {
    #[cfg(test)]
    if TEST_FAILPOINT.with(|value| value.borrow().as_deref() == Some(name)) {
        bail!("test failpoint reached: {name}");
    }
    // Integration tests inject failures into spawned debug binaries through
    // the environment; release builds compile both hooks out. FAILPOINT
    // returns an error (exercising in-process compensation), CRASHPOINT
    // aborts without unwinding (leaving a genuine interrupted journal).
    #[cfg(debug_assertions)]
    {
        if std::env::var("SAVESTATE_TEST_CRASHPOINT").ok().as_deref() == Some(name) {
            std::process::abort();
        }
        if std::env::var("SAVESTATE_TEST_FAILPOINT").ok().as_deref() == Some(name) {
            bail!("test failpoint reached: {name}");
        }
    }
    Ok(())
}

fn durable_rename(source: &Path, destination: &Path) -> Result<()> {
    fs::rename(source, destination)?;
    #[cfg(unix)]
    {
        use std::fs::File;
        if let Some(parent) = source.parent() {
            File::open(parent)?.sync_all()?;
        }
        if destination.parent() != source.parent()
            && let Some(parent) = destination.parent()
        {
            File::open(parent)?.sync_all()?;
        }
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    static TEST_FAILPOINT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
#[path = "filesystem_tests.rs"]
mod tests;
