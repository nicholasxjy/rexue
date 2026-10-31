# Verification

Rexue's supported workflows are Rex sessions, zoxide directory history and Worktrunk worktrees. The current verification platform is macOS.

The current scope reduction passed all 114 Rust tests (76 unit, 33 process integration, 5 scope/configuration), `cargo fmt --check`, and `cargo clippy --all-targets -- -D warnings`. Actual fzf 0.74.5 passed 34 PTY cases; actual Worktrunk 0.81.0 and Git passed 21 isolated acceptance checks. Expected missing/old/failing-tool cases deliberately exit with errors and are asserted by the scripts.

Current receipts: [fzf](verification/fzf.json), [Worktrunk](verification/worktree.json), and [public CLI, all five shell completions and example configuration](verification/public-cli.json). The three focused shell/wildcard probes below also passed. Local Markdown links and `git diff --check` were checked.

## Rust checks

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Unit and process integration tests cover configuration loading and imports, startup inheritance and disable flags, shell path placeholders, aliases, source filtering and ordering, directory history, session creation/reuse, full IDs, client selection, last-session history, previews, rename validation and removal. Tests in `tests/scope.rs` check the final command set, removed command/options/configuration, metadata and picker normalization.

Rex integration uses executable test doubles with separated process arguments and JSON fixtures. These tests do not create or delete real Rex sessions.

## Interactive picker acceptance

```sh
python3 scripts/pty_smoke.py --binary target/debug/rexue --output docs/verification/fzf.json
```

This script runs the installed fzf in a controlling PTY with isolated Rex/Worktrunk fixtures. It covers cancellation and terminal restoration, filtering, preview, aliases and numbered shortcuts, rename and deletion confirmations, query persistence, worktree creation/refresh/removal, paths and labels containing shell punctuation, and missing/old/failing fzf. It also checks that inherited fzf environment settings cannot change the selection protocol.

## Worktrunk acceptance

```sh
python3 scripts/worktree_smoke.py --binary target/debug/rexue --output docs/verification/worktree.json
```

This script runs real Git and Worktrunk with private configuration, temporary repositories, checkout templates and hooks. Rex is a stateful test double. It covers discovery, branch/path selection, create/connect reuse, detached worktrees, Worktrunk hook approvals, safe removal, checkout recreation, and session naming when another checkout already owns a repository label. Its fixtures and state are removed on exit.

## Focused CLI probes

```sh
python3 scripts/shell_smoke.py --binary target/debug/rexue
python3 scripts/shell_scope_smoke.py --binary target/debug/rexue
python3 scripts/wildcard_smoke.py --binary target/debug/rexue
```

These probes verify nested shell placeholders, path binding across positional scopes, and wildcard character classes through the executable CLI.

## Limits

The isolated checks establish CLI behavior, process protocol, actual fzf interaction, and actual Worktrunk/Git behavior. They do not establish live Rex app rendering or client switching, a real keypress on the example Lua binding, or behavior on other platforms. No personal configuration, live session, or existing user worktree is modified by this verification.
