# Verification

Run checks from the repository root. The verified platform is macOS. Automated checks use isolated configuration and Rex fixtures; app rendering, client switching and Lua shortcuts require a live Rex check.

## Rust checks

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build
```

Build before running the scripts below: they execute `target/debug/rexue` directly. [Public contract tests](../tests/scope.rs) cover supported commands/configuration, imports, the editable example, keybindings and defaults. Unit and [process tests](../tests/backend.rs) cover session reuse, full IDs, agent targeting, startup, previews and removal without touching live Rex sessions.

## Interactive picker acceptance

Requires Python 3 and fzf **0.63+** on PATH:

```sh
python3 scripts/pty_smoke.py --binary target/debug/rexue --output docs/verification/fzf.json
python3 scripts/worktree_removal_smoke.py --binary target/debug/rexue
```

Runs real fzf in a controlling PTY with Rex/Worktrunk test doubles. Covers filtering, aliases, numbered selection, configurable shortcuts, previews, rename/removal, worktree creation/refresh and terminal restoration. Includes empty/non-repository worktree lists, recovery after refresh, shell punctuation, inherited fzf settings and missing/old/failing fzf.

The focused removal script uses real Git/Worktrunk and a stateful Rex fixture. It checks removal of the current worktree from both pickers, selection or creation of the next session, preservation of the original session after a failed switch, cancellation, dirty-checkout protection, full session IDs and terminal restoration.

## Extended keyboard acceptance

Run this as well for input decoding or navigation changes:

```sh
python3 scripts/keyboard_smoke.py --binary target/debug/rexue --output docs/verification/keyboard.json
```

Checks Ctrl+J/K with ordinary control bytes, CSI-u (text, repeat and alternate key reports) and xterm modifyOtherKeys; also Ctrl+N/P, arrows and suspend/resume terminal restoration. Uses real fzf and private Rex fixtures. Unit tests cover fragmented reports, release events, Unicode and bracketed paste.

## Worktrunk acceptance

Requires Python 3, Git and Worktrunk **0.81.0+** (`wt` on PATH):

```sh
python3 scripts/worktree_smoke.py --binary target/debug/rexue --output docs/verification/worktree.json
```

Uses real Git/Worktrunk, temporary repositories/configuration/hooks and a stateful Rex test double. Covers discovery, both list schemas, detached checkouts, path templates, create/connect reuse, hook approvals and protected removal. Verifies that the `worktree remove` command keeps the Rex session and that recreated checkouts handle session-name conflicts. Picker removal and navigation are covered by interactive picker acceptance above.

## Focused CLI probes

Run the relevant probe for shell placeholders, positional scope or wildcard changes:

```sh
python3 scripts/shell_smoke.py --binary target/debug/rexue
python3 scripts/shell_scope_smoke.py --binary target/debug/rexue
python3 scripts/wildcard_smoke.py --binary target/debug/rexue
```

These use isolated fixtures to exercise the executable CLI.

## Agent picker navigation

Connecting an agent must refresh its metadata and focus its session, current tab and exact pane. Rows include pane numbers to distinguish agents sharing a tab. Exited processes must disappear even if status records remain; a `done` status alone does not mean exit.

The [floating shortcut](../examples/rex-picker-floating.lua) must resolve `ctx.block_id` through `session.describe_block`, then pass that block's `window_id` to `session.new_layer`. The server's active window can point to another tab after an agent switch.

For navigation or shortcut changes, check in the live app:

1. Connect to an agent and verify the selected session, tab and pane.
2. From that terminal, invoke the configured floating shortcut twice. Each time, verify the list is visible in that tab and cancel the picker.
3. Restore the initial session/tab; close only test-created panes.

Validate both Lua examples without installing them:

```sh
rex config check examples/rex-picker.lua
rex config check examples/rex-picker-floating.lua
```

## Verification records

[Picker](verification/fzf.json), [keyboard](verification/keyboard.json), [Worktrunk](verification/worktree.json) and [public CLI/completions](verification/public-cli.json) record earlier runs. [Worktree removal](verification/worktree-removal.json) records the focused removal checks. [Session/agent fixtures](verification/session-agents.json), [live agent navigation](verification/agent-picker.json) and [performance samples](verification/performance.json) retain focused evidence. They do not certify the current checkout.

Update receipts only from checks actually run; omit `--output` for local checks without replacing a receipt. Report commands, outcomes and whether evidence used fixtures or the live app. Performance samples combine live read-only listing with fixture-based picker timing; they do not measure live app rendering. Other platforms require separate verification.
