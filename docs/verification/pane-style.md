# Pane row display verification

Date: 2026-11-08. Status: **verification complete under the accepted fzf content and layout contract**.

The implementation changes both the session tree's pane rows and the flat agent picker. They now render the pane's terminal title rather than the app/pane-number/tab composite. The original context stays in the search column and identity still uses complete session/window/block IDs. Titles that are empty, whitespace-only or have no visible non-whitespace content after the shared display cleaner fall back to the foreground script, argv0 or executable name. Valid titles retain their internal spaces and `|` separators. Agent status follows the title. Zsh and unknown programs use Rex's default terminal artwork when native images are available.

## Native baseline

The initial baseline was Rex Beta 0.1.0 (1875). Controlled panes in one tab established that OSC titles preserve `|` and spaces, pane renaming does not override an existing terminal title, and empty titles fall back to a program name. An unknown program used the same default terminal image as Zsh. `Blocked` appeared at the right, and the current pane had a separate checkmark.

The running app was updated to 0.1.0 (1880) during verification. The new app's `SettingsIconTerminal` export is byte-for-byte identical to the added `shell.png`. Native title results were less consistent: old panes and some freshly created shell/agent fixtures showed synthesized program names while the server continued returning their OSC titles. A controlled program reporting again after the client attached did appear with its own title. The renderer uses the real terminal title API; these native cache and fallback differences are accepted under the revised goal rather than being emulated.

Images are actual captures, not mockups:

- [Rex 1875 native pane baseline](pane-style/native-1875-light.png)
- [Rexue tree with a selected pane](pane-style/rexue-tree-light.png)
- [Rexue flat agent picker, narrow window, dark theme](pane-style/rexue-agents-dark.png)
- [Rex 1880 native rows with synthesized titles](pane-style/native-1880-light.png)

## Isolated fixtures

A regression for invisible titles first failed on whitespace-only input, then passed after title validation was changed to use the same cleaner as the renderer. It covers whitespace, ANSI-only input, control-only input and zero-width input (including zero-width characters mixed with spaces) in both tree and flat catalogs, through both the native snapshot and legacy CLI paths. A valid title with repeated spaces and `|` retains its original contents.

The final source passed:

- `cargo fmt --check`
- `cargo test`: 162 passed, 1 explicitly ignored
- `cargo clippy --all-targets -- -D warnings`
- `cargo build`
- `python3 scripts/pty_smoke.py --binary target/debug/rexue`: 91 cases passed, including title search and exact pane selection from both entrances
- `python3 scripts/worktree_removal_smoke.py --binary target/debug/rexue`: 8 cases passed
- Focused [process-budget probes](pane-style/process-budgets.json): modern agent discovery used 3 Rex calls and selection used 1; legacy discovery used 56; neither made idle requests, and both restored the terminal. These were one-run debug-binary budget checks, not release timing benchmarks.

The legacy opening budget increased from 52 to 56 because the fixture's four ordinary shell panes now also need their own title. The modern Lua snapshot already includes this metadata and keeps its existing process budget. The documented legacy probe was updated accordingly.

The icon exporter was run against the installed 1880 app bundle into a temporary directory, and its default terminal PNG was compared with the embedded asset. No installed binary was replaced.

## Live evidence and limits

The current debug binary was opened in test-owned floating panes. The session tree showed ordinary terminals, an unknown executable, a long title, a Pi agent, an empty-title fallback and an OSC title under a custom pane label. A selected pane retained its image. The flat agent picker rendered in both light and dark themes and in a narrower floating window. These images and captures verify rendering separately from the automated PTY fixtures.

Earlier spot checks did not establish final focus after the floating picker closed. A dedicated live check now waits for the picker child to exit with `rex wait`, then requires three consecutive observations of the expected complete window/block IDs. All four cases passed: an ordinary pane from the tree, an agent from the tree, an agent from the flat picker in the same tab, and an agent from the flat picker in another tab. Every picker exited with status 0. Both same-tab agent cases also use a whitespace-only OSC title and verify that the visible title falls back to `zsh` in each entrance. The [live focus receipt](pane-style/live-focus.json) records expected IDs and all observations. No focus implementation change was needed.

The live check used a temporary configuration and state directory, test-owned sessions and floating layers, and an explicit macOS client ID so the connected iOS client could not be selected accidentally. The test session was removed and the original session/tab/pane restored.

The following fzf content and layout differences are accepted:

- Terminal font metrics, row spacing and rectangular selection backgrounds differ from the native proportional-font rows and rounded selection background.
- Status text follows the title rather than forming a pill aligned against the right edge. A long title can hide it.
- Elapsed status time and the separate current-pane checkmark are absent.
- Program artwork is rendered inside terminal cells and retains the existing neutral backplates, rather than reproducing every native badge decoration.

The revised goal explicitly retains fzf and accepts these content and layout differences. The implementation keeps real metadata, searchable context and complete target IDs; it does not introduce another rendering layer or dependencies to imitate native decoration. The required automated and live acceptance checks passed.
