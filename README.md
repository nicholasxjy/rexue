# rexue

An **fzf picker for Rex sessions, coding agents and Git worktrees**. Connect to a project once; later visits reuse its session. Verified on macOS.

## Demo

![Git worktree picker](assets/s.png)

## Install

Install and open [Rex](https://www.superlogical.com/rex/docs/installation), then run inside a Rex terminal:

```sh
brew install rust fzf
cargo install --locked rexue
```

Requires Rust **1.90+**, fzf **0.63+** and a C compiler. On macOS, `xcode-select --install` installs missing compiler tools. If `rexue` is not found, add `export PATH="$HOME/.cargo/bin:$PATH"` to `~/.zshrc` and reopen the terminal.

Optional dependencies:

```sh
brew install git worktrunk # Worktrees: wt 0.81.0+.
```

For a local checkout, use `cargo install --locked --path .`.

## Use

```sh
rexue                        # Open the picker.
rexue list                   # List live sessions.
rexue list --agents          # List running agents.
rexue connect ~/Projects/app # Create or reuse a session.
rexue connect app            # Resolve a session name or configured alias.
rexue last                   # Return to the previous session.
```

The picker lists live Rex sessions. Sessions expand into tabs and panes; Enter focuses the selected target. `picker --agents` shows a flat agent list. See [picker settings](docs/CONFIGURATION.md#picker-settings) for titles, icons and initial focus.

**Enter** connects, **Esc** cancels, **Ctrl+J/K** moves, **Ctrl+O** toggles preview, **Ctrl+R** renames a session and **Ctrl+X** requests removal. Customize these in [`[tui.keybindings]`](docs/CONFIGURATION.md#keybindings).

For worktrees, run inside a Git repository or add `--path ~/Projects/app`:

```sh
rexue worktree picker
rexue worktree create feature/widgets
rexue worktree connect feature/widgets
rexue worktree remove feature/widgets --no-delete-branch
```

[Worktrunk](https://worktrunk.dev/config/) controls paths, hooks, approvals and removal policy. Removing a checkout also closes its associated Rex sessions; see [removal behavior](docs/CONFIGURATION.md#worktrees).

Select a Rex app with `--client NAME` when several are connected. Use `rexue <command> --help` for options and `rexue completion --help` for supported shells.

## Configure

Configuration is optional. Save this as `~/.config/rexue/rexue.toml`, or `$XDG_CONFIG_HOME/rexue/rexue.toml` if XDG_CONFIG_HOME is nonempty:

```toml
strict_mode = true

[[session]]
name = "app"
path = "~/Projects/app"
alias = "a"

[tui]
preview = true
```

Run `rexue connect a`. Add `startup_command` to run a command on creation, or `[[worktree]]` with a repository `path` to register it for worktree commands. Configured projects remain available through `connect`; they appear in the session picker once running. Override the file with `-C PATH`. See the [editable example](examples/rexue.toml) and [configuration reference](docs/CONFIGURATION.md).

For shortcuts, merge [Alt+S: floating picker](examples/rex-picker-floating.lua) or [Cmd+Shift+P: new tab](examples/rex-picker.lua) into [Rex's `init.lua`](https://www.superlogical.com/rex/docs/customize/config), then run `rex config check && rex config reload`.

## Development

See [Rust checks](docs/VERIFICATION.md#rust-checks), [interactive acceptance](docs/VERIFICATION.md#interactive-picker-acceptance) and [performance measurements](docs/VERIFICATION.md#startup-and-idle-performance).

[MIT license](LICENSE) · [Third-party notices](THIRD_PARTY_LICENSES)
