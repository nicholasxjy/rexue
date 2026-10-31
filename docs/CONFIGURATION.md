# Configuration

rexue reads TOML from `$XDG_CONFIG_HOME/rexue/rexue.toml`, falling back to `~/.config/rexue/rexue.toml`. A missing default file uses defaults; an explicitly selected missing file is an error.

```sh
rexue -C ./projects.toml picker
rexue --config-file ./projects.toml list --config
```

Business boolean flags accept bare flags and explicit Go boolean values such as `--icons=false`. Picker `--icons=false`, `--preview=false`, and `--separator-aware=false` override enabled configuration values; `--no-preview=false` leaves preview enabled.

CLI text flags retain the pinned source's presence rules: explicitly empty picker `--prompt`/`--placeholder` remain empty, and `list --format=` emits empty rows and conflicts with enabled JSON. Empty optional clone parent/directory/VCS and worktree path selectors use their normal fallbacks. Empty window target/path/command and connect/mkdir command values also fall back; an explicitly empty window `--target` still takes precedence over `--session` before selecting the current context. Nonempty values, whitespace and raw trailing Git arguments are preserved. These CLI rules are distinct from configuration inheritance below.

`-C/--config-file` selects the file. `list/picker -c/--config` selects configured session rows. Rex's own `rex -C` selects an app client, so these flags must not be forwarded between tools.

See [the example](../examples/rexue.toml), the [command reference](../README.md#command-reference), and [mapping decisions](DECISIONS.md). This document describes the Rust implementation; [PARITY.md](PARITY.md) describes the upstream contract and acceptance work.

## Validation and paths

Unknown fields produce a diagnostic and are ignored by default. Set root `strict_mode = true` to reject them in the primary file. Imported files use permissive field handling, matching the pinned sesh loader. Wrong types and invalid known values are always errors. tmux-specific keys (`tmux*`) are rejected with migration guidance even in permissive mode; the documented `icons.tmux` / `sort_order=["tmux"]` source aliases migrate to `rex`, and `icons.tmux-pane` migrates to the independent `rex-block` source before this check.

Paths expand `~`, `$VARIABLE`, and `${VARIABLE}`. Missing environment variables and `~otheruser` are errors. Empty paths are errors except where a documented default or window inheritance applies. Expansion never executes shell substitutions. Raw configuration strings expand once; characters such as `$` introduced by an environment variable value remain literal path data. Already-resolved paths are normalized without interpreting variables again. Invalid configured entries produce contextual errors rather than disappearing from the list. Relative session paths resolve against **rexue's working directory**. Configured window paths resolve relative to the session directory; relative startup-script filenames resolve against the corresponding terminal directory. Ordinary paths do not resolve relative to the TOML file. Import paths have different semantics below. Matching normalizes paths lexically and session reuse resolves symlinks when possible.

Duplicate session/window names, case-insensitive duplicate aliases, duplicate worktree repository paths, invalid values and references to undefined windows are errors. Retired worktree settings and `[browser]` are errors in both strict and permissive mode; see the migration notes below.

## Imports

```toml
strict_mode = true
import = ["projects/work.toml", "${HOME}/shared/rex.toml"]
cache = false

[default_session]
startup_command = ""
```

Imports match the pinned sesh loader: environment/home expansion applies to exact file paths, with relative paths resolved from the **process working directory**. Glob characters are literal filename characters. Unreadable files and malformed TOML report their import context.

Only `session`, `window`, and `wildcard` arrays append, in declaration order, after the primary file's entries. Imported defaults, TUI options, worktrees, icons, and other root settings do not merge. Nested imports are ignored, so import cycles do not recurse. Unknown imported fields remain permissive even when the primary file enables strict mode; invalid types in recognized imported entries still fail.

Repeated imports append repeatedly rather than deduplicating. Final validation still diagnoses duplicate identities and invalid references. This preserves the existing sesh configuration import semantics.

## Root options

| Key | Default | Meaning |
| --- | --- | --- |
| `strict_mode` | `false` | Turn unknown-field diagnostics into errors. |
| `import` | `[]` | Exact TOML file paths; semantics above. |
| `cache` | `false` | Cache frecency rows for five seconds; live Rex/config rows remain fresh. |
| `terminal` | `""` | Optional macOS application name or absolute `.app` path, e.g. `"Rex Beta"`; activate it after a successful foreground session/pane/window connection. Empty or non-macOS skips activation. Detached/background connections skip it; activation failure warns without undoing the connection. |
| `blacklist` | `[]` | Regex patterns matched against entry names; malformed regex patterns are ignored. |
| `sort_order` | `[]` | Source names or nested arrays of sources to interleave by score. |
| `dir_length` | `1` | Components retained for directory fallback naming. |
| `git_dir_length` | `1` | Components retained in the Git repository anchor. |
| `git_namer_use_worktree_root` | `false` | Anchor Git naming at the selected worktree instead of the main checkout. |
| `separator_aware` | `false` | Normalize `-`, `_`, `/`, and `\` to spaces during fuzzy matching. |
| `vcs` | `"git"` | Default clone VCS; `git` or `jj`. Workspace-aware naming also reads `.jj/repo` metadata; an actual JJ executable is needed only for cloning. |

Default source order is `rex`, `config`, `zoxide`. Source selection flags form a union and do not change their order. For example:

```toml
sort_order = ["rex", ["config", "zoxide"]]
blacklist = ["^scratch-", "^/private/"]
```

Nested source groups rank entries by frecency score; matching paths borrow a zoxide score and ties retain their previous order. Unmentioned sources append in default order. `rex-block` is selected by `list --panes` and unions with other explicit source flags; it participates in sorting, blacklist and duplicate filtering. `directory` and `worktree` are recognized names but are not separate default session catalogs. Plain pane names (including source-icon decorated rows) can be passed back to preview/connect; duplicate pane names require full block IDs.

`list --blacklisted` returns only matching rows instead of the normal list. `--hide-attached` hides the calling context's live session before duplicate filtering. Duplicate filtering uses fixed live/config/frecency priority independently of display sorting: live names suppress matching configs, and retained live/config paths suppress frecency rows. Distinct live sessions and pane rows remain visible.

## Sessions, defaults, and wildcards

Wildcard patterns follow Go `path.Match` syntax after home/environment expansion. `*` and `?` do not cross `/`; `[^x]` negates a character class, while `[!x]` matches a literal `!` or `x`. Backslash escapes metacharacters. Repeated stars in the middle stay within one path component. The Sesh extension at the end, `/**`, matches descendants of its literal prefix and excludes the prefix directory itself; wildcard characters in that prefix remain literal. The same first matching block supplies startup, windows, preview and icon inheritance.

```toml
[default_session]
startup_command = "printf 'Opening %s\\n' {}"
preview_command = "ls -la {}"
windows = ["editor"]

[[window]]
name = "editor"
startup_command = "nvim"

[[wildcard]]
pattern = "~/Projects/**"
startup_command = "git status --short"
icon = "P"

[[session]]
name = "app"
path = "~/Projects/app"
alias = "a"
alias_auto_connect = false
startup_command = ""
windows = []
```

An explicit session matches by name first, then expanded path; only the first matching wildcard participates. Window lists, preview commands and icons use the first **nonempty explicit session → matching wildcard → default** value. Empty strings and `windows = []` continue fallback instead of clearing inherited settings. The example therefore inherits the `editor` window, matching wildcard startup/icon, and default preview. Aliases and alias auto-connect remain local to explicit sessions. Boolean guards follow their specific startup/UI strategies rather than a universal overlay rule.

| Location | Accepted keys |
| --- | --- |
| `[default_session]`, `[[session]]` | `name`, `path`, `startup_command`, `startup_script`, `preview_command`, `disable_startup_command`, `windows`, `icon`, `alias`, `alias_auto_connect`. Explicit sessions require nonempty name/path. |
| `[[wildcard]]` | `pattern`, `startup_command`, `startup_script`, `preview_command`, `disable_startup_command`, `windows`, `icon`. |
| `[[window]]` | `name`, optional `path`, `startup_command`, `startup_script`. Names must be unique and nonempty. |

All boolean fields default false; strings/optional commands have no value unless supplied; window lists default empty. Aliases are case-insensitive in the picker. Prefer distinct aliases without whitespace; duplicates differing only in case are rejected. `alias_auto_connect` opts into timed normal-mode selection. Alias-mode exact matching has its own auto-connect rule described in the [README](../README.md#native-picker-keys).

Wildcard patterns support `*`, `?`, character classes, and recursive `**`. `~/Projects/*` matches direct children; a trailing `~/Projects/**` matches descendants at any depth, excludes `~/Projects` itself, and does not match `~/Projects-other`.

### Startup and windows

Session startup and configured windows execute only when a **new Rex session** is created. Attaching to an existing session runs neither. A nonempty CLI `connect --command` wins; an empty override continues to configured strategies. Those try nonempty explicit-session startup, then an enabled first-matching wildcard, then enabled defaults. Empty commands continue to the next strategy.

`disable_startup_command` is strategy-specific, matching the pinned source: an explicit session command still runs; a session disable flag suppresses defaults but does not veto an enabled wildcard command. A wildcard disable flag suppresses that wildcard command and, for a wildcard-created session, default fallback. `startup_script` on sessions/defaults/wildcards is a Rex extension; it stays paired with its layer and precedes that same layer's command, so an inherited script cannot shadow a higher-priority command. The default-session disable extension suppresses only its default strategy.

A startup script can name an existing file, whose contents are read, or contain script text. Commands run through `sh`; `{}` is the session/window path placeholder, including within quoted nested command substitutions such as `printf '%s' "$(basename '{}')"`. The path is passed as argument data and captured once in a fresh readonly shell binding. Functions and `set --` keep their ordinary positional parameters while `{}` continues to mean the original project path. Spaces, quotes and shell metacharacters in directory names remain literal. Startup is followed by an interactive login shell so the terminal remains usable after setup finishes. Configured preview commands also run through `sh` in the selected directory; default directory preview lists files without invoking an external listing command. A configured preview command that produces no output also falls back to directory preview.

`windows` names templates from `[[window]]`; each is created as an additional Rex tab beside the initial `Terminal` tab. An omitted or empty window path inherits the session directory; a relative window path resolves beneath it. Absolute/home-expanded paths can target another directory.

If a configured window fails, the already created session remains discoverable and the error includes its ID. A retry may reuse that session without rerunning setup; inspect and repair the incomplete layout explicitly.

## Naming substitutions

```toml
[[name_substitution]]
find = '^~/Projects/(.*)$'
replace = '$1'
regex = true
```

Each rule accepts nonempty `find`, `replace` (default empty), and `regex` (default false). Rules run in declaration order against the home-abbreviated normalized path. Regex replacement supports captures such as `$1`; invalid regexes are errors. If the final substitution differs and is nonempty, it is the name; otherwise normal Git/directory naming runs. Git naming retains repository/worktree-relative suffixes. Native Rex labels permit punctuation that tmux naming traditionally normalized.

## Frecency

```toml
[frecency]
list_command = "zoxide query --list --score"
query_command = "zoxide query {}"
add_command = "zoxide add {}"
remove_command = "zoxide remove {}"
```

Each nonempty command replaces its corresponding default zoxide operation. Omitting a field or setting it to `""` uses the default for that operation, matching sesh for all four fields. A missing default zoxide executable yields an empty history source and does not block ordinary directory connections. Nonempty invalid/custom commands still report errors; whitespace-only text is not an empty default. A command is parsed into an executable and arguments with shell-style quoting, then `{}` is substituted within each argument. The value remains argument data. Pipes, redirection, and other shell operators are not interpreted; use a wrapper executable if needed. Listing output contains one path per line, optionally preceded by a numeric score and whitespace. Source identity remains `zoxide` even with a custom backend. A missing default zoxide executable yields no frecency entries; failures from a configured list/query command are reported. A failure to record an already connected directory is a warning.

## TUI and icons

```toml
[tui]
show_icons = true
show_windows = false
window_name_format = "{window_name}"
prompt = "> "
placeholder = "Filter Sessions..."
pointer = "> "
alias_auto_connect_delay = "150ms"
alias_filter_prefix = "/"
preview = false
preview_width = 60
preview_min_width = 100
preview_border = "line"
group_separator = false

[icons.rex]
icon = "R"
color = "blue"

[icons.config]
icon = "C"
color = "bright-black"
```

The `[tui]` table is retained for compatibility and configures both system-fzf pickers; fzf 0.63 or newer must be on `PATH`. Prompt and placeholder map to fzf's prompt and ghost text. Pointer maps to fzf's pointer. Configured empty prompt/placeholder values still normalize to defaults; explicit empty CLI overrides stay empty. Icon, source color and window format settings decorate rows without changing their hidden selection IDs.

`preview_width <= 0` falls back to 60; positive values clamp to 10–90 percent. The value maps directly to fzf's right-hand preview percentage. `preview_min_width <= 0` falls back to 100 and maps to fzf's narrow-terminal hidden layout. Borders map as follows: `none` → none, `line` → sharp, `thick` → bold, `double` → double; unknown/empty values normalize to `line`. Ctrl+O toggles the session preview. Fzf owns preview scheduling, cancellation, clipping and terminal restoration; the previous 60ms native debounce, 60-column list cap, exact cursor styling and modal geometry no longer apply. Preview text is stripped of terminal control sequences. Worktree previews remain disabled.

Alias delay accepts Go duration syntax; empty uses 150ms, and zero/negative durations connect immediately. Prefix must be empty (disabled) or one non-whitespace character. Alias mode, exact Unicode alias matching and normal-mode `alias_auto_connect` are preserved. `--no-alias-auto` disables both timed paths, and a prefilled `--query` never arms a timer. A `#` query enables numbered shortcuts 1–9 unless `#` is the alias prefix. `group_separator = true` displays each source group's title on its own line above the first item when the query is empty. Titles use fzf's multiline display within that item's record, so navigation, selection and numbered shortcuts count only real entries.

Fuzzy matching and ranking use `fzf --filter` with case-insensitive matching, extended query syntax and stable ties. `separator_aware` normalizes `-`, `_`, `/` and `\\` to spaces in both names and queries. The visible fzf picker displays these ranked results without an additional search pass or match highlights, allowing alias/index modes to share the same query field. Window labels, icons and aliases are excluded from ordinary name matching. Fzf defaults from `FZF_DEFAULT_OPTS`, `FZF_DEFAULT_OPTS_FILE` and `FZF_DEFAULT_COMMAND` are cleared to protect the row/callback protocol. Navigation and basic query editing otherwise follow fzf; page keys and the documented Ctrl navigation overrides are available. Reopening after renaming, deletion or manual refresh preserves the query and resets selection/preview visibility to defaults.

Window formats support `{window_name}`/`{name}`, `{window_index}` (Rex's 1-based window order), `{window_active}` (`0` or `1`), `{active}` (`*` or empty), and `{session_name}`. Ordinary sesh fields `#{window_name}`/`#W`, `#{window_index}`/`#I`, `#{window_active}`, and `#{session_name}`/`#S` map to those values. `##` escapes a literal hash; native `{{`/`}}` escape braces. Template text is interpreted once, so braces in actual names remain literal and duplicate window names use the actual active window index. Unsupported tmux variables, conditionals, styles and `#(...)` commands produce migration errors.

Recognized source icon tables are `[icons.rex]`, `[icons.rex-block]`, `[icons.config]`, `[icons.zoxide]`, `[icons.directory]`, and `[icons.worktree]`, each with `icon` and `color` strings. Colors accepted by configuration are black/red/green/yellow/blue/magenta/cyan/white and corresponding `bright-*` names, case-insensitive, with `gray`/`grey` aliases; empty means default. Unknown sources/colors are errors. Legacy `[icons.tmux]` migrates to `rex` with a diagnostic; an explicit `rex` table wins. Legacy `[icons.tmux-pane]` (or `[icons.rex-pane]`) migrates to `[icons.rex-block]`; an explicit native table wins. Live session glyphs default blue, block/pane glyphs green.

Both pickers show Nerd Font icons by default: `` for live sessions, `` for blocks/panes, `` for configured sessions, `` for directories (including zoxide history), and `` for worktrees. Configured session/wildcard icons override source glyphs in the TUI. Use a Nerd Font to render these glyphs, or disable them with `--icons=false` or `[tui] show_icons = false`. Aliases use ASCII `[a]` labels in either icon mode. Plain `list --icons` intentionally uses resolved source glyphs, not session icons. `list --icons-exclude rex,config` hides selected sources' glyphs; `--no-color` removes ANSI color.

## Worktrunk worktrees

Install [Worktrunk](https://worktrunk.dev) **0.81.0 or newer** and put `wt` on PATH, or set `REXUE_WT` to its executable. Missing/older versions produce an actionable error; rexue does not fall back to direct Git worktree management. Both Worktrunk list JSON schemas (1 and 2) are supported.

Repository-scoped commands use `--path REPO` or the current directory, including a linked worktree or nested directory. They do not require registration, a GitHub remote, gh, or authentication. `create BRANCH [--base REF]` creates a branch/checkout and prints its actual path without opening Rex. `connect [BRANCH_OR_PATH]` uses an existing branch (creating its checkout if necessary); without a target it connects the current checkout. `list` includes non-numbered and detached checkouts. `picker` selects a returned path, with Ctrl+W creation, Ctrl+O preview, Ctrl+R refresh and Ctrl+X confirmed removal. Ctrl+W prompts for a new branch name, creates through Worktrunk in the selected repository, restores the picker terminal and connects Rex using the returned path. Esc cancels the prompt without changing the search. Empty names and Worktrunk errors keep the picker open; creation also works with no matching rows. Creation uses Worktrunk defaults/hooks, with no additional switch that would rerun hooks. `--refresh` is accepted for explicit initial refresh; every fetch already reads current local state.

Optional registrations add worktrees to the unfiltered main list/picker and set their icons:

```toml
[[worktree]]
path = "~/Projects/repo"
icon = ""
```

Only `path` and `icon` belong to rexue. Worktrunk reads its own user and project configuration, environment overrides, path templates, defaults and hooks; rexue does not parse or overwrite them. For example, set `worktree-path = "../{{ repo }}.{{ branch | sanitize }}"` in Worktrunk user configuration. Actual paths always come from wt, so existing checkouts remain discoverable after a template change.

Mutation commands preserve Worktrunk's stdin/stderr for native hook approval. rexue never inserts `--yes` or `--no-hooks`. Approve trusted project commands using Worktrunk's own workflow (`wt config approvals add`) when using a noninteractive caller. Worktrunk may skip unapproved project commands when approval is declined or input is unavailable; its diagnostics remain visible. Rex session startup still uses ordinary session/wildcard/default-session configuration.

`remove BRANCH_OR_PATH` requires an existing registered checkout and waits for completion. Worktrunk decides whether to delete an integrated branch according to its configuration. `--no-delete-branch` retains it; `--force` and `--force-delete` are explicit opt-ins for dirty checkout removal and unmerged branch deletion. None is added by default or by the picker. Removing a worktree keeps its Rex session. When a default directory name is already owned by another checkout, rexue generates an available name with the current branch as a suffix (for example `repo@main`), and reuses that session by actual path on later switches. Detached/non-Git directories use a short path fingerprint; occupied suffixes receive a numeric extension. Explicit `--name` values and configured session names still report cross-directory conflicts; the existing session is never silently retargeted. Worktrunk failures are reported, with previous picker rows retained on refresh failure.

Migration from the old GitHub-specific implementation is explicit: remove `repo`, `worktree_dir`, `branch_template`, `base_branch`, `fetch`, `create_command` and `startup_command` from each rexue `[[worktree]]`, retaining `path` and optional `icon`. Move directory templates and lifecycle hooks to Worktrunk, and session startup to Rex session settings. Remove `[browser]`. These retired fields cause a migration error even when `strict_mode = false`. Replace numeric/PR/browser commands with branches or checkout paths; `--repo`, `--pr`, `--browser` and `worktree browse` no longer exist. Existing directories and branches are not moved, renamed or deleted during migration. Old GitHub cache files are no longer read or written.

## State and cache

Local state records known session paths and per-client session history used by `last`, with locking and atomic JSON replacement. `doctor` reports the effective state directory. Override its base with `REXUE_STATE_DIR`; otherwise the platform state/data-local directory is used. A server/binary fingerprint scopes state below that base.

`cache = true` caches only frecency entries for five seconds. Rex sessions and resolved configuration remain live. `--no-cache` bypasses reading the cache while allowing refreshed data to be written. `cache refresh` removes the existing frecency cache and synchronously reloads sources. This differs from Sesh's session-cache TTL and background refresh.

Worktree lists read current Worktrunk state on each fetch. There is no rexue GitHub title/state cache or background metadata refresh. Ctrl+R reloads Worktrunk state; failures remain visible while the picker keeps its previous rows.
