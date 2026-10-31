# Worktrunk refactor verification

The worktree provider now delegates to external **Worktrunk wt ≥ 0.81.0**. This acceptance run used **wt v0.81.0**, **fzf 0.74.5 (Homebrew)** and isolated temporary Git repositories on macOS. Full output: [verification/worktrunk.log](verification/worktrunk.log).

## Original refactor results

| Check | Result |
| --- | --- |
| `cargo fmt --all` | Passed; `git diff --check` also passed. |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed. |
| `cargo test --lib --tests` | 144 passed: 101 unit tests and 43 process integration tests; zero failures. |
| `cargo build` | Passed. |
| `python3 scripts/worktree_smoke.py` | 21 checks passed with real wt/Git and a stateful Rex fake. |
| `python3 scripts/pty_smoke.py --binary target/debug/rexue` | 31 cases passed with real fzf and controlling PTYs; successful interactive cases restored termios. Missing/old/error fzf cases correctly returned failure. |
| Legacy implementation search | No GitHub metadata cache/query, issue/PR parser, browser URL adapter or fixed worktree-root helper remains in `src`. |

## Goal evidence

1. **Generic branch lifecycle and existing checkouts.** `worktree create`, `connect`, `list`, `picker` and `remove` use wt. Real fixtures cover `feature/widgets`, custom directories with spaces/quotes, an arbitrary existing detached checkout and discovery from a linked checkout. `create` returns the path without creating a Rex session; `connect` passes the actual path to Rex and reuses the existing session on repeated connections. List JSON schemas 1 and 2 are both verified. Configured worktree sources survive global ordering and are selectable in the main picker.
2. **Worktrunk configuration and hooks.** A user-configured path template changes actual creation locations. Worktrunk's default base is used unless the user supplies `--base`. Project `pre-start` and `pre-remove` hooks execute. The first rexue creation is driven in a real PTY and displays Worktrunk's native approval prompt; the test approves only its temporary fixture commands. Further approval for removal is stored through Worktrunk's native terminal workflow. Production code supplies neither `--yes` nor `--no-hooks`; it leaves configuration parsing, precedence and removal policy to wt.
3. **Protected removal and errors.** A dirty checkout is rejected with its unsaved file intact. Foreground removal completes before JSON is returned, `--no-delete-branch` keeps the branch, and no Rex kill command is issued. Branch-only removal is rejected by rexue. Missing wt, wt 0.80.0 and a failing subprocess all produce actionable errors; no fallback implementation runs. After changing the template, a kept branch can get a new checkout; `connect --name` preserves the retained old Rex session while connecting the new path.
4. **Picker behavior.** Real fzf tests search by branch/path, preserve rows on failed refresh, connect the selected actual path despite a conflicting alias, select the main picker's worktree source, cancel removal without deleting, then confirm safe removal and observe the refreshed empty result. Preview, other source filters, aliases, confirmation, terminal resize and literal-name regressions remain covered.
5. **Explicit migration.** `[[worktree]]` retains only `path` and `icon`; `[browser]`, `repo`, `worktree_dir`, `branch_template`, `base_branch`, `fetch`, `create_command` and worktree `startup_command` are rejected with migration instructions. Tests cover both raw parsing and real file loading before permissive unknown-key filtering. README, example TOML, configuration reference, decisions and current verification describe the generic workflow; the pinned Sesh inventory is explicitly historical.

## Follow-up: switching after a terminal changes checkout

A reported failure was reproduced through the actual `worktree connect` CLI: a live session named `zed-bar_nvim` had moved to `.worktrees/wk-test`, so connecting the main checkout attempted to reuse its occupied label and failed with `already belongs to …; choose a different --name`. A read-only live query confirmed the reported label and checkout path.

Path-based reuse now runs before default-name disambiguation. If an automatic directory name belongs to another session, an available branch-suffixed label is generated (for example `zed-bar_nvim@main`); detached/non-Git paths use a short path fingerprint, with numeric suffixes when needed. Existing sessions are kept intact. Explicit `--name` and configured session labels keep their cross-directory conflict checks.

The CLI regression went from the exact reported failure to passing, and also asserts same-path reuse, no duplicate creation, no kill/rename of the existing session, and protection for explicit/configured names. The final check passed **145 Rust tests** (101 unit, 44 process), strict Clippy, build, the existing **31 PTY cases**, and real Worktrunk acceptance. The extended real-Worktrunk suite passed **22 checks** against the installed release executable, including automatic naming when switching back to main, repeat reuse and preservation of the old checkout session. Evidence: [worktrunk-session-name.log](verification/worktrunk-session-name.log).

The release executable at `/Users/nick/.cargo/bin/rexue` was updated after verification; its SHA-256 matches `target/release/rexue`. The previous executable was backed up under `/tmp/rexue-worktree-name-backup.FWpigK/rexue`. No live user session or configuration was renamed, deleted or retargeted during verification.

## Follow-up: Ctrl+W creates from the worktree picker

The dedicated worktree picker now opens a branch-name prompt with Ctrl+W. Enter creates through Worktrunk using the selected repository and its configured defaults/hooks, then connects Rex using the returned absolute path. Esc returns to the picker with the search preserved. Empty branch names and Worktrunk failures show a diagnostic and keep the picker usable. Creation works even when there are no matching rows; fzf's no-match exit status retains the requested action.

The picker distinguishes created checkouts from ordinary selections. It releases the picker bridge and restores the terminal before connecting Rex, and does not switch a second time after creation. Real user-configured pre-switch/pre-start hooks are asserted to run exactly once. The existing session picker retains its original public `pick` API and Ctrl+W query-editing behavior.

Validation passed: **146 Rust tests** (102 unit, 44 process), strict Clippy, debug/release builds, diff/format checks, and **34 real fzf PTY cases**. Added PTY cases cover canceling from empty results without creating a branch/session, empty/existing-branch error recovery, and successful creation from empty results with the configured path, real Git branch, single hook execution, exact Rex cwd and restored termios. Evidence: [worktrunk-picker-create.log](verification/worktrunk-picker-create.log).

## Boundary review

`src/worktree.rs` does not invoke Git lifecycle commands or infer paths from branch names. One ordinary `git worktree list --porcelain` query remains in session naming to preserve main/bare/separate-Git-directory naming behavior; it does not discover or manage the worktree source. Direct Git worktree creation elsewhere occurs only in isolated test fixtures. Shared optional `rename --enrich` remains separate from worktree management and still uses gh.

No system dependency was installed or upgraded, no user configuration/worktree/branch was migrated or deleted, and no Git commit, push or publication was made. This run verifies Rex interaction through a fake that records argv and paths; it does not add a new live Rex desktop/app-switch claim. Other operating systems and other wt versions were not exercised; the supported minimum is checked at runtime.
