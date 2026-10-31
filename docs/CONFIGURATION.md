# Configuration

rexue reads TOML from `$XDG_CONFIG_HOME/rexue/rexue.toml`, falling back to `~/.config/rexue/rexue.toml`. A missing default file uses defaults; an explicitly selected missing file is an error.

```sh
rexue -C ./projects.toml picker
rexue --config-file ./projects.toml list --config
```

Business boolean flags accept bare flags and explicit Go boolean values such as `--icons=false`. Picker `--icons=false`, `--preview=false`, and `--separator-aware=false` override enabled configuration values; `--no-preview=false` leaves preview enabled.

CLI text flags retain the pinned source's presence rules: explicitly empty picker `--prompt`/`--placeholder` remain empty, and `list --format=` emits empty rows and conflicts with enabled JSON. Empty optional clone parent/directory/VCS and worktree repo/path selectors use their normal fallbacks. Empty window target/path/command and connect/mkdir command values also fall back; an explicitly empty window `--target` still takes precedence over `--session` before selecting the current context. Nonempty values, whitespace and raw trailing Git arguments are preserved. These CLI rules are distinct from configuration inheritance below.

`-C/--config-file` selects the file. `list/picker -c/--config` selects configured session rows. Rex's own `rex -C` selects an app client, so these flags must not be forwarded between tools.

See [the example](../examples/rexue.toml), the [command reference](../README.md#command-reference), and [mapping decisions](DECISIONS.md). This document describes the Rust implementation; [PARITY.md](PARITY.md) describes the upstream contract and acceptance work.

## Validation and paths

Unknown fields produce a diagnostic and are ignored by default. Set root `strict_mode = true` to reject them in the primary file. Imported files use permissive field handling, matching the pinned sesh loader. Wrong types and invalid known values are always errors. tmux-specific keys (`tmux*`) are rejected with migration guidance even in permissive mode; the documented `icons.tmux` / `sort_order=["tmux"]` source aliases migrate to `rex`, and `icons.tmux-pane` migrates to the independent `rex-block` source before this check.

Paths expand `~`, `$VARIABLE`, and `${VARIABLE}`. Missing environment variables and `~otheruser` are errors. Empty paths are errors except where a documented default or window inheritance applies. Expansion never executes shell substitutions. Raw configuration strings expand once; characters such as `$` introduced by an environment variable value remain literal path data. Already-resolved paths are normalized without interpreting variables again. Invalid configured entries produce contextual errors rather than disappearing from the list. Relative session paths resolve against **rexue's working directory**. Configured window paths resolve relative to the session directory; relative startup-script filenames resolve against the corresponding terminal directory. Ordinary paths do not resolve relative to the TOML file. Import paths have different semantics below. Matching normalizes paths lexically and session reuse resolves symlinks when possible.

Duplicate session/window names, case-insensitive duplicate aliases, duplicate worktree repos, and references to undefined window templates are errors. Directory lengths at or below zero fall back to 1; VCS must be `git` or `jj`.

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
show_icons = false
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

The snippet shows TUI defaults. `preview_width <= 0` falls back to 60; positive values clamp to 10–90 percent. The pane can grow to keep the list at most 60 columns and leaves at least 40 columns for it. Preview appears only at or above `preview_min_width`; values at or below zero fall back to 100, and large positive thresholds remain valid. Borders are `none`, `line`, `thick`, or `double`; unknown or empty values fall back to `line`. Preview requests debounce by 60ms, and stale results are discarded.

Empty prompt/placeholder values fall back to the defaults. Empty pointers or pointers wider than two terminal cells fall back to `> `; control characters are rejected. Alias delay accepts Go duration syntax, including signed/compound values and unitless `0`; an empty value uses 150ms and valid negative values fire immediately. Alias prefix must be empty (disabled) or one non-whitespace character. Group separators appear between consecutive source groups only when the query is empty.

Window formats support `{window_name}`/`{name}`, `{window_index}` (Rex's 1-based window order), `{window_active}` (`0` or `1`), `{active}` (`*` or empty), and `{session_name}`. Ordinary sesh fields `#{window_name}`/`#W`, `#{window_index}`/`#I`, `#{window_active}`, and `#{session_name}`/`#S` map to those values. `##` escapes a literal hash; native `{{`/`}}` escape braces. Template text is interpreted once, so braces in actual names remain literal and duplicate window names use the actual active window index. Unsupported tmux variables, conditionals, styles and `#(...)` commands produce migration errors.

Recognized source icon tables are `[icons.rex]`, `[icons.rex-block]`, `[icons.config]`, `[icons.zoxide]`, `[icons.directory]`, and `[icons.worktree]`, each with `icon` and `color` strings. Colors accepted by configuration are black/red/green/yellow/blue/magenta/cyan/white and corresponding `bright-*` names, case-insensitive, with `gray`/`grey` aliases; empty means default. Unknown sources/colors are errors. Legacy `[icons.tmux]` migrates to `rex` with a diagnostic; an explicit `rex` table wins. Legacy `[icons.tmux-pane]` (or `[icons.rex-pane]`) migrates to `[icons.rex-block]`; an explicit native table wins. Live session glyphs default blue, block/pane glyphs green.

Configured session/wildcard icons override source glyphs in the TUI. A Nerd Font is optional; `show_icons = false` uses ASCII alias-chip caps. Plain `list --icons` intentionally uses resolved source glyphs, not session icons. `list --icons-exclude rex,config` hides selected sources' glyphs; `--no-color` removes ANSI color.

## GitHub worktrees and browser

```toml
[[worktree]]
repo = "owner/repo"
path = "~/Projects/repo"
worktree_dir = ".wk"
branch_template = "issue/{number}"
base_branch = "origin/main"
fetch = true
create_command = "npm install"
startup_command = "nvim"
icon = "W"

[browser]
application = "Safari"
# Optional AppleScript expression, not a shell command:
url_command = "URL of current tab of front window"
```

A worktree requires `repo = "owner/repo"` and a repository `path`. Defaults are `.wk`, `{number}`, `origin/main`, and `fetch = true`; commands/icon are optional. `worktree_dir` is relative to the configured repository unless absolute/home/environment-prefixed. Resulting directories are numbered. Branch templates substitute `{number}` and must produce a valid Git branch name.

`--repo` selects explicitly and takes precedence over `--path`; otherwise selection detects the configured repository from cwd or a linked worktree. `connect NUMBER` checks GitHub metadata before Git mutation. An issue uses a named branch. An own PR prefers its first closing issue, then a `#N` reference in title/body; otherwise a detached PR checkout is used. A foreign PR uses a detached checkout. Existing registered worktrees are reused; an unregistered directory occupying the target is an error. Fetch errors are propagated and fetch runs when creating a worktree.

`create_command` is selected only for a new worktree and does **not** fall back to `startup_command`. Existing worktrees select `startup_command`. The selected command is handed to session creation and runs only if its Rex session is new. The worktree picker preserves repository/path scope and sends the selected number through worktree preparation before connecting. Configured worktree icons are carried into session and worktree entries.

`worktree browse` derives the numbered worktree from process cwd, including nested paths; if cwd is unrelated, it falls back to the calling Rex session or the selected client's recorded session directory. It it opens an issue URL, or obtains a PR URL with `--pr`. `--print` prints without opening a browser. macOS uses `open`, optionally `-a application`; the code has an `xdg-open` fallback, but Linux is not the current real target.

`worktree connect --browser` reads the configured browser's front-window active-tab URL using macOS `osascript`. It accepts a GitHub issue/PR URL and selects its repository and number. A positional number conflicts with `--browser`; an explicit `--repo` overrides the URL repository, and `--pr` can force PR treatment. macOS may ask for Automation permission. Safari defaults to `URL of current tab of front window`; other configured apps default to `URL of active tab of front window`. `url_command` overrides that **AppleScript expression**, not a command-line program.

## State and cache

Local state records known session paths and per-client session history used by `last`, with locking and atomic JSON replacement. `doctor` reports the effective state directory. Override its base with `REXUE_STATE_DIR`; otherwise the platform state/data-local directory is used. A server/binary fingerprint scopes state below that base.

`cache = true` caches only frecency entries for five seconds. Rex sessions and resolved configuration remain live. `--no-cache` bypasses reading the cache while allowing refreshed data to be written. `cache refresh` removes the existing frecency cache and synchronously reloads sources. This differs from Sesh's session-cache TTL and background refresh.

GitHub title/state cache is separate: the platform cache directory plus `rexue/github-issues-v1.json`. Positive titles use a 24-hour TTL and negative lookups one hour; listing batches at most 50 numbers per GraphQL request. The worktree picker shows cached rows first, then refreshes in the background. Ctrl+R and `--refresh` bypass the cache; explicit refresh failures are reported while the picker keeps its rows. Ordinary offline listing retains stale titles or bare numbers. Session-cache settings do not disable this cache.
