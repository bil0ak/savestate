use std::fs;

use anyhow::Result;
use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn run_returns_the_child_process_exit_code() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join(".savestate.toml"), "")?;
    fs::write(project.path().join("value"), "before")?;

    #[cfg(unix)]
    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args(["run", "--", "sh", "-c", "exit 7"])
        .assert()
        .code(7);

    #[cfg(windows)]
    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args(["run", "--", "cmd", "/C", "exit 7"])
        .assert()
        .code(7);

    Ok(())
}

#[test]
fn no_command_onboards_without_modifying_an_uninitialized_directory() -> Result<()> {
    let project = tempdir()?;

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Savestate protects your project"))
        .stdout(predicate::str::contains("savestate init"))
        .stdout(predicate::str::contains("savestate integrate codex"));

    assert!(!project.path().join(".savestate.toml").exists());
    assert!(!project.path().join(".savestate").exists());
    Ok(())
}

#[test]
fn human_output_uses_color_when_the_terminal_requests_it() -> Result<()> {
    let project = tempdir()?;

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .env_remove("NO_COLOR")
        .env("CLICOLOR_FORCE", "1")
        .assert()
        .success()
        .stdout(predicate::str::contains("\u{1b}["));
    Ok(())
}

#[test]
fn no_color_disables_styling_even_when_color_is_forced() -> Result<()> {
    let project = tempdir()?;

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .env("CLICOLOR_FORCE", "1")
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(predicate::str::contains("\u{1b}[").not());
    Ok(())
}

#[test]
fn json_output_never_contains_terminal_styling() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join(".savestate.toml"), "")?;

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .env("CLICOLOR_FORCE", "1")
        .args(["list", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\u{1b}[").not());
    Ok(())
}

#[test]
fn no_command_shows_status_and_commands_for_an_initialized_project() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join(".savestate.toml"), "")?;

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Project:"))
        .stdout(predicate::str::contains("Latest checkpoint: none"))
        .stdout(predicate::str::contains("Useful commands:"))
        .stdout(predicate::str::contains("savestate doctor"));
    Ok(())
}

#[test]
fn init_explains_the_next_agent_integration_step() -> Result<()> {
    let project = tempdir()?;

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Savestate initialized"))
        .stdout(predicate::str::contains("✓ Snapshot scope configured"))
        .stdout(predicate::str::contains("Next:"))
        .stdout(predicate::str::contains("savestate integrate codex"));

    assert!(project.path().join(".savestate.toml").is_file());
    Ok(())
}

#[test]
fn guided_setup_never_waits_for_input_in_noninteractive_execution() -> Result<()> {
    for arguments in [
        vec!["create", "--filesystem-only"],
        vec!["integrate", "codex"],
    ] {
        let project = tempdir()?;
        Command::cargo_bin("savestate")?
            .current_dir(project.path())
            .args(arguments)
            .assert()
            .failure()
            .stderr(predicate::str::contains("run `savestate init` first"))
            .stderr(predicate::str::contains("interactive terminal"));
        assert!(!project.path().join(".savestate.toml").exists());
        assert!(!project.path().join(".savestate").exists());
        assert!(!project.path().join(".codex").exists());
    }
    Ok(())
}

#[test]
fn integration_completion_explains_codex_trust_and_new_task_requirements() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join(".savestate.toml"), "")?;

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args(["integrate", "codex"])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Codex integration installed"))
        .stdout(predicate::str::contains("trust the Savestate hooks"))
        .stdout(predicate::str::contains("Start a new task"))
        .stdout(predicate::str::contains("savestate doctor"));
    Ok(())
}

#[test]
fn integration_completion_explains_claude_turn_checkpointing() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join(".savestate.toml"), "")?;

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args(["integrate", "claude"])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Claude integration installed"))
        .stdout(predicate::str::contains("Complete a changed turn"))
        .stdout(predicate::str::contains("after the first completed turn"));
    Ok(())
}

#[test]
fn help_leads_with_the_real_quick_start() -> Result<()> {
    Command::cargo_bin("savestate")?
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Quick start:"))
        .stdout(predicate::str::contains("savestate init"))
        .stdout(predicate::str::contains("savestate integrate codex"));
    Ok(())
}

#[test]
fn doctor_passes_for_a_writable_initialized_manual_project() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join(".savestate.toml"), "")?;

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("Project configuration: ok"))
        .stdout(predicate::str::contains("Filesystem store: writable"))
        .stdout(predicate::str::contains(
            "Doctor: all required checks passed",
        ));
    Ok(())
}

#[test]
fn doctor_recognizes_installed_trusted_codex_hooks_and_path() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join(".savestate.toml"), "")?;
    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args(["integrate", "codex"])
        .assert()
        .success();

    let hooks = project.path().join(".codex/hooks.json").canonicalize()?;
    let codex_home = project.path().join("codex-home");
    fs::create_dir(&codex_home)?;
    fs::write(
        codex_home.join("config.toml"),
        format!(
            "[hooks.state.\"{}:session_start:0:0\"]\ntrusted_hash = \"test\"\n\n[hooks.state.\"{}:user_prompt_submit:0:0\"]\ntrusted_hash = \"test\"\n\n[hooks.state.\"{}:stop:0:0\"]\ntrusted_hash = \"test\"\n",
            hooks.display(),
            hooks.display(),
            hooks.display()
        ),
    )?;
    let current_exe = std::env::current_exe()?;
    let debug_dir = current_exe
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap();
    let path = std::env::join_paths(std::iter::once(debug_dir.to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))?;

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .env("CODEX_HOME", &codex_home)
        .env("PATH", path)
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("Codex integration: installed"))
        .stdout(predicate::str::contains(
            "Agent command: `savestate` is available on PATH",
        ))
        .stdout(predicate::str::contains(
            "Codex hook trust: recorded for all Savestate hooks",
        ))
        .stdout(predicate::str::contains(
            "Automatic checkpoints: none detected yet",
        ));
    Ok(())
}

#[test]
fn create_warns_but_continues_and_status_reports_scope() -> Result<()> {
    let project = tempdir()?;
    fs::write(
        project.path().join(".savestate.toml"),
        "[limits]\nwarn_files = 1\nwarn_size = \"2GiB\"\n",
    )?;
    fs::write(project.path().join("one"), "1")?;
    fs::write(project.path().join("two"), "2")?;

    Command::cargo_bin("savestate")?
        .args(["--root", project.path().to_str().unwrap(), "create"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Warning: checkpoint scope contains",
        ));

    Command::cargo_bin("savestate")?
        .args(["--root", project.path().to_str().unwrap(), "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Snapshot policy: Git ignore enabled",
        ))
        .stdout(predicate::str::contains("Selected scope:"));
    Ok(())
}

#[test]
fn non_git_project_reports_that_dot_gitignore_rules_are_inactive() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join(".savestate.toml"), "")?;
    fs::write(project.path().join(".gitignore"), "hidden.md\n")?;
    fs::write(project.path().join("hidden.md"), "captured")?;

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args(["create", "--filesystem-only"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            ".gitignore found, but its rules are ignored because this is not a Git repository",
        ));

    let id = fs::read_to_string(project.path().join(".savestate/HEAD"))?;
    let manifest = fs::read_to_string(
        project
            .path()
            .join(".savestate/snapshots")
            .join(id.trim())
            .join("manifest.json"),
    )?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest)?;
    assert!(
        manifest["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == "hidden.md")
    );

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Snapshot policy: Git ignore inactive (.gitignore ignored: not a Git repository)",
        ));
    Ok(())
}

#[test]
fn init_falls_back_from_a_malformed_git_repository() -> Result<()> {
    let project = tempdir()?;
    fs::create_dir(project.path().join(".git"))?;
    fs::write(project.path().join(".gitignore"), ".env\n")?;
    fs::write(project.path().join(".env"), "TOKEN=secret\n")?;

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .arg("init")
        .assert()
        .success()
        .stderr(predicate::str::contains("Git integration is unavailable"));

    let source = fs::read_to_string(project.path().join(".savestate.toml"))?;
    let value: toml::Value = toml::from_str(&source)?;
    assert!(value["include"].as_array().unwrap().is_empty());

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args(["create", "--filesystem-only"])
        .assert()
        .success()
        .stderr(predicate::str::contains("Git-aware capture is unavailable"));

    let id = fs::read_to_string(project.path().join(".savestate/HEAD"))?;
    let manifest = fs::read_to_string(
        project
            .path()
            .join(".savestate/snapshots")
            .join(id.trim())
            .join("manifest.json"),
    )?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest)?;
    assert!(
        manifest["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == ".env")
    );
    Ok(())
}

#[test]
fn capture_falls_back_when_git_discovery_fails_after_repository_detection() -> Result<()> {
    let project = tempdir()?;
    std::process::Command::new("git")
        .current_dir(project.path())
        .args(["init", "-q"])
        .status()?;
    fs::create_dir(project.path().join(".git/index"))?;
    fs::write(project.path().join(".savestate.toml"), "")?;
    fs::write(project.path().join(".gitignore"), "ignored.txt\n")?;
    fs::write(project.path().join("ignored.txt"), "captured")?;

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args(["create", "--filesystem-only"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "capturing the full filesystem scope",
        ));

    let id = fs::read_to_string(project.path().join(".savestate/HEAD"))?;
    let manifest = fs::read_to_string(
        project
            .path()
            .join(".savestate/snapshots")
            .join(id.trim())
            .join("manifest.json"),
    )?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest)?;
    assert!(
        manifest["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == "ignored.txt")
    );
    Ok(())
}

#[test]
fn initialized_repository_works_when_git_is_not_on_path() -> Result<()> {
    let project = tempdir()?;
    std::process::Command::new("git")
        .current_dir(project.path())
        .args(["init", "-q"])
        .status()?;
    fs::write(project.path().join(".savestate.toml"), "")?;
    fs::write(project.path().join(".gitignore"), "ignored.txt\n")?;
    fs::write(project.path().join("ignored.txt"), "captured")?;

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .env("PATH", "")
        .args(["create", "--filesystem-only"])
        .assert()
        .success()
        .stderr(predicate::str::contains("Git integration is unavailable"));

    let id = fs::read_to_string(project.path().join(".savestate/HEAD"))?;
    let manifest = fs::read_to_string(
        project
            .path()
            .join(".savestate/snapshots")
            .join(id.trim())
            .join("manifest.json"),
    )?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest)?;
    assert!(
        manifest["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == "ignored.txt")
    );
    Ok(())
}

#[test]
fn hook_warning_stays_on_stderr_and_stdout_remains_json() -> Result<()> {
    let project = tempdir()?;
    fs::write(
        project.path().join(".savestate.toml"),
        "[limits]\nwarn_files = 0\nwarn_size = \"2GiB\"\n",
    )?;
    Command::cargo_bin("savestate")?
        .args([
            "--root",
            project.path().to_str().unwrap(),
            "hook",
            "codex",
            "user-prompt",
        ])
        .write_stdin(
            r#"{"session_id":"s","turn_id":"t","hook_event_name":"UserPromptSubmit","prompt":"change a file"}"#,
        )
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
    fs::write(project.path().join("changed"), "yes")?;
    Command::cargo_bin("savestate")?
        .args([
            "--root",
            project.path().to_str().unwrap(),
            "hook",
            "codex",
            "stop",
        ])
        .write_stdin(
            r#"{"session_id":"s","turn_id":"t","hook_event_name":"Stop","last_assistant_message":"done"}"#,
        )
        .assert()
        .success()
        .stdout("{}\n")
        .stderr(predicate::str::contains("Warning: checkpoint scope contains"));
    Ok(())
}

#[test]
fn stop_hook_failures_are_non_blocking_and_always_return_json() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join(".savestate.toml"), "")?;
    Command::cargo_bin("savestate")?
        .args([
            "--root",
            project.path().to_str().unwrap(),
            "hook",
            "codex",
            "stop",
        ])
        .write_stdin("not-json")
        .assert()
        .success()
        .stdout("{}\n")
        .stderr(predicate::str::contains("parse Codex hook input"));
    Ok(())
}

#[test]
fn claude_hooks_keep_protocol_output_machine_readable() -> Result<()> {
    let project = tempdir()?;
    fs::write(
        project.path().join(".savestate.toml"),
        "[limits]\nwarn_files = 0\nwarn_size = \"2GiB\"\n",
    )?;
    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args(["hook", "claude", "user-prompt"])
        .write_stdin(
            r#"{"session_id":"s","hook_event_name":"UserPromptSubmit","prompt":"change a file"}"#,
        )
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
    fs::write(project.path().join("changed"), "yes")?;
    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args(["hook", "claude", "stop"])
        .write_stdin(
            r#"{"session_id":"s","hook_event_name":"Stop","last_assistant_message":"done","stop_hook_active":false}"#,
        )
        .assert()
        .success()
        .stdout("{}\n")
        .stderr(predicate::str::contains("Warning: checkpoint scope contains"));

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args(["hook", "claude", "stop"])
        .write_stdin("not-json")
        .assert()
        .success()
        .stdout("{}\n")
        .stderr(predicate::str::contains("parse Claude hook input"));
    Ok(())
}

#[test]
fn malformed_hook_commands_are_non_blocking() -> Result<()> {
    Command::cargo_bin("savestate")?
        .args(["hook", "codex"])
        .assert()
        .success()
        .stderr(predicate::str::contains("required"));

    Command::cargo_bin("savestate")?
        .args(["hook", "codex", "stop", "--invalid-hook-option"])
        .assert()
        .success()
        .stdout("{}\n")
        .stderr(predicate::str::contains("unexpected argument"));
    Ok(())
}

#[test]
fn commands_discover_the_nearest_initialized_project_without_git() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join(".savestate.toml"), "")?;
    fs::write(project.path().join("tracked-without-git"), "content")?;
    let nested = project.path().join("one/two/three");
    fs::create_dir_all(&nested)?;

    Command::cargo_bin("savestate")?
        .current_dir(&nested)
        .args(["create", "--filesystem-only"])
        .assert()
        .success();

    assert!(project.path().join(".savestate/HEAD").is_file());
    assert!(!nested.join(".savestate").exists());
    Ok(())
}

#[test]
fn custom_project_store_is_excluded_and_refused_if_tracked() -> Result<()> {
    let project = tempdir()?;
    std::process::Command::new("git")
        .current_dir(project.path())
        .args(["init", "-q"])
        .status()?;
    fs::write(
        project.path().join(".savestate.toml"),
        "store = \".local/checkpoints\"\n",
    )?;
    fs::create_dir_all(project.path().join(".local/checkpoints"))?;
    fs::write(project.path().join(".local/checkpoints/tracked"), "unsafe")?;
    std::process::Command::new("git")
        .current_dir(project.path())
        .args(["add", "-f", ".local/checkpoints/tracked"])
        .status()?;

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args(["create"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is tracked by Git"));
    Ok(())
}

#[test]
fn noninteractive_restore_without_an_id_keeps_latest_behavior() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join(".savestate.toml"), "")?;
    fs::write(project.path().join("value"), "checkpoint")?;
    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args([
            "create",
            "--filesystem-only",
            "--label",
            "automation baseline",
        ])
        .assert()
        .success();
    let checkpoint = fs::read_to_string(project.path().join(".savestate/HEAD"))?;
    fs::write(project.path().join("value"), "current")?;

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args(["restore", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(checkpoint.trim()))
        .stdout(predicate::str::contains("Dry run: no changes made."));
    assert_eq!(fs::read_to_string(project.path().join("value"))?, "current");

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .arg("restore")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "restore confirmation requires a terminal",
        ));
    assert_eq!(fs::read_to_string(project.path().join("value"))?, "current");

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args(["restore", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Restored"));
    assert_eq!(
        fs::read_to_string(project.path().join("value"))?,
        "checkpoint"
    );
    Ok(())
}

#[test]
fn restore_preview_samples_four_paths_per_change_type() -> Result<()> {
    let project = tempdir()?;
    fs::write(project.path().join(".savestate.toml"), "")?;
    for index in 0..6 {
        fs::write(
            project.path().join(format!("removed-{index}")),
            "checkpoint",
        )?;
    }
    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args(["create", "--filesystem-only"])
        .assert()
        .success();
    for index in 0..6 {
        fs::remove_file(project.path().join(format!("removed-{index}")))?;
    }

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args(["restore", "latest", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("+ repo:removed-0"))
        .stdout(predicate::str::contains("+ repo:removed-3"))
        .stdout(predicate::str::contains("repo:removed-4").not())
        .stdout(predicate::str::contains("2 more; inspect every path with"))
        .stdout(predicate::str::contains("savestate diff"));
    Ok(())
}

#[test]
fn hook_mode_refuses_an_uninitialized_directory() -> Result<()> {
    let project = tempdir()?;

    Command::cargo_bin("savestate")?
        .current_dir(project.path())
        .args(["--require-project", "create", "--filesystem-only"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no .savestate.toml found"))
        .stderr(predicate::str::contains("savestate init"));

    assert!(!project.path().join(".savestate").exists());
    Ok(())
}

#[test]
fn explicit_root_takes_precedence_over_nearest_project_discovery() -> Result<()> {
    let nearest = tempdir()?;
    fs::write(nearest.path().join(".savestate.toml"), "")?;
    let nested = nearest.path().join("nested");
    fs::create_dir(&nested)?;

    let explicit = tempdir()?;
    fs::write(explicit.path().join(".savestate.toml"), "")?;
    fs::write(explicit.path().join("value"), "explicit")?;

    Command::cargo_bin("savestate")?
        .current_dir(&nested)
        .args([
            "--root",
            explicit.path().to_str().unwrap(),
            "create",
            "--filesystem-only",
        ])
        .assert()
        .success();

    assert!(explicit.path().join(".savestate/HEAD").is_file());
    assert!(!nearest.path().join(".savestate").exists());
    Ok(())
}
