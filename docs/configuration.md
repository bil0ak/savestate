# Configuration

Savestate reads `.savestate.toml` from the discovered project root. A configuration file must be a regular, non-symlink file.

```toml
store = ".savestate"
respect_gitignore = true
include = [".env", "data/development.sqlite"]
exclude = []
external_paths = ["../tool-state"]

[limits]
warn_files = 100000
warn_size = "2GiB"

[retention]
manual = "keep"
agent = 20
recovery = 10
run = 10

[experimental]
databases = true

[[sqlite]]
path = "data/development.sqlite"

[[postgres]]
name = "development"
url_env = "DATABASE_URL"
maintenance_db = "postgres"
allow_restore = false
allow_remote_restore = false
```

## Paths and selection

The default store is `.savestate` inside the project. Relative paths resolve from the project root; a leading `~` expands to the current user's home directory. An external store is namespaced by canonical project identity. A valid legacy global store containing exactly one project's checkpoints remains readable; mixed ownership is refused.

`respect_gitignore = true` captures tracked files and non-ignored untracked files. `include` can select ignored paths, while `exclude` wins over `include`. Patterns must remain relative to a capture root. Mandatory exclusions for `.git`, the active store, and restore transaction data always win. `--include-ignored` disables Git-ignore filtering for one checkpoint only.

External paths are normalized and must not overlap the project, store, or one another. SQLite paths must belong to the project or a registered external root.

## Limits and retention

`warn_files` and `warn_size` are warning thresholds, not truncation limits. Binary (`KiB`, `MiB`, `GiB`, `TiB`) and decimal (`KB`, `MB`, `GB`, `TB`) sizes are accepted. Invalid values fail during configuration loading.

Manual checkpoints are kept. Agent, recovery, and `savestate run` checkpoints have independent positive limits. Pins protect any checkpoint from automatic retention. Relabeling never changes retention class.

The legacy top-level `keep_last` field remains readable and maps to agent retention when no newer value overrides it.

## Secrets

Configuration names environment variables for database credentials; it does not store their values. Captured files and database contents can still contain secrets. See [SECURITY.md](../SECURITY.md).
