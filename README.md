# rexue

A native terminal **session picker for [Rex](https://www.superlogical.com/rex)**, inspired by [Sesh](https://github.com/joshmedeski/sesh). Find running sessions, named projects, and frequently visited directories in one fuzzy list; select a project to create or reuse its Rex session. The picker runs in your terminal and needs no fzf or external launcher.

**macOS is the current verified platform.** The Rust port includes native session/worktree pickers and the Rex workflows described below. See [verification evidence](docs/VERIFICATION.md), [native mapping decisions](docs/DECISIONS.md), and the [pinned upstream parity inventory](docs/PARITY.md) for the exact scope and platform limits.

## Install and start

Install Rust **1.88 or newer**, a C compiler (provided by Xcode Command Line Tools on macOS), and Rex; make the `rex` CLI available on `PATH`. The C compiler builds the bundled shell parser; running rexue requires no Go or JavaScript runtime. Open Rex before using the picker: rexue deliberately does not autostart its server.

From this checkout:

```sh
cargo install --path .
rexue
```

Or run directly from source:

```sh
cargo run --release
# Pass options after Cargo's --:
cargo run --release -- picker --preview
```

**No arguments opens the native picker.** Type to filter, use Up/Down to select, and press Enter to connect. Esc cancels. With one connected Rex app, selection switches that app to the chosen session.

Other tools are optional, depending on your workflow:

| Tool | Used for |
| --- | --- |
| Rex | Required for live sessions, creation, preview capture, and app switching. Config-only listing can run independently. |
| zoxide | Directory history and frecency; absent zoxide yields an empty history source. Custom frecency commands can replace it. |
| git | Git naming/root detection, Git clone, and worktrees. |
| gh | Authenticated GitHub issue/PR worktrees, titles, and enriched names; authenticate separately with `gh auth login`. |
| jj | `clone --vcs jj`; workspace naming also reads `.jj/repo` metadata without requiring a running JJ process. |
| Browser | Opening worktree issue/PR URLs; macOS active-tab lookup also uses `osascript` and a configured browser. |
| Nerd Font | Optional source glyphs and alias chip caps; normal text mode works without one. |

## Choose the Rex app

A Rex **window is a tab**, a **block is a pane**, and a **client is a connected app**. Creating a server session and displaying it in an app are separate operations.

```sh
rexue doctor
rexue --client my-mac
rexue --client 'client:FULL_ID' connect ~/Projects/app
REX_CLIENT=my-mac rexue picker
rexue --detach connect ~/Projects/app
```

If multiple apps are connected, choose `--client` or set `REX_CLIENT`; rexue errors instead of choosing arbitrarily. Selection accepts a full ID, a case-insensitive label, or an unambiguous ID suffix of at least four characters. `doctor` prints connection context, connected app clients, and the state directory. The selected app must advertise `session.select`.

`--detach` creates/reuses a session without switching an app. It applies to connection workflows; `last` always switches to a recorded previous session. Window background creation also accepts `--background`.

Global options are `-C/--config-file PATH`, `--client CLIENT` (`REX_CLIENT`), `-S/--server SERVER` (`REX_SERVER`), `--rex-bin PATH` (`REXUE_REX`, default `rex`), and `--detach`. `-h/--help` and `-V/--version` are available. Server selection is exposed, but multihost orchestration is outside the current target.

## Projects, sources, and startup

Configuration defaults to `$XDG_CONFIG_HOME/rexue/rexue.toml`, or `~/.config/rexue/rexue.toml`. A missing default file is fine. Start with the [example TOML](examples/rexue.toml) and the [configuration reference](docs/CONFIGURATION.md).

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

Startup commands and configured windows run **once when a new Rex session is created**. Reconnecting does not rerun them. A nonempty `connect --command` overrides configured startup; empty commands and window lists fall through to wildcard/default strategies. Startup scripts stay paired with their layer. See the [configuration reference](docs/CONFIGURATION.md#startup-and-windows) for the source-specific disable guards and precedence.

The default list combines live Rex sessions, configured sessions, then zoxide directories. Source flags select a union:

```sh
rexue list
rexue list --rex --config --hide-duplicates
rexue picker --zoxide --query Projects
rexue -C ./projects.toml list --config
rexue picker --rex --hide-attached --no-cache
```

`-t/--rex` (legacy alias `--tmux`), `-c/--config`, and `-z/--zoxide` select sources. `-H/--hide-attached` hides the calling attached live row; `-d/--hide-duplicates` gives live sessions priority over configured and directory rows while retaining distinct live sessions. **`-C/--config-file` chooses a TOML file; `list/picker -c/--config` selects configured session rows.** The legacy `--tmux` spelling still selects Rex resources.

Pane rows also round-trip through external pickers: `rexue list --panes` names can be passed directly to `rexue preview NAME` or `rexue connect NAME`. Combine `--panes` with `--config`, `--rex`, or `--zoxide`; sorting and blacklist rules apply to the combined list. Duplicate pane names require the full `block:` ID from JSON.

## Native picker keys

These are the current native bindings. The [parity notes](docs/PARITY.md) record upstream Sesh's exact bindings; native mapping differences are listed in [DECISIONS](docs/DECISIONS.md).

| Key | Action |
| --- | --- |
| Enter | Connect highlighted row; ignored during loading; empty results exit. |
| Esc / Ctrl+C | Cancel. |
| Up / Ctrl+K / Ctrl+P | Move up one row. |
| Down / Ctrl+J / Ctrl+N | Move down one row. |
| PageUp / PageDown | Move one visible page up/down. |
| Ctrl+U / Ctrl+D | Move half the visible rows up/down; preserve the filter. |
| Ctrl+O | Session picker: toggle preview; hidden in terminals narrower than the configured minimum. |
| Ctrl+R | Worktree picker: reload entries; inert during an active refresh. |
| Ctrl+X | Session picker: confirm killing a live session or removing a zoxide entry. Configured rows cannot be removed here. |
| Left/Ctrl+B, Right/Ctrl+F; Home/Ctrl+A; End/Ctrl+E | Edit query caret position. |
| Backspace / Delete | Delete previous/next character. |

Movement clamps at the ends. Tab does not navigate list rows. In the removal dialog, **Yes is selected initially**: Enter applies it, Y/y confirms, N/n/q/Esc cancels; Left/Right/Tab/Shift+Tab/h/l toggles the button. Ctrl+C exits the picker.

A leading `/` filters aliases (configurable with `alias_filter_prefix`); `#` enters index mode unless used as the alias prefix. Exact aliases match case-insensitively. Normal-mode timed auto-connect requires `alias_auto_connect = true`; an exact alias in alias mode auto-connects after the configured delay regardless of that opt-in. `--no-alias-auto` disables this. A prefilled `--query` does not trigger auto-connect by itself.

The worktree picker uses a full-width number/state/title layout, with numeric ordering, red fuzzy-match highlights, and OPEN/CLOSED/MERGED badges. It supports Ctrl+R refresh; session aliases, index mode, preview, and removal are disabled. Remaining refresh and editing differences are tracked in [DECISIONS](docs/DECISIONS.md).

Enable icons with `picker --icons`, `list --icons`, or `[tui] show_icons = true`. Select a Nerd Font in Rex if glyphs appear as boxes; leave icons off for ASCII alias chips. `list --no-color` removes ANSI color while preserving enabled glyphs. TUI settings include window labels, prompt, placeholder, pointer, source glyphs, group separators, and preview sizing; see [configuration](docs/CONFIGURATION.md).

## Command reference

Every command supports `--help`. Aliases below are accepted spellings.

| Command (aliases) | Arguments and local options |
| --- | --- |
| `list` (`l`) | Source flags above; `-j/--json`, `-i/--icons`, `-n/--no-color`, `-p/--panes`, `-b/--blacklisted`, `--format STRING`, `--icons-exclude CSV`. |
| `connect` (`cn`) | `TARGET…` joined with spaces; `-s/--switch`, `-c/--command SCRIPT`, `-r/--root`, `--name NAME`, `--json`. |
| `picker` (`pick`, `pk`) | Source flags; `-i/--icons`, `-s/--separator-aware`, `-p/--prompt TEXT`, `--placeholder TEXT`, `-q/--query TEXT`, `--no-alias-auto`, `--preview`, `--no-preview`. |
| `root` (`r`) | `[TARGET]`; print the root **name**: original-path substitution, then Git/JJ root naming, then ordinary directory naming. |
| `preview` (`p`) | `TARGET`; `-w/--watch`, `--interval DURATION` (default `500ms`, must be positive). |
| `last` (`L`) | Switch to previous live session recorded for the selected client. |
| `clone` (`cl`) | `REPO [-- CLONE_FLAGS…]`; `-c/--cmdDir` (also `--cmd-dir`) `PARENT`, `-d/--dir NAME`, `--vcs git\|jj`. |
| `mkdir` (`md`) | `PATH`; `-s/--switch`, `-c/--command SCRIPT`, `--json`. Creates parents then connects. |
| `window` (`w`) `list` (`l`, `ls`) | `-t/--target SESSION` (also `--session`), `-j/--json`. |
| `window` (`w`) `connect` (`cn`, `c`) | `[NAME]`; `-t/--target SESSION` (also `--session`), `-c/--command SCRIPT`, `-p/--path DIR`, `-b/--background`, `-s/--switch`, `--new`. |
| `worktree` (`wt`) `connect` (`c`) | `[NUMBER]`; `-r/--repo OWNER/REPO`, `--path DIR`, `-p/--pr`, `-s/--switch`, `-b/--browser`, `--json`. |
| `worktree` (`wt`) `list` (`l`, `ls`) | `-r/--repo OWNER/REPO`, `--path DIR`, `-j/--json`, `--refresh`. |
| `worktree` (`wt`) `picker` (`pick`, `pk`, `p`) | `-r/--repo OWNER/REPO`, `--path DIR`, `--refresh`, `-s/--switch`, `-i/--icons`, `-p/--prompt TEXT`, `--placeholder TEXT`, `-q/--query TEXT`. |
| `worktree` (`wt`) `browse` (`b`) | `-p/--pr`, `--print`. Uses current working directory inside a configured numbered worktree. |
| `rename` | `[TARGET]`; `--enrich` or `--to NAME`. Without either, no-op. |
| `cache refresh` | Invalidate and synchronously reload the list; see cache scope below. |
| `completion` | `bash`, `elvish`, `fish`, `powershell`, or `zsh`; print a shell script. |
| `doctor` | Print JSON context, connected clients, and state directory. |
| `help` | `[COMMAND…]`; generated help. |

`--switch` is accepted for Sesh compatibility: Rex connection workflows already select the chosen app unless detached/backgrounded. `window connect` reuses a matching window unless `--new`; an explicit command on an existing window is sent to its focused terminal, which may be running an interactive application. Use a new window when you need a new process.

`connect --root` optionally replaces an input directory with its detected Git/JJ root before normal target resolution. If no root is found, including when optional Git is unavailable, it keeps the original target. Named configurations, aliases, live session IDs and panes retain their normal resolution when they are not directory inputs.

Plain connection output is a session ID; window connection output is a window ID. JSON uses Rust `snake_case` fields, **not Sesh's Go JSON schema**. `list --json` and `--format` conflict. Format fields: `{name}` (decorated), `{session}` (raw), `{source}`, `{path}`, `{active_window_name}`, `{active_window_name_prefix}`, and `{fg:COLOR}…{/fg}`. Unknown placeholders are errors. Arbitrary formatted rows are not connect targets.

```sh
rexue connect app
rexue --detach connect ~/Projects/app --name app --json
rexue list --format '{source} {session} {path}' --no-color
rexue preview app
rexue preview app --watch --interval 1s
rexue window connect editor --target app --path ~/Projects/app
rexue clone owner/repo --cmd-dir ~/Projects
rexue worktree connect 42 --repo owner/repo
rexue worktree connect 43 --repo owner/repo --pr
rexue worktree connect --browser
rexue worktree browse --print
```

`preview --watch` repeatedly captures a live session, repainting only changed nonempty output; Ctrl+C stops it. Directory/config previews render once even with `--watch`. The picker's preview pane is debounced on selection changes and keeps the last displayed preview until its replacement arrives. It does not continuously watch terminal output.

GitHub workflows require a configured `[[worktree]]`. Own PRs prefer a linked issue number; other PRs use detached checkouts. Metadata errors stop creation. The worktree's `create_command` is chosen for a newly created worktree, `startup_command` for an existing one; neither reruns when its Rex session already exists. Listing can retain cached titles when GitHub is unavailable. See [configuration](docs/CONFIGURATION.md) for browser and cache details.

## Shell completion

Generate completion scripts with rexue itself:

```sh
# Bash: load for this shell
source <(rexue completion bash)

# Zsh: load for this shell after compinit
source <(rexue completion zsh)

# Fish: load for this shell
rexue completion fish | source
```

For persistence, save the output to your shell's completion directory, for example `rexue completion fish > ~/.config/fish/completions/rexue.fish`. These are generated CLI completions, not dynamic project/session suggestions.

## Rex key binding

[examples/rex-picker.lua](examples/rex-picker.lua) defines an official Lua custom action and binds Cmd+Shift+P to open the picker in a new Rex terminal tab. It passes the invoking client's full ID to rexue so multiple apps remain unambiguous. Ensure `rexue` is on that terminal's PATH, or replace its executable with an absolute path.

The sample's **configuration syntax was tested** with:

```sh
rex --autostart=false config check examples/rex-picker.lua --json
```

The installed CLI reported one binding, one action, and no diagnostics. This validates loading; it does not establish a live keypress/action test. To install it yourself, merge the fragment into Rex's `init.lua` or place it beside `init.lua` and `require` it, then run `rex config check` and `rex config reload`. Rex's Lua configuration is separate from rexue's TOML.

API references: [configuration](https://www.superlogical.com/rex/docs/customize/config.md), [keyboard shortcuts](https://www.superlogical.com/rex/docs/customize/keyboard-shortcuts.md), and [Lua API](https://www.superlogical.com/rex/docs/reference/lua.md).

## Attribution and scope

rexue is licensed under the [MIT License](LICENSE), copyright **2026 jy**. It is inspired by **Sesh by joshmedeski and contributors**, also [MIT licensed](https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/LICENSE). Its session lifecycle, configuration, picker, and worktree behavior draw on that project's implementation; the pinned reference and detailed attribution links are in [PARITY.md](docs/PARITY.md). Preserve upstream copyright and license notices with any substantial copied logic.

The goal is a Rex session picker with familiar Sesh workflows. tmux protocols/backends, tmuxp/tmuxinator/tmuxifier plugin compatibility, external GUI launchers, Windows, and multihost orchestration are excluded. Native mappings and explicit validation errors are intentional; other missing behaviors remain gaps, not claims of completed parity.
