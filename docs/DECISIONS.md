# Rex mappings and verification limits

[PARITY.md](PARITY.md) inventories Sesh commit `9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a`. [VERIFICATION.md](VERIFICATION.md) records executable evidence for this implementation. These mappings make the backend and output formats explicit.

| Area | Native behavior and consequence |
| --- | --- |
| Resources | Map sessions to Rex sessions, windows to tabs, and panes to blocks. Resolve human labels, then mutate with full IDs. Ambiguous labels produce errors. |
| App selection | Preflight a connected app and `session.select` before session creation. Require `--client`/`REX_CLIENT` when multiple apps exist; `--detach` creates/reuses without moving any app. |
| Application activation | Root `terminal` names a macOS app or absolute bundle path. After successful foreground connection, use system `open -a` with independent arguments; this needs no AppleEvent/JavaScript runtime. Empty, non-macOS and detached/background connections skip activation; failures warn. |
| Root connection | `connect --root` detects an optional Git/bare/JJ root on the input directory before resolving configuration/alias/session identity. Detection failure preserves the target; missing optional Git does not prevent plain directory connections. This is separate from the `root` naming command. |
| Recent sessions | Keep successful rexue switches per Rex client. `last` toggles between the latest live recorded sessions; stale inherited launcher context cannot override history. Switches made directly in other applications are not a source of history because the documented Rex client listing does not report a last-attached timestamp. |
| External browse | Prefer the worktree containing process cwd; otherwise use the calling Rex session, then the selected client's recorded live session directory. |
| File selection | Use `-C/--config-file` for TOML; reserve local `list/picker -c/--config` for source selection. The long name differs from Sesh to remove global/local ambiguity. |
| Source aliases | Accept `--tmux` as an alias for `--rex`; migrate legacy source names in configuration, including `tmux-pane` to independent `rex-block` with its own icon and green color. No tmux process or protocol is used. |
| JSON | Expose native snake_case fields, full Rex IDs and lowercase sources. This is a native schema rather than Sesh's larger Go-field schema. Window JSON comes from Rex. |
| Names and expressions | Use native labels; map ordinary tmux window/session name, index and active tokens to native metadata. Interpret templates once and preserve braces in names. Do not evaluate arbitrary tmux expressions or impose tmux's punctuation restrictions on Rex labels. Root naming tries substitutions on the original path before Git/JJ root strategies and ordinary-directory fallback; it does not require a repository. Git/JJ anchors, path substitutions, enriched-name reuse and ambiguous matches are covered by tests. |
| Matching | Use system `fzf --filter` for case-insensitive fuzzy matching and stable ties. The interactive fzf instance reloads ranked rows, supporting alias and index modes in one query field without a second search pass or match highlights. Alias equality retains Unicode simple non-Turkic folding. |
| Imports | Match the pinned loader: resolve exact paths from process cwd; append only session/window/wildcard in declaration order. Ignore nested imports and other imported settings; primary strictness does not propagate to imports. |
| Configuration errors | Warn about unknown fields by default, reject under strict mode. Reject bad types/values, unsupported tmux settings, duplicate identities and undefined windows. Empty window/preview/icon values continue session→wildcard→default fallback; startup follows source-specific command and disable strategies. Aliases remain session-local. |
| Startup | Support script content/file paths and commands. Run startup and window templates only when creating a session; keep/report an already-created session if a later window fails. |
| Process boundaries | Pass arguments independently. Startup/preview scripts are explicit shell programs; frecency strings are executable-plus-arguments. Quoted placeholders remain data. Bound process time/output and terminate timed-out process groups. |
| Caches | Cache optional frecency rows for five seconds while keeping Rex/config live. GitHub cache is separate, with 24-hour positive and one-hour negative TTLs. Worktree picker opens cached-first and refreshes asynchronously; manual failures stay visible without discarding rows. This does not reproduce Sesh's full-session stale-while-refresh cache. |
| Worktree safeguards | Resolve metadata before creation, validate registered worktrees and branches, and report fetch/explicit refresh failures. Preserve preexisting worktrees on failed foreign PR checkout. |
| UI | Both session and worktree pickers invoke system fzf ≥ 0.63. Preserve sources, aliases, numbered shortcuts, previews, explicit deletion confirmation and worktree refresh through private callbacks and stable row IDs. Fzf owns layout, query editing, cursor, clipping and terminal restoration; old native palette and dialog geometry are historical. |
| Input and refresh | Source collection completes before fzf opens. Worktree cached rows refresh once asynchronously; manual refresh failures retain old rows and appear in the header. Reopening preserves the query, with selection and preview reset. Forward termination signals to fzf so it can restore terminal modes. See FZF_VERIFICATION.md for current PTY evidence. |
| Reference bugs | Honor separator-aware CLI settings; preserve worktree `--path` scope on connection; reject number+browser; let explicit repository/PR flags override inferred browser values. Do not reproduce documented upstream wiring bugs. |

## Verification limits

macOS and the installed Rex server/app are the real runtime target. Ordinary issue worktrees, Git clone/root, cache refresh, URL printing, native terminal input and session lifecycle have live evidence. Own/foreign PRs, negative cache/error paths, browser URL parsing/AppleScript adapter and optional JJ metadata are exercised with isolated fixtures. The current machine has no `jj` executable, so an actual `jj git clone` invocation was not exercised. No real browser was opened or inspected; Automation-permission prompts and actual browser UI behavior remain environment-dependent. Linux URL opening and Windows are not claimed as real-device checks.

Headless Rex terminal sizes revert to 80×24 when a transient resizing CLI disconnects. The live Rex smoke test records the effective size and uses an explicit preview threshold below it. A real Unix PTY independently exercises 120×30 and 42×12 resizing. This is a test-environment rule, not a preview-layout workaround in the product.

## Exclusions

No tmux backend, tmuxp/tmuxinator/tmuxifier compatibility, desktop/Web GUI, external launcher extensions, Windows port, multihost orchestration, upstream changes, package-manager publication or remote publication is included.

## Attribution

This Rust implementation adapts Sesh's session, configuration, picker and GitHub worktree behavior. Sesh is by joshmedeski and contributors, MIT licensed. The original copyright and permission notice are retained in [LICENSE-Sesh](../LICENSE-Sesh), alongside this project's [LICENSE](../LICENSE).
