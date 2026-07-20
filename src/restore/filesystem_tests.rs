use super::*;
use crate::model::{CaptureScope, GitIgnoreSource, RootManifest, SwapRecord};
use chrono::Utc;
use tempfile::tempdir;

fn fixture() -> Result<(tempfile::TempDir, Store, RestoreJournal, SnapshotManifest)> {
    let directory = tempdir()?;
    let live = directory.path().join("project/src");
    let staged = directory.path().join("transaction/new/src");
    let old = directory.path().join("transaction/old/src");
    let held = directory
        .path()
        .join("transaction/preserved/repo/src/cache");
    fs::create_dir_all(live.join("cache"))?;
    fs::write(live.join("value"), "current")?;
    fs::write(live.join("cache/state"), "ignored")?;
    fs::create_dir_all(&staged)?;
    fs::write(staged.join("value"), "checkpoint")?;
    let store = Store::open(directory.path().join("store"))?;
    let journal = RestoreJournal {
        version: 3,
        target_id: "target".into(),
        recovery_id: "recovery".into(),
        phase: JournalPhase::Committing,
        commit_started: true,
        swaps: vec![SwapRecord {
            root_id: "repo".into(),
            root_path: directory.path().join("project"),
            entry_relative: PathBuf::from("src"),
            live: live.clone(),
            staged,
            old,
            live_existed: true,
            target_existed: true,
            committed: false,
        }],
        database_swaps: Vec::new(),
        preserved_paths: vec![PreserveRecord {
            live: live.join("cache"),
            held,
            detached: false,
            reattached: false,
        }],
        preservation_root: directory.path().join("transaction/preserved"),
    };
    store.write_journal(&journal)?;
    let target = SnapshotManifest {
        schema_version: 3,
        id: "target".into(),
        created_at: Utc::now(),
        label: None,
        kind: Some(crate::model::CheckpointKind::Manual),
        project_root: directory.path().join("project"),
        platform: "test".into(),
        engine: "test".into(),
        consistency: "test".into(),
        capture_started_at: Utc::now(),
        capture_finished_at: Utc::now(),
        roots: vec![RootManifest {
            id: "repo".into(),
            path: directory.path().join("project"),
            repository: true,
            identity: None,
        }],
        files: Vec::new(),
        services: Vec::new(),
        capture_scope: CaptureScope::default(),
    };
    Ok((directory, store, journal, target))
}

#[test]
fn every_rename_gap_can_roll_back_without_losing_ignored_state() -> Result<()> {
    for point in [
        "preserve_detached:0",
        "swap_detached:0",
        "target_installed:0",
        "preserve_reattached:0",
    ] {
        let (directory, store, mut journal, target) = fixture()?;
        TEST_FAILPOINT.with(|value| *value.borrow_mut() = Some(point.into()));
        assert!(
            commit_filesystem(&store, &mut journal, &target, &[]).is_err(),
            "{point}"
        );
        TEST_FAILPOINT.with(|value| *value.borrow_mut() = None);
        rollback_filesystem(&store, &mut journal)?;
        let live = directory.path().join("project/src");
        assert_eq!(
            fs::read_to_string(live.join("value"))?,
            "current",
            "{point}"
        );
        assert_eq!(
            fs::read_to_string(live.join("cache/state"))?,
            "ignored",
            "{point}"
        );
    }
    Ok(())
}

#[test]
fn ignored_state_created_after_planning_is_found_in_the_detached_tree() -> Result<()> {
    let (directory, store, mut journal, mut target) = fixture()?;
    journal.preserved_paths.clear();
    store.write_journal(&journal)?;
    target.capture_scope.respect_gitignore = true;
    target.capture_scope.git_ignore_sources = vec![GitIgnoreSource {
        root_id: "repo".into(),
        path: PathBuf::from(".gitignore"),
        kind: "gitignore".into(),
        contents: "src/cache/\n".into(),
    }];

    commit_filesystem(&store, &mut journal, &target, &[])?;
    let live = directory.path().join("project/src");
    assert_eq!(fs::read_to_string(live.join("value"))?, "checkpoint");
    assert_eq!(fs::read_to_string(live.join("cache/state"))?, "ignored");
    assert!(journal.preserved_paths.iter().any(|record| {
        record.live == live.join("cache") && record.detached && record.reattached
    }));
    Ok(())
}

#[test]
fn late_preservation_rename_gap_rolls_back_safely() -> Result<()> {
    let (directory, store, mut journal, mut target) = fixture()?;
    journal.preserved_paths.clear();
    store.write_journal(&journal)?;
    target.capture_scope.respect_gitignore = true;
    target.capture_scope.git_ignore_sources = vec![GitIgnoreSource {
        root_id: "repo".into(),
        path: PathBuf::from(".gitignore"),
        kind: "gitignore".into(),
        contents: "src/cache/\n".into(),
    }];
    TEST_FAILPOINT.with(|value| *value.borrow_mut() = Some("late_preserve_detached:0".into()));
    assert!(commit_filesystem(&store, &mut journal, &target, &[]).is_err());
    TEST_FAILPOINT.with(|value| *value.borrow_mut() = None);
    rollback_filesystem(&store, &mut journal)?;
    let live = directory.path().join("project/src");
    assert_eq!(fs::read_to_string(live.join("value"))?, "current");
    assert_eq!(fs::read_to_string(live.join("cache/state"))?, "ignored");
    Ok(())
}

#[test]
fn selected_current_paths_are_not_reclassified_as_ignored() -> Result<()> {
    let (directory, store, mut journal, mut target) = fixture()?;
    journal.preserved_paths.clear();
    store.write_journal(&journal)?;
    target.capture_scope.respect_gitignore = true;
    target.capture_scope.git_ignore_sources = vec![GitIgnoreSource {
        root_id: "repo".into(),
        path: PathBuf::from(".gitignore"),
        kind: "gitignore".into(),
        contents: "src/cache/\n".into(),
    }];
    let tracked = directory.path().join("project/src/cache/tracked");
    fs::write(&tracked, "tracked current state")?;
    let current_owned = vec![FileEntry {
        root_id: "repo".into(),
        path: PathBuf::from("src/cache/tracked"),
        kind: EntryKind::File,
        size: 21,
        mode: 0,
        uid: 0,
        gid: 0,
        modified_secs: 0,
        modified_nanos: 0,
        object_hash: None,
        symlink_target: None,
        hardlink_group: None,
        xattrs: std::collections::BTreeMap::default(),
    }];

    commit_filesystem(&store, &mut journal, &target, &current_owned)?;

    let live = directory.path().join("project/src");
    assert!(!live.join("cache/tracked").exists());
    assert_eq!(fs::read_to_string(live.join("cache/state"))?, "ignored");
    Ok(())
}

#[test]
fn interrupted_new_entry_install_rolls_back_without_stranding_target_content() -> Result<()> {
    let (directory, store, mut journal, target) = fixture()?;
    let live = directory.path().join("project/addition");
    let staged = directory.path().join("transaction/new/addition");
    fs::write(&staged, "checkpoint")?;
    journal.preserved_paths.clear();
    journal.swaps = vec![SwapRecord {
        root_id: "repo".into(),
        root_path: directory.path().join("project"),
        entry_relative: PathBuf::from("addition"),
        live: live.clone(),
        staged,
        old: directory.path().join("transaction/old/addition"),
        live_existed: false,
        target_existed: true,
        committed: false,
    }];
    store.write_journal(&journal)?;
    TEST_FAILPOINT.with(|value| *value.borrow_mut() = Some("target_installed:0".into()));
    assert!(commit_filesystem(&store, &mut journal, &target, &[]).is_err());
    TEST_FAILPOINT.with(|value| *value.borrow_mut() = None);
    rollback_filesystem(&store, &mut journal)?;
    assert!(!live.exists());
    Ok(())
}

#[test]
fn rollback_keeps_user_files_where_no_install_could_have_run() -> Result<()> {
    let (directory, store, mut journal, _target) = fixture()?;
    let live = directory.path().join("project/addition");
    fs::write(&live, "user data")?;
    journal.preserved_paths.clear();
    journal.phase = JournalPhase::Prepared;
    journal.commit_started = false;
    journal.swaps = vec![SwapRecord {
        root_id: "repo".into(),
        root_path: directory.path().join("project"),
        entry_relative: PathBuf::from("addition"),
        live: live.clone(),
        staged: directory.path().join("transaction/new/addition"),
        old: directory.path().join("transaction/old/addition"),
        live_existed: false,
        target_existed: true,
        committed: false,
    }];
    store.write_journal(&journal)?;
    rollback_filesystem(&store, &mut journal)?;
    assert_eq!(fs::read_to_string(&live)?, "user data");
    Ok(())
}

#[test]
fn ignored_state_recreated_at_a_planned_boundary_survives_restore() -> Result<()> {
    let (directory, store, mut journal, mut target) = fixture()?;
    target.capture_scope.respect_gitignore = true;
    target.capture_scope.git_ignore_sources = vec![GitIgnoreSource {
        root_id: "repo".into(),
        path: PathBuf::from(".gitignore"),
        kind: "gitignore".into(),
        contents: "src/cache/\n".into(),
    }];
    let live = directory.path().join("project/src");
    fs::remove_dir_all(live.join("cache"))?;
    store.write_journal(&journal)?;

    // The planned boundary is absent when preservation detaches, then a
    // concurrent writer recreates it before the swap commits — the only
    // copy of the recreated state rides the detached tree.
    detach_preserved(&store, &mut journal)?;
    assert!(!journal.preserved_paths[0].detached);
    fs::create_dir_all(live.join("cache"))?;
    fs::write(live.join("cache/state"), "recreated")?;
    commit_swaps(&store, &mut journal, &target, &[])?;
    reattach_preserved(&store, &mut journal)?;

    assert!(preservation_is_live(&journal)?);
    cleanup(&journal)?;
    assert_eq!(fs::read_to_string(live.join("value"))?, "checkpoint");
    assert_eq!(fs::read_to_string(live.join("cache/state"))?, "recreated");
    assert!(journal.preserved_paths.iter().any(|record| {
        record.live == live.join("cache") && record.detached && record.reattached
    }));
    Ok(())
}

#[test]
fn nested_ignored_boundaries_create_one_recoverable_preservation_record() -> Result<()> {
    let (directory, _store, _journal, target) = fixture()?;
    let records = plan_preservation(
        &target,
        &[
            ScopedPath {
                root_id: "repo".into(),
                path: PathBuf::from("src/cache"),
            },
            ScopedPath {
                root_id: "repo".into(),
                path: PathBuf::from("src/cache/state"),
            },
        ],
        &directory.path().join("transaction"),
    )?;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].live, directory.path().join("project/src/cache"));
    Ok(())
}
