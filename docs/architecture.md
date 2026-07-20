# Architecture

Savestate is a synchronous, single-process Rust application. The CLI is the product boundary: command names, flags, exit behavior, JSON, configuration, store layout, and manifest schemas 1–4 are compatibility contracts. The pre-1.0 Rust API may evolve.

## Dependency direction

```text
CLI → application use cases → capture / store / restore / adapters
                           ↘ model
```

- `src/main.rs` owns process termination and nothing else.
- `src/cli/` owns clap parsing, project discovery, terminal prompts, hook protocol output, and all human/JSON rendering.
- `src/app/` coordinates use cases through the `Savestate` facade.
- `src/model/` contains persisted and validated values without filesystem, process, configuration, or terminal I/O.
- `src/capture/` discovers roots, applies Git and explicit selection rules, captures stable files and metadata, materializes objects, and calculates diffs.
- `src/store/` owns the concrete content-addressed store, catalog, labels, pins, retention, journals, locking, validation, and atomic publication.
- `src/restore/` implements filesystem transaction mechanics. `src/adapters/` handles SQLite and PostgreSQL.
- `src/integrations/` owns Claude and Codex configuration and hook records.
- `src/platform.rs` and `src/terminal_signals.rs` are the only modules allowed to contain unsafe code.

The design deliberately avoids repositories, an async runtime, service locators, and dependency-injection frameworks. A concrete `Store` is split into responsibility-based implementation modules. Traits exist only where behavior is genuinely interchangeable, such as checkpoint adapters and deterministic restore prompting.

## Rust facade

`Savestate` is the documented entry point. New callers should prefer the silent, structured methods:

- `create_checkpoint(CreateOptions)` returns a `CreateOutcome`;
- `checkpoints()` returns `CheckpointSummary` values;
- `plan_restore(CheckpointSelector)` returns an opaque `RestorePlan`;
- `apply_restore(RestorePlan)` refreshes the plan under the mutation lock before applying it.

Validated types include `CheckpointId`, `CheckpointSelector`, `ObjectHash`, `RootId`, `OperationId`, and `RelativePath`. Their serialization remains the underlying string or path representation. Manifest structs retain their historical wire fields so schemas 1–4 load without migration.

Internal code uses `anyhow` for contextual error chains. The facade maps important caller actions to `SavestateError`, including missing or ambiguous checkpoints, unstable capture, pending recovery, stale restore plans, and external-tool preflight failures.

## Mutation and lock ordering

The store's exclusive lock is the single mutation boundary. Code acquires that lock before publishing or deleting checkpoints, changing catalog metadata, applying retention, or entering restore mutation. Restore code then durably advances its journal; it never obtains a second store lock while holding a lower-level resource lock.

Atomic files are written to a sibling temporary file, synchronized, renamed, and followed by directory synchronization. Staging directories use RAII cleanup guards. Content objects are immutable and addressed by BLAKE3 hash.
