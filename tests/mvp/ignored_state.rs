use super::*;

#[test]
fn git_ignored_top_level_and_nested_state_survives_restore() -> Result<()> {
    let project = tempdir()?;
    git(project.path(), &["init", "-q"])?;
    fs::write(project.path().join(".gitignore"), "node_modules/\n")?;
    fs::create_dir_all(project.path().join("node_modules/pkg"))?;
    fs::create_dir_all(project.path().join("src/node_modules/pkg"))?;
    fs::write(project.path().join("node_modules/pkg/state"), "top-before")?;
    fs::write(
        project.path().join("src/node_modules/pkg/state"),
        "nested-before",
    )?;
    fs::write(project.path().join("src/value"), "captured-before")?;

    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    let store = Store::open(project.path().join(".savestate"))?;
    let manifest = store.load(&checkpoint)?;
    assert!(
        !manifest
            .files
            .iter()
            .any(|entry| entry.path.starts_with("node_modules"))
    );
    assert!(
        !manifest
            .files
            .iter()
            .any(|entry| entry.path.starts_with("src/node_modules"))
    );

    fs::write(project.path().join("node_modules/pkg/state"), "top-after")?;
    fs::write(
        project.path().join("node_modules/pkg/new-after-capture"),
        "new",
    )?;
    fs::write(
        project.path().join("src/node_modules/pkg/state"),
        "nested-after",
    )?;
    fs::write(project.path().join("src/value"), "captured-after")?;
    app.restore(Some(&checkpoint), false, true)?;

    assert_eq!(
        fs::read_to_string(project.path().join("src/value"))?,
        "captured-before"
    );
    assert_eq!(
        fs::read_to_string(project.path().join("node_modules/pkg/state"))?,
        "top-after"
    );
    assert_eq!(
        fs::read_to_string(project.path().join("node_modules/pkg/new-after-capture"))?,
        "new"
    );
    assert_eq!(
        fs::read_to_string(project.path().join("src/node_modules/pkg/state"))?,
        "nested-after"
    );
    Ok(())
}

#[test]
fn tracked_descendant_does_not_delete_ignored_siblings() -> Result<()> {
    let project = tempdir()?;
    git(project.path(), &["init", "-q"])?;
    fs::create_dir_all(project.path().join("src/ignored-dir"))?;
    fs::write(project.path().join(".gitignore"), "src/ignored-dir/\n")?;
    fs::write(
        project.path().join("src/ignored-dir/tracked.txt"),
        "checkpoint",
    )?;
    fs::write(
        project.path().join("src/ignored-dir/precious.cache"),
        "local-only",
    )?;
    git(
        project.path(),
        &["add", "-f", "src/ignored-dir/tracked.txt"],
    )?;

    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(Some("baseline".into()), false)?;
    fs::write(
        project.path().join("src/ignored-dir/tracked.txt"),
        "changed",
    )?;
    app.restore(Some(&checkpoint), false, true)?;

    assert_eq!(
        fs::read_to_string(project.path().join("src/ignored-dir/tracked.txt"))?,
        "checkpoint"
    );
    assert_eq!(
        fs::read_to_string(project.path().join("src/ignored-dir/precious.cache"))?,
        "local-only"
    );
    Ok(())
}

#[test]
fn explicit_include_inside_ignored_parent_is_restored_without_touching_neighbors() -> Result<()> {
    let project = tempdir()?;
    git(project.path(), &["init", "-q"])?;
    fs::write(project.path().join(".gitignore"), "data/\n")?;
    fs::write(
        project.path().join(".savestate.toml"),
        "include = [\"data/state.env\"]\n",
    )?;
    fs::create_dir_all(project.path().join("data/cache"))?;
    fs::write(project.path().join("data/state.env"), "included-before")?;
    fs::write(project.path().join("data/cache/value"), "ignored-before")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;

    fs::write(project.path().join("data/state.env"), "included-after")?;
    fs::write(project.path().join("data/cache/value"), "ignored-after")?;
    app.restore(Some(&checkpoint), false, true)?;
    assert_eq!(
        fs::read_to_string(project.path().join("data/state.env"))?,
        "included-before"
    );
    assert_eq!(
        fs::read_to_string(project.path().join("data/cache/value"))?,
        "ignored-after"
    );
    fs::remove_file(project.path().join("data/state.env"))?;
    fs::write(project.path().join("data/cache/value"), "ignored-later")?;
    app.restore(Some(&checkpoint), false, true)?;
    assert_eq!(
        fs::read_to_string(project.path().join("data/state.env"))?,
        "included-before"
    );
    assert_eq!(
        fs::read_to_string(project.path().join("data/cache/value"))?,
        "ignored-later"
    );
    Ok(())
}

#[test]
fn nested_explicit_exclude_is_preserved_during_restore() -> Result<()> {
    let project = tempdir()?;
    fs::write(
        project.path().join(".savestate.toml"),
        "exclude = [\"src/cache\"]\n",
    )?;
    fs::create_dir_all(project.path().join("src/cache"))?;
    fs::write(project.path().join("src/value"), "before")?;
    fs::write(project.path().join("src/cache/state"), "cache-before")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    fs::write(project.path().join("src/value"), "after")?;
    fs::write(project.path().join("src/cache/state"), "cache-after")?;
    app.restore(Some(&checkpoint), false, true)?;
    assert_eq!(
        fs::read_to_string(project.path().join("src/value"))?,
        "before"
    );
    assert_eq!(
        fs::read_to_string(project.path().join("src/cache/state"))?,
        "cache-after"
    );
    Ok(())
}

#[test]
fn tracked_files_win_over_git_ignore_and_savestate_exclude_wins_over_include() -> Result<()> {
    let project = tempdir()?;
    git(project.path(), &["init", "-q"])?;
    fs::write(project.path().join("tracked.log"), "tracked")?;
    git(project.path(), &["add", "tracked.log"])?;
    fs::write(project.path().join(".gitignore"), "*.log\n")?;
    fs::write(
        project.path().join(".savestate.toml"),
        "include = [\"secret.log\"]\nexclude = [\"secret.log\"]\n",
    )?;
    fs::write(project.path().join("secret.log"), "excluded")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    let manifest = Store::open(project.path().join(".savestate"))?.load(&checkpoint)?;
    assert!(
        manifest
            .files
            .iter()
            .any(|entry| entry.path == std::path::Path::new("tracked.log"))
    );
    assert!(
        !manifest
            .files
            .iter()
            .any(|entry| entry.path == std::path::Path::new("secret.log"))
    );
    fs::write(project.path().join("tracked.log"), "changed")?;
    fs::write(project.path().join("secret.log"), "preserved")?;
    app.restore(Some(&checkpoint), false, true)?;
    assert_eq!(
        fs::read_to_string(project.path().join("tracked.log"))?,
        "tracked"
    );
    assert_eq!(
        fs::read_to_string(project.path().join("secret.log"))?,
        "preserved"
    );
    Ok(())
}

#[test]
fn ripgrep_ignore_files_are_not_snapshot_rules_and_include_ignored_is_one_off() -> Result<()> {
    let project = tempdir()?;
    git(project.path(), &["init", "-q"])?;
    fs::write(project.path().join(".gitignore"), "ignored/\n")?;
    fs::write(project.path().join(".ignore"), "ordinary.txt\n")?;
    fs::write(project.path().join("ordinary.txt"), "ordinary")?;
    fs::create_dir(project.path().join("ignored"))?;
    fs::write(project.path().join("ignored/value"), "ignored")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let normal = app.create(None, false)?;
    let full = app.create_with_options(None, false, true)?;
    let store = Store::open(project.path().join(".savestate"))?;
    assert!(
        store
            .load(&normal)?
            .files
            .iter()
            .any(|entry| entry.path == std::path::Path::new("ordinary.txt"))
    );
    let full = store.load(&full)?;
    assert!(
        full.files
            .iter()
            .any(|entry| entry.path == std::path::Path::new("ignored/value"))
    );
    assert!(full.capture_scope.include_ignored);
    Ok(())
}

#[test]
fn v1_manifest_without_scope_metadata_remains_readable() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join("value"), "before")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    let manifest_path = project
        .path()
        .join(".savestate/snapshots")
        .join(&checkpoint)
        .join("manifest.json");
    let checksum_path = project
        .path()
        .join(".savestate/snapshots")
        .join(&checkpoint)
        .join("manifest.blake3");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    value["schema_version"] = 1.into();
    value.as_object_mut().unwrap().remove("capture_scope");
    let bytes = serde_json::to_vec_pretty(&value)?;
    fs::write(&manifest_path, &bytes)?;
    fs::write(&checksum_path, blake3::hash(&bytes).to_hex().as_bytes())?;
    let manifest = Store::open(project.path().join(".savestate"))?.load(&checkpoint)?;
    assert_eq!(manifest.schema_version, 1);
    assert!(!manifest.capture_scope.respect_gitignore);
    app.verify(Some(&checkpoint))?;
    app.list(false)?;
    fs::write(project.path().join("value"), "after")?;
    app.diff(Some(&checkpoint), "current", false)?;
    app.restore(Some(&checkpoint), false, true)?;
    assert_eq!(fs::read_to_string(project.path().join("value"))?, "before");
    app.delete(&checkpoint, true)?;
    assert!(
        !Store::open(project.path().join(".savestate"))?
            .ids()?
            .contains(&checkpoint)
    );
    Ok(())
}

#[test]
fn init_adds_only_exact_ignored_env_and_sqlite_paths() -> Result<()> {
    let project = tempdir()?;
    git(project.path(), &["init", "-q"])?;
    fs::write(project.path().join(".gitignore"), ".env\nstate/\n")?;
    fs::write(
        project.path().join(".env"),
        "TOKEN=secret\nDATABASE_URL=postgresql://localhost/app\n",
    )?;
    fs::create_dir(project.path().join("state"))?;
    let database = project.path().join("state/app.sqlite");
    Connection::open(&database)?.execute("CREATE TABLE item(id INTEGER)", [])?;
    fs::write(project.path().join("state/not-a-database.db"), "plain")?;

    let mut app = App::open(project.path().to_path_buf())?;
    app.init()?;
    let source = fs::read_to_string(project.path().join(".savestate.toml"))?;
    let value: toml::Value = toml::from_str(&source)?;
    let includes = value["include"].as_array().unwrap();
    assert!(includes.iter().any(|value| value.as_str() == Some(".env")));
    assert!(
        includes
            .iter()
            .any(|value| value.as_str() == Some("state/app.sqlite"))
    );
    assert!(
        !includes
            .iter()
            .any(|value| value.as_str() == Some("state/not-a-database.db"))
    );
    assert!(!source.contains("*.sqlite"));
    assert!(!source.lines().any(|line| line.trim() == "[[postgres]]"));
    Ok(())
}

#[test]
fn restore_uses_recorded_ignore_rules_even_when_live_rules_changed() -> Result<()> {
    let project = tempdir()?;
    git(project.path(), &["init", "-q"])?;
    fs::write(project.path().join(".gitignore"), "*.log\n")?;
    fs::write(project.path().join("value"), "before")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;

    fs::write(project.path().join(".gitignore"), "")?;
    fs::write(project.path().join("created-after.log"), "preserve")?;
    fs::write(project.path().join("value"), "after")?;
    app.restore(Some(&checkpoint), false, true)?;
    assert_eq!(fs::read_to_string(project.path().join("value"))?, "before");
    assert_eq!(
        fs::read_to_string(project.path().join("created-after.log"))?,
        "preserve"
    );
    assert_eq!(
        fs::read_to_string(project.path().join(".gitignore"))?,
        "*.log\n"
    );
    Ok(())
}

#[test]
fn nested_gitignore_and_negation_follow_git_semantics() -> Result<()> {
    let project = tempdir()?;
    git(project.path(), &["init", "-q"])?;
    fs::create_dir_all(project.path().join("nested/build"))?;
    fs::write(
        project.path().join("nested/.gitignore"),
        "build/*\n!build/keep.txt\n",
    )?;
    fs::write(project.path().join("nested/build/drop.txt"), "drop")?;
    fs::write(project.path().join("nested/build/keep.txt"), "keep")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    let manifest = Store::open(project.path().join(".savestate"))?.load(&checkpoint)?;
    assert!(
        manifest
            .files
            .iter()
            .any(|entry| entry.path == std::path::Path::new("nested/build/keep.txt"))
    );
    assert!(
        !manifest
            .files
            .iter()
            .any(|entry| entry.path == std::path::Path::new("nested/build/drop.txt"))
    );
    Ok(())
}

#[test]
fn repository_and_global_git_excludes_are_honored() -> Result<()> {
    let sandbox = tempdir()?;
    let project = sandbox.path().join("project");
    fs::create_dir(&project)?;
    git(&project, &["init", "-q"])?;
    let global = sandbox.path().join("global-ignore");
    fs::write(&global, "global.tmp\n")?;
    git(
        &project,
        &["config", "core.excludesFile", global.to_str().unwrap()],
    )?;
    let git_dir = git_output(&project, &["rev-parse", "--git-dir"])?;
    fs::write(
        project.join(git_dir.trim()).join("info/exclude"),
        "info.tmp\n",
    )?;
    fs::write(project.join("global.tmp"), "ignored")?;
    fs::write(project.join("info.tmp"), "ignored")?;
    fs::write(project.join("kept.tmp"), "kept")?;
    let mut app = App::open(project.clone())?;
    let checkpoint = app.create(None, false)?;
    let manifest = Store::open(project.join(".savestate"))?.load(&checkpoint)?;
    assert!(
        !manifest
            .files
            .iter()
            .any(|entry| entry.path == std::path::Path::new("global.tmp"))
    );
    assert!(
        !manifest
            .files
            .iter()
            .any(|entry| entry.path == std::path::Path::new("info.tmp"))
    );
    assert!(
        manifest
            .files
            .iter()
            .any(|entry| entry.path == std::path::Path::new("kept.tmp"))
    );
    Ok(())
}

#[test]
fn linked_git_worktree_uses_git_aware_scope() -> Result<()> {
    let sandbox = tempdir()?;
    let main = sandbox.path().join("main");
    let worktree = sandbox.path().join("worktree");
    fs::create_dir(&main)?;
    git(&main, &["init", "-q"])?;
    fs::write(main.join(".gitignore"), "node_modules/\n")?;
    fs::write(main.join("tracked"), "tracked")?;
    git(&main, &["add", ".gitignore", "tracked"])?;
    git(
        &main,
        &[
            "-c",
            "user.name=Savestate Test",
            "-c",
            "user.email=savestate@example.invalid",
            "commit",
            "-q",
            "-m",
            "fixture",
        ],
    )?;
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "fixture-worktree",
            worktree.to_str().unwrap(),
        ],
    )?;
    fs::create_dir(worktree.join("node_modules"))?;
    fs::write(worktree.join("node_modules/value"), "ignored")?;
    let mut app = App::open(worktree.clone())?;
    let checkpoint = app.create(None, false)?;
    let manifest = Store::open(worktree.join(".savestate"))?.load(&checkpoint)?;
    assert!(
        manifest
            .files
            .iter()
            .any(|entry| entry.path == std::path::Path::new("tracked"))
    );
    assert!(
        !manifest
            .files
            .iter()
            .any(|entry| entry.path.starts_with("node_modules"))
    );
    Ok(())
}
