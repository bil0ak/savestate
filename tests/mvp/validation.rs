use super::*;

#[test]
fn verification_detects_object_corruption() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join("file"), b"trusted")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    let store = Store::open(project.path().join(".savestate"))?;
    let manifest = store.load(&checkpoint)?;
    let hash = manifest
        .files
        .iter()
        .find_map(|entry| entry.object_hash.as_ref())
        .unwrap();
    fs::write(store.object_path(hash), b"corrupt")?;
    assert!(app.verify(Some(&checkpoint)).is_err());
    Ok(())
}

#[test]
fn verification_rejects_manifest_path_traversal() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join("file"), b"trusted")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    let store = Store::open(project.path().join(".savestate"))?;
    let mut manifest = store.load(&checkpoint)?;
    manifest.files[0].path = "../../escape".into();
    assert!(store.verify(&manifest).is_err());
    Ok(())
}

#[test]
fn verification_rejects_scope_patterns_that_escape_the_project() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join("value"), "checkpoint")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    let snapshot = project
        .path()
        .join(".savestate/snapshots")
        .join(&checkpoint);
    let manifest_path = snapshot.join("manifest.json");
    let checksum_path = snapshot.join("manifest.blake3");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    value["capture_scope"]["exclude"] = serde_json::json!(["../outside"]);
    let bytes = serde_json::to_vec_pretty(&value)?;
    fs::write(&manifest_path, &bytes)?;
    fs::write(&checksum_path, blake3::hash(&bytes).to_hex().as_bytes())?;

    let error = Store::open(project.path().join(".savestate"))?
        .load(&checkpoint)
        .expect_err("escaping scope pattern must be refused");
    assert!(format!("{error:#}").contains("stay relative"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn store_refuses_symlinked_internal_directories() -> Result<()> {
    for directory in ["objects", "hooks/codex", "hooks/claude"] {
        let project = tempdir()?;
        let victim = tempdir()?;
        fs::write(project.path().join(".savestate.toml"), "")?;
        drop(App::open(project.path().to_path_buf())?);
        let store_directory = project.path().join(".savestate").join(directory);
        fs::remove_dir(&store_directory)?;
        std::os::unix::fs::symlink(victim.path(), &store_directory)?;

        let error = match App::open(project.path().to_path_buf()) {
            Ok(_) => panic!("symlinked store directory should be refused"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("not a real directory"), "{error}");
        assert_eq!(fs::read_dir(victim.path())?.count(), 0);
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn store_refuses_symlinked_control_files() -> Result<()> {
    let project = tempdir()?;
    let victim = project.path().join("victim");
    fs::write(&victim, b"do not touch")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    let store = Store::open(project.path().join(".savestate"))?;

    std::os::unix::fs::symlink(&victim, store.path().join("journal.json"))?;
    assert!(store.journal().is_err());
    assert!(store.clear_journal().is_err());
    assert_eq!(fs::read(&victim)?, b"do not touch");
    fs::remove_file(store.path().join("journal.json"))?;

    std::os::unix::fs::symlink(&victim, store.path().join("pins").join(&checkpoint))?;
    assert!(store.is_pinned(&checkpoint).is_err());
    assert!(store.prune(0, false).is_err());
    assert!(store.load(&checkpoint).is_ok());
    assert_eq!(fs::read(&victim)?, b"do not touch");
    Ok(())
}

#[cfg(unix)]
#[test]
fn integration_refuses_symlinked_project_configuration_directories() -> Result<()> {
    let project = tempdir()?;
    let victim = tempdir()?;
    std::os::unix::fs::symlink(victim.path(), project.path().join(".codex"))?;

    let error = integrations::configure(project.path(), Agent::Codex, false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("not a real directory"), "{error}");
    assert_eq!(fs::read_dir(victim.path())?.count(), 0);
    Ok(())
}

#[cfg(unix)]
#[test]
fn project_configuration_must_not_be_a_symlink() -> Result<()> {
    let project = tempdir()?;
    let external = project.path().join("external.toml");
    fs::write(&external, "")?;
    std::os::unix::fs::symlink(&external, project.path().join(".savestate.toml"))?;

    let error = match App::open(project.path().to_path_buf()) {
        Ok(_) => panic!("symlinked project configuration should be refused"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("not a regular file"), "{error}");
    Ok(())
}

#[test]
fn store_reports_incomplete_published_snapshots_as_corruption() -> Result<()> {
    let project = tempdir()?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    let store = Store::open(project.path().join(".savestate"))?;
    fs::remove_file(
        store
            .path()
            .join("snapshots")
            .join(&checkpoint)
            .join("complete"),
    )?;

    let error = store.ids().unwrap_err().to_string();
    assert!(error.contains("incomplete or corrupt"), "{error}");
    fs::write(project.path().join("another"), "state")?;
    assert!(app.create(None, false).is_err());
    let published = fs::read_dir(store.path().join("snapshots"))?
        .filter_map(std::result::Result::ok)
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .count();
    assert_eq!(published, 1);
    Ok(())
}

#[cfg(unix)]
#[test]
fn existence_checks_refuse_dangling_symlink_ancestors() -> Result<()> {
    let directory = tempdir()?;
    std::os::unix::fs::symlink("missing", directory.path().join("redirect"))?;
    let error = savestate::filesystem::path_exists(&directory.path().join("redirect/child"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("descends through"), "{error}");
    Ok(())
}
