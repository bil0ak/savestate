use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{App, normalize_label, store::atomic_write};

#[derive(Clone, Copy, Debug)]
pub enum Agent {
    Claude,
    Codex,
}

const SESSION_MARKER: &str = "savestate-session-start";
const PROMPT_MARKER: &str = "savestate-user-prompt";
const TURN_MARKER: &str = "savestate-turn-complete";

#[derive(Clone, Copy, Debug)]
pub enum HookEvent {
    SessionStart,
    UserPrompt,
    Stop,
}

pub type CodexHookEvent = HookEvent;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HookHealth {
    pub updated_at: DateTime<Utc>,
    pub event: String,
    pub success: bool,
    pub detail: String,
}

#[derive(Debug, Deserialize)]
struct CodexHookInput {
    session_id: String,
    turn_id: String,
    hook_event_name: String,
    prompt: Option<String>,
    last_assistant_message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TurnContext {
    prompt: String,
}

pub fn configure(root: &Path, agent: Agent, remove: bool) -> Result<PathBuf> {
    let settings = configuration_path(root, agent);
    let directory = settings
        .parent()
        .context("integration path has no parent")?;
    ensure_local_directory(root, directory)?;
    let mut value: Value = if regular_file_exists(&settings, "agent settings")? {
        serde_json::from_slice(&fs::read(&settings)?)
            .with_context(|| format!("parse {}", settings.display()))?
    } else {
        json!({})
    };
    let root_object = value
        .as_object_mut()
        .context("agent settings must be a JSON object")?;
    let hooks = root_object.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks
        .as_object_mut()
        .context("hooks must be a JSON object")?;

    let agent_name = match agent {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
    };
    let base_command = format!("savestate --require-project hook {agent_name} session-start");

    {
        let session = hooks.entry("SessionStart").or_insert_with(|| json!([]));
        let session = session
            .as_array_mut()
            .context("SessionStart hooks must be an array")?;
        remove_managed_hooks(session, SESSION_MARKER);
        if !remove {
            session.push(json!({
                "matcher": "startup|resume",
                "hooks": [{
                    "type": "command",
                    "command": format!(
                        "{base_command} # {SESSION_MARKER}"
                    )
                }]
            }));
        }
    }

    if matches!(agent, Agent::Codex) {
        {
            let prompt = hooks.entry("UserPromptSubmit").or_insert_with(|| json!([]));
            let prompt = prompt
                .as_array_mut()
                .context("UserPromptSubmit hooks must be an array")?;
            remove_managed_hooks(prompt, PROMPT_MARKER);
            if !remove {
                prompt.push(json!({
                    "hooks": [{
                        "type": "command",
                        "command": format!(
                            "savestate --require-project hook codex user-prompt # {PROMPT_MARKER}"
                        )
                    }]
                }));
            }
        }

        let stop = hooks.entry("Stop").or_insert_with(|| json!([]));
        let stop = stop.as_array_mut().context("Stop hooks must be an array")?;
        remove_managed_hooks(stop, TURN_MARKER);
        if !remove {
            stop.push(json!({
                "hooks": [{
                    "type": "command",
                    "command": format!(
                        "savestate --require-project hook codex stop # {TURN_MARKER}"
                    )
                }]
            }));
        }
    }
    atomic_write(&settings, &serde_json::to_vec_pretty(&value)?)?;
    Ok(settings)
}

fn remove_managed_hooks(groups: &mut Vec<Value>, marker: &str) {
    let suffix = format!("# {marker}");
    for group in groups.iter_mut() {
        let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            continue;
        };
        hooks.retain(|hook| {
            !hook
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| {
                    hook.get("type").and_then(Value::as_str) == Some("command")
                        && command.starts_with("savestate --require-project hook ")
                        && command.ends_with(&suffix)
                })
        });
    }
    groups.retain(|group| {
        group
            .get("hooks")
            .and_then(Value::as_array)
            .is_none_or(|hooks| !hooks.is_empty())
    });
}

pub fn configuration_path(root: &Path, agent: Agent) -> PathBuf {
    match agent {
        Agent::Claude => root.join(".claude/settings.json"),
        Agent::Codex => root.join(".codex/hooks.json"),
    }
}

pub fn is_configured(root: &Path, agent: Agent) -> Result<bool> {
    let path = configuration_path(root, agent);
    if !regular_file_exists(&path, "agent settings")? {
        return Ok(false);
    }
    let value: Value = serde_json::from_slice(&fs::read(&path)?)
        .with_context(|| format!("parse {}", path.display()))?;
    let agent_name = match agent {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
    };
    let session =
        format!("savestate --require-project hook {agent_name} session-start # {SESSION_MARKER}");
    let configured = has_managed_command(&value, "SessionStart", &session)
        && match agent {
            Agent::Claude => true,
            Agent::Codex => {
                has_managed_command(
                    &value,
                    "UserPromptSubmit",
                    &format!(
                        "savestate --require-project hook codex user-prompt # {PROMPT_MARKER}"
                    ),
                ) && has_managed_command(
                    &value,
                    "Stop",
                    &format!("savestate --require-project hook codex stop # {TURN_MARKER}"),
                )
            }
        };
    Ok(configured)
}

fn has_managed_command(value: &Value, event: &str, expected: &str) -> bool {
    value
        .get("hooks")
        .and_then(|hooks| hooks.get(event))
        .and_then(Value::as_array)
        .is_some_and(|groups| {
            groups.iter().any(|group| {
                group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|hooks| {
                        hooks.iter().any(|hook| {
                            hook.get("type").and_then(Value::as_str) == Some("command")
                                && hook.get("command").and_then(Value::as_str) == Some(expected)
                        })
                    })
            })
        })
}

pub fn run_hook(app: &mut App, agent: Agent, event: HookEvent) -> Result<Option<String>> {
    let mut input = String::new();
    if !matches!(event, HookEvent::SessionStart) {
        io::stdin().lock().read_to_string(&mut input)?;
    }
    let result = match (agent, event) {
        (_, HookEvent::SessionStart) => app
            .create_quiet(
                Some(format!("agent:{}:session-start", agent_name(agent))),
                true,
            )
            .map(|_| None),
        (Agent::Codex, event) => handle_codex_hook(app, event, &input),
        (Agent::Claude, _) => bail!("Claude only supports the session-start Savestate hook"),
    };
    match &result {
        Ok(_) => record_hook_health(app, event, true, "checkpoint hook completed")?,
        Err(error) => {
            // Health is diagnostic only and must never replace the original error.
            let _ = record_hook_health(app, event, false, &format!("{error:#}"));
        }
    }
    result
}

pub fn run_codex_hook(app: &mut App, event: HookEvent) -> Result<Option<String>> {
    run_hook(app, Agent::Codex, event)
}

pub fn handle_codex_hook(app: &mut App, event: HookEvent, input: &str) -> Result<Option<String>> {
    let input: CodexHookInput = serde_json::from_str(input).context("parse Codex hook input")?;
    if input.session_id.trim().is_empty() || input.turn_id.trim().is_empty() {
        bail!("Codex hook input is missing a session or turn ID");
    }
    let expected_event = match event {
        HookEvent::UserPrompt => "UserPromptSubmit",
        HookEvent::Stop => "Stop",
        HookEvent::SessionStart => bail!("session-start does not accept Codex JSON input"),
    };
    if input.hook_event_name != expected_event {
        bail!(
            "expected {expected_event} hook input, received {}",
            input.hook_event_name
        );
    }

    let context_path = turn_context_path(app, &input.session_id, &input.turn_id);
    match event {
        HookEvent::UserPrompt => {
            let prompt = input
                .prompt
                .context("UserPromptSubmit input is missing prompt")?;
            let prompt = normalize_label(&prompt, 240, true).unwrap_or_default();
            ensure_absent_or_regular(&context_path, "Codex turn context")?;
            atomic_write(&context_path, &serde_json::to_vec(&TurnContext { prompt })?)?;
            Ok(None)
        }
        HookEvent::Stop => {
            let prompt = if regular_file_exists(&context_path, "Codex turn context")? {
                serde_json::from_slice::<TurnContext>(&fs::read(&context_path)?)
                    .ok()
                    .map(|context| context.prompt)
            } else {
                None
            };
            let label =
                automatic_turn_label(prompt.as_deref(), input.last_assistant_message.as_deref());
            app.create_quiet(Some(label), true)?;
            if regular_file_exists(&context_path, "Codex turn context")? {
                fs::remove_file(context_path)?;
            }
            Ok(Some("{}".into()))
        }
        HookEvent::SessionStart => bail!("session-start is handled before Codex input parsing"),
    }
}

pub fn hook_health(app: &App) -> Result<Option<HookHealth>> {
    let path = app.store.path().join("hooks/health.json");
    if !regular_file_exists(&path, "hook health record")? {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&fs::read(path)?)?))
}

fn record_hook_health(app: &App, event: HookEvent, success: bool, detail: &str) -> Result<()> {
    let health = HookHealth {
        updated_at: Utc::now(),
        event: match event {
            HookEvent::SessionStart => "session_start",
            HookEvent::UserPrompt => "user_prompt",
            HookEvent::Stop => "stop",
        }
        .into(),
        success,
        detail: detail.chars().take(500).collect(),
    };
    let path = app.store.path().join("hooks/health.json");
    ensure_absent_or_regular(&path, "hook health record")?;
    atomic_write(&path, &serde_json::to_vec_pretty(&health)?)
}

fn ensure_local_directory(root: &Path, directory: &Path) -> Result<()> {
    let relative = directory
        .strip_prefix(root)
        .context("integration path is outside the project")?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    bail!(
                        "integration path is not a real directory: {}",
                        current.display()
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn regular_file_exists(path: &Path, description: &str) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => bail!("{description} is not a regular file: {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn ensure_absent_or_regular(path: &Path, description: &str) -> Result<()> {
    let _ = regular_file_exists(path, description)?;
    Ok(())
}

fn agent_name(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
    }
}

fn turn_context_path(app: &App, session_id: &str, turn_id: &str) -> PathBuf {
    let mut hasher = blake3::Hasher::new();
    hasher.update(session_id.as_bytes());
    hasher.update(&[0]);
    hasher.update(turn_id.as_bytes());
    app.store
        .path()
        .join("hooks/codex")
        .join(format!("{}.json", hasher.finalize().to_hex()))
}

fn automatic_turn_label(prompt: Option<&str>, assistant: Option<&str>) -> String {
    let prompt = prompt
        .and_then(|value| normalize_label(value, 72, true).ok())
        .filter(|value| !is_acknowledgement(value));
    let candidate = if let Some(prompt) = prompt {
        prompt
    } else if let Some(assistant) =
        assistant.and_then(|value| normalize_label(value, 72, true).ok())
    {
        assistant
    } else {
        "Turn completed".into()
    };
    format!("Codex · {candidate}")
}

fn is_acknowledgement(value: &str) -> bool {
    let value = value
        .trim_matches(|character: char| !character.is_alphanumeric())
        .to_ascii_lowercase();
    matches!(
        value.as_str(),
        "ok" | "okay"
            | "yes"
            | "yep"
            | "sure"
            | "continue"
            | "go ahead"
            | "proceed"
            | "do it"
            | "sounds good"
    )
}
