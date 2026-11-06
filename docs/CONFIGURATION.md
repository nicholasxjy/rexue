# Configuration

Rexue loads `$XDG_CONFIG_HOME/rexue/rexue.toml`, or `~/.config/rexue/rexue.toml` when XDG_CONFIG_HOME is unset or empty. Override it with `-C/--config-file PATH`. A missing default file uses defaults; an explicit file must exist.

See the [editable example](../examples/rexue.toml). Unknown keys warn and are ignored unless `strict_mode = true`; type errors always fail. Configuration paths expand `~` and environment variables. Relative paths, including imports, resolve from the working directory.

## General settings

| Key | Default | Purpose |
| --- | --- | --- |
| `strict_mode` | `false` | Reject unknown keys. |
| `import` | `[]` | Append session, wildcard and worktree definitions from exact filenames, in order. No globs, nested imports or imported global settings. |
| `blacklist` | `[]` | Regexes matching entry names; `list --blacklisted` shows matches instead. |
| `sort_order` | `[]` | Order sources; nested arrays interleave sources by history score. Default: `rex`, `agent`, `config`, `zoxide`, `directory`, `worktree`. |
| `cache` | `false` | Cache directory history for five seconds; `--no-cache` bypasses it. Live sessions and agents stay fresh. |
| `dir_length` | `1` | Trailing path components for directory names. |
| `git_dir_length` | `1` | Trailing components of the Git repository naming anchor. |
| `git_namer_use_worktree_root` | `false` | Use the checkout root as the naming anchor. |
| `separator_aware` | `false` | Rank picker name segments separately. |

Naming lengths of zero or less become 1. `[[name_substitution]]` applies `find` → `replace` before directory/Git naming; `regex = true` enables regex matching (default: `false`). `find` must be nonempty.

## Sessions and startup

Each `[[session]]` requires a unique nonempty `name` and a nonempty `path`.

| Optional field | Behavior |
| --- | --- |
| `startup_command` | Shell command for a new session. |
| `startup_script` | Shell text or an existing script file, relative to the session directory; takes priority over the command in the same layer. |
| `preview_command` | Directory preview command; empty output falls back to a directory listing. |
| `disable_startup_command` | Suppress default startup fallback (default: `false`). Explicit session startup still runs. |
| `alias` | Unique alias, compared using Unicode simple case folding. |
| `alias_auto_connect` | Auto-connect an exact alias outside alias-filter mode (default: `false`). |
| `icon` | Custom picker icon; falls back to a matching wildcard. |

`[default_session]` supplies startup, preview and default-disable settings. `[[wildcard]]` accepts `pattern`, startup/preview settings, `disable_startup_command` and `icon`. Only the first matching wildcard participates. Patterns support `*` within a path component, `?`, character classes (`[a-z]`, `[^x]`) and backslash escapes. A trailing `/**` matches descendants of its literal prefix, excluding the prefix itself.

Startup uses the first nonempty choice: `connect --command`, explicit session startup, enabled wildcard startup, then enabled defaults. Each layer keeps its script and command together; empty values fall through. A wildcard disable flag skips its startup and, without an explicit session, suppresses defaults. An explicit session's disable flag controls its default fallback; the default disable flag suppresses default startup globally. Reconnecting never reruns startup.

Preview uses the first nonempty session, wildcard or default command. Startup and preview scripts run through `sh`; `{}` inserts the session path as data, preserving spaces and shell punctuation. Live previews capture the focused terminal; agent previews capture that agent's pane, including in inactive tabs.

```toml
[default_session]
preview_command = "ls -la {}"

[[wildcard]]
pattern = "~/Projects/**"
startup_command = "git status --short"

[[session]]
name = "app"
path = "~/Projects/app"
alias = "a"
startup_command = "nvim"
```

## Directory history

`[frecency]` accepts `list_command`, `query_command`, `add_command` and `remove_command`. Each absent or empty value uses the corresponding zoxide operation. Missing zoxide yields an empty history source; failing custom commands report errors.

Custom commands use shell-style quoting but execute directly, without shell expansion. Use a wrapper script for pipes or redirection. `{}` substitutes the query/path within each argument, preserving it as data.

List output is one directory per line, optionally prefixed by a numeric score. Connecting records the directory; picker removal deletes its history entry. Scores rank merged groups, for example:

```toml
sort_order = ["rex", "agent", ["config", "zoxide"], "directory", "worktree"]

[frecency]
query_command = "zoxide query -- {}"
add_command = "zoxide add -- {}"
```

## Worktrees

`[[worktree]]` registers a repository in the main list/picker with a required `path` and optional `icon`. Paths must be unique after normalization. Repository-scoped commands use the current checkout or `--path` without requiring registration.

Requires Worktrunk **0.81.0+** (`wt` on PATH, or `REXUE_WT`). Configure checkout paths, branch defaults, hooks, approvals and removal policy in [Worktrunk](https://worktrunk.dev/config/).

Removing a checkout from either picker or with `worktree remove` also closes all associated Rex sessions, including their tabs, panes and running agents. Sessions are matched by their current directory or Rexue's recorded connection path, including subdirectories and symlinked paths; names and picker visibility do not determine ownership. Unrelated sessions stay open. Cancelled or failed Worktrunk removal does not close any sessions.

When the launching session belongs to the removed checkout, Rexue first switches to the first remaining worktree in worktree picker order. If no worktrees remain, it switches to the first live session outside the removed checkout. It then closes the associated sessions, with the launching session last and after picker teardown. If there is no remaining destination, it closes them without switching. A failed switch preserves the sessions for recovery, and session-close failures are reported with the full session ID.

The worktree picker stays open when discovery fails or finds no checkouts, showing `Not a Git repository or no worktrees found` and any error. Refresh retries discovery. Create and cancel remain available with an empty list or no filter matches.

## Picker settings

With an empty initial query, the session picker focuses the launching Rex session by its full ID. The worktree picker focuses the checkout containing the current directory (or `--path`), including subdirectories and symlinked paths. If that entry is unavailable, the first row stays selected. Initial focus applies once when opening; a prefilled `--query` uses normal search ordering.

Requires fzf **0.63+** on PATH. `[tui]` applies to both pickers:

| Key | Default | Behavior |
| --- | --- | --- |
| `show_icons` | `true` | Show source/custom icons and tab type icons (`` ordinary, `󰚩` live agent). |
| `prompt` | `"> "` | Search prompt; empty uses the default. |
| `placeholder` | `"Filter Sessions..."` | Empty uses the default. Worktrees use `"Filter worktrees..."` unless customized. |
| `pointer` | `"> "` | Empty or wider than two terminal cells uses the default. |
| `alias_filter_prefix` | `"/"` | One non-whitespace character; empty disables alias filtering. |
| `alias_auto_connect_delay` | `"150ms"` | Duration; empty uses the default, zero/negative connects immediately. Supports ns, us/µs/μs, ms, s, m, h, fractions and combinations. |
| `preview` | `false` | Initial preview visibility. |
| `preview_width` | `60` | Percentage, clamped to 10–90; zero/negative uses 60. |
| `preview_min_width` | `100` | Minimum terminal width for preview; zero/negative uses 100. |
| `preview_border` | `"line"` | `none`, `line`, `thick`, `double`; unknown uses `line`. |
| `group_separator` | `false` | Nonselectable source boundaries; Agents always shows a boundary with an empty query. |

CLI overrides: `picker --icons`, `--separator-aware`, `--prompt`, `--placeholder`, `--query`, `--preview`, `--no-preview`, `--no-alias-auto`. `--no-preview` wins over `--preview`. Boolean values use `=`, e.g. `--icons=false`. Worktree picker accepts icons, prompt, placeholder and query overrides. CLI prompt/placeholder overrides may be empty.

In the session picker, `/` filters aliases and an exact alias auto-connects after typing; outside this mode, auto-connect requires `alias_auto_connect = true`. A prefilled `--query` does not auto-connect. A query starting with `#` enables numbered selection. These modes do not apply to the worktree picker.

`[icons.SOURCE]` accepts `icon` and `color` for `agent`, `rex`, `config`, `zoxide`, `directory`, `worktree`. Colors: black/red/green/yellow/blue/magenta/cyan/white, optionally `bright-`, or gray/grey. Custom session, repository and wildcard picker icons take priority over source icons, in that order.

### Keybindings

`[tui.keybindings]` maps actions to arrays of fzf key names. Omit an action to keep its defaults; an array replaces them, and `[]` disables its shortcuts. Headers/prompts show the first keyboard shortcut and omit disabled actions.

```toml
[tui.keybindings]
rename = ["ctrl-r", "alt-r", "f2"]
toggle_preview = ["alt-o"]
select_2 = ["2", "alt-2"]
```

| Action | Default keys | Context |
| --- | --- | --- |
| `accept` | `enter`, `double-click` | Connect; submit text or the selected confirmation row. |
| `cancel` | `esc`, `ctrl-c`, `ctrl-g`, `ctrl-q` | Both pickers and dialogs. |
| `up` / `down` | `up`, `ctrl-k`, `ctrl-p` / `down`, `ctrl-j`, `ctrl-n` | Both pickers. |
| `half_page_up` / `half_page_down` | `ctrl-u` / `ctrl-d` | Both pickers. |
| `toggle_preview` | `ctrl-o` | Both pickers. |
| `rename` | `ctrl-r` | Session picker; live Rex sessions only. |
| `remove` | `ctrl-x` | Confirm removal of a live session, history entry or worktree. Agent/config rows cannot be removed. |
| `refresh` | `ctrl-r` | Worktree picker. |
| `create_worktree` | `ctrl-w` | Worktree picker; create and connect, even with no matching rows. |
| `select_1` … `select_9` | `1` … `9` | Session picker; choose a numbered row in `#` mode, otherwise insert the digit. |
| `confirm_yes` / `confirm_no` | `y` / `n`, `q` | Confirm removal / cancel. |
| `confirm_up` / `confirm_down` | `up`, `ctrl-k`, `ctrl-p`, `left`, `btab` / `down`, `ctrl-j`, `ctrl-n`, `right`, `tab` | Removal confirmation. |

Keys include `enter`, `esc`, `tab`, `space`, arrows, `page-down`, `f1`–`f12`, `ctrl-a`–`ctrl-z`, `alt-r` and modified navigation such as `ctrl-up`. Single printable characters are accepted except `: , + ( ) [ ] { }` and backslash. Names are case-sensitive; aliases such as `return`/`ctrl-m`, `shift-tab`, `pgup` and `pgdn` normalize before validation.

Invalid keys and duplicate assignments in the same context fail loading. Reassign a key by replacing/disabling its previous action. Separate contexts may share keys, e.g. session rename and worktree refresh. Removing a digit shortcut restores digit input. Other fzf editing/mouse bindings remain available. Shortcuts that launch Rexue belong in Rex's `init.lua`; see the [examples](../README.md#configure).
