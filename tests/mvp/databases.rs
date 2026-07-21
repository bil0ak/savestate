use super::*;

#[test]
fn sqlite_uses_a_consistent_local_backup() -> Result<()> {
    let project = tempdir()?;
    let database = project.path().join("state-without-an-extension");
    {
        let connection = Connection::open(&database)?;
        connection.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT); INSERT INTO items(value) VALUES ('before');")?;
    }
    fs::write(
        project.path().join(".savestate.toml"),
        "[experimental]\ndatabases = true\n\n[[sqlite]]\npath = \"state-without-an-extension\"\n",
    )?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(Some("sqlite".into()), false)?;
    {
        let connection = Connection::open(&database)?;
        connection.execute("INSERT INTO items(value) VALUES ('after')", [])?;
    }
    app.restore(Some(&checkpoint), false, true)?;
    let connection = Connection::open(&database)?;
    let count: i64 = connection.query_row("SELECT count(*) FROM items", [], |row| row.get(0))?;
    assert_eq!(count, 1);
    let value: String = connection.query_row("SELECT value FROM items", [], |row| row.get(0))?;
    assert_eq!(value, "before");
    Ok(())
}

#[test]
fn sqlite_wal_backup_restores_when_physical_file_size_differs() -> Result<()> {
    let project = tempdir()?;
    let database = project.path().join("state.sqlite");
    fs::write(
        project.path().join(".savestate.toml"),
        "[experimental]\ndatabases = true\n\n[[sqlite]]\npath = \"state.sqlite\"\n",
    )?;

    let connection = Connection::open(&database)?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA wal_autocheckpoint=0;
         CREATE TABLE items(value BLOB);
         WITH RECURSIVE values_to_add(value) AS (
             VALUES(1) UNION ALL SELECT value + 1 FROM values_to_add WHERE value < 512
         )
         INSERT INTO items SELECT randomblob(2000) FROM values_to_add;",
    )?;

    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(Some("wal".into()), false)?;
    let store = Store::open(project.path().join(".savestate"))?;
    let manifest = store.load(&checkpoint)?;
    let file_size = manifest
        .files
        .iter()
        .find(|entry| entry.path == std::path::Path::new("state.sqlite"))
        .map(|entry| entry.size)
        .expect("SQLite file entry");
    let backup_hash = manifest
        .services
        .iter()
        .find_map(|service| match service {
            savestate::model::ServiceArtifact::Sqlite { object_hash, .. } => Some(object_hash),
            savestate::model::ServiceArtifact::Postgres { .. } => None,
        })
        .expect("SQLite backup artifact");
    let backup_size = fs::metadata(store.object_path(backup_hash))?.len();
    assert_ne!(
        file_size, backup_size,
        "the fixture must exercise WAL/main-file layout divergence"
    );

    drop(connection);
    Connection::open(&database)?.execute("DELETE FROM items", [])?;
    app.restore(Some(&checkpoint), false, true)?;

    let count: i64 =
        Connection::open(&database)?
            .query_row("SELECT count(*) FROM items", [], |row| row.get(0))?;
    assert_eq!(count, 512);
    Ok(())
}

#[cfg(unix)]
#[test]
fn hardlinked_sqlite_is_refused_before_checkpoint_publication() -> Result<()> {
    let project = tempdir()?;
    let database = project.path().join("state.sqlite");
    Connection::open(&database)?.execute("CREATE TABLE items(id INTEGER)", [])?;
    fs::hard_link(&database, project.path().join("state-copy.sqlite"))?;
    fs::write(
        project.path().join(".savestate.toml"),
        "[experimental]\ndatabases = true\n\n[[sqlite]]\npath = \"state.sqlite\"\n",
    )?;
    let mut app = App::open(project.path().to_path_buf())?;

    let error = app.create(None, false).unwrap_err().to_string();
    assert!(error.contains("hardlink"), "{error}");
    assert!(
        Store::open(project.path().join(".savestate"))?
            .ids()?
            .is_empty()
    );
    Ok(())
}

#[test]
fn configured_sqlite_is_untouched_by_automatic_agent_checkpoints() -> Result<()> {
    automatic_agent_checkpoint_excludes_database(Agent::Codex)?;
    automatic_agent_checkpoint_excludes_database(Agent::Claude)
}

fn automatic_agent_checkpoint_excludes_database(agent: Agent) -> Result<()> {
    let project = tempdir()?;
    let database = project.path().join("state.sqlite");
    {
        let connection = Connection::open(&database)?;
        connection.execute_batch(
            "CREATE TABLE items(id INTEGER PRIMARY KEY); INSERT INTO items DEFAULT VALUES;",
        )?;
    }
    fs::write(
        project.path().join(".savestate.toml"),
        "[experimental]\ndatabases = true\n\n[[sqlite]]\npath = \"state.sqlite\"\n",
    )?;
    fs::write(project.path().join("source"), "before")?;
    let mut app = App::open(project.path().to_path_buf())?;

    let prompt = match agent {
        Agent::Codex => serde_json::json!({
            "session_id": "sqlite-session",
            "turn_id": "sqlite-turn",
            "hook_event_name": "UserPromptSubmit",
            "prompt": "change source and database"
        }),
        Agent::Claude => serde_json::json!({
            "session_id": "sqlite-session",
            "hook_event_name": "UserPromptSubmit",
            "prompt": "change source and database"
        }),
    };
    match agent {
        Agent::Codex => integrations::handle_codex_hook(
            &mut app,
            CodexHookEvent::UserPrompt,
            &prompt.to_string(),
        )?,
        Agent::Claude => integrations::handle_claude_hook(
            &mut app,
            integrations::HookEvent::UserPrompt,
            &prompt.to_string(),
        )?,
    };
    fs::write(project.path().join("source"), "after-turn")?;
    Connection::open(&database)?.execute("INSERT INTO items DEFAULT VALUES", [])?;
    let stop = match agent {
        Agent::Codex => serde_json::json!({
            "session_id": "sqlite-session",
            "turn_id": "sqlite-turn",
            "hook_event_name": "Stop",
            "last_assistant_message": "done"
        }),
        Agent::Claude => serde_json::json!({
            "session_id": "sqlite-session",
            "hook_event_name": "Stop",
            "last_assistant_message": "done"
        }),
    };
    match agent {
        Agent::Codex => {
            integrations::handle_codex_hook(&mut app, CodexHookEvent::Stop, &stop.to_string())?
        }
        Agent::Claude => integrations::handle_claude_hook(
            &mut app,
            integrations::HookEvent::Stop,
            &stop.to_string(),
        )?,
    };
    let store = Store::open(project.path().join(".savestate"))?;
    let automatic = store.load("latest")?;
    assert!(automatic.services.is_empty());
    assert!(
        !automatic
            .files
            .iter()
            .any(|entry| entry.path == std::path::Path::new("state.sqlite"))
    );

    fs::write(project.path().join("source"), "later")?;
    Connection::open(&database)?.execute("INSERT INTO items DEFAULT VALUES", [])?;
    app.restore(Some(&automatic.id), false, true)?;
    assert_eq!(
        fs::read_to_string(project.path().join("source"))?,
        "after-turn"
    );
    let count: i64 =
        Connection::open(&database)?
            .query_row("SELECT count(*) FROM items", [], |row| row.get(0))?;
    assert_eq!(count, 3);
    Ok(())
}

#[test]
fn sqlite_requires_both_explicit_configuration_and_experimental_switch() -> Result<()> {
    let project = tempdir()?;
    let database = project.path().join("state.sqlite");
    Connection::open(&database)?.execute("CREATE TABLE items(id INTEGER)", [])?;
    fs::write(
        project.path().join(".savestate.toml"),
        "[[sqlite]]\npath = \"state.sqlite\"\n",
    )?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    let manifest = Store::open(project.path().join(".savestate"))?.load(&checkpoint)?;
    assert!(manifest.services.is_empty());
    assert!(
        !manifest
            .files
            .iter()
            .any(|entry| entry.path == std::path::Path::new("state.sqlite"))
    );
    Ok(())
}

#[test]
fn disabled_experimental_postgres_does_not_run_database_tools() -> Result<()> {
    let project = tempdir()?;
    fs::write(
        project.path().join(".savestate.toml"),
        "[[postgres]]\nname = \"disabled\"\nurl_env = \"SAVESTATE_MISSING_DATABASE_URL\"\nmaintenance_db = \"postgres\"\n",
    )?;
    fs::write(project.path().join("source"), "captured")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(None, false)?;
    let manifest = Store::open(project.path().join(".savestate"))?.load(&checkpoint)?;
    assert!(manifest.services.is_empty());
    Ok(())
}

#[test]
fn database_restore_requires_experimental_opt_in() -> Result<()> {
    let project = tempdir()?;
    let database = project.path().join("state.sqlite");
    {
        let connection = Connection::open(&database)?;
        connection.execute_batch(
            "CREATE TABLE items(id INTEGER PRIMARY KEY); INSERT INTO items DEFAULT VALUES;",
        )?;
    }
    let config = project.path().join(".savestate.toml");
    fs::write(
        &config,
        "[experimental]\ndatabases = true\n\n[[sqlite]]\npath = \"state.sqlite\"\n",
    )?;
    let mut enabled = App::open(project.path().to_path_buf())?;
    let checkpoint = enabled.create(None, false)?;
    Connection::open(&database)?.execute("INSERT INTO items DEFAULT VALUES", [])?;

    fs::write(
        &config,
        "[experimental]\ndatabases = false\n\n[[sqlite]]\npath = \"state.sqlite\"\n",
    )?;
    let mut disabled = App::open(project.path().to_path_buf())?;
    let error = disabled
        .restore(Some(&checkpoint), false, true)
        .unwrap_err();
    assert!(error.to_string().contains("experimental databases"));
    let count: i64 =
        Connection::open(&database)?
            .query_row("SELECT count(*) FROM items", [], |row| row.get(0))?;
    assert_eq!(count, 2);
    Ok(())
}

#[test]
#[ignore = "requires SAVESTATE_TEST_POSTGRES_URL pointing to a disposable database with CREATEDB"]
fn postgres_dump_restore_and_cutover() -> Result<()> {
    let url = std::env::var("SAVESTATE_TEST_POSTGRES_URL")?;
    let project = tempdir()?;
    fs::write(
        project.path().join(".savestate.toml"),
        "[experimental]\ndatabases = true\n\n[[postgres]]\nname = \"test\"\nurl_env = \"SAVESTATE_TEST_POSTGRES_URL\"\nmaintenance_db = \"postgres\"\nallow_restore = true\n",
    )?;
    psql(
        &url,
        "DROP TABLE IF EXISTS savestate_items; CREATE TABLE savestate_items(id integer primary key, value text); INSERT INTO savestate_items VALUES (1, 'before')",
    )?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(Some("postgres".into()), false)?;
    psql(&url, "INSERT INTO savestate_items VALUES (2, 'after')")?;
    app.restore(Some(&checkpoint), false, true)?;
    let count = psql(&url, "SELECT count(*) FROM savestate_items")?;
    assert_eq!(count.trim(), "1");
    Ok(())
}
