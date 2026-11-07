# Verification

Run from the repository root. Verified platform: macOS. Script checks use Python 3; picker suites require fzf **0.63+**, and worktree suites require Git and Worktrunk **0.81.0+** (`wt`). Automated checks use isolated fixtures; app focus, rendering and shortcuts require live Rex checks.

## Rust checks

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build
```

Build before script acceptance: scripts execute `target/debug/rexue` directly. [Public contracts](../tests/scope.rs) cover commands, configuration and examples; [backend tests](../tests/backend.rs) cover session reuse, IDs, agents, previews, startup and removal.

## Interactive picker acceptance

Includes worktree cases; requires both fzf and Worktrunk.

```sh
python3 scripts/pty_smoke.py --binary target/debug/rexue
python3 scripts/worktree_removal_smoke.py --binary target/debug/rexue
```

Real fzf in a PTY covers selection, filtering, aliases, shortcuts, previews, rename/removal, worktree create/refresh and terminal restoration. Focused removal uses real Git/Worktrunk and fake Rex sessions to check ownership, handoff, dirty-checkout protection, failure recovery and unrelated-session preservation.

For cleanup against live Rex:

```sh
python3 scripts/live_worktree_cleanup_smoke.py --binary target/debug/rexue
```

Creates a temporary repository and test-owned sessions/panes/processes, verifies their removal, and cleans up only its own resources without changing the app's selected session.

## Extended keyboard acceptance

Run for input decoding or navigation-key changes:

```sh
python3 scripts/keyboard_smoke.py --binary target/debug/rexue
```

Covers control bytes, CSI-u, xterm modifyOtherKeys, navigation and suspend/resume restoration. Unit tests cover fragmented reports, release events, Unicode and bracketed paste.

## Worktrunk acceptance

Run for discovery, create/connect or removal changes:

```sh
python3 scripts/worktree_smoke.py --binary target/debug/rexue
```

Uses real Git/Worktrunk, temporary configuration/hooks and fake Rex sessions. Covers both list schemas, detached checkouts, path templates, reuse, approvals, removal ownership and recreated-session name conflicts.

## Focused CLI probes

Run the relevant probe for shell placeholders, positional scope or wildcard matching:

```sh
python3 scripts/shell_smoke.py --binary target/debug/rexue
python3 scripts/shell_scope_smoke.py --binary target/debug/rexue
python3 scripts/wildcard_smoke.py --binary target/debug/rexue
```

## Startup and idle performance

```sh
cargo build --release
python3 scripts/performance_smoke.py --binary target/release/rexue --max-rex-calls 3
python3 scripts/performance_smoke.py --binary target/release/rexue --legacy-rex --max-rex-calls 52
```

Uses real fzf with four fixture sessions, each with one unnamed tab, three agents and one shell. Each Rex process waits 15ms to model startup/connection cost. Reports five-run medians for visible-list time, Rex process count, idle RSS and CPU; checks idle requests and terminal restoration.

Native discovery uses one fresh Lua snapshot; `--legacy-rex` checks the fallback for CLIs without Lua. Backend tests cover snapshot equivalence, freshness and errors. Use `--runs N`, `--idle-seconds N` or `--output PATH` to customize measurements.

Compare identical build settings without concurrent acceptance suites. Picker timing includes Python PTY startup; RSS sums the picker and descendants; idle CPU excludes the wrapper. Per-RPC network latency, live app rendering and peak memory are outside this fixture. Omit the call limit for older binaries. [Recorded samples](verification/performance-startup.json) also include read-only live listing.

## Agent picker navigation

See [picker behavior](CONFIGURATION.md#picker-settings). Preserve full session/window/block IDs and refresh agent metadata before focus. Before focusing a pane, refresh its current tab; selecting a tab must focus that tab. Omit exited processes even when status records remain. A `done` status alone does not mean exit.

The [floating shortcut](../examples/rex-picker-floating.lua) must resolve `ctx.block_id` with `session.describe_block` and pass that block's `window_id` to `session.new_layer`; the server's active window may refer to another tab after an agent switch.

For focus or shortcut changes, check live Rex:

1. Connect to an agent; verify its session, tab and exact pane.
2. Invoke the floating shortcut twice from that pane. Test empty and prefilled queries; verify the list appears in that tab, excludes only its temporary pane and retains other floating panes. Cancel each picker.
3. Restore the initial session/tab; close only test-created panes.

Validate examples without installing them:

```sh
rex config check examples/rex-picker.lua
rex config check examples/rex-picker-floating.lua
```

## Verification records

Receipts describe recorded runs, not the current checkout. Update only after executing checks; use `--output PATH` when intentionally saving a script receipt. Report commands, outcomes and fixture versus live evidence. Other platforms need separate verification.

| Evidence | Receipts |
| --- | --- |
| Automated | [Picker](verification/fzf.json), [keyboard](verification/keyboard.json), [Worktrunk](verification/worktree.json), [removal](verification/worktree-removal.json), [CLI](verification/public-cli.json), [session/agent fixtures](verification/session-agents.json) |
| Live navigation | [Session tree](verification/session-tree.json), [initial focus](verification/initial-focus.json), [agents](verification/agent-picker.json) |
| Performance | [Startup](verification/performance-startup.json), [earlier samples](verification/performance.json) |
