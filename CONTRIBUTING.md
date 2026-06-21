# Contributing

Contributions are welcome, especially compatibility reports for new Codex versions and operating systems.

## Before opening an issue

- Search existing issues.
- Reproduce the problem with the latest release.
- Run Diagnostics from the application where possible.
- Remove conversations, credentials, usernames, machine names, and private paths from screenshots and logs.
- Never upload your complete `.codex` directory.

## Development setup

```bash
cargo fmt --check
cargo clippy --all-targets --features gui -- -D warnings
cargo test --all-targets --features gui
cargo run --features gui --bin codex-migrate-gui
```

## Pull requests

1. Keep changes focused.
2. Add tests for migration, path, transaction, or parser behavior.
3. Preserve backward compatibility unless the change is explicitly documented.
4. Do not add telemetry, network uploads, authentication migration, or source database copying without prior discussion.
5. Update the README and changelog when behavior changes.

## Architecture

- `scanner`: reads rollout JSONL and optional SQLite metadata.
- `merge`: classifies UUID/hash/prefix conflicts.
- `path_mapper`: normalizes POSIX, Windows, UNC, and WSL paths.
- `operations`: coordinates import, export, repair, and validation.
- `transaction`: creates and restores rollback snapshots.
- `sqlite_adapter`: performs schema-aware metadata updates.
- `desktop_state` and `session_index`: register migrated sessions for visibility.
- `html_export`: creates self-contained conversation exports.
- `src/bin/codex-migrate-gui.rs`: native `egui/eframe` interface.

By submitting a contribution, you agree that it may be distributed under the MIT License.

