use super::*;

#[test]
fn restores_registered_external_directory() -> Result<()> {
    let sandbox = tempdir()?;
    let project = sandbox.path().join("project");
    let external = sandbox.path().join("external");
    fs::create_dir(&project)?;
    fs::create_dir(&external)?;
    fs::write(external.join("tool.conf"), b"safe")?;
    fs::write(
        project.join(".savestate.toml"),
        format!(
            "external_paths = [{}]\n",
            toml::Value::String(external.to_string_lossy().into())
        ),
    )?;
    let mut app = App::open(project)?;
    let checkpoint = app.create(None, false)?;
    fs::write(external.join("tool.conf"), b"changed")?;
    app.restore(Some(&checkpoint), false, true)?;
    assert_eq!(fs::read(external.join("tool.conf"))?, b"safe");
    Ok(())
}

#[test]
fn restore_never_detaches_a_nested_project_store() -> Result<()> {
    let project = tempdir()?;
    fs::create_dir_all(project.path().join(".local"))?;
    fs::write(
        project.path().join(".savestate.toml"),
        "store = \".local/checkpoints\"\n",
    )?;
    fs::write(project.path().join(".local/settings"), "checkpoint")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    fs::write(project.path().join(".local/settings"), "changed")?;
    fs::write(project.path().join(".local/remove-me"), "temporary")?;

    app.restore(Some(&checkpoint), false, true)?;

    assert_eq!(
        fs::read_to_string(project.path().join(".local/settings"))?,
        "checkpoint"
    );
    assert!(!project.path().join(".local/remove-me").exists());
    let store = Store::open(project.path().join(".local/checkpoints").canonicalize()?)?;
    assert!(store.load(&checkpoint).is_ok());
    assert!(store.journal()?.is_none());
    Ok(())
}

#[test]
fn global_store_is_namespaced_per_project() -> Result<()> {
    let sandbox = tempdir()?;
    let global = sandbox.path().join("global-store");
    let first_root = sandbox.path().join("first-project");
    let second_root = sandbox.path().join("second-project");
    fs::create_dir(&first_root)?;
    fs::create_dir(&second_root)?;
    let config = format!(
        "store = {}\n",
        toml::Value::String(global.to_string_lossy().into_owned())
    );
    fs::write(first_root.join(".savestate.toml"), &config)?;
    fs::write(second_root.join(".savestate.toml"), &config)?;
    fs::write(first_root.join("value"), "first")?;
    fs::write(second_root.join("value"), "second")?;
    let mut first = App::open(first_root)?;
    let mut second = App::open(second_root)?;
    let first_id = first.create(None, false)?;
    let second_id = second.create(None, false)?;

    first.verify(Some(&first_id))?;
    second.verify(Some(&second_id))?;
    assert!(first.verify(Some(&second_id)).is_err());
    assert!(first.delete(&second_id, true).is_err());
    second.verify(Some(&second_id))?;
    assert_eq!(fs::read_dir(global.join("projects"))?.count(), 2);
    Ok(())
}

#[test]
fn single_project_legacy_global_store_remains_readable() -> Result<()> {
    let sandbox = tempdir()?;
    let project = sandbox.path().join("project");
    let global = sandbox.path().join("legacy-global");
    fs::create_dir(&project)?;
    fs::write(project.join("value"), "checkpoint")?;
    let mut app = App::open(project.clone())?;
    let checkpoint = app.create(None, false)?;
    drop(app);
    fs::rename(project.join(".savestate"), &global)?;
    fs::write(
        project.join(".savestate.toml"),
        format!(
            "store = {}\n",
            toml::Value::String(global.to_string_lossy().into_owned())
        ),
    )?;

    App::open(project)?.verify(Some(&checkpoint))?;
    assert!(!global.join("projects").exists());
    Ok(())
}

#[test]
fn restore_refuses_an_external_root_that_changed_type() -> Result<()> {
    let project = tempdir()?;
    let external_parent = tempdir()?;
    let external = external_parent.path().join("tool-state");
    fs::create_dir(&external)?;
    fs::write(external.join("value"), "checkpoint")?;
    fs::write(
        project.path().join(".savestate.toml"),
        format!(
            "external_paths = [{}]\n",
            toml::Value::String(external.display().to_string())
        ),
    )?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(Some("baseline".into()), false)?;

    fs::remove_dir_all(&external)?;
    fs::write(&external, "replacement file")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let error = app
        .restore(Some(&checkpoint), false, true)
        .expect_err("root identity mismatch must be refused");
    assert!(format!("{error:#}").contains("changed identity"));
    assert_eq!(fs::read_to_string(&external)?, "replacement file");
    Ok(())
}

#[cfg(unix)]
#[test]
fn restore_refuses_an_external_root_recreated_at_the_same_path() -> Result<()> {
    let project = tempdir()?;
    let external_parent = tempdir()?;
    let external = external_parent.path().join("tool-state");
    fs::create_dir(&external)?;
    fs::write(external.join("value"), "checkpoint")?;
    fs::write(
        project.path().join(".savestate.toml"),
        format!(
            "external_paths = [{}]\n",
            toml::Value::String(external.display().to_string())
        ),
    )?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(Some("baseline".into()), false)?;

    fs::remove_dir_all(&external)?;
    fs::create_dir(&external)?;
    fs::write(external.join("value"), "replacement")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let error = app
        .restore(Some(&checkpoint), false, true)
        .expect_err("recreated root must be refused");
    assert!(format!("{error:#}").contains("changed identity"));
    assert_eq!(fs::read_to_string(external.join("value"))?, "replacement");
    Ok(())
}
