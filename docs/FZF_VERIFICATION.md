# System-fzf picker verification

The session picker (including the default command) and worktree picker now run the system `fzf` executable. The old native TUI and its crossterm, ratatui and nucleo dependencies have been removed. The minimum supported fzf version is 0.63; this verification uses **fzf 0.74.5 (Homebrew), macOS**.

## Reproduce

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release
python3 scripts/pty_smoke.py --binary target/release/rexue --output docs/verification/fzf-pty.json
```

The checked-in [PTY receipt](verification/fzf-pty.json) records the real fzf version and each case's result. The Rust suite contains 152 passing tests (110 unit tests and 42 process integration tests). These checks cover the backend/configuration/CLI behavior, with additional picker regressions for stable identities across duplicates and refresh, Unicode alias folding with hidden targets, terminal-text sanitization and one-pass window formatting.

## Terminal and integration evidence

The PTY test starts a real system fzf under a controlling Unix terminal. It drives the default entry, `picker`, `pick`, `pk`, and worktree aliases, rather than simulating an fzf result. `XDG_CACHE_HOME` explicitly isolates worktree metadata even on macOS. Rex and GitHub executables are isolated fixtures; Git worktree preparation uses a real temporary local repository. No real Rex sessions, history database, GitHub repositories, installed binaries or user configuration are modified.

The cases cover:

- Initial list, real preview callbacks, a no-match query, resizing, cancellation and restored terminal configuration.
- Default/alias command dispatch; duplicate display names with spaces, Chinese, quotes, shell punctuation and literal `{q}`; selecting the second row returns its distinct full session ID. No injected command is executed. Real fzf selections of literal configuration names `📁 chosen` and ` chosen` must request creation with that exact name and its wanted directory, despite another configuration named `chosen`. The same assertion covers a configured alias excluded by source flags.
- Unicode alias-mode auto-connect; prefilled queries that do not auto-connect; `--no-alias-auto`; numbered result shortcuts.
- Yes/No removal confirmation, declined removal with no callback, confirmed removal exactly once and list refresh. Removing one of two identically named live targets preserves the alias for the remaining ID; removing the last retires the alias and all removed IDs. A unit regression and real PTY regression cover querying the alias after the last removal without auto-connecting.
- Explicitly hidden preview followed by Ctrl+O; SIGTERM sent only to rexue and forwarded to fzf, restoring terminal modes.
- Scoped worktree listing through `--path`, asynchronous cached-to-fresh metadata, manual refresh failure retaining rows, no-match filtering and cancellation.
- Worktree selection through `--repo --refresh`, actual registered Git worktree preparation and connection to its existing fixture Rex session, even when another configuration has an alias equal to the prepared worktree path.
- Missing fzf, unsupported old fzf and an fzf error exit, each producing a clear failure without connection/removal. Personal fzf defaults are deliberately hostile in PTY tests to verify they cannot change the protocol.

Termios checks compare every configurable flag, speed and control character. They exclude only macOS `PENDIN`, a transient kernel state meaning “retype pending input,” which may be set after canonical-mode restoration and clears on input. The initial overly strict equality check reproduced this state-only difference; it did not show raw mode, echo or control-character loss. Rexue also saves/restores incoming terminal configuration and forwards termination signals so fzf can clean up its display.

## Behavior and verification limits

[Fzf keys](../README.md#fzf-picker-keys) and the [configuration reference](CONFIGURATION.md#tui-and-icons) describe the migration. In particular, the visible finder reloads results ranked by `fzf --filter`; alias/index handling shares its query box, so there is no second filtering pass or match highlighting. Fzf supplies editing, clipping and layout. Initial source collection precedes opening fzf; manually reopened pickers retain the query and reset cursor/preview state. Connection and worktree business operations share the existing implementation. Session selection passes its original `Entry` to `connect_entry`, while a prepared worktree passes its known `Path` to `connect_path`; neither re-enters the human-target parser. Synthesized config aliases resolve by exact configuration key to restore their original directory. A live ID that disappears before connection fails rather than creating another session. Text-based `connect` keeps its existing parsing and alias behavior.

This verification establishes the fzf integration and fixture business effects. It does not claim a new live Rex app-switch, GitHub network or browser test. Historical live backend receipts remain in [VERIFICATION.md](VERIFICATION.md); their native TUI screen/cursor assertions no longer describe the current picker. The existing `worktree_smoke.py` terminal probe has been updated for fzf, but that script's live Rex/GitHub workflows are not needed for this isolated acceptance run. Windows and Linux device-level testing remain outside the verified scope.
