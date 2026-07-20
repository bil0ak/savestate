# Experimental database support

Database capture is disabled unless `[experimental] databases = true` and a target is explicitly configured. Manual `create` and `run` commands can include configured databases. Automatic Claude and Codex hook checkpoints are always filesystem-only.

## SQLite

SQLite capture uses the online backup API to obtain a consistent local copy. Savestate refuses symlink targets, replacement races, and incomplete hardlink topology. It runs `quick_check` and verifies restored row content through a second consistent-backup digest.

Configured SQLite files are excluded from ordinary file copying and restored only through the adapter. A detected but unconfigured database remains ordinary preserved state.

## PostgreSQL

PostgreSQL requires compatible `pg_dump`, `pg_restore`, and `psql` executables. Credentials come from the configured environment variable. Passwords are removed from URL arguments and passed only through `PGPASSWORD`; they are not written to manifests or persisted diagnostics.

Snapshotting brackets the dump with cluster identifier, database OID, and inventory checks. Restore requires PostgreSQL 13 or newer, target ownership, `CREATEDB` or superuser, `allow_restore = true`, and a distinct maintenance database. Staged databases use operation-scoped randomized names. Every destructive rename or drop verifies the recorded OID first.

Remote TCP restore additionally requires `allow_remote_restore = true` and an interactive `database@host` confirmation. Savestate checks the server-reported live address and refuses libpq service indirection. Tunnels can make a remote server appear loopback-local, so the gate is not a substitute for operator review.

Savestate refuses PostgreSQL databases whose lossless recreation would require unsupported database-level ACLs, role settings, comments, locale/provider/encoding/tablespace changes, connection limits, or template state. Object ownership and grants inside the dump archive are retained.

Use one Savestate project per PostgreSQL database. Independent projects targeting the same database can race their staged cutovers.

## Optional integration test

The disposable PostgreSQL suite is never run against an arbitrary database. Set `SAVESTATE_TEST_POSTGRES_URL` to a disposable database owned by a role with `CREATEDB`, then run:

```sh
cargo test --all-features databases::postgres_dump_restore_and_cutover -- --ignored --exact
```
