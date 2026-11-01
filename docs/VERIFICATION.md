# Verification

Rexue's supported workflows are Rex sessions, zoxide directory history and Worktrunk worktrees. The current verification platform is macOS.

The configurable keybindings update passed all 136 Rust tests (88 unit, 40 process integration, 8 scope/configuration), `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo build`. Actual fzf 0.74.5 passed 54 picker PTY cases, including 17 cases for configurable shortcuts, and 9 keyboard cases. Real Worktrunk 0.81.0 and Git passed 21 isolated acceptance checks. The new cases cover multiple keys per action, updated hints, overridden and disabled defaults, confirmation and input dialogs, and worktree create/refresh shortcuts with no matching rows. All interactive cases restored the terminal. Expected missing/old/failing-tool cases deliberately exit with errors and are asserted by the scripts.

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

## Agent picker navigation

Agent rows include a terminal pane number before the session and tab names, so multiple instances in one tab remain distinct even with a narrow preview layout. Connecting an agent refreshes its pane metadata and selects both its session and current window in the app. Ordinary session switches keep the app's usual tab selection.

Floating picker shortcuts must target the window containing `ctx.block_id`. Using the server's active window can create an invisible picker in a different tab after an agent switch. The [floating picker example](../examples/rex-picker-floating.lua) uses `session.describe_block` and supplies the resulting `window_id` to `session.new_layer`.

The [live navigation receipt](verification/agent-picker.json) records switching to a real agent, opening the configured picker twice in that agent's tab, checking that the list appears, and cancelling each picker. Only test-created panes are closed, and the initial session/tab is restored. Rust regression coverage checks explicit window selection using refreshed pane metadata, distinct names for agents in the same tab, and removal of exited agents despite retained status records. A `done` status alone does not mean a process has exited. The final change passed 132 Rust tests, formatting, Clippy, 37 fzf acceptance cases and 9 extended-keyboard cases. Both Lua examples and the active personal configuration passed `rex config check`.

## Performance

Session and agent discovery runs up to four session queries and four pane queries per session concurrently, preserving the original catalog order. Connecting a selected live row refreshes only that session and, for agents, the selected pane. Explicit naming and directory reuse still check all live sessions for conflicts. No session or agent cache is introduced.

The performance change passed all 131 Rust tests (87 unit, 39 process integration, 5 scope/configuration), `cargo fmt --check`, and `cargo clippy --all-targets -- -D warnings`. The real-fzf PTY and extended-keyboard acceptance scripts also passed. Regression tests assert overlapping session queries, stable row ordering, targeted connection requests, and rejection of exited agents.

The [performance receipt](verification/performance.json) records medians and individual samples from three runs before and after the change. Live listing measurements only read the local Rex server. Picker opening and Enter-to-completion measurements use real fzf with isolated Rex fixtures containing four sessions and three agent panes per session, with 15ms latency per fixture request. Those timings include process startup and terminal restoration; they do not measure live Rex app rendering.

## Limits

The isolated checks establish CLI behavior, process protocol, actual fzf interaction, and actual Worktrunk/Git behavior. They do not establish live Rex app rendering or client switching, a real keypress on the example Lua binding, or behavior on other platforms. No personal configuration, live session, or existing user worktree is modified by this verification.
