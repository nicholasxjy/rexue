# rexue

Manage **Rex sessions and Git worktrees** from the command line and the system **fzf** picker. Choose a running Rex session, a configured project, a directory from zoxide history, or a registered repository's worktree. Connecting creates a session when needed and reuses it on later visits.

The current verified platform is macOS. Worktree discovery and lifecycle use [Worktrunk](https://worktrunk.dev); Rex displays the selected session.

## Install

Install Rust 1.90 or newer and a C compiler, then run:

```sh
cargo install --path .
rexue
```

Interactive pickers require fzf 0.63 or newer. Put `rex` on `PATH` and open Rex before connecting; rexue does not start its server. Install `wt` 0.81.0 or newer and Git for worktrees. zoxide is optional; without it, directory history is empty. Configured session listing works without Rex.

## Sessions

Running `rexue` with no arguments opens the session picker. Enter connects; Esc cancels.

```sh
rexue list
rexue list --rex --config --hide-duplicates
rexue list --config --json
rexue picker --zoxide --query Projects
rexue connect ~/Projects/app
rexue --detach connect ~/Projects/app --name app --json
rexue connect app --command 'nvim'
rexue preview app
rexue preview app --watch --interval 1s
rexue rename app --to editor
rexue last
```

Default listing combines running sessions, configured sessions, zoxide history and worktrees from `[[worktree]]` registrations. `--rex`, `--config` and `--zoxide` select a union of those sources. `--hide-attached` hides the invoking live session; `--hide-duplicates` prioritizes live sessions without merging distinct live IDs. `--no-cache` bypasses the optional five-second directory-history cache.

`connect` accepts a session name or full ID, configured alias, directory, or zoxide query. `--root` resolves directory inputs to their Git root before connecting. Names use the Git repository and checkout path by default; explicit names can be set with `--name`. `--command` overrides startup only for a newly created session. Reconnecting never reruns startup. `last` switches to the selected client's previous running session.

`list` supports `--json`, `--icons`, `--no-color`, `--blacklisted`, `--icons-exclude CSV`, and `--format`. Format fields are `{name}` (decorated), `{session}` (raw name), `{source}`, `{path}`, and `{fg:COLOR}…{/fg}`. JSON and custom formatting are mutually exclusive. Normal names and full session IDs can be passed to `connect` or `preview`; arbitrary custom formatted rows are display output.

## Select a Rex client

Session creation and displaying that session are separate operations. With one connected Rex app, rexue selects it automatically. With several apps, specify a client label or full ID:

```sh
rexue --client my-mac connect app
REX_CLIENT=my-mac rexue picker
rexue --detach connect app
```

Client IDs can also be selected by an unambiguous suffix of at least four characters. The selected client must support `session.select`. `--detach` creates or reuses a session without switching the app. `last` always switches.

Global options are `-C/--config-file PATH`, `--client CLIENT` (`REX_CLIENT`), `-S/--server SERVER` (`REX_SERVER`), `--rex-bin PATH` (`REXUE_REX`), and `--detach`. Use `--help` on any command for its full options.

## Worktrees

Worktrunk owns checkout paths, branch defaults, hooks, approvals and removal protections. Run inside a checkout or supply `--path REPO`:

```sh
rexue worktree create feature/widgets --path ~/Projects/app
rexue worktree create feature/widgets --base main --path ~/Projects/app
rexue worktree list --path ~/Projects/app --json
rexue worktree connect feature/widgets --path ~/Projects/app
rexue worktree connect --path ~/Projects/app
rexue worktree picker --path ~/Projects/app
rexue worktree remove feature/widgets --path ~/Projects/app --no-delete-branch
```

`create` creates the checkout without opening Rex. `connect` accepts a branch or path; without a target it connects the current registered checkout. `list` includes detached worktrees. `remove` uses Worktrunk protections and keeps any Rex session. `--force`, `--force-delete` and `--no-delete-branch` control removal. Set `REXUE_WT` to override the `wt` executable.

Repository-scoped commands need no rexue registration. Optional `[[worktree]]` registrations add repositories' worktrees to the main session list and picker. Configure Worktrunk itself at [worktrunk.dev/config](https://worktrunk.dev/config/).

## Picker controls

| Key | Action |
| --- | --- |
| Enter | Connect the highlighted session or worktree. |
| Esc / Ctrl+C | Cancel. |
| Up / Ctrl+K / Ctrl+P | Move up. |
| Down / Ctrl+J / Ctrl+N | Move down. |
| PageUp / PageDown | Move a page. |
| Ctrl+U / Ctrl+D | Move half a page. |
| Ctrl+O | Toggle preview. |
| Ctrl+X | Confirm removal of a live session, history entry or worktree. Configured sessions cannot be removed. |
| Ctrl+R | Rename a live session in the session picker; reload state in the worktree picker. |
| Ctrl+W | Worktree picker: prompt for a branch, create its checkout, and connect. |

Rename starts with the current name; Enter saves and Esc cancels. Removal confirmation initially selects Yes; Enter/Y confirms, N/q/Esc cancels, and arrows or Tab move between choices. Refresh preserves the search query; cursor and preview toggles return to configured defaults.

In the session picker, `/` filters configured aliases. Exact aliases connect after `alias_auto_connect_delay`; outside alias mode this also requires `alias_auto_connect = true`. `--no-alias-auto` disables automatic selection. Prefilled queries do not arm it. A leading `#` enables result selection with 1–9; an alias prefix of `#` takes priority. Worktree rows show branch and checkout path.

Both pickers use the installed fzf. Rexue clears inherited `FZF_DEFAULT_OPTS`, `FZF_DEFAULT_OPTS_FILE` and `FZF_DEFAULT_COMMAND` for child processes to preserve its bindings and selection protocol. Preview captures on selection changes; it does not continuously watch terminal output. The standalone `preview --watch` command watches live session capture.

Traditional keyboard input, CSI-u/Kitty text and Ctrl-key reports, and xterm modifyOtherKeys reports are supported. Rexue normalizes extended reports for fzf so bindings such as Ctrl+J/K work with extended keyboard reporting. Key releases do not repeat actions, and bracketed paste is passed through unchanged.

## Configuration and shell integration

The default config is `$XDG_CONFIG_HOME/rexue/rexue.toml`, or `~/.config/rexue/rexue.toml`. A missing default file is fine; an explicit config path must exist. See [the example](examples/rexue.toml) and [configuration reference](docs/CONFIGURATION.md).

```toml
strict_mode = true

[[session]]
name = "app"
path = "~/Projects/app"
alias = "a"
startup_command = "nvim"

[tui]
preview = true
```

Generate completions with `rexue completion bash`, `zsh`, `fish`, `elvish` or `powershell`. For example:

```sh
rexue completion fish | source
```

[examples/rex-picker.lua](examples/rex-picker.lua) binds Cmd+Shift+P in Rex and passes the invoking client's full ID. Merge it into your Rex Lua configuration, then use Rex's configuration check and reload commands.

## Development and license

Run `cargo fmt --check`, `cargo test` and `cargo clippy --all-targets -- -D warnings`. [Verification](docs/VERIFICATION.md) describes the isolated picker and worktree checks and their limits.

MIT licensed; see [LICENSE](LICENSE). Required third-party copyright and license notices are retained in [THIRD_PARTY_LICENSES](THIRD_PARTY_LICENSES).
