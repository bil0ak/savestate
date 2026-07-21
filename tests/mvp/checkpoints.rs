use super::*;

#[test]
fn if_changed_deduplicates_checkpoints() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join("file"), b"same")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let first = app.create(None, false)?;
    let second = app.create(None, true)?;
    assert_eq!(first, second);
    let store = Store::open(project.path().join(".savestate"))?;
    assert_eq!(store.ids()?.len(), 1);
    Ok(())
}

#[test]
fn if_changed_never_reuses_a_checkpoint_with_different_retention_semantics() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join("file"), "same")?;
    let mut app = App::open(project.path().to_path_buf())?;
    assert!(
        integrations::run_hook(
            &mut app,
            Agent::Claude,
            integrations::HookEvent::SessionStart,
        )?
        .is_none()
    );
    let manual = app.create(None, true)?;
    let store = Store::open(project.path().join(".savestate"))?;
    assert_eq!(store.ids()?.len(), 2);
    assert_eq!(
        store.load(&manual)?.resolved_kind(),
        savestate::model::CheckpointKind::Manual
    );
    Ok(())
}

#[test]
fn if_changed_does_not_reuse_a_checkpoint_with_different_roots() -> Result<()> {
    let sandbox = tempdir()?;
    let project = sandbox.path().join("project");
    let first_external = sandbox.path().join("external-one");
    let second_external = sandbox.path().join("external-two");
    fs::create_dir(&project)?;
    fs::create_dir(&first_external)?;
    fs::create_dir(&second_external)?;
    let path_value = |path: &std::path::Path| {
        toml::Value::String(path.to_string_lossy().into_owned()).to_string()
    };
    let config = project.join(".savestate.toml");
    fs::write(
        &config,
        format!(
            "exclude = [\".savestate.toml\"]\nexternal_paths = [{}]\n",
            path_value(&first_external)
        ),
    )?;

    let mut app = App::open(project.clone())?;
    let first = app.create(None, false)?;
    fs::write(
        &config,
        format!(
            "exclude = [\".savestate.toml\"]\nexternal_paths = [{}, {}]\n",
            path_value(&first_external),
            path_value(&second_external)
        ),
    )?;

    let mut app = App::open(project.clone())?;
    let second = app.create(None, true)?;
    assert_ne!(first, second);
    let store = Store::open(project.join(".savestate"))?;
    assert_eq!(store.ids()?.len(), 2);
    assert_eq!(store.load(&first)?.roots.len(), 2);
    assert_eq!(store.load(&second)?.roots.len(), 3);
    Ok(())
}

#[test]
fn integrations_merge_and_remove_only_savestate_hook() -> Result<()> {
    let project = tempdir()?;
    fs::create_dir(project.path().join(".claude"))?;
    fs::write(
        project.path().join(".claude/settings.json"),
        r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"echo keep savestate-session-start"}]}],"UserPromptSubmit":[{"hooks":[{"type":"command","command":"echo keep prompt"}]}],"Stop":[{"hooks":[{"type":"command","command":"echo keep stop"}]}]}}"#,
    )?;
    integrations::configure(project.path(), Agent::Claude, false)?;
    integrations::configure(project.path(), Agent::Claude, false)?;
    let installed = fs::read_to_string(project.path().join(".claude/settings.json"))?;
    assert_eq!(
        installed
            .matches(
                "savestate --require-project hook claude session-start # savestate-session-start"
            )
            .count(),
        1
    );
    assert_eq!(installed.matches("savestate-user-prompt").count(), 1);
    assert_eq!(installed.matches("savestate-turn-complete").count(), 1);
    assert!(installed.contains("hook claude user-prompt"));
    assert!(installed.contains("hook claude stop"));
    assert!(installed.contains("echo keep savestate-session-start"));
    assert!(installed.contains("echo keep prompt"));
    assert!(installed.contains("echo keep stop"));
    assert!(installed.contains("savestate --require-project"));
    assert!(!installed.contains(&project.path().display().to_string()));
    assert!(!installed.contains(&std::env::current_exe()?.display().to_string()));
    integrations::configure(project.path(), Agent::Claude, true)?;
    let removed = fs::read_to_string(project.path().join(".claude/settings.json"))?;
    assert!(!removed.contains(
        "savestate --require-project hook claude session-start # savestate-session-start"
    ));
    assert!(!removed.contains("savestate-user-prompt"));
    assert!(!removed.contains("savestate-turn-complete"));
    assert!(removed.contains("echo keep savestate-session-start"));
    assert!(removed.contains("echo keep prompt"));
    assert!(removed.contains("echo keep stop"));

    fs::create_dir_all(project.path().join(".codex"))?;
    fs::write(
        project.path().join(".codex/config.toml"),
        "[features]\nhooks = false\n",
    )?;
    integrations::configure(project.path(), Agent::Codex, false)?;
    integrations::configure(project.path(), Agent::Codex, false)?;
    let codex = fs::read_to_string(project.path().join(".codex/hooks.json"))?;
    assert!(codex.contains("startup|resume"));
    assert_eq!(codex.matches("savestate-session-start").count(), 1);
    assert_eq!(codex.matches("savestate-user-prompt").count(), 1);
    assert_eq!(codex.matches("savestate-turn-complete").count(), 1);
    assert!(codex.contains("hook codex user-prompt"));
    assert!(codex.contains("hook codex stop"));
    assert!(codex.contains("hook codex session-start"));
    assert!(codex.contains("savestate --require-project"));
    assert!(!codex.contains(&project.path().display().to_string()));
    assert!(!codex.contains(&std::env::current_exe()?.display().to_string()));
    assert_eq!(
        fs::read_to_string(project.path().join(".codex/config.toml"))?,
        "[features]\nhooks = false\n"
    );

    integrations::configure(project.path(), Agent::Codex, true)?;
    let codex = fs::read_to_string(project.path().join(".codex/hooks.json"))?;
    assert!(!codex.contains("savestate-session-start"));
    assert!(!codex.contains("savestate-user-prompt"));
    assert!(!codex.contains("savestate-turn-complete"));
    Ok(())
}

#[test]
fn legacy_claude_session_only_installation_requires_an_upgrade() -> Result<()> {
    let project = tempdir()?;
    fs::create_dir(project.path().join(".claude"))?;
    fs::write(
        project.path().join(".claude/settings.json"),
        r#"{"hooks":{"SessionStart":[{"matcher":"startup|resume","hooks":[{"type":"command","command":"savestate --require-project hook claude session-start # savestate-session-start"}]}]}}"#,
    )?;

    assert!(!integrations::is_configured(project.path(), Agent::Claude)?);
    integrations::configure(project.path(), Agent::Claude, false)?;
    assert!(integrations::is_configured(project.path(), Agent::Claude)?);
    Ok(())
}

#[test]
fn codex_hooks_label_changed_turns_from_the_user_request() -> Result<()> {
    let project = tempdir()?;
    let mut app = App::open(project.path().to_path_buf())?;
    let prompt = serde_json::json!({
        "session_id": "session-1",
        "turn_id": "turn-1",
        "hook_event_name": "UserPromptSubmit",
        "prompt": "Add dark mode to the settings page"
    });
    assert!(
        integrations::handle_codex_hook(&mut app, CodexHookEvent::UserPrompt, &prompt.to_string())?
            .is_none()
    );
    fs::write(project.path().join("settings.css"), b"dark")?;
    let stop = serde_json::json!({
        "session_id": "session-1",
        "turn_id": "turn-1",
        "hook_event_name": "Stop",
        "last_assistant_message": "Implemented the requested dark theme."
    });
    assert_eq!(
        integrations::handle_codex_hook(&mut app, CodexHookEvent::Stop, &stop.to_string())?,
        Some("{}".into())
    );
    let store = Store::open(project.path().join(".savestate"))?;
    assert_eq!(
        store.load("latest")?.label.as_deref(),
        Some("Codex · Add dark mode to the settings page")
    );

    let prompt = serde_json::json!({
        "session_id": "session-1",
        "turn_id": "turn-2",
        "hook_event_name": "UserPromptSubmit",
        "prompt": "ok"
    });
    integrations::handle_codex_hook(&mut app, CodexHookEvent::UserPrompt, &prompt.to_string())?;
    fs::write(project.path().join("settings.css"), b"dark and polished")?;
    let stop = serde_json::json!({
        "session_id": "session-1",
        "turn_id": "turn-2",
        "hook_event_name": "Stop",
        "last_assistant_message": "Polished the dark theme transitions."
    });
    integrations::handle_codex_hook(&mut app, CodexHookEvent::Stop, &stop.to_string())?;
    assert_eq!(
        store.load("latest")?.label.as_deref(),
        Some("Codex · Polished the dark theme transitions.")
    );

    let prompt = serde_json::json!({
        "session_id": "session-1",
        "turn_id": "turn-3",
        "hook_event_name": "UserPromptSubmit",
        "prompt": "   "
    });
    integrations::handle_codex_hook(&mut app, CodexHookEvent::UserPrompt, &prompt.to_string())?;
    fs::write(
        project.path().join("image-result.txt"),
        b"created from image",
    )?;
    let stop = serde_json::json!({
        "session_id": "session-1",
        "turn_id": "turn-3",
        "hook_event_name": "Stop",
        "last_assistant_message": null
    });
    integrations::handle_codex_hook(&mut app, CodexHookEvent::Stop, &stop.to_string())?;
    assert_eq!(
        store.load("latest")?.label.as_deref(),
        Some("Codex · Turn completed")
    );
    Ok(())
}

#[test]
fn claude_hooks_share_turn_labels_and_replace_stale_prompt_context() -> Result<()> {
    let project = tempdir()?;
    let mut app = App::open(project.path().to_path_buf())?;
    let first_prompt = serde_json::json!({
        "session_id": "session-1",
        "hook_event_name": "UserPromptSubmit",
        "prompt": "This stale prompt should be replaced"
    });
    integrations::handle_claude_hook(
        &mut app,
        integrations::HookEvent::UserPrompt,
        &first_prompt.to_string(),
    )?;
    let replacement_prompt = serde_json::json!({
        "session_id": "session-1",
        "hook_event_name": "UserPromptSubmit",
        "prompt": "Add dark mode to the settings page"
    });
    integrations::handle_claude_hook(
        &mut app,
        integrations::HookEvent::UserPrompt,
        &replacement_prompt.to_string(),
    )?;
    let context_directory = project.path().join(".savestate/hooks/claude");
    assert_eq!(fs::read_dir(&context_directory)?.count(), 1);

    fs::write(project.path().join("settings.css"), b"dark")?;
    let stop = serde_json::json!({
        "session_id": "session-1",
        "hook_event_name": "Stop",
        "last_assistant_message": "Implemented the requested dark theme.",
        "stop_hook_active": false
    });
    assert_eq!(
        integrations::handle_claude_hook(
            &mut app,
            integrations::HookEvent::Stop,
            &stop.to_string(),
        )?,
        Some("{}".into())
    );
    let store = Store::open(project.path().join(".savestate"))?;
    assert_eq!(
        store.load("latest")?.label.as_deref(),
        Some("Claude · Add dark mode to the settings page")
    );
    assert_eq!(fs::read_dir(context_directory)?.count(), 0);

    let prompt = serde_json::json!({
        "session_id": "session-1",
        "hook_event_name": "UserPromptSubmit",
        "prompt": "ok"
    });
    integrations::handle_claude_hook(
        &mut app,
        integrations::HookEvent::UserPrompt,
        &prompt.to_string(),
    )?;
    fs::write(project.path().join("settings.css"), b"dark and polished")?;
    let stop = serde_json::json!({
        "session_id": "session-1",
        "hook_event_name": "Stop",
        "last_assistant_message": "Polished the dark theme transitions."
    });
    integrations::handle_claude_hook(&mut app, integrations::HookEvent::Stop, &stop.to_string())?;
    assert_eq!(
        store.load("latest")?.label.as_deref(),
        Some("Claude · Polished the dark theme transitions.")
    );

    let prompt = serde_json::json!({
        "session_id": "session-1",
        "hook_event_name": "UserPromptSubmit",
        "prompt": "   "
    });
    integrations::handle_claude_hook(
        &mut app,
        integrations::HookEvent::UserPrompt,
        &prompt.to_string(),
    )?;
    fs::write(
        project.path().join("image-result.txt"),
        b"created from image",
    )?;
    let stop = serde_json::json!({
        "session_id": "session-1",
        "hook_event_name": "Stop",
        "last_assistant_message": null
    });
    integrations::handle_claude_hook(&mut app, integrations::HookEvent::Stop, &stop.to_string())?;
    assert_eq!(
        store.load("latest")?.label.as_deref(),
        Some("Claude · Turn completed")
    );
    Ok(())
}

#[test]
fn agent_hooks_validate_provider_ids_and_event_names() -> Result<()> {
    let project = tempdir()?;
    let mut app = App::open(project.path().to_path_buf())?;

    let missing_claude_session = serde_json::json!({
        "session_id": " ",
        "hook_event_name": "UserPromptSubmit",
        "prompt": "change a file"
    });
    let error = integrations::handle_claude_hook(
        &mut app,
        integrations::HookEvent::UserPrompt,
        &missing_claude_session.to_string(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("missing a session ID"));

    let mismatched_claude_event = serde_json::json!({
        "session_id": "session-1",
        "hook_event_name": "Stop",
        "prompt": "change a file"
    });
    let error = integrations::handle_claude_hook(
        &mut app,
        integrations::HookEvent::UserPrompt,
        &mismatched_claude_event.to_string(),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("expected UserPromptSubmit hook input, received Stop")
    );

    let missing_codex_turn = serde_json::json!({
        "session_id": "session-1",
        "turn_id": " ",
        "hook_event_name": "UserPromptSubmit",
        "prompt": "change a file"
    });
    let error = integrations::handle_codex_hook(
        &mut app,
        CodexHookEvent::UserPrompt,
        &missing_codex_turn.to_string(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("missing a session or turn ID"));
    Ok(())
}

#[test]
fn claude_stop_keeps_prompt_context_when_checkpointing_fails() -> Result<()> {
    let project = tempdir()?;
    let mut app = App::open(project.path().to_path_buf())?;
    let prompt = serde_json::json!({
        "session_id": "session-1",
        "hook_event_name": "UserPromptSubmit",
        "prompt": "Keep this context until checkpointing succeeds"
    });
    integrations::handle_claude_hook(
        &mut app,
        integrations::HookEvent::UserPrompt,
        &prompt.to_string(),
    )?;
    let context_directory = project.path().join(".savestate/hooks/claude");
    assert_eq!(fs::read_dir(&context_directory)?.count(), 1);
    fs::write(
        project.path().join(".savestate/journal.json"),
        r#"{"version":1,"target_id":"target","recovery_id":"recovery","phase":"prepared","commit_started":false,"swaps":[],"database_swaps":[],"preserved_paths":[],"preservation_root":""}"#,
    )?;

    let stop = serde_json::json!({
        "session_id": "session-1",
        "hook_event_name": "Stop",
        "last_assistant_message": "done"
    });
    let error = integrations::handle_claude_hook(
        &mut app,
        integrations::HookEvent::Stop,
        &stop.to_string(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("interrupted restore"));
    assert_eq!(fs::read_dir(context_directory)?.count(), 1);
    Ok(())
}

#[test]
fn relabel_uses_atomic_metadata_without_rewriting_the_manifest() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join("file"), b"state")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let checkpoint = app.create(Some("automatic".into()), false)?;
    let manifest_path = project
        .path()
        .join(".savestate/snapshots")
        .join(&checkpoint)
        .join("manifest.json");
    let manifest_before = fs::read(&manifest_path)?;

    app.label(&checkpoint[..12], "Known good settings state".into())?;

    assert_eq!(fs::read(manifest_path)?, manifest_before);
    let store = Store::open(project.path().join(".savestate"))?;
    assert_eq!(
        store.load(&checkpoint)?.label.as_deref(),
        Some("Known good settings state")
    );
    Ok(())
}

#[test]
fn delete_updates_head_and_collects_only_unreferenced_objects() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join("shared"), b"keep")?;
    let mut app = App::open(project.path().to_path_buf())?;
    let first = app.create(Some("first".into()), false)?;
    fs::write(project.path().join("unique"), b"delete with checkpoint")?;
    let second = app.create(Some("second".into()), false)?;
    app.label(&second, "temporary".into())?;

    let store = Store::open(project.path().join(".savestate"))?;
    let second_manifest = store.load(&second)?;
    let unique_hash = second_manifest
        .files
        .iter()
        .find(|entry| entry.path == std::path::Path::new("unique"))
        .and_then(|entry| entry.object_hash.clone())
        .unwrap();
    assert!(store.object_path(&unique_hash).exists());

    app.delete("latest", true)?;

    assert_eq!(store.ids()?, vec![first.clone()]);
    assert_eq!(store.load("latest")?.id, first);
    assert!(!store.object_path(&unique_hash).exists());
    assert!(
        !project
            .path()
            .join(".savestate/labels")
            .join(second)
            .exists()
    );
    Ok(())
}
