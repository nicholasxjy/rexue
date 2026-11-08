# AGENTS.md

Rexue connects Rex sessions and agent panes through fzf, with Worktrunk worktrees.

## Boundaries

- Delegate checkout discovery, paths, hooks, approvals and removal policy to Worktrunk.
- Preserve full session/window/block IDs through selection and mutation; labels are display data. Refresh agent metadata before focusing its tab and pane.
- Keep process arguments separate. Pass shell-template paths as positional data; keep fzf binding source constant and carry queries/selections through the environment/socket protocol.
- Test configuration precedence through `Config::load`, which normalizes empty settings before picker resolution. Cover omitted/empty settings and CLI overrides; CLI prompt/placeholder values may intentionally be empty.

## Task references

- **Commands, settings or defaults:** consult [configuration](docs/CONFIGURATION.md), the [example](examples/rexue.toml) and [contract tests](tests/scope.rs); align affected docs/examples.
- **Focus, layer placement or shortcuts:** read [agent navigation](docs/VERIFICATION.md#agent-picker-navigation), including the floating window-selection requirement.

## Checks

- **Rust:** run [Rust checks](docs/VERIFICATION.md#rust-checks). Build the current binary before script acceptance.
- **Picker rendering, selection or terminal behavior:** run [interactive acceptance](docs/VERIFICATION.md#interactive-picker-acceptance); add [keyboard acceptance](docs/VERIFICATION.md#extended-keyboard-acceptance) for decoding/navigation keys.
- **Worktrunk discovery, create/connect or removal:** run [Worktrunk acceptance](docs/VERIFICATION.md#worktrunk-acceptance).
- **Shell substitution, positional scope or wildcards:** run the relevant [CLI probes](docs/VERIFICATION.md#focused-cli-probes).
- **Startup or resource use:** run [performance checks](docs/VERIFICATION.md#startup-and-idle-performance).
- Use isolated fixtures for automation and the navigation recipe for live checks. Record fixture versus live evidence; update receipts only from checks run.
