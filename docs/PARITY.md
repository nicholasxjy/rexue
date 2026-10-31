# Sesh → Rex parity contract

**Picker migration:** this pinned inventory records the original Sesh/native-TUI target. Both current interactive pickers use system fzf. Native cursor, layout and highlight assertions below are historical; current behavior is described in [README](../README.md#fzf-picker-keys), [configuration](CONFIGURATION.md#tui-and-icons) and [fzf verification](FZF_VERIFICATION.md).

Pinned upstream inventory for goal `mvv20p1t-tgkq2f`. Source inspection establishes the reference; final implementation evidence is in [VERIFICATION.md](VERIFICATION.md) and native mappings in [DECISIONS.md](DECISIONS.md).

## Evidence and boundary

Reference: `/tmp/rex-bar-reference/joshmedeski-sesh-9f9a7ad`, supplied commit **`9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a`**. This directory has no `.git`; the SHA is the supplied identity, not independently verified with `git rev-parse`. All Sesh links below pin that exact commit. Command registration and executable code take precedence over README examples and design proposals. Dependency behavior is cited at resolved dependency commits.

Rex evidence: official [CLI documentation][rex-cli], [configuration][rex-config], [keyboard shortcuts][rex-keys], installed `rex __internal skill`, and installed `--help` output. Executable resolved to `/Users/nick/Library/Application Support/com.superlogical.rex/RexServer/Versions/Current/Rex Server.app/Contents/MacOS/rex`. Help was read for root, version, new, ls, session, session inspect/rename, window, window new/ls, block, block ls, focus, capture, send, send-key, client, actions, do, config, completion. No session/client state queries, creation, selection, renaming, input, or other session manipulation were performed. No Sesh executable was run (its initializer writes logs).

Rex docs are live official pages, **not commit-addressable source**; no Rex source SHA is asserted. Installed help is a local observation, not a GitHub citation or runtime test. The implementation subsequently queried the server version and client action schemas, including `session.select`; see VERIFICATION.md for the supported runtime and actual acceptance results.

In scope: public session-management behavior, configuration, built-in session/worktree TUIs, browser active-tab and URL-opening flows, Git/Jujutsu naming/cloning, GitHub and worktrees. Excluded: tmux backend/protocol and plugin compatibility (including tmuxp/tmuxinator/tmuxifier), external GUI launcher extensions, Windows, and multihost orchestration. Legacy tmux-named public selectors are recorded to make the inventory complete; they map to Rex resources, not a requirement to retain a tmux backend. Excluded integration knobs are identified explicitly below.

## Public commands and flags

Unless stated otherwise, boolean flags default false and string flags default empty. Every command has generated `-h/--help`; inherited `-C/--config PATH` selects the Sesh TOML file. **Local `list/picker --config/-c` is a boolean source selector that shadows the long global name**: put file selection before the subcommand or use global `-C`. Root sets `Version`; Fang is called with `WithoutVersion` (it skips Fang's override, not Cobra's version handling), so Cobra supplies `--version`, without Fang's `-v`. Cobra supplies `help` and shell completion; Fang adds hidden `man`. [Root][root] [entry point][main] [Fang dependency][fang]

| Command; aliases | Arguments / behavior | All local flags |
| --- | --- | --- |
| `list`; `l` | Ordered session names, one per line; optionally JSON array. | `-c/--config`, `-j/--json`, `-t/--tmux`, `-z/--zoxide`, `-H/--hide-attached`, `-i/--icons`, `-n/--no-color`, `-T/--tmuxinator` (excluded integration), `-d/--hide-duplicates`, `-p/--panes`, `-b/--blacklisted`, `--format STRING`, `--icons-exclude CSV` (repeatable string slice), `--no-cache`. [Source][list] |
| `connect`; `cn` | At least one word; joins words with spaces; empty name no-ops; resolve and create/reuse. | `-s/--switch`, `-c/--command STRING` (new session only), `-r/--root`, `-T/--tmuxinator` (excluded). [Source][connect] |
| `last`; `L` | Switch to second most recently attached session, rather than trusting a stale last-session pointer. | None. [Source][last] |
| `clone`; `cl` | `REPO [-- EXTRA_CLONE_FLAGS…]`; clone then connect. | `-c/--cmdDir DIR` (clone parent), `-d/--dir NAME` (new directory), `--vcs git\|jj` (overrides configuration). [CLI][clone] [Implementation][cloner] |
| `mkdir`; `md` | Exactly one `PATH`; create parents if needed, expand home/resolve relative path, then connect. | `-s/--switch`, `-c/--command STRING`. [Source][mkdir] |
| `root`; `r` | Print attached session's computed root **name**, not a filesystem path; error when no attached session. | None. [Source][root-name] |
| `rename` | `[NAME]` identifies target session, otherwise attached session. Only enrichment implemented; without flag it no-ops. | `--enrich`. [Source][rename] |
| `preview`; `p` | Exactly one `NAME_OR_DIRECTORY`; render preview without added newline. | `-w/--watch`, `--interval DURATION` (500ms). [Source][preview] |
| `picker`; `pick`, `pk` | Built-in session TUI; selection connects, cancellation succeeds without connecting. | `-c/--config`, `-t/--tmux`, `-z/--zoxide`, `-H/--hide-attached`, `-i/--icons`, `-T/--tmuxinator` (excluded), `-d/--hide-duplicates`, `-s/--separator-aware`, `-p/--prompt STRING`, `--placeholder STRING`, `-q/--query STRING`, `--no-alias-auto`, `--preview`, `--no-preview`, `--no-cache`. [Source][picker-cli] |
| `cache refresh` | Refresh session cache synchronously; no-op if disabled. `cache` has no other subcommands/flags. | None. [Source][cache-cli] |
| `window`; `w` | Group; no arguments shows help; old `window NAME` form errors with migration instruction. | None. [Source][window-cli] |
| `window list`; `l`, `ls` | No arguments; list target's window names or JSON. Target must exist; outside a session requires target. | `-t/--target SESSION`, deprecated `--session SESSION`, `-j/--json`. [Source][window-cli] |
| `window connect`; `cn`, `c` | `[NAME_OR_DIRECTORY…]` joined by spaces; select matching name or create. Omitted name always creates. | `-t/--target SESSION`, deprecated `--session SESSION` (target wins), `-c/--command STRING`, `-p/--path DIR`, `-b/--background`, `-s/--switch`, `--new`. [Source][window-cli] |
| `worktree`; `wt` | Group. | None. [Source][worktree-cli] |
| `worktree connect`; `c` | `[NUMBER]`, at most one integer; exactly one of number or browser mode required. | `-r/--repo ORG/REPO`, `-p/--pr`, `-s/--switch`, `-b/--browser`. [Source][worktree-cli] |
| `worktree list`; `l`, `ls` | No arguments; number plus two spaces plus known title, or bare number; optional JSON. | `--path ROOT`, `-r/--repo ORG/REPO`, `-j/--json`, `--refresh`. [Source][worktree-cli] |
| `worktree picker`; `pick`, `pk`, `p` | No arguments; worktree TUI; selected number reconnects through worktree workflow. | `--path ROOT`, `-r/--repo ORG/REPO`, `--refresh`, `-s/--switch`, `-i/--icons`, `-p/--prompt STRING`, `--placeholder STRING`, `-q/--query STRING`. [Source][worktree-cli] |
| `worktree browse`; `b` | No arguments; focused session's issue URL, or branch PR URL. | `-p/--pr`, `--print`. [Source][worktree-cli] |
| `completion` | Cobra-generated `bash`, `zsh`, `fish`, `powershell`; generated `--no-descriptions` on all four shell subcommands. Shell-script generation, not session interaction; Windows use excluded. | Generated `--help`; root configuration inherited. [README][readme] [Cobra dependency][cobra-completion] |
| `help [COMMAND…]`, hidden `man` | Command help / generated roff manpage; `man` accepts no positional arguments. | No application-specific flags. [Root][root] [Fang][fang] |

`list --format` and `--json` conflict. Format placeholders: `{name}` (icon-decorated when enabled), `{session}` (raw), `{source}`, `{path}`, `{active_window_name}`, `{active_window_name_prefix}` (name plus space, otherwise empty). `{fg:COLOR}…{/fg}` supports the color names below; `--no-color` removes color tokens too. Explicit empty format produces empty rows. `--icons-exclude` compares trimmed source names case-insensitively. Unknown ordinary placeholders remain text; unsupported/unterminated foreground tokens error. Arbitrarily formatted rows cannot be passed back as raw connect targets. JSON bypasses display decoration. [CLI][list] [Formatter][formatter]

## Resolution, lists, output, and lifecycle

- Alias resolution precedes the connect chain: existing pane → existing live session → excluded tmuxinator → named config → wildcard config → directory → frecency query. Source icon prefixes are stripped by `connect`/`preview`; arbitrary formatted columns are not. Reconnecting to an enriched session reuses its base name association. [Resolver][resolver] [Connect][connect] [Existing-session matching][session-matching]
- Naming resolves symlinks, then tries ordered name substitutions → Git → Jujutsu → directory. Git/JJ keep repository/workspace-relative suffixes; `git_namer_use_worktree_root` changes the anchor; `git_dir_length` controls repo-anchor components, `dir_length` controls directory fallback. Dots and colons become underscores; whitespace is normalized. Root naming substitutes Git/JJ root strategies. [Namer][namer] [Git naming][git-name] [JJ naming][jj-name] [Conversion][convert]
- Default list source order: live sessions, config, excluded tmuxinator, frecency. Source flags select a union; panes are separately selectable. `sort_order` rearranges selected sources, appending unmentioned sources; nested source arrays merge a group by descending frecency score (same path borrows score, unknown score 0, stable ties). Sources load concurrently. Blacklist patterns are regular expressions; invalid regexes are skipped; `--blacklisted` returns matches rather than normal entries. [List engine][list-engine] [Sources][sources] [Blacklist][blacklist]
- Hide-attached removes the calling attached live-session row before deduplication. Dedup priority is live sessions over config over frecency: config loses to same live name; frecency loses to kept live/config path. Live rows and pane rows survive; changing display sort does not change priority. [List engine][list-engine] [Dedup][dedup]
- `list --json` is an ordered array with Go field names: `Src`, `Name`, `Path`, `StartupCommand`, `PreviewCommand`, `DisableStartupCommand`, `Tmuxinator` (excluded), `Attached`, `Windows`, `WindowConfigs`, `WindowNames`, `Score`, `Icon`, `Alias`, `AliasAutoConnect`, `Tmuxp` (excluded), `Wildcard`, `TmuxWindows`, `Worktree`, `Created`, `LastAttached`, `Activity`, `Alerts`; internal `Group` is omitted. Nulls remain possible. Resolve startup/preview/windows fallbacks and custom icons; live windows are included regardless of `show_windows`. `WindowConfigs` carry `Name/StartupScript/Path`; live windows `Name/Index/Path/Active`; `Worktree` is null or `Repo/Number/Title/State` read from issue cache. [Model][session-model] [Resolution][list-format]
- Startup command precedence includes explicit CLI override, configured session/wildcard/default fallbacks, and disable flag. Commands only run for newly created sessions, not live reattachment. Configured windows reuse named definitions, inheriting session path when absent. `window connect` reuses name unless `--new`; a directory supplies basename and cwd; missing target can be resolved/created. A command on a new window is its process; on an existing window it is typed into the running application. Background creation preserves selection. [Window CLI][window-cli] [Window resolver][window-connect] [Startup][startup]
- Preview chain: live capture → named config command → default-config strategy → directory listing. Default directory command is `ls {}`; configured commands substitute path. Watch mode only loops for an existing live session, requires positive interval, repaints changed nonempty output (clear-screen after first frame), preserves last output across empty captures, and stops on cancellation. Built-in preview pane is separately debounced, not a continuous watch. [Preview engine][preview-engine] [Directory command][ls] [Watch][preview]
- Session cache is opt-in: `$XDG_CACHE_HOME/sesh/sessions.gob`, otherwise `~/.cache/sesh/sessions.gob`; 5s soft TTL, stale data returned while refresh runs, cold miss fetched live. `--no-cache` skips reading but writes refreshed data. `connect`, picker removal, and explicit cache refresh refresh it. GitHub issue cache is separate and does not require session cache. [Session cache][session-cache] [File cache][file-cache] [Worktree list][worktree-list]

## Complete configuration-key inventory

Runtime structures, defaults and validation are authoritative: [model][config-model], [loader][config-loader], [schema][schema]. No proposed-only key is included. Empty strings generally mean unset; booleans default false unless specified.

| Location | Keys, types, defaults / interpretation |
| --- | --- |
| Root | `cache: bool=false`; `strict_mode: bool=false`; `import: string[]`; `blacklist: string[]` regexes; `sort_order: (string\|string[])[]`; `dir_length: int=1` (values <1 reset); `git_dir_length: int=1` (same); `git_namer_use_worktree_root: bool=false`; `vcs: string` (`git` fallback; `jj` supported); `separator_aware: bool=false`; `terminal: string` (macOS app activation); `tmux_command: string` (excluded backend override). |
| `[default_session]` | `startup_command: string`, `preview_command: string`, `windows: string[]`; excluded integration keys `tmuxp: string`, `tmuxinator: string`. `{}` substitutes session path in commands. |
| `[[session]]` | `name: string`, `path: string`, `disable_startup_command: bool=false`, `alias: string`, `alias_auto_connect: bool=false`, `icon: string`; all `[default_session]` keys also accepted here. |
| `[[window]]` | `name: string`, `startup_script: string`, `path: string` (inherits session path). **`startup_script` is a window key; it is not a supported session/default-session key.** |
| `[[wildcard]]` | `pattern: string`, `startup_command: string`, `disable_startup_command: bool=false`, `preview_command: string`, `windows: string[]`, `icon: string`. Explicit session wins; first matching wildcard wins. Go `path.Match` syntax: `^` negates classes, `!` remains literal, backslash escapes, and stars stay inside path components. Only trailing `/**` enables recursion through a literal prefix; middle `**` has ordinary star semantics. |
| `[[name_substitution]]` | `find: string` (nonempty), `replace: string`, `regex: bool=false`; ordered rewrites over home-collapsed path; regex supports `$1` captures; no changed result falls back to normal naming. Invalid regex errors at load. |
| `[[worktree]]` | `repo: string` (`org/repo`), `path: string` (home expansion), `worktree_dir: string=".wk"` (relative to path or absolute), `branch_template: string="{number}"`, `base_branch: string="origin/main"`, `fetch: bool=true`, `startup_command: string`, `create_command: string`, `icon: string`. |
| `[browser]` | `application: string` (empty disables active-tab lookup), `url_command: string="URL of active tab of front window"`; Safari needs `URL of current tab of front window`. macOS active-tab lookup uses AppleScript. |
| `[frecency]` | `list_command: string="zoxide query --list --score"`; `query_command: string="zoxide query {}"`; `add_command: string="zoxide add {}"`; `remove_command: string="zoxide remove {}"`. Empty strings fall back to the corresponding default zoxide command. One binary plus arguments, no shell pipes/redirection. One path per output line, optional leading numeric score. Source name remains `zoxide`. |
| `[tui]` | `show_icons: bool=false`, `show_windows: bool=false`, `window_name_format: string="#{window_name}"` (ordinary window/session name, index and active fields map to native metadata; arbitrary tmux expressions remain backend-specific), `prompt: string="> "`, `placeholder: string` (normal loader default **`Filter Sessions...`**), `pointer: string="> "`, `alias_auto_connect_delay: duration-string="150ms"`, `alias_filter_prefix: string="/"`, `preview: bool=false`, `preview_width: int=60`, `preview_min_width: int=100`, `preview_border: string="line"`, `group_separator: bool=false`. |
| `[icons.SOURCE]` / `[icons] SOURCE={…}` | Sources `tmux`, `config`, `zoxide`, `tmux-pane`, excluded `tmuxinator`; each has `icon: string`, `color: string`. Missing fields retain built-ins. Unknown source/color errors. Colors: `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`; all corresponding `bright-*`; `gray` and `grey` alias `bright-black`. |

Config file defaults to `$XDG_CONFIG_HOME/sesh/sesh.toml` or `~/.config/sesh/sesh.toml`; missing default file is tolerated, explicitly selected missing file errors. Imports expand environment/home and append **sessions, windows and wildcards only**, not other settings or worktree blocks; relative import paths resolve from process cwd. Strict mode rejects unknown fields in the primary file; imports use plain TOML unmarshal. Duplicate aliases are rejected case-insensitively. Alias delay must parse as a Go duration (negative values are accepted and fire immediately); alias prefix is empty (disabled) or one non-whitespace rune. `preview_width` falls back for <=0 and clamps positive values to 10–90; `preview_min_width<=0` falls back to 100; invalid border falls back to `line`. Pointer width >2 warns and falls back. [Loader][config-loader] [Picker defaults][picker] [Model][tui]

## Built-in TUI behavior, keys, layout, colors and glyphs

This inventory concerns the built-in TUIs, **not** README fzf/television/gum recipes. In particular Tab/Shift+Tab are not list-navigation bindings, Ctrl+A/T/G/F are not source selectors, and session Ctrl+D is half-page movement, not kill. No keymap configuration, help footer, mouse-select handler, source-switch tabs or multi-selection is implemented in these models. [Session model][tui] [Worktree model][worktree-tui]

| Context | Keys / exact behavior |
| --- | --- |
| Both lists | Enter chooses highlighted row and exits (ignored during initial loading; empty list exits without selection). Esc / Ctrl+C cancel. Up / Ctrl+K / Ctrl+P move up one; Down / Ctrl+J / Ctrl+N move down one; Ctrl+U / Ctrl+D move by half visible rows. Movement clamps, never wraps. Text changes reset cursor and scroll offset. |
| Session list | Ctrl+O toggles preview (inert without preview function); Ctrl+X requests remove/kill confirmation for frecency/live row only; other sources show status. A leading `/` enters alias mode (configurable/disableable); leading `#` enters index mode unless alias prefix is `#`. In index mode 1–9 immediately choose corresponding filtered-list row; nonexistent position is swallowed; positions >9 unnumbered. |
| Session aliases | Exact case-insensitive raw alias overrides fuzzy ranking, including a target absent from loaded sources. Alias mode orders alias-prefix matches before name-substring matches. In normal mode auto-connect requires session opt-in; in alias mode exact alias auto-connects regardless of opt-in. 150ms default delay allows longer aliases; stale timers invalidated. `--no-alias-auto` suppresses it. Prefilled `--query` supports sigils but never auto-connects by itself. |
| Worktree list | Ctrl+R refetches titles/states ignoring cache, preserving usable rows/query/cursor position; inert while loading/refreshing. No session alias/index modes, previews or Ctrl+X removal handler. |
| Confirmation | Starts on **Yes**. Y/y confirms; N/n, lowercase q, or Esc cancel (uppercase Q is not bound). Enter applies focused button. Left/Right/Tab/Shift+Tab/h/l toggle focus. Ctrl+C quits entire picker. All other keys swallowed. Backend work is async; row removed only on success, cache refreshed; failure leaves row and status. |
| Filter editing inherited from Bubbles | Right/Ctrl+F and Left/Ctrl+B move character; Alt+Right/Ctrl+Right/Alt+F and Alt+Left/Ctrl+Left/Alt+B move word; Alt+Backspace/Ctrl+W/Ctrl+Backspace delete previous word; Alt+Delete/Alt+D/Ctrl+Delete delete next word; Backspace/Ctrl+H delete previous character; Delete deletes next character; Home/Ctrl+A and End/Ctrl+E move to ends; Ctrl+V pastes. Ctrl+K/U/D and Up/Down/Ctrl+N/P are intercepted by picker movement, so their textinput delete/suggestion meanings do not apply. Tab is suggestion acceptance, but suggestions are not populated. [Pinned dependency][textinput] |

Session matching uses `sahilm/fuzzy`, stable descending score with unmatched-length penalty capped at 5 so long strong matches remain competitive. `separator_aware` normalizes `-`, `_`, `/`, `\` to spaces for fuzzy matching, not exact aliases. Window labels are display-only, not searchable/selectable values. [Session model][tui]

| Visual surface | Inventory / dimensions |
| --- | --- |
| Shared frame | Alternate screen, terminal-height frame preserving prior scrollback. Two-column filter indent; filter row then blank/status row; remaining `height-2` rows for list (fallback 5). Default prompt `> `, pointer `> ` inside exactly 2 cells; custom narrower pointer padded. Scroll just enough to keep cursor visible. No header/title/footer or outer list border. |
| Session row | Pointer; optional source/custom icon; optional alias chip plus space; highlighted name; optional dim window names separated by one space, elided as ` +N`. List width capped at 60 columns; very narrow width <30 falls back to 40. No explicit clipping of normal session names in list-only mode. Group boundaries are nonselectable `─` rules with 2-cell indent, suppressed when query nonempty. |
| Preview pane | Opt-in; terminal must reach default 100 cols. Preview share is at least configured percentage (default 60%), gains all space beyond list's 60-col cap, but leaves at least 40 cols for list. One cell padding after left divider; border choices normal `│`, thick `┃`, double `║`, none. Starts content below two header rows. Clips lines/width with ANSI reset containment. Debounce 60ms; stale async responses discarded; returning from hidden preview refetches. |
| Alias chip | Nerd Font caps `` U+E0B6, `` U+E0B4 around reverse-video alias; otherwise reversed `[alias]`; trailing space. Matching leading alias runes/caps green. |
| Worktree row | Pointer, right-aligned number (width of widest loaded number), 2 spaces, state badge column 9 cells, title. Search combines `<number> <title>`; empty query preserves numeric ascending order; fuzzy uses same capped scorer. Title clipped with `…` U+2026. Full terminal width; no preview or session-source icon column. |
| State badges | `OPEN`, `CLOSED`, `MERGED`; unknown/unresolved state is blank 9-cell column. Nerd Font caps as above, otherwise filled spaces; reverse-video label uses terminal background for text. |
| Confirmation | Centered rounded box `╭─╮│╰─╯`; inner width 40–60 (attempts terminal width minus 6, minimum 40), padding 1 vertical / 2 horizontal. If too small, dialog alone. Prompt, blank, bold truncated name, optional faint path when removing a differently named frecency entry, blank, ` Yes ` + 3 spaces + ` No `. Focused button reverse+bold; other faint. |
| Status text | Session: `Loading sessions...`, `No aliases configured`; no special general no-results label. Preview: `Loading preview...`, `No preview`, `Preview unavailable: ERROR`. Removal status names unsupported source or backend error; clears on next key. Worktree: `Loading worktrees...`, `No worktrees found`, `Refreshing worktrees...`, `Refresh failed: ERROR` (header clipped with ellipsis). |

Visual sources: [session model][tui], [confirmation][confirm], [worktree model][worktree-tui], [icon cells][icon-cell]. Glyphs are font-dependent; there are no fixed RGB theme colors. ANSI palette indices and SGR values below specify the contract, so a terminal theme determines actual shades.

| Color / glyph role | Exact default |
| --- | --- |
| Pointer; session fuzzy matches | ANSI 2 green, bold. |
| Alias match; chip | ANSI 2; reverse video label, ordinary default-foreground caps before matching. |
| Index numbers; confirm border | ANSI 4 blue. |
| Window names; separators; preview border | ANSI 8 bright-black/gray (window labels also faint). |
| Session removal/error status | ANSI 3 yellow. |
| Worktree fuzzy number/title matches | **ANSI 1 red**, bold (different from session matches). |
| Worktree OPEN / CLOSED / MERGED | ANSI 2 green / 1 red / 5 magenta-purple, reverse-filled labels. |
| Ordinary names / custom icons | Terminal default foreground; per-session/wildcard/worktree icons intentionally unstyled. |
| Loading/empty/preview status | Faint terminal foreground. |
| Text input dependency defaults | Focused prompt indexed color `7`, placeholder/suggestion `240`, text default foreground. Cursor default color `7`, block, blink; session model explicitly selects real terminal cursor, clears explicit cursor color and disables blinking. Worktree model keeps textinput's virtual cursor defaults. [Pinned styles][input-styles] |
| Source `tmux` → live Rex session | `` U+EBC8, blue SGR 34 / ANSI 4. |
| Source `config` | `` U+E615, bright-black SGR 90 / ANSI 8. |
| Source `zoxide` | `` U+F114, cyan SGR 36 / ANSI 6. |
| Source `tmux-pane` → Rex block | `` U+EBC8, green SGR 32 / ANSI 2. |
| Excluded source `tmuxinator` | `` U+EBEB, yellow SGR 33 / ANSI 3; listed solely to document removed surface. |
| Unknown source fallback | `? `, ANSI 8; used by icon-cell rendering. |

[Glyph definitions][icons]. Custom icon precedence: exact configured session name → configured path → worktree root → first matching wildcard → source glyph. Icon column pads to widest configured session/wildcard/source icon display width (at least 1; **worktree-only icons are omitted by `ColumnWidth`**), trailing spaces are manual compensation and excluded from column-width calculation. `show_icons=false` hides custom/source icons and uses ASCII chip caps. `list --icons` keeps source glyphs even when custom icon is configured; JSON reports resolved custom icon. [Resolver][icon-resolver] [Cell rendering][icon-cell]

## Browser, GitHub, worktree contract

- Resolve configured repo via explicit `--repo`, or detect main Git worktree from cwd and compare real paths. List/picker `--path` selects configured worktree root; `--repo` wins when both are supplied. Worktree directories default `.wk/NUMBER`, branch template `{number}`, base `origin/main`. Fetch defaults true and its errors are ignored. [Workflow][worktree] [Listing][worktree-list]
- Probe `gh pr view NUMBER --repo REPO` for author, closing references, title/body. A foreign PR uses detached worktree plus `gh pr checkout`; existing foreign PR checkout refreshes and attempts pull (pull errors ignored). Own PR prefers first closing issue, then first `#N` title/body reference, otherwise detached PR checkout. Plain issue creates named branch. Initial fetch happens even when worktree exists. `gh` authentication is external; PR lookup command failures are treated as not-found, not all propagated errors. [Workflow][worktree] [GitHub adapter][github]
- `create_command` and `startup_command` are exclusive: creation chooses create command; existing directory chooses startup command. Live session reattachment still runs neither. No implicit fallback from absent create command to startup command. [Workflow][worktree]
- Numeric directories only, ascending integer order; no `.git` validation required to include an entry. Listing JSON fields: `Number`, `Title`, `State`, `Path`, `Created` (`.git` modification time when stat succeeds), `Icon`. Missing root means empty list. Batch GraphQL issue/PR title+state lookups use `$XDG_CACHE_HOME/sesh/github-issues.v1.json` (default `~/.cache/sesh`), positive TTL 24h, negative TTL 1h. Failed network refresh retains stale titles or bare numbers. [List implementation][worktree-list] [JSON model][git-model] [Batch queries][issues]
- Worktree picker first displays cached data then refreshes once behind it; automatic refresh failure is quiet, explicit Ctrl+R error is visible. `--refresh` refreshes initial load and disables duplicate automatic refresh. [Worktree TUI][worktree-tui] [CLI][worktree-cli]
- Active browser lookup requires macOS and configured app; `osascript` reads active front-window tab URL. Accept GitHub issue/pull URL, optional scheme/www, trailing path/query/fragment; extract repo/number/kind. Number and `--browser` together error; explicit repo overrides URL repo, PR flag ORs with URL kind. [Browser][browser] [Workflow][worktree]
- Browse derives numbered root directory from focused session path (calling session inside terminal, resolved focused client outside), supports nested paths/symlinks. Default URL is `https://github.com/ORG/REPO/issues/N`; PR mode asks `gh pr view --json url --jq .url` in worktree. Print only avoids opening. macOS opens in configured app or default; Linux uses `xdg-open`. [Browser][browser] [Workflow][worktree] [CLI][worktree-cli]
- Enrichment recomputes base name from path, parses first digits in branch, asks `gh issue view`, sanitizes title, and uses `BASE — TITLE` (em dash U+2014 with surrounding spaces). Missing issue removes stale suffix; plain `rename` is intentionally unimplemented. [Rename][rename] [GitHub][github]

## Rex mapping and hard mismatches

These are documented building blocks and adapter requirements, not commands executed during this research. **No direct native Sesh CLI/TOML/TUI parity is established.** Absence statements concern inspected official docs and installed help; they do not rule out uninspected app features or custom actions.

| Sesh behavior | Official/installed Rex interface | Hard mismatch / required adaptation |
| --- | --- | --- |
| Existing session list | `rex ls/list/session ls --json`; `-q/--quiet` IDs only, `--short=false` full IDs. | Rex JSON is object with `sessions`, not Sesh array; labels/IDs are not source names/path catalog. Sesh filters, frecency/config merge, blacklist, dedup, formats and aliases need adapter. |
| Create session + startup | `rex new [LABEL] -c/--cwd DIR [-- COMMAND…]`; `--window`, `--keep-open`, `--shell`, `--json`. | Creates new session; no documented atomic resolve-or-create or directory-derived naming. New session is not automatically displayed in app. Persist returned ID; independently resolve reuse and client selection. |
| Switch/show a session | Documented client action `session.select session_id=ID` (optional `window_id`); `rex -C CLIENT do …`. Docs also show `session.switch` as interactive chooser. | Client action requires actual connected client/action availability; chooser is not exact target selection. No terminal attach replacement demonstrated. No assumption that creating session changes app focus. |
| Target resolution | Full ID → case-insensitive label → 1-based position → ID suffix >=4 chars; ambiguous/not-found status 3. | Rex labels may duplicate; Sesh aliases/path/source/naming and enriched base-name reuse must be resolved separately. Full IDs are essential after lookup. |
| Last/root/enriched rename | `rex session inspect [SESSION] --json`, `rex session rename [SESSION] LABEL`. | No documented Sesh MRU/root/enrichment command. Need local MRU/naming/GitHub policy; raw rename alone does not supply it. |
| List/create/select window | `rex window ls -s SESSION --json`; `rex window new -s SESSION [LABEL] -c DIR --focus=false [-- COMMAND…]`; `rex focus -s SESSION WINDOW_ID`. | Rex `window` means tab. New always creates; Sesh name-or-directory reuse and missing-session creation need adapter. `--focus=false` only preserves existing focus when there is an active window. Server focus and app session selection differ. |
| Command in existing window | `rex send -s SESSION -w WINDOW -b BLOCK TEXT`, then `rex send-key … enter`. | Window may contain several blocks; must resolve focused block. Sending targets running program, not guaranteed shell. No combined Sesh `window connect --command` behavior. |
| Pane list/select | `rex block ls -s SESSION [-w WINDOW] --json`; `rex focus -s SESSION BLOCK_ID`. | Convert live blocks into legacy pane source rows; Rex IDs, not tmux pane handles. |
| Live preview | `rex capture -s SESSION [-w WINDOW] [-b BLOCK] --format text\|html\|vt --trim --unwrap`. | Captures screen + up to about 64MiB scrollback; Sesh previews a live pane screen. Need focused-block choice and clipping; no documented native session/directory preview command or preview watch option. |
| Delete/remove | `rex kill SESSION` / `rex session kill`; separate frecency backend remove command. | Confirmation UI, source distinction and cache refresh belong to adapter. No mutation was tested here. |
| Config/key bindings | Rex `init.lua`; `$XDG_CONFIG_HOME/rex/init.lua` or `~/.config/rex/init.lua`; server `REX_CONFIG`; `rex.bind`, `rex.unbind`, `require`, custom actions; `config check/reload`. | Sesh TOML is not Rex config. `-C` means **client** in Rex, config file in Sesh. Rex `-s` means **session selector**, Sesh `-s` often switch/separator-aware; do not forward flags blindly. |
| Built-in picker UI | Official docs expose client actions, bindings, modes and command palette; no equivalent terminal picker in inspected CLI help. | Entire key/layout/color/glyph/alias/preview/confirmation contract above remains adapter work; a native chooser is not visual or behavioral parity. |
| Clone/mkdir/Git/JJ naming | Rex supplies cwd + command process creation only. | Filesystem and VCS setup, startup distinctions and naming must happen outside native Rex CLI. |
| GitHub/worktrees/browser | No worktree/GitHub/browser subcommands in installed root help or inspected official CLI docs. | Preserve workflows with Git, gh, AppleScript/open/xdg-open and adapter config; do not substitute app browser features or exclude them. |
| Session/issue caches | Rex lists live resources and exposes events/Lua automation. | Sesh cache policy and issue enrichment are adapter-owned; event support does not prove matching cache semantics. |
| Completion/help | `rex completion bash/zsh/fish/powershell/nu`, `rex help`, `--help`. | Native Rex scripts complete Rex, not the Sesh-compatible adapter surface. |

Rex evidence for this table: [official CLI taxonomy/targets/JSON/exit codes][rex-cli], [configuration][rex-config], [bindings/actions][rex-keys], and the installed help commands named under Evidence. Global Rex flags observed: `-S/--server`, `-C/--client`, `--autostart` (true), `--timeout` (10s), `--color auto|always|never` (auto), `-v/--verbose` repeatable, `-h/--help`. Multihost use is excluded. Documented CLI exit codes are 0 success, 1 failure, 2 invalid CLI, 3 target failure/ambiguity, 4 unreachable server, 130 interrupted; waited processes may return their own status.

## Reference inconsistencies to preserve as explicit decisions

- **Session `picker --separator-aware` is wired but ineffective:** CLI sets `PickerOptions.SeparatorAware`, while `RealPicker.Pick` passes `p.config.SeparatorAware` directly into model options. Configuration works; invocation flag does not override. [CLI][picker-cli] [Picker][picker]
- **Session `picker --icons=false` enables icons:** it checks flag presence then sets true. Worktree picker correctly reads boolean value. `--no-preview=false` still hides preview because presence wins. [Session CLI][picker-cli] [Worktree CLI][worktree-cli]
- **Worktree picker `--path` loses repo selection on connect:** list resolves path, but chosen-number connect receives only `listOpts.Repo`; without explicit `--repo`, it falls back to cwd, possibly failing or selecting another configured repo. A path-scoped list is not proof of correctly scoped connect. [pickWorktree][worktree-cli]
- **Placeholder casing differs:** config loader injects `Filter Sessions...`; picker fallback is `Filter sessions...`; worktree fallback `Filter worktrees...` is overridden by common configured/loader placeholder. Do not claim README sample capitalization is exact runtime default. [Loader][config-loader] [Picker][picker]
- **Worktree title clipping uses rune count against cell budget:** wide title glyphs can still exceed available display width despite declared clipping intent. Status clipping has a similar limitation. [Worktree rendering][worktree-tui]
- **Worktree-only custom icons do not affect icon-column sizing:** `IconResolver` returns them, but `ColumnWidth` only scans session, wildcard and source icons, so a wide worktree icon can break alignment. [Resolver][icon-resolver] [Width calculation][icon-cell]
- **Design/status prose is not a command:** registered root has no public `status`, `worktree create`, `worktree remove`, or standalone browser command. Do not implement a plan's proposed command as established parity. [Root registration][root] [Worktree registration][worktree-cli]

## Acceptance evidence

The reference inventory above remains unchanged. These checks describe the native port, with explicit differences in DECISIONS.md; they do not assert byte-for-byte JSON compatibility or identical fuzzy scores.

| Area | Evidence |
| --- | --- |
| Baseline | GitHub commit API and archive URL fixed to `9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a`; commit links retained. |
| Public interface | CLI tree registration tests, source-selector/config-file separation, all documented help paths, three shell completion scripts, and real command workflows. |
| Rex protocol boundary | Installed whoami/list/inspect/action schemas, JSON/argument/error integration tests, duplicate target tests, real app selection and cleanup. |
| Session lifecycle | Canonical paths, intelligent Git/JJ naming, enriched-name reuse, stable IDs, startup once, templates, last/history, clone/mkdir/root, previews and deletion. |
| Configuration | Load/strict/import/cycle tests, first-wildcard/explicit-empty precedence, aliases, source icons, frecency argument tests and valid example. |
| Listing | Isolated sources, stable merged score order, regex blacklist inversion, fixed dedup priority, current-row exclusion and native formatting/JSON tests. |
| Native TUIs | Unit state/render tests at 80/120/narrow widths, Unicode editing, aliases/index/group/confirmation, bounded async previews and worktree refresh; real PTY and Rex frames. |
| Removal | Yes-first confirmation and errors in fixtures; actual cancel then confirm closes only a temporary Rex session. Frecency remove is tested with an isolated command backend. |
| Git/GitHub | Real Git clone/root and GitHub issue worktree; own/foreign PR, branch checks, offline/negative-cache/refresh cases in fixtures. JJ metadata in fixtures; real JJ clone not run because `jj` is absent. |
| Browser/platform | GitHub URL parsing and AppleScript adapter fixtures; actual browse URL printing. Real browser UI/Automation prompts and Linux URL opening are not device-verified. |
| Decisions | Native JSON/config-name/cache/matcher mappings and repaired upstream inconsistencies are recorded explicitly in DECISIONS.md. |
| Scope | No tmux backend/plugins, desktop extensions, Windows port, multihost orchestration or remote publication. |

See [VERIFICATION.md](VERIFICATION.md) for exact commands, pass counts, live smoke scripts and receipts.

## Pinned source index

[root]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/seshcli/root_command.go
[main]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/main.go
[list]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/seshcli/list.go
[connect]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/seshcli/connect.go
[last]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/seshcli/last.go
[clone]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/seshcli/clone.go
[cloner]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/cloner/cloner.go
[mkdir]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/seshcli/mkdir.go
[root-name]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/seshcli/root.go
[rename]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/seshcli/rename.go
[preview]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/seshcli/preview.go
[picker-cli]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/seshcli/picker.go
[cache-cli]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/seshcli/cache.go
[window-cli]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/seshcli/window.go
[worktree-cli]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/seshcli/worktree.go
[readme]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/README.md
[config-model]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/model/config.go
[config-loader]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/configurator/configurator.go
[schema]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/sesh.schema.json
[resolver]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/connector/connect.go
[session-matching]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/lister/tmux.go
[namer]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/namer/namer.go
[git-name]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/namer/git.go
[jj-name]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/namer/jj.go
[convert]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/namer/convert.go
[list-engine]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/lister/list.go
[sources]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/lister/srcs.go
[blacklist]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/lister/blacklist.go
[dedup]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/lister/dedup.go
[session-model]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/model/sesh_session.go
[list-format]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/lister/format.go
[formatter]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/formatter/formatter.go
[window-connect]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/connector/connect_window.go
[startup]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/startup/startup.go
[preview-engine]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/previewer/previewer.go
[ls]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/ls/ls.go
[session-cache]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/lister/caching_lister.go
[file-cache]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/cache/cache.go
[picker]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/picker/picker.go
[tui]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/picker/tui.go
[confirm]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/picker/confirm.go
[worktree-tui]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/picker/worktree.go
[icons]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/icon/icon.go
[icon-cell]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/icon/cell.go
[icon-resolver]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/lister/icon.go
[worktree]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/worktree/worktree.go
[worktree-list]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/worktree/list.go
[git-model]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/model/connect_opts.go
[github]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/github/github.go
[issues]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/github/issues.go
[browser]: https://github.com/joshmedeski/sesh/blob/9f9a7ad682d3c1a5917af2ff66fba7d0f4a9456a/browser/browser.go
[textinput]: https://github.com/charmbracelet/bubbles/blob/490948109eb4731927ba2f4cd464d8175a9630d7/textinput/textinput.go
[input-styles]: https://github.com/charmbracelet/bubbles/blob/490948109eb4731927ba2f4cd464d8175a9630d7/textinput/styles.go
[fang]: https://github.com/charmbracelet/fang/blob/d89b30af5d6d3a6e3871631d0b62afe2e8c97739/fang.go
[cobra-completion]: https://github.com/spf13/cobra/blob/88b30ab89da2d0d0abb153818746c5a2d30eccec/completions.go
[rex-cli]: https://www.superlogical.com/rex/docs/automate/cli.md
[rex-config]: https://www.superlogical.com/rex/docs/customize/config.md
[rex-keys]: https://www.superlogical.com/rex/docs/customize/keyboard-shortcuts.md
