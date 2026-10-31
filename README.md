# rexue

An **fzf session picker for [Rex](https://www.superlogical.com/rex)**, inspired by [Sesh](https://github.com/joshmedeski/sesh). Find running sessions, named projects, and frequently visited directories in one fuzzy list; select a project to create or reuse its Rex session. Both session and worktree pickers directly use the system `fzf` executable.

**macOS is the current verified platform.** The Rust implementation includes fzf session/worktree pickers and the Rex workflows described below. See [verification evidence](docs/VERIFICATION.md), [mapping decisions](docs/DECISIONS.md), and the [pinned upstream parity inventory](docs/PARITY.md) for the exact scope and platform limits.

## Install and start

Install Rust **1.90 or newer**, a C compiler (provided by Xcode Command Line Tools on macOS), **fzf 0.63 or newer** (`brew install fzf` on macOS), and Rex; make the `rex` CLI available on `PATH`. The C compiler builds the bundled shell parser; running rexue requires no Go or JavaScript runtime. Open Rex before using the picker: rexue deliberately does not autostart its server.

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

**No arguments opens the fzf session picker.** Type to filter, use Up/Down to select, and press Enter to connect. Esc cancels. With one connected Rex app, selection switches that app to the chosen session.

Other tools are optional, depending on your workflow:

| Tool | Used for |
| --- | --- |
| fzf ≥ 0.63 | Required for both interactive pickers; `list`, `connect` and other noninteractive commands work independently. |
| Rex | Required for live sessions, creation, preview capture, and app switching. Config-only listing can run independently. |
| zoxide | Directory history and frecency; absent zoxide yields an empty history source. Custom frecency commands can replace it. |
| git | Git naming/root detection, Git clone, and worktrees. |
| gh | Optional GitHub title enrichment via `rename --enrich`; authenticate separately with `gh auth login`. Generic worktree management does not use gh. |
| jj | `clone --vcs jj`; workspace naming also reads `.jj/repo` metadata without requiring a running JJ process. |
| wt ≥ 0.81.0 | [Worktrunk](https://worktrunk.dev) manages worktree paths, configuration, hooks and lifecycle. Override its executable with `REXUE_WT` when needed. |
| Nerd Font | Renders the source icons shown by default; use `--icons=false` or `[tui] show_icons = false` for plain text. |

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

## fzf picker keys

Both pickers use system fzf for the terminal interface. Rexue keeps source collection, aliases and connection logic in Rust; fuzzy ranking uses `fzf --filter` with case-insensitive matching and stable ties. The visible finder receives the ranked results through `reload`, so it does not add a second filtering pass or draw fuzzy-match highlights. Initial source collection finishes before fzf opens. No native TUI fallback is bundled.

| Key | Action |
| --- | --- |
| Enter | Connect the highlighted row. |
| Esc / Ctrl+C | Cancel without connecting. |
| Up / Ctrl+K / Ctrl+P | Move up one row. |
| Down / Ctrl+J / Ctrl+N | Move down one row. |
| PageUp / PageDown | Move one page. |
| Ctrl+U / Ctrl+D | Move half a page. |
| Ctrl+W | Worktree picker: enter a new branch name, create its worktree with Worktrunk, then connect Rex. Esc cancels the input. |
| Ctrl+O | Toggle preview in either picker. |
| Ctrl+X | Confirm killing a live Rex session, removing a history row, or safely removing a Worktrunk checkout. Configured sessions cannot be removed. |
| Ctrl+R | Session picker: rename the highlighted live session. Worktree picker: reload Worktrunk state. Both retain the query and report errors in the header. |
| Left/Right, Home/End, Backspace/Delete | Use fzf's query editing. |

The rename prompt starts with the current session name. Edit it and press Enter to save, or Esc/Ctrl+C to cancel. Empty or whitespace-only names are rejected. Configured sessions, directories and pane rows cannot be renamed here. Saving refreshes the list without connecting to the session.

The confirmation initially selects **Yes**. Enter accepts the selected choice, Y confirms, and N/q/Esc cancels. Up/Down, Left/Right and Tab/Shift+Tab move between choices. Canceling confirmation returns to the picker; deletion refreshes the list. A refreshed/reopened picker retains its query; cursor position and preview toggles return to their configured defaults.

A leading `/` filters configured aliases (change `alias_filter_prefix` to customize it). Exact alias matching uses Unicode case folding. An exact alias in alias mode connects after `alias_auto_connect_delay`; outside that mode it also requires `alias_auto_connect = true`. `--no-alias-auto` disables both. A prefilled `--query` does not arm auto-connect. `#` enters index mode: filter by name, then press 1–9 to select that result directly; if `#` is the alias prefix, alias mode takes priority.

The worktree picker shows branch names and actual checkout paths, including detached worktrees, in branch/path order. Ctrl+W opens a branch-name prompt, creates with Worktrunk configuration and connects the new checkout; it also works when the current search has no matches. Empty names and creation errors return to the picker with a diagnostic. Ctrl+O toggles preview, Ctrl+R reloads local Worktrunk state, and Ctrl+X offers confirmed removal with Worktrunk's normal protections. Aliases and numbered shortcuts remain session-picker features. Selecting a worktree uses its returned path to connect Rex.

Existing picker CLI options and `[tui]` configuration remain available, including icons, window labels, prompt, placeholder, pointer and preview sizing. Source-group separators appear on their own line above the first item in each group; they do not add selectable entries. Preview border styles map to fzf equivalents. Fzf owns query editing, cursor appearance, clipping, layout and preview cancellation; the old native palette, precise dialog geometry and input caret behavior are no longer used. See [configuration](docs/CONFIGURATION.md#tui-and-icons) for details.

Rexue clears `FZF_DEFAULT_OPTS`, `FZF_DEFAULT_OPTS_FILE` and `FZF_DEFAULT_COMMAND` for its child processes so personal fzf defaults cannot change the selection protocol or run unexpected bindings. It creates a private temporary callback socket and removes it on exit. Display text is kept separate from row IDs, so duplicate names, ANSI decoration, spaces and shell punctuation do not change which target is selected. Missing/old fzf and abnormal exit statuses are reported explicitly.

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
| `worktree` (`wt`) `create` | `BRANCH`; `-b/--base REF`, `--path REPO`, `--json`. Creates a branch/checkout without opening Rex. |
| `worktree` (`wt`) `connect` (`c`) | `[BRANCH_OR_PATH]`; `--path REPO`, `--name NAME`, `--json`. Omitted target connects the current worktree. |
| `worktree` (`wt`) `list` (`l`, `ls`) | `--path REPO`, `-j/--json`. Lists all registered checkouts, including detached ones. |
| `worktree` (`wt`) `picker` (`pick`, `pk`, `p`) | `--path REPO`, `--refresh`, `-i/--icons`, `-p/--prompt TEXT`, `--placeholder TEXT`, `-q/--query TEXT`. |
| `worktree` (`wt`) `remove` (`rm`, `r`) | `BRANCH_OR_PATH`; `--path REPO`, `--force`, `-D/--force-delete`, `--no-delete-branch`, `--json`. Keeps Rex sessions. |
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
rexue worktree create feature/widgets --path ~/Projects/app
rexue worktree connect feature/widgets --path ~/Projects/app
rexue worktree picker --path ~/Projects/app
rexue worktree remove feature/widgets --path ~/Projects/app --no-delete-branch
```

`preview --watch` repeatedly captures a live session, repainting only changed nonempty output; Ctrl+C stops it. Directory/config previews render once even with `--watch`. The picker's preview pane is debounced on selection changes and keeps the last displayed preview until its replacement arrives. It does not continuously watch terminal output.

Worktrees use the external Worktrunk CLI. No GitHub account or `[[worktree]]` registration is needed for repository-scoped commands; use `--path` or run inside a checkout. Optional `[[worktree]]` path/icon registrations add repository worktrees to the main list and picker. Worktrunk owns directory templates, defaults, hooks and removal policy; its configuration and approval behavior remain active. Old issue/PR numbering, browser flows and rexue worktree lifecycle settings were removed; see [migration](docs/CONFIGURATION.md#worktrunk-worktrees).

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
