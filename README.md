# Savestate

<p align="center">
  <img src="https://raw.githubusercontent.com/bil0ak/savestate/main/.github/assets/social-preview.jpg" alt="Savestate — Let the agent cook. Keep an undo button." width="100%">
</p>

<p align="center">
  <strong>Verified local checkpoints for coding-agent sessions.</strong>
</p>

<p align="center">
  <a href="https://github.com/bil0ak/savestate/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-65F2B1" alt="MIT license"></a>
  <img src="https://img.shields.io/badge/rust-1.85%2B-65F2B1" alt="Rust 1.85 or newer">
</p>

Git protects your code history. Savestate protects the local working state around it.

Before a coding agent starts changing your project, Savestate creates a verified checkpoint of project files and explicitly configured local state. You can inspect what changed, preview a rollback, and recover safely even if a restore is interrupted.

```text
checkpoint  →  let the agent work  →  inspect the diff  →  keep it or roll it back
```

## Why Savestate?

- **Built for agent sessions.** Install hooks for Codex or Claude, or wrap any command with `savestate run`.
- **More than tracked files.** Capture non-ignored untracked files, opt into ignored paths, register external roots, and configure local databases.
- **Verify before trusting.** Checkpoints are content-addressed and verified when created, explicitly checked, and restored.
- **Preview before changing anything.** Every restore has a dry-run, and live state is checked again before mutation.
- **Recover instead of guessing.** Durable restore journals support explicit resume or rollback after interruption.

Savestate complements Git; it does not replace it. Git remains the source of truth for shared code history. Savestate is the local undo button for the messy state between commits.

## Quick start

```sh
cd your-project
savestate init
savestate integrate codex  # or: savestate integrate claude
```

That is enough to create automatic filesystem checkpoints from supported agent hooks. You can also work manually:

```sh
savestate create --label "before the refactor"

# Let the agent cook.

savestate diff
savestate restore --dry-run
savestate restore
```

Or checkpoint immediately before any command:

```sh
savestate run -- your-command --with-arguments
```

Run `savestate` without a command for project-aware guidance. Run `savestate --help` for the complete command list.

## Install

Download a release archive, verify it against `SHA256SUMS`, and place `savestate` on your `PATH`.

To install from source:

```sh
git clone https://github.com/bil0ak/savestate.git
cd savestate
cargo install --path . --locked
```

Savestate requires Rust 1.85 or newer when building from source.

## What gets checkpointed?

By default, Savestate captures tracked files and non-ignored untracked files. Selection is controlled by `.savestate.toml`:

```toml
store = ".savestate"
respect_gitignore = true
include = [".env", "data/development.sqlite"]
exclude = ["tmp/**"]
external_paths = ["../tool-state"]
```

Ignored files are included only when explicitly selected in configuration or when a manual checkpoint uses `--include-ignored`. `.git`, the active Savestate store, and restore transaction data are always excluded.

SQLite and PostgreSQL checkpoints are available as explicit experimental opt-ins. Automatic agent-hook checkpoints remain filesystem-only.

## The safety model

Restoring local state is destructive, so Savestate is deliberately conservative:

1. Verify the target checkpoint and every referenced object.
2. Capture and verify the state that is about to be replaced.
3. Materialize the restore away from live targets.
4. Journal every preservation, swap, verification, and cleanup transition.
5. Commit the staged state and verify it again.

If a restore stops before completion, Savestate blocks new mutations until you explicitly resume or roll it back:

```sh
savestate recover --resume
# or
savestate recover --rollback
```

## Useful commands

| Command | What it does |
| --- | --- |
| `savestate create` | Create and verify a checkpoint |
| `savestate list` | List available checkpoints |
| `savestate diff` | Compare a checkpoint with current state or another checkpoint |
| `savestate verify` | Re-verify checkpoint contents and configured services |
| `savestate restore --dry-run` | Preview a restore without changing live state |
| `savestate restore` | Restore a selected checkpoint |
| `savestate status` | Show snapshot scope, store details, and integration health |
| `savestate doctor` | Diagnose project and agent integration setup |
| `savestate pin` | Protect a checkpoint from automatic retention |
| `savestate prune --dry-run` | Preview retention cleanup |

## Security

> [!WARNING]
> Checkpoints may contain credentials, ignored files, and database contents. The store is not encrypted. Keep it owner-only and on a local filesystem; never share one store over NFS or SMB.

Database restore is intentionally gated. PostgreSQL restore requires explicit configuration and additional confirmation for remote targets. See the [security policy](https://github.com/bil0ak/savestate/blob/main/SECURITY.md) and [database limitations](https://github.com/bil0ak/savestate/blob/main/docs/databases.md) before enabling it.

## Documentation

- [Configuration reference](https://github.com/bil0ak/savestate/blob/main/docs/configuration.md)
- [Restore safety and crash recovery](https://github.com/bil0ak/savestate/blob/main/docs/restore-safety.md)
- [Database support and limitations](https://github.com/bil0ak/savestate/blob/main/docs/databases.md)
- [Architecture and Rust API](https://github.com/bil0ak/savestate/blob/main/docs/architecture.md)
- [Contributing](https://github.com/bil0ak/savestate/blob/main/CONTRIBUTING.md)
- [Security policy](https://github.com/bil0ak/savestate/blob/main/SECURITY.md)

Savestate is synchronous, CLI-first, and intentionally conservative. Windows support is experimental.

## License

MIT © Bilal Akkil. See the [license](https://github.com/bil0ak/savestate/blob/main/LICENSE).
