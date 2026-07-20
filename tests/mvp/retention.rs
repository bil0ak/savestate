use super::*;

#[test]
fn manual_checkpoints_are_not_automatically_pruned() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join(".savestate.toml"), "keep_last = 2\n")?;
    fs::write(project.path().join("file"), b"one")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let manual = app.create(Some("Codex · manually named".into()), false)?;
    fs::write(project.path().join("file"), b"two")?;
    app.create(None, false)?;
    fs::write(project.path().join("file"), b"three")?;
    app.create(None, false)?;
    let store = Store::open(project.path().join(".savestate"))?;
    assert_eq!(store.ids()?.len(), 3);
    assert_eq!(
        store.load(&manual)?.resolved_kind(),
        savestate::model::CheckpointKind::Manual
    );
    Ok(())
}

#[test]
fn agent_retention_is_separate_from_manual_checkpoints() -> Result<()> {
    let project = tempdir()?;
    fs::write(
        project.path().join(".savestate.toml"),
        "[retention]\nagent = 2\n",
    )?;
    fs::write(project.path().join("file"), b"one")?;
    let mut app = App::open(project.path().to_path_buf())?;
    integrations::run_hook(
        &mut app,
        Agent::Codex,
        integrations::HookEvent::SessionStart,
    )?;
    fs::write(project.path().join("file"), b"two")?;
    integrations::run_hook(
        &mut app,
        Agent::Codex,
        integrations::HookEvent::SessionStart,
    )?;
    fs::write(project.path().join("file"), b"three")?;
    integrations::run_hook(
        &mut app,
        Agent::Codex,
        integrations::HookEvent::SessionStart,
    )?;
    let store = Store::open(project.path().join(".savestate"))?;
    assert_eq!(store.ids()?.len(), 2);
    Ok(())
}

#[test]
fn claude_turn_checkpoints_use_agent_retention() -> Result<()> {
    let project = tempdir()?;
    fs::write(
        project.path().join(".savestate.toml"),
        "[retention]\nagent = 1\n",
    )?;
    let mut app = App::open(project.path().to_path_buf())?;

    for (turn, contents) in [("first", "one"), ("second", "two")] {
        let prompt = serde_json::json!({
            "session_id": "claude-session",
            "hook_event_name": "UserPromptSubmit",
            "prompt": format!("make the {turn} change")
        });
        integrations::handle_claude_hook(
            &mut app,
            integrations::HookEvent::UserPrompt,
            &prompt.to_string(),
        )?;
        fs::write(project.path().join("file"), contents)?;
        let stop = serde_json::json!({
            "session_id": "claude-session",
            "hook_event_name": "Stop",
            "last_assistant_message": "done"
        });
        integrations::handle_claude_hook(
            &mut app,
            integrations::HookEvent::Stop,
            &stop.to_string(),
        )?;
    }

    let store = Store::open(project.path().join(".savestate"))?;
    assert_eq!(store.ids()?.len(), 1);
    assert_eq!(
        store.load("latest")?.label.as_deref(),
        Some("Claude · make the second change")
    );
    Ok(())
}

#[test]
fn pre_restore_checkpoint_never_prunes_the_selected_target() -> Result<()> {
    let project = tempdir()?;
    fs::write(
        project.path().join(".savestate.toml"),
        "[retention]\nagent = 10\nrecovery = 1\nrun = 1\n",
    )?;
    fs::write(project.path().join("value"), "one")?;
    let mut app = App::open(project.path().to_path_buf())?;
    assert!(
        integrations::run_hook(
            &mut app,
            Agent::Claude,
            integrations::HookEvent::SessionStart,
        )?
        .is_none()
    );
    let store = Store::open(project.path().join(".savestate"))?;
    let target = store.ids()?[0].clone();
    fs::write(project.path().join("value"), "two")?;
    assert!(
        integrations::run_hook(
            &mut app,
            Agent::Claude,
            integrations::HookEvent::SessionStart,
        )?
        .is_none()
    );

    fs::write(
        project.path().join(".savestate.toml"),
        "[retention]\nagent = 1\nrecovery = 1\nrun = 1\n",
    )?;
    let mut app = App::open(project.path().to_path_buf())?;
    app.restore(Some(&target), false, true)?;

    assert_eq!(fs::read_to_string(project.path().join("value"))?, "one");
    assert!(store.load(&target).is_ok());
    Ok(())
}

#[test]
fn pinned_agent_checkpoint_survives_retention_until_unpinned() -> Result<()> {
    let project = tempdir()?;
    fs::write(
        project.path().join(".savestate.toml"),
        "[retention]\nagent = 1\n",
    )?;
    fs::write(project.path().join("file"), "one")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let first = integrations::run_hook(
        &mut app,
        Agent::Codex,
        integrations::HookEvent::SessionStart,
    )?;
    assert!(first.is_none());
    let store = Store::open(project.path().join(".savestate"))?;
    let pinned = store.resolve_id("latest")?;
    app.pin(&pinned)?;

    fs::write(project.path().join("file"), "two")?;
    integrations::run_hook(
        &mut app,
        Agent::Codex,
        integrations::HookEvent::SessionStart,
    )?;
    assert_eq!(store.ids()?.len(), 2);
    assert!(store.load(&pinned).is_ok());

    app.unpin(&pinned)?;
    app.prune(false)?;
    assert_eq!(store.ids()?.len(), 1);
    assert!(store.load(&pinned).is_err());
    Ok(())
}
