use std::{
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};

pub(crate) mod output;
pub(crate) mod prompts;
pub(crate) mod restore_prompt;

const QUICK_START: &str = "Quick start:\n  cd your-project\n  savestate init\n  savestate integrate codex\n\nRun `savestate` without a command for project-aware guidance.";

#[derive(Parser, Debug)]
#[command(
    name = "savestate",
    version,
    about = "Checkpoint and restore local coding-agent state",
    long_about = "Savestate creates verified local checkpoints of project files and configured state before coding-agent changes.",
    after_help = QUICK_START
)]
struct Cli {
    #[arg(
        long,
        global = true,
        help = "Use this project root instead of discovering .savestate.toml"
    )]
    root: Option<PathBuf>,
    #[arg(long, global = true, hide = true)]
    require_project: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Initialize Savestate in the current project
    Init,
    /// Create a verified checkpoint
    Create {
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        if_changed: bool,
        #[arg(long, help = "Capture Git-ignored paths for this checkpoint")]
        include_ignored: bool,
        #[arg(long, help = "Do not snapshot configured experimental databases")]
        filesystem_only: bool,
    },
    /// Change a checkpoint's label
    Label { id: String, text: String },
    /// Permanently remove a checkpoint
    Delete {
        id: String,
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Protect a checkpoint from automatic retention
    Pin { id: String },
    /// Allow a checkpoint to be automatically retained again
    Unpin { id: String },
    /// Choose and restore a checkpoint, or restore a specified ID directly
    Restore {
        id: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// List available checkpoints
    List {
        #[arg(long)]
        json: bool,
    },
    /// Compare a checkpoint with current state or another checkpoint
    Diff {
        id: Option<String>,
        #[arg(long, default_value = "current")]
        to: String,
        #[arg(long)]
        json: bool,
    },
    /// Verify checkpoint checksums and configured services
    Verify { id: Option<String> },
    /// Create a checkpoint, then run a command
    Run {
        #[arg(required = true, trailing_var_arg = true)]
        command: Vec<String>,
    },
    /// Install or remove coding-agent hooks
    Integrate {
        agent: Agent,
        #[arg(long)]
        remove: bool,
    },
    #[command(hide = true)]
    Hook { agent: HookAgent, event: HookEvent },
    /// Show snapshot scope and store information
    Status,
    /// Diagnose project and agent integration setup
    Doctor,
    /// Apply the configured checkpoint retention policy
    Prune {
        #[arg(long)]
        dry_run: bool,
    },
    /// Resolve an interrupted restore transaction
    Recover {
        #[arg(long, conflicts_with = "resume")]
        rollback: bool,
        #[arg(long, conflicts_with = "rollback")]
        resume: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Agent {
    Claude,
    Codex,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum HookAgent {
    Claude,
    Codex,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum HookEvent {
    SessionStart,
    UserPrompt,
    Stop,
}

/// Runs the command-line interface and returns the process exit code.
#[must_use]
pub fn run() -> i32 {
    let hook_event = hook_invocation_event();
    // Agents parse hook stdout as JSON, so hook mode must fail soft: a panic degrades to the
    // same path as an error (report to stderr, `{}` for stop hooks, exit 0). The panic hook
    // reports the panic to stderr once, replacing the default message.
    let result = match hook_event {
        Some(_) => {
            std::panic::set_hook(Box::new(|panic| {
                crate::ui::error(format_args!("{panic}"));
            }));
            std::panic::catch_unwind(dispatch)
                .unwrap_or_else(|_| Err(anyhow::anyhow!("hook aborted by panic")))
        }
        None => dispatch(),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            crate::ui::error(format_args!("{error:#}"));
            if let Some(event) = hook_event {
                if event == "stop" {
                    println!("{{}}");
                }
                return 0;
            }
            1
        }
    }
}

fn hook_invocation_event() -> Option<&'static str> {
    let arguments = std::env::args().collect::<Vec<_>>();
    hook_invocation_event_from(&arguments)
}

fn hook_invocation_event_from(arguments: &[String]) -> Option<&'static str> {
    let mut index = 1;
    while let Some(argument) = arguments.get(index).map(String::as_str) {
        match argument {
            "--require-project" => index += 1,
            "--root" => index += 2,
            value if value.starts_with("--root=") => index += 1,
            "hook" => {
                if !matches!(
                    arguments.get(index + 1).map(String::as_str),
                    Some("claude" | "codex")
                ) {
                    return None;
                }
                return Some(match arguments.get(index + 2).map(String::as_str) {
                    Some("session-start") => "session-start",
                    Some("user-prompt") => "user-prompt",
                    Some("stop") => "stop",
                    _ => "unknown",
                });
            }
            _ => return None,
        }
    }
    None
}

fn dispatch() -> Result<i32> {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) if hook_invocation_event().is_some() => return Err(error.into()),
        Err(error) => {
            let exit_code = error.exit_code();
            error.print()?;
            return Ok(exit_code);
        }
    };
    let Some(command) = cli.command else {
        show_project_home(cli.root)?;
        return Ok(0);
    };
    let root = resolve_root(cli.root, cli.require_project, &command)?;
    let initialized = root.join(".savestate.toml").is_file();
    let guided_init = !initialized
        && matches!(
            &command,
            Command::Create { .. } | Command::Integrate { remove: false, .. }
        );
    if guided_init {
        confirm_initialize(&root)?;
    } else if !initialized && matches!(&command, Command::Doctor) {
        bail!(
            "this project is not initialized; run `savestate init` in {}",
            root.display()
        );
    }

    let mut app = crate::App::open(root.clone())?;
    if guided_init {
        app.init_with_guidance(false)?;
        app = crate::App::open(root)?;
    }
    let exit_code = match command {
        Command::Init => app.init().map(|()| 0),
        Command::Create {
            label,
            if_changed,
            include_ignored,
            filesystem_only,
        } => {
            if filesystem_only {
                app.create_filesystem_only(label, if_changed, include_ignored)
                    .map(|_| 0)
            } else {
                app.create_with_options(label, if_changed, include_ignored)
                    .map(|_| 0)
            }
        }
        Command::Label { id, text } => app.label(&id, text).map(|()| 0),
        Command::Delete { id, yes } => app.delete(&id, yes).map(|()| 0),
        Command::Pin { id } => app.pin(&id).map(|()| 0),
        Command::Unpin { id } => app.unpin(&id).map(|()| 0),
        Command::Restore { id, dry_run, yes } => {
            app.restore(id.as_deref(), dry_run, yes).map(|()| 0)
        }
        Command::List { json } => app.list(json).map(|()| 0),
        Command::Diff { id, to, json } => app.diff(id.as_deref(), &to, json).map(|()| 0),
        Command::Verify { id } => app.verify(id.as_deref()).map(|()| 0),
        Command::Run { command } => app.run(command),
        Command::Integrate { agent, remove } => {
            let agent = match agent {
                Agent::Claude => crate::integrations::Agent::Claude,
                Agent::Codex => crate::integrations::Agent::Codex,
            };
            app.integrate(agent, remove).map(|()| 0)
        }
        Command::Hook { agent, event } => {
            let agent = match agent {
                HookAgent::Claude => crate::integrations::Agent::Claude,
                HookAgent::Codex => crate::integrations::Agent::Codex,
            };
            let event = match event {
                HookEvent::SessionStart => crate::integrations::HookEvent::SessionStart,
                HookEvent::UserPrompt => crate::integrations::HookEvent::UserPrompt,
                HookEvent::Stop => crate::integrations::HookEvent::Stop,
            };
            let output = crate::integrations::run_hook(&mut app, agent, event)?;
            if let Some(output) = output {
                println!("{output}");
            }
            Ok(0)
        }
        Command::Status => app.status().map(|()| 0),
        Command::Doctor => app.doctor().map(|()| 0),
        Command::Prune { dry_run } => app.prune(dry_run).map(|()| 0),
        Command::Recover { rollback, resume } => app.recover(rollback, resume).map(|()| 0),
    }?;
    Ok(exit_code)
}

fn show_project_home(explicit_root: Option<PathBuf>) -> Result<()> {
    let current = match explicit_root {
        Some(root) => root,
        None => std::env::current_dir()?,
    };
    if let Some(root) = discover_project_root(&current) {
        let app = crate::App::open(root)?;
        app.status()?;
        println!();
        crate::ui::heading("Useful commands");
        for command in [
            "savestate create --label \"known good\"",
            "savestate list",
            "savestate restore latest --dry-run",
            "savestate doctor",
        ] {
            crate::ui::line(format_args!("  {}", crate::ui::command(command)));
        }
        return Ok(());
    }

    crate::ui::line(format_args!(
        "{} protects your project before coding-agent changes.",
        crate::ui::checkpoint("Savestate")
    ));
    println!();
    crate::ui::line(format_args!(
        "{}",
        crate::ui::muted("This project is not initialized.")
    ));
    println!();
    crate::ui::heading("Get started");
    crate::ui::line(format_args!("  {}", crate::ui::command("savestate init")));
    println!();
    crate::ui::heading("Then connect your agent");
    crate::ui::line(format_args!(
        "  {}",
        crate::ui::command("savestate integrate codex")
    ));
    crate::ui::line(format_args!(
        "  {}",
        crate::ui::command("savestate integrate claude")
    ));
    println!();
    crate::ui::heading("Learn more");
    crate::ui::line(format_args!("  {}", crate::ui::command("savestate help")));
    Ok(())
}

fn confirm_initialize(root: &Path) -> Result<()> {
    if !io::stdin().is_terminal() {
        bail!(
            "this project is not initialized; run `savestate init` first (guided initialization requires an interactive terminal)"
        );
    }
    crate::ui::notice(
        "Project not initialized",
        format_args!("{}", root.display()),
    );
    anstream::print!("Initialize it now? {} ", crate::ui::command("[Y/n]"));
    anstream::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    ) {
        return Ok(());
    }
    bail!("initialization cancelled")
}

fn resolve_root(
    explicit_root: Option<PathBuf>,
    require_project: bool,
    command: &Command,
) -> Result<PathBuf> {
    if let Some(root) = explicit_root {
        if require_project && !matches!(command, Command::Init) {
            require_project_marker(&root)?;
        }
        return Ok(root);
    }

    let current = std::env::current_dir()?;
    if matches!(command, Command::Init) {
        return Ok(current);
    }

    if let Some(root) = discover_project_root(&current) {
        return Ok(root);
    }

    if require_project {
        bail!(
            "no .savestate.toml found from {}; run `savestate init` in the project root",
            current.display()
        );
    }

    Ok(current)
}

fn discover_project_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|ancestor| ancestor.join(".savestate.toml").is_file())
        .map(Path::to_path_buf)
}

fn require_project_marker(root: &std::path::Path) -> Result<()> {
    if root.join(".savestate.toml").is_file() {
        return Ok(());
    }
    bail!(
        "no .savestate.toml found at {}; run `savestate init` in the project root",
        root.display()
    )
}

#[cfg(test)]
mod tests {
    use super::hook_invocation_event_from;

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn hook_detection_only_accepts_the_actual_subcommand() {
        assert_eq!(
            hook_invocation_event_from(&arguments(&[
                "savestate",
                "--require-project",
                "hook",
                "codex",
                "stop",
            ])),
            Some("stop")
        );
        assert_eq!(
            hook_invocation_event_from(&arguments(&[
                "savestate",
                "run",
                "echo",
                "hook",
                "codex",
                "stop",
            ])),
            None
        );
        assert_eq!(
            hook_invocation_event_from(&arguments(&[
                "savestate",
                "--root=/tmp/project",
                "hook",
                "claude",
                "session-start",
            ])),
            Some("session-start")
        );
    }
}
