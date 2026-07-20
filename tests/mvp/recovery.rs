use super::*;

#[test]
fn recover_rolls_back_a_recorded_interrupted_swap() -> Result<()> {
    let project = tempdir()?;
    let root = project.path().canonicalize()?;
    let live = root.join("file");
    fs::write(&live, b"before")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let recovery = app.create(Some("recovery".into()), false)?;
    let transaction = root.join(".savestate-transaction/fake");
    let old = transaction.join("old/file");
    let staged = transaction.join("new/file");
    fs::create_dir_all(old.parent().unwrap())?;
    fs::rename(&live, &old)?;
    fs::write(&live, b"half-restored")?;
    let store = Store::open(project.path().join(".savestate"))?;
    store.write_journal(&RestoreJournal {
        version: 1,
        target_id: recovery.clone(),
        recovery_id: recovery,
        phase: JournalPhase::Committing,
        commit_started: true,
        swaps: vec![SwapRecord {
            root_id: "repo".into(),
            root_path: root,
            entry_relative: PathBuf::from("file"),
            live: live.clone(),
            staged,
            old,
            live_existed: true,
            target_existed: true,
            committed: true,
        }],
        database_swaps: Vec::new(),
        preserved_paths: Vec::new(),
        preservation_root: transaction.join("preserved"),
    })?;
    app.recover(true, false)?;
    assert_eq!(fs::read(live)?, b"before");
    assert!(store.journal()?.is_none());
    assert!(!transaction.exists());
    Ok(())
}

#[test]
fn recover_refuses_journal_paths_outside_the_transaction() -> Result<()> {
    let project = tempdir()?;
    let root = project.path().canonicalize()?;
    let live = root.join("file");
    fs::write(&live, "trusted live state")?;
    let mut app = App::open(root.clone())?;
    let checkpoint = app.create(Some("recovery".into()), false)?;
    let transaction = root.join(".savestate-transaction").join(&checkpoint);
    let victim = tempdir()?;
    let outside = victim.path().join("journal-victim");
    fs::write(&outside, "must survive")?;
    let store = Store::open(root.join(".savestate"))?;
    store.write_journal(&RestoreJournal {
        version: 3,
        target_id: checkpoint.clone(),
        recovery_id: checkpoint,
        phase: JournalPhase::Committing,
        commit_started: true,
        swaps: vec![SwapRecord {
            root_id: "repo".into(),
            root_path: root.clone(),
            entry_relative: PathBuf::from("file"),
            live: live.clone(),
            staged: transaction.join("new/repo/file"),
            old: outside.clone(),
            live_existed: true,
            target_existed: true,
            committed: true,
        }],
        database_swaps: Vec::new(),
        preserved_paths: Vec::new(),
        preservation_root: transaction.join("preserved"),
    })?;

    assert!(app.recover(true, false).is_err());
    assert_eq!(fs::read_to_string(&live)?, "trusted live state");
    assert_eq!(fs::read_to_string(&outside)?, "must survive");
    assert!(store.journal()?.is_some());
    Ok(())
}

#[test]
fn v2_recovery_preserves_ignored_state_after_reattach_boundary() -> Result<()> {
    let project = tempdir()?;
    let root = project.path().canonicalize()?;
    git(project.path(), &["init", "-q"])?;
    fs::write(project.path().join(".gitignore"), "node_modules/\n")?;
    fs::create_dir_all(project.path().join("src/node_modules"))?;
    fs::write(project.path().join("src/value"), "before")?;
    fs::write(project.path().join("src/node_modules/state"), "ignored")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let recovery = app.create(Some("recovery".into()), false)?;

    let transaction = root.join(".savestate-transaction/fake");
    let old = transaction.join("old/src");
    let staged = transaction.join("new/src");
    fs::create_dir_all(old.parent().unwrap())?;
    fs::rename(project.path().join("src"), &old)?;
    fs::create_dir_all(project.path().join("src/node_modules"))?;
    fs::write(project.path().join("src/value"), "half-restored")?;
    fs::rename(
        old.join("node_modules/state"),
        project.path().join("src/node_modules/state"),
    )?;
    fs::remove_dir(old.join("node_modules"))?;
    let store = Store::open(project.path().join(".savestate"))?;
    store.write_journal(&RestoreJournal {
        version: 2,
        target_id: recovery.clone(),
        recovery_id: recovery,
        phase: JournalPhase::Committing,
        commit_started: true,
        swaps: vec![SwapRecord {
            root_id: "repo".into(),
            root_path: root.clone(),
            entry_relative: PathBuf::from("src"),
            live: root.join("src"),
            staged,
            old,
            live_existed: true,
            target_existed: true,
            committed: true,
        }],
        database_swaps: Vec::new(),
        preserved_paths: vec![PreserveRecord {
            live: root.join("src/node_modules"),
            held: transaction.join("preserved/repo/src/node_modules"),
            detached: true,
            reattached: true,
        }],
        preservation_root: transaction.join("preserved"),
    })?;
    app.recover(true, false)?;
    assert_eq!(
        fs::read_to_string(project.path().join("src/value"))?,
        "before"
    );
    assert_eq!(
        fs::read_to_string(project.path().join("src/node_modules/state"))?,
        "ignored"
    );
    assert!(!transaction.exists());
    Ok(())
}
