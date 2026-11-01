# Verification

Rexue's supported workflows are Rex sessions, zoxide directory history and Worktrunk worktrees. The current verification platform is macOS.

The keyboard compatibility update passed all 121 Rust tests (83 unit, 33 process integration, 5 scope/configuration), `cargo fmt --check`, and `cargo clippy --all-targets -- -D warnings`. Actual fzf 0.74.5 passed the existing 34 picker PTY cases and 9 keyboard cases. The existing Worktrunk 0.81.0 and Git receipt covers 21 isolated acceptance checks. Expected missing/old/failing-tool cases deliberately exit with errors and are asserted by the scripts.

Current receipts: [fzf](verification/fzf.json), [keyboard compatibility](verification/keyboard.json), [Worktrunk](verification/worktree.json), and [public CLI, all five shell completions and example configuration](verification/public-cli.json). The three focused shell/wildcard probes below also passed. Local Markdown links and `git diff --check` were checked.

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

## Extended keyboard acceptance

```sh
python3 scripts/keyboard_smoke.py --binary target/debug/rexue --output docs/verification/keyboard.json
```

This uses the same controlling-PTY harness with private configuration and a Rex test double. Actual fzf must move down on Ctrl+J and up on Ctrl+K for ordinary control bytes, CSI-u, CSI-u with text, repeat events and alternate key codes, and xterm modifyOtherKeys. Extended Ctrl+N/P and ordinary arrow keys are also checked. A suspend/resume case verifies canonical input is restored while stopped, raw input returns on resume, navigation still works, and final terminal settings are restored. The receipt is [keyboard.json](verification/keyboard.json).

Rexue routes interactive fzf through a private PTY to normalize its keyboard input while keeping catalog stdin and selection stdout separate. Unit tests cover fragmented reports, release events, unknown sequences, Unicode text, and bracketed paste containing literal keyboard sequences. Existing picker PTY cases check resize, cancellation, signals, confirmation and rename prompts, and worktree actions through this same terminal relay.

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

## Performance

Session and agent discovery runs up to four session queries and four pane queries per session concurrently, preserving the original catalog order. Connecting a selected live row refreshes only that session and, for agents, the selected pane. Explicit naming and directory reuse still check all live sessions for conflicts. No session or agent cache is introduced.

The performance change passed all 131 Rust tests (87 unit, 39 process integration, 5 scope/configuration), `cargo fmt --check`, and `cargo clippy --all-targets -- -D warnings`. The real-fzf PTY and extended-keyboard acceptance scripts also passed. Regression tests assert overlapping session queries, stable row ordering, targeted connection requests, and rejection of exited agents.

The [performance receipt](verification/performance.json) records medians and individual samples from three runs before and after the change. Live listing measurements only read the local Rex server. Picker opening and Enter-to-completion measurements use real fzf with isolated Rex fixtures containing four sessions and three agent panes per session, with 15ms latency per fixture request. Those timings include process startup and terminal restoration; they do not measure live Rex app rendering.

## Limits

The isolated checks establish CLI behavior, process protocol, actual fzf interaction, and actual Worktrunk/Git behavior. They do not establish live Rex app rendering or client switching, a real keypress on the example Lua binding, or behavior on other platforms. No personal configuration, live session, or existing user worktree is modified by this verification.
