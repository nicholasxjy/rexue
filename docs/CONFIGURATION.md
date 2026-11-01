# Configuration

Rexue reads `$XDG_CONFIG_HOME/rexue/rexue.toml`, falling back to `~/.config/rexue/rexue.toml`. Use `-C/--config-file PATH` to select another file. A missing default file uses defaults; a missing explicit file is an error. Paths support `~` and environment variables without executing shell expressions. Relative session and repository paths resolve from the process working directory.

The complete editable example is [examples/rexue.toml](../examples/rexue.toml). Unknown settings warn and are ignored by default. Set `strict_mode = true` to reject them; invalid types always fail. Only the fields documented here are supported.

## Shared session settings

| Key | Default | Behavior |
| --- | --- | --- |
| `strict_mode` | `false` | Reject unknown settings. |
| `import` | `[]` | Read exact filenames and append session, wildcard and worktree definitions in order. Relative filenames resolve from the process working directory; nested imports and imported global settings do not apply. No glob expansion. |
| `blacklist` | `[]` | Regular expressions matching entry names; use `list --blacklisted` to list matches instead. |
| `sort_order` | `[]` | Source order; nested lists interleave sources by history score. Sources: `agent`, `rex`, `config`, `zoxide`, `directory`, `worktree`. With no explicit order, Rex sessions come first, followed by agents, configured sessions, zoxide history, directories and worktrees. |
| `cache` | `false` | Cache directory-history discovery for five seconds. Agents, live sessions and configured entries remain fresh. `--no-cache` bypasses it. |
| `dir_length` | `1` | Number of trailing path components used for ordinary directory session names. |
| `git_dir_length` | `1` | Number of components in Git repository name anchors. |
| `git_namer_use_worktree_root` | `false` | Name from the selected checkout instead of the common repository root. |
| `separator_aware` | `false` | Rank picker name segments separately. |

Nonpositive naming lengths normalize to 1. `[[name_substitution]]` entries have `find`, `replace` and optional `regex = false`; substitutions run before directory/Git naming. Invalid regular expressions fail validation.

## Sessions and startup

Each `[[session]]` requires a unique nonempty `name` and an expandable nonempty `path`.

| Field | Default | Behavior |
| --- | --- | --- |
| `startup_command` | absent | Shell command run when a new session is created. |
| `startup_script` | absent | Shell text or a script filename relative to the session directory. Takes priority over a command in the same layer. |
| `preview_command` | absent | Shell command for directory/config previews; empty output falls back to directory listing. |
| `disable_startup_command` | `false` | Prevent fallback to default startup; explicit session startup still applies. |
| `alias` | absent | Unique alias compared using Unicode simple case folding. |
| `alias_auto_connect` | `false` | Allow an exact alias to connect outside alias-filter mode. |
| `icon` | absent | Row icon; matching wildcards can supply a fallback. |

`[default_session]` provides startup and preview defaults. `[[wildcard]]` supplies `pattern`, `startup_command`, `startup_script`, `preview_command`, `disable_startup_command` and `icon`. The first matching wildcard participates. Patterns support `*` within a path component, `?`, character classes such as `[a-z]` and `[^x]`, and backslash escapes. A trailing `/**` matches descendants of its literal prefix, excluding the prefix itself.

For startup, a nonempty `connect --command` takes priority, then explicit session startup, then the first enabled matching wildcard startup, then enabled defaults. Each layer keeps its script and command paired. Empty commands/scripts fall through. A wildcard disable flag prevents its own startup and suppresses default startup for directories without an explicit session. An explicit session's disable flag controls default fallback. The default disable flag also prevents default startup. Reconnecting never reruns startup.

Preview uses the first nonempty explicit session, wildcard or default preview command. `{}` in startup, preview and frecency shell commands is a path/value placeholder and remains data even with spaces and shell punctuation. Commands execute through `sh`. Live session previews capture the focused terminal instead of running directory preview commands. Agent previews capture the agent's exact pane, including panes in inactive tabs.

```toml
[default_session]
preview_command = "ls -la {}"

[[wildcard]]
pattern = "~/Projects/**"
startup_command = "git status --short"
icon = "G"

[[session]]
name = "app"
path = "~/Projects/app"
alias = "a"
startup_command = "nvim"
```

## Directory history

Without overrides, zoxide supplies list/query/add/remove operations. Missing optional zoxide yields an empty history source. `[frecency]` supports `list_command`, `query_command`, `add_command` and `remove_command`. Absent or empty strings use zoxide; a nonempty override replaces that operation. Override failures are reported.

List output is one directory per line, optionally preceded by a numeric score. Scores affect merged source groups. Connecting records the resolved directory; picker deletion removes the selected history row. `query_command`, `add_command` and `remove_command` receive their argument through `{}`.

```toml
sort_order = ["rex", "agent", ["config", "zoxide"], "directory", "worktree"]

[frecency]
list_command = "zoxide query --list --score"
query_command = "zoxide query -- {}"
add_command = "zoxide add -- {}"
remove_command = "zoxide remove -- {}"
```

## Worktrees

`[[worktree]]` accepts only a required `path` and optional `icon`. Each path registers a repository for the main session list and picker. Paths must be unique after normalization. Repository-scoped worktree commands work without registration, using the current checkout or `--path`.

```toml
[[worktree]]
path = "~/Projects/app"
icon = "G"
```

Install Worktrunk 0.81.0 or newer. `REXUE_WT` selects another executable. Worktrunk owns checkout path templates, branch defaults, hooks, approvals and removal policies; configure these in [Worktrunk](https://worktrunk.dev/config/). Removing a checkout does not kill its Rex session.

## Picker settings

| `[tui]` key | Default | Behavior |
| --- | --- | --- |
| `show_icons` | `true` | Show source/custom icons. |
| `prompt` | `"> "` | Search prompt. |
| `placeholder` | `"Filter Sessions..."` | Empty values normalize to the default. |
| `pointer` | `"> "` | Empty strings or values wider than two cells use the default. |
| `alias_filter_prefix` | `"/"` | One non-whitespace character; empty disables alias filtering. |
| `alias_auto_connect_delay` | `"150ms"` | Exact-alias delay. Units: ns, us/µs/μs, ms, s, m, h; combinations and fractions are accepted. Zero/negative durations select immediately. |
| `preview` | `false` | Initial preview visibility. |
| `preview_width` | `60` | Percentage; nonpositive uses 60, positive clamps to 10–90. |
| `preview_min_width` | `100` | Minimum terminal width for preview; nonpositive uses 100. |
| `preview_border` | `"line"` | `none`, `line`, `thick`, `double`; unknown values use `line`. |
| `group_separator` | `false` | Draw a nonselectable separator above each source group. The Agents group always shows its boundary when the query is empty. |

`picker --icons`, `--separator-aware`, `--prompt`, `--placeholder`, `--query`, `--preview`, `--no-preview` and `--no-alias-auto` override the corresponding behavior. Worktree picker supports icons, prompt, placeholder and query overrides; Ctrl+O toggles its preview. fzf owns terminal layout and editing.

`[icons.agent]`, `[icons.rex]`, `[icons.config]`, `[icons.zoxide]`, `[icons.directory]` and `[icons.worktree]` accept `icon` and `color`. Colors are black/red/green/yellow/blue/magenta/cyan/white with optional `bright-`, or gray/grey. A session, wildcard or repository icon takes priority over its source icon.
