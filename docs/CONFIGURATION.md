# Configuration

Rexue loads `$XDG_CONFIG_HOME/rexue/rexue.toml`, falling back to `~/.config/rexue/rexue.toml` when XDG_CONFIG_HOME is unset or empty. Override with `-C/--config-file PATH`. A missing default file uses defaults; an explicit file must exist.

Unknown keys warn and are ignored unless `strict_mode = true`; type errors always fail. Paths expand `~` and environment variables. Relative paths and imports resolve from the working directory. See the [editable example](../examples/rexue.toml).

## General settings

| Key | Default | Behavior |
| --- | --- | --- |
| `strict_mode` | `false` | Reject unknown keys. |
| `import` | `[]` | Append session, wildcard and worktree definitions from exact filenames, in order. No globs, nested imports or imported globals. |
| `blacklist` | `[]` | Entry-name regexes; `list --blacklisted` shows matches instead. |
| `sort_order` | `[]` | Source order when sessions and agents are requested together; default: `rex`, `agent`. Nested arrays follow their listed order. |
| `dir_length` | `1` | Trailing path components in directory names. |
| `git_dir_length` | `1` | Trailing components of the Git naming anchor. |
| `git_namer_use_worktree_root` | `false` | Use the checkout root as the Git naming anchor. |
| `separator_aware` | `false` | Rank picker name segments separately. |

Naming lengths ≤0 become 1. `[[name_substitution]]` applies nonempty `find` → `replace` before naming; `regex = true` enables regex matching (default: `false`).

## Sessions and startup

Each `[[session]]` requires a unique nonempty `name` and a nonempty `path`. Definitions provide `connect` targets and metadata for matching live sessions; unconnected projects are excluded from the main picker.

| Optional field | Behavior |
| --- | --- |
| `startup_command` | Shell command for a new session. |
| `startup_script` | Shell text or an existing file relative to the session directory; overrides the command in its layer. |
| `preview_command` | Directory preview; empty output falls back to a directory listing. |
| `disable_startup_command` | Suppress default startup fallback; explicit session startup still runs. Default: `false`. |
| `alias` | Unique alias, compared using Unicode simple case folding. |
| `alias_auto_connect` | Auto-connect an exact alias outside alias-filter mode. Default: `false`. |
| `icon` | Custom picker icon; falls back to the matching wildcard. |

`[default_session]` supplies startup, preview and disable settings. `[[wildcard]]` accepts these settings plus `pattern` and `icon`; the first match wins. Patterns support component-local `*`, `?`, classes (`[a-z]`, `[^x]`) and backslash escapes. A trailing `/**` matches descendants of its literal prefix, excluding that prefix.

Startup takes the first nonempty layer: `connect --command` → session → enabled wildcard → enabled defaults. Scripts take precedence over commands within each layer; empty values fall through. A wildcard disable skips its startup and, without an explicit session, suppresses defaults. A session disable controls its default fallback; a default disable suppresses default startup globally. Reconnecting never reruns startup.

Preview takes the first nonempty session, wildcard or default command. Startup/preview scripts run through `sh`; `{}` passes the session path as data, preserving spaces and shell punctuation. Live previews capture the focused terminal of a session's active tab, a selected tab's focused terminal, or the exact selected pane.

## Worktrees

`[[worktree]]` registers a repository with a required `path` and optional `icon`. Paths must be unique after normalization. Worktree commands use the current checkout or `--path` without registration.

Requires Worktrunk **0.81.0+** (`wt` on PATH, or `REXUE_WT`). Configure paths, branch defaults, hooks, approvals and removal policy in [Worktrunk](https://worktrunk.dev/config/).

Successful removal also closes every associated Rex session, tab, pane and agent. Ownership follows current directories or recorded connection paths, including subdirectories and symlinks, regardless of names or picker visibility. Unrelated sessions remain open; cancelled or failed Worktrunk removal closes none.

If the launching session belongs to the removed checkout, Rexue switches to the first remaining worktree in picker order, otherwise the first unrelated live session. It closes associated sessions, with the launcher last and after picker teardown. With no destination, it closes them without switching. Switch failures preserve sessions; close failures report the full session ID.

An empty or failed worktree list keeps the picker open with `Not a Git repository or no worktrees found` and any error. Refresh retries; create and cancel remain available with no rows or matches.

## Picker settings

The main picker and `list` show live sessions only. Session rows form a session → tab → pane tree; Enter selects that exact target. `picker --agents` stays flat. Rename and session removal apply only to roots. Agent panes show session, app, pane number and state.

Tabs use a custom name, otherwise the focused terminal's title, then its pane label. Live agent titles are appended once per tab; an unnamed agent tab without a title uses its app name. Ordinary titles replace `|` separators with spaces and collapse adjacent spaces. Session rows default to the terminal icon (``); tabs share the Rex icon (``), regardless of the program or agents inside. Custom session/source icons still take precedence. In Rex, pane rows use Rex’s bundled artwork for their actual foreground program or agent (for example zsh, Pi, Claude and Codex). Unknown programs, `NO_COLOR`, and other terminals use the existing character fallback for panes. `show_icons = false` disables both image and character icons. List rows follow Rex's built-in picker: spaced icons, plain tab titles and a theme-aware highlight across the current row. Both pickers and their dialogs default to fzf’s 16-color scheme, using the terminal palette so colors follow live dark/light theme changes. Custom `--color` and `--no-color` settings are inherited from `FZF_DEFAULT_OPTS_FILE`, then `FZF_DEFAULT_OPTS` in fzf’s usual order. Selected-row colors and attributes follow that scheme without Rexue overrides; `NO_COLOR` keeps fzf’s monochrome default. Other global fzf options are ignored to preserve the picker’s input/output protocol and configured actions. Tree connectors, aliases, worktree paths and agent states are subdued; configured icon colors remain in use for character fallbacks. Pane rows put the app and pane number before the title, with agent state first and session context last so narrow windows retain the identifying information. Search still uses the complete original name. Terminal fonts and cell geometry determine the exact appearance.

With an empty query, initial focus selects the current pane, then its tab or session. `--focus-pane BLOCK_ID` preserves a shortcut's invoking pane; legacy floating shortcuts select their current tab. A floating picker excludes its own temporary pane. Worktree pickers select the checkout containing the current directory or `--path`, including subdirectories and symlinks, otherwise the first row. Initial focus applies once; a prefilled query keeps normal search ordering.

Requires fzf **0.63+**. `[tui]` applies to both pickers:

| Key | Default | Behavior |
| --- | --- | --- |
| `show_icons` | `true` | Show source, custom and tab icons. |
| `prompt` | `"> "` | Empty configuration uses the default. |
| `placeholder` | `"Filter Sessions..."` | Empty uses the default; worktrees use `"Filter worktrees..."` unless customized. |
| `pointer` | `"> "` | Empty or wider than two terminal cells uses the default. |
| `alias_filter_prefix` | `"/"` | One non-whitespace character; empty disables alias filtering. |
| `alias_auto_connect_delay` | `"150ms"` | Empty uses the default; ≤0 connects immediately. Supports ns, us/µs/μs, ms, s, m, h, fractions and combinations. |
| `preview` | `false` | Initial preview visibility. |
| `preview_width` | `60` | Percentage clamped to 10–90; ≤0 uses 60. |
| `preview_min_width` | `100` | Minimum terminal width for preview; ≤0 uses 100. |
| `preview_border` | `"line"` | `none`, `line`, `thick`, `double`; unknown uses `line`. |

CLI overrides: `--focus-pane`, `--icons`, `--separator-aware`, `--prompt`, `--placeholder`, `--query`, `--preview`, `--no-preview`, `--no-alias-auto`. Boolean values use `=`, e.g. `--icons=false`; `--no-preview` wins. CLI prompt/placeholder values may be empty. Worktree picker supports icons, prompt, placeholder and query overrides.

In the session picker, `/` filters aliases of live sessions and auto-connects an exact alias after typing. Outside this mode, auto-connect requires `alias_auto_connect = true`. Prefilled queries never auto-connect. `#` starts numbered selection unless it is the alias prefix. These modes do not apply to worktree pickers.

`[icons.SOURCE]` accepts `icon` and `color` for `agent`, `rex`, `config`, `directory`, `worktree`. Colors: black, red, green, yellow, blue, magenta, cyan, white, optional `bright-`, or gray/grey. For session/source rows and character fallbacks, custom icons take priority: session → repository → wildcard → source. Native program artwork keeps its original colors.

### Keybindings

`[tui.keybindings]` maps actions to fzf key arrays. Omission keeps defaults; an array replaces them; `[]` disables the action's shortcuts. Headers/prompts show its first shortcut and omit disabled actions.

```toml
[tui.keybindings]
rename = ["ctrl-r", "alt-r", "f2"]
toggle_preview = ["alt-o"]
select_2 = ["2", "alt-2"]
```

| Action | Defaults | Context |
| --- | --- | --- |
| `accept` | `enter`, `double-click` | Connect or submit text/confirmation. |
| `cancel` | `esc`, `ctrl-c`, `ctrl-g`, `ctrl-q` | Both pickers and dialogs. |
| `up` / `down` | `up`, `ctrl-k`, `ctrl-p` / `down`, `ctrl-j`, `ctrl-n` | Both pickers. |
| `half_page_up` / `half_page_down` | `ctrl-u` / `ctrl-d` | Both pickers. |
| `toggle_preview` | `ctrl-o` | Both pickers. |
| `rename` | `ctrl-r` | Live session roots. |
| `remove` | `ctrl-x` | Confirm session or worktree removal; agent, tab and pane rows are ineligible. |
| `refresh` | `ctrl-r` | Worktree picker. |
| `create_worktree` | `ctrl-w` | Worktree picker, including empty results. |
| `select_1` … `select_9` | `1` … `9` | Select a numbered row in `#` mode; otherwise insert the digit. |
| `confirm_yes` / `confirm_no` | `y` / `n`, `q` | Confirm / cancel removal. |
| `confirm_up` / `confirm_down` | `up`, `ctrl-k`, `ctrl-p`, `left`, `btab` / `down`, `ctrl-j`, `ctrl-n`, `right`, `tab` | Removal confirmation. |

Keys include `enter`, `esc`, `tab`, `space`, arrows, `page-down`, `f1`–`f12`, `ctrl-a`–`ctrl-z`, `alt-r` and modified navigation (`ctrl-up`). Single printable characters are allowed except `: , + ( ) [ ] { }` and backslash. Names are case-sensitive; `return`/`ctrl-m`, `shift-tab`, `pgup` and `pgdn` normalize before validation.

Invalid keys or duplicate assignments in one context fail loading. Reassign a key by replacing/disabling its previous action; separate contexts may share keys. Removing a digit shortcut restores digit input. Other fzf editing/mouse bindings remain available. Launch shortcuts belong in [Rex's `init.lua`](../README.md#configure).
