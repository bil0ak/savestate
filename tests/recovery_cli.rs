//! End-to-end recovery behavior driven through the CLI binary: a restore is
//! genuinely killed mid-commit (via `SAVESTATE_TEST_CRASHPOINT`, which aborts
//! the debug binary without unwinding), leaving a real interrupted journal
//! for `savestate recover` to operate on.

use std::{fs, path::Path};

use anyhow::Result;
use assert_cmd::Command;
use predicates::prelude::*;
use savestate::store::Store;
use tempfile::tempdir;

fn savestate(project: &Path) -> Result<Command> {
    let mut command = Command::cargo_bin("savestate")?;
    command.current_dir(project);
    Ok(command)
}

fn git(dir: &Path, args: &[&str]) -> Result<()> {
    let status = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()?;
    anyhow::ensure!(status.success(), "git {args:?} failed");
    Ok(())
}

/// Kills a real restore right after the install rename for `newfile.txt`
/// (swap index 2; swaps follow the sorted top-level entries `.savestate.toml`,
/// `file.txt`, `newfile.txt`) and before its `committed` journal write.
fn crash_restore_mid_commit(project: &Path) -> Result<String> {
    fs::write(project.join(".savestate.toml"), "")?;
    fs::write(project.join("file.txt"), "original")?;
    fs::write(project.join("newfile.txt"), "added by checkpoint")?;
    savestate(project)?
        .args(["create", "--label", "target"])
        .assert()
        .success();
    let store = Store::open(project.join(".savestate"))?;
    let target = store.ids()?[0].clone();

    fs::write(project.join("file.txt"), "modified")?;
    fs::remove_file(project.join("newfile.txt"))?;

    let output = savestate(project)?
        .args(["restore", &target, "--yes"])
        .env("SAVESTATE_TEST_CRASHPOINT", "target_installed:2")
        .output()?;
    assert!(
        !output.status.success(),
        "restore must die at the crashpoint"
    );
    assert!(
        store.journal()?.is_some(),
        "an interrupted journal must remain"
    );

    savestate(project)?
        .args(["create", "--label", "blocked"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("savestate recover"));
    Ok(target)
}

#[test]
fn killed_restore_rolls_back_including_uncommitted_new_entries() -> Result<()> {
    let project = tempdir()?;
    crash_restore_mid_commit(project.path())?;

    savestate(project.path())?
        .args(["recover", "--rollback"])
        .assert()
        .success();

    assert_eq!(
        fs::read_to_string(project.path().join("file.txt"))?,
        "modified"
    );
    assert!(
        !project.path().join("newfile.txt").exists(),
        "the half-installed new entry must not survive rollback"
    );
    let store = Store::open(project.path().join(".savestate"))?;
    assert!(store.journal()?.is_none());
    savestate(project.path())?
        .args(["create", "--label", "after"])
        .assert()
        .success();
    Ok(())
}

#[test]
fn killed_restore_resumes_to_the_full_target_state() -> Result<()> {
    let project = tempdir()?;
    crash_restore_mid_commit(project.path())?;

    savestate(project.path())?
        .args(["recover", "--resume"])
        .assert()
        .success();

    assert_eq!(
        fs::read_to_string(project.path().join("file.txt"))?,
        "original"
    );
    assert_eq!(
        fs::read_to_string(project.path().join("newfile.txt"))?,
        "added by checkpoint"
    );
    let store = Store::open(project.path().join(".savestate"))?;
    assert!(store.journal()?.is_none());
    Ok(())
}

#[test]
fn concurrent_creates_serialize_on_the_store_lock() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join(".savestate.toml"), "")?;
    fs::write(project.path().join("file.txt"), "content")?;

    let handles: Vec<_> = (0..4)
        .map(|index| {
            let root = project.path().to_path_buf();
            std::thread::spawn(move || -> Result<()> {
                let mut command = Command::cargo_bin("savestate")?;
                command
                    .current_dir(&root)
                    .args(["create", "--label", &format!("concurrent-{index}")])
                    .assert()
                    .success();
                Ok(())
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("create thread panicked")?;
    }

    let store = Store::open(project.path().join(".savestate"))?;
    assert_eq!(store.ids()?.len(), 4);
    store.validate_catalog()?;
    Ok(())
}

#[test]
fn crash_debris_in_the_catalog_heals_on_the_next_checkpoint() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join(".savestate.toml"), "")?;
    fs::write(project.path().join("file.txt"), "one")?;
    savestate(project.path())?
        .args(["create", "--label", "one"])
        .assert()
        .success();
    fs::write(project.path().join("file.txt"), "two")?;
    savestate(project.path())?
        .args(["create", "--label", "two"])
        .assert()
        .success();

    let store_root = project.path().join(".savestate");
    let store = Store::open(store_root.clone())?;
    let ids = store.ids()?;
    let (newest, older) = (ids[0].clone(), ids[1].clone());

    // The states routine crashes leave behind: a lagging HEAD (kill between
    // publish and HEAD write), an orphan label (kill mid-retirement), and a
    // stale retirement directory.
    fs::write(store_root.join("HEAD"), &older)?;
    fs::write(
        store_root.join("labels/01ARZ3NDEKTSV4RRFFQ69G5FAV"),
        "orphan",
    )?;
    let stale = store_root.join("snapshots/.01ARZ3NDEKTSV4RRFFQ69G5FAV.deleting");
    fs::create_dir_all(&stale)?;
    fs::write(stale.join("manifest.json"), "{}")?;
    assert!(store.validate_catalog().is_err());

    fs::write(project.path().join("file.txt"), "three")?;
    savestate(project.path())?
        .args(["create", "--label", "three"])
        .assert()
        .success();

    store.validate_catalog()?;
    assert_ne!(store.ids()?[0], newest, "the new checkpoint is newest");
    assert!(
        !store_root
            .join("labels/01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .exists()
    );
    assert!(!stale.exists());
    Ok(())
}

#[test]
fn include_ignored_restore_can_be_undone_from_its_recovery_checkpoint() -> Result<()> {
    let project = tempdir()?;
    git(project.path(), &["init", "-q"])?;
    fs::write(project.path().join(".gitignore"), "secret.txt\n")?;
    fs::write(project.path().join(".savestate.toml"), "")?;
    fs::write(project.path().join("secret.txt"), "old-secret")?;
    fs::write(project.path().join("tracked.txt"), "v1")?;
    savestate(project.path())?
        .args(["create", "--include-ignored", "--label", "full"])
        .assert()
        .success();
    let store = Store::open(project.path().join(".savestate"))?;
    let full = store.ids()?[0].clone();

    fs::write(project.path().join("secret.txt"), "new-secret")?;
    fs::write(project.path().join("tracked.txt"), "v2")?;

    savestate(project.path())?
        .args(["restore", &full, "--yes"])
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(project.path().join("secret.txt"))?,
        "old-secret"
    );
    assert_eq!(
        fs::read_to_string(project.path().join("tracked.txt"))?,
        "v1"
    );

    // The recovery checkpoint was captured under the target's scope, so the
    // ignored file the restore replaced is recoverable.
    let recovery = store
        .ids()?
        .into_iter()
        .map(|id| store.load(&id))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .find(|manifest| {
            manifest
                .label
                .as_deref()
                .is_some_and(|label| label.starts_with("pre-restore:"))
        })
        .expect("restore must create a pre-restore checkpoint");
    assert!(
        recovery
            .files
            .iter()
            .any(|entry| entry.path == Path::new("secret.txt")),
        "the recovery checkpoint must capture the ignored file the restore replaced"
    );

    savestate(project.path())?
        .args(["restore", &recovery.id, "--yes"])
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(project.path().join("secret.txt"))?,
        "new-secret"
    );
    assert_eq!(
        fs::read_to_string(project.path().join("tracked.txt"))?,
        "v2"
    );
    Ok(())
}
