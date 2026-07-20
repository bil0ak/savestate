# Contributing to Savestate

Savestate protects state that may be difficult or impossible to recreate. Changes should preserve compatibility and fail closed when ownership or recovery information is uncertain.

## Toolchain

The minimum supported Rust version is 1.85 and the crate uses edition 2024. Install `rustfmt` and Clippy, then run:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features --locked
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
git diff --check
```

The same default gates must pass on Linux, macOS, and Windows. Do not weaken or silently remove a compatibility, corruption, failpoint, recovery, hook-protocol, or CLI characterization test.

## Architecture rules

- Keep command parsing, prompts, and terminal/JSON rendering under `src/cli/`.
- Return errors and structured results from application and infrastructure modules.
- Terminate the process only in `src/main.rs`.
- Keep model types free from filesystem, subprocess, terminal, and configuration I/O.
- Use the concrete store and its exclusive mutation lock; do not add repository or dependency-injection abstractions without a demonstrated interchangeable implementation.
- Keep unsafe code in `platform.rs` or `terminal_signals.rs`, with a `SAFETY` explanation for every block.
- Preserve manifest schemas 1–4, configuration fields, JSON shapes, and the on-disk store without migration unless a separately reviewed compatibility change says otherwise.

Run `cargo fmt` after each mechanical move. Prefer focused module tests for private invariants and integration tests for the public facade or binary behavior.

## Optional suites

The PostgreSQL integration test requires `SAVESTATE_TEST_POSTGRES_URL` pointing to a disposable owned database with `CREATEDB`. It performs destructive staged cutover operations.

The 200,000-file scope test is intentionally ignored because it is expensive:

```sh
cargo test performance::ignored_200k_file_tree_is_pruned_but_include_ignored_captures_it -- --ignored --exact
```

Report either suite as unverified when its required environment was unavailable; never imply it passed.
