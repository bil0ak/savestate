use super::*;

#[test]
fn restores_files_and_removes_new_paths() -> Result<()> {
    let project = tempdir()?;
    fs::create_dir(project.path().join("nested"))?;
    fs::write(project.path().join("nested/original.txt"), b"before")?;
    #[cfg(unix)]
    std::os::unix::fs::symlink("nested/original.txt", project.path().join("link"))?;
    #[cfg(unix)]
    std::os::unix::fs::symlink("missing", project.path().join("broken"))?;

    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(Some("baseline".into()), false)?;
    fs::write(project.path().join("nested/original.txt"), b"after")?;
    fs::write(project.path().join("new.txt"), b"remove me")?;

    app.restore(Some(&checkpoint), false, true)?;
    assert_eq!(
        fs::read(project.path().join("nested/original.txt"))?,
        b"before"
    );
    assert!(!project.path().join("new.txt").exists());
    #[cfg(unix)]
    assert_eq!(
        fs::read_link(project.path().join("link"))?,
        std::path::PathBuf::from("nested/original.txt")
    );
    #[cfg(unix)]
    assert_eq!(
        fs::read_link(project.path().join("broken"))?,
        std::path::PathBuf::from("missing")
    );
    app.verify(Some(&checkpoint))?;
    Ok(())
}

#[test]
fn restore_refuses_stale_transaction_state_before_creating_recovery_checkpoint() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join("value"), "before")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    fs::write(project.path().join("value"), "after")?;
    fs::create_dir_all(project.path().join(".savestate-transaction/stale"))?;
    fs::write(
        project.path().join(".savestate-transaction/stale/unknown"),
        "keep",
    )?;
    let store = Store::open(project.path().join(".savestate"))?;
    let count = store.ids()?.len();

    let error = app
        .restore(Some(&checkpoint), false, true)
        .unwrap_err()
        .to_string();
    assert!(error.contains("stale or ambiguous"), "{error}");
    assert_eq!(store.ids()?.len(), count);
    assert_eq!(
        fs::read_to_string(project.path().join(".savestate-transaction/stale/unknown"))?,
        "keep"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn restore_preserves_complete_hardlink_groups() -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let project = tempdir()?;
    let first = project.path().join("first");
    let second = project.path().join("second");
    fs::write(&first, "checkpoint")?;
    fs::hard_link(&first, &second)?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    fs::write(&first, "changed")?;

    app.restore(Some(&checkpoint), false, true)?;

    assert_eq!(fs::read_to_string(&first)?, "checkpoint");
    assert_eq!(fs::metadata(&first)?.ino(), fs::metadata(&second)?.ino());
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn restore_does_not_leak_xattrs_between_deduplicated_files() -> Result<()> {
    let project = tempdir()?;
    let first = project.path().join("a");
    let second = project.path().join("b");
    fs::write(&first, "same content")?;
    fs::write(&second, "same content")?;
    let attribute = if cfg!(target_os = "macos") {
        "com.savestate.test"
    } else {
        "user.savestate"
    };
    xattr::set(&first, attribute, b"first only")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    fs::write(&first, "changed")?;
    fs::write(&second, "changed")?;

    app.restore(Some(&checkpoint), false, true)?;

    assert_eq!(
        xattr::get(&first, attribute)?.as_deref(),
        Some(b"first only".as_slice())
    );
    assert_eq!(xattr::get(&second, attribute)?, None);
    Ok(())
}

#[cfg(unix)]
#[test]
fn restore_preserves_pre_epoch_modification_times() -> Result<()> {
    let project = tempdir()?;
    let path = project.path().join("historical");
    fs::write(&path, "before")?;
    let expected = filetime::FileTime::from_unix_time(-2, 123_456_789);
    filetime::set_file_mtime(&path, expected)?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    fs::write(&path, "after")?;

    app.restore(Some(&checkpoint), false, true)?;

    let restored = filetime::FileTime::from_last_modification_time(&fs::metadata(&path)?);
    assert_eq!(restored.unix_seconds(), expected.unix_seconds());
    assert_eq!(restored.nanoseconds(), expected.nanoseconds());
    Ok(())
}

#[cfg(unix)]
#[test]
fn capture_refuses_hardlinks_crossing_an_ignored_boundary() -> Result<()> {
    let project = tempdir()?;
    git(project.path(), &["init", "-q"])?;
    fs::write(project.path().join(".gitignore"), "ignored/\n")?;
    fs::create_dir(project.path().join("ignored"))?;
    let selected = project.path().join("selected");
    fs::write(&selected, "shared inode")?;
    fs::hard_link(&selected, project.path().join("ignored/link"))?;
    let mut app = App::open(project.path().to_path_buf())?;

    let error = app.create(None, false).unwrap_err().to_string();
    assert!(
        error.contains("outside the selected snapshot scope"),
        "{error}"
    );
    Ok(())
}

#[test]
fn dry_run_does_not_mutate() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join("value"), b"one")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    fs::write(project.path().join("value"), b"two")?;
    app.restore(Some(&checkpoint), true, false)?;
    assert_eq!(fs::read(project.path().join("value"))?, b"two");
    Ok(())
}
