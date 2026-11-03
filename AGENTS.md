# AGENTS.md

Rexue connects Rex sessions and agent panes through fzf, with zoxide directory history and Worktrunk worktrees.

## Integration boundaries

- Delegate checkout discovery, paths, hooks, approvals and removal policy to Worktrunk.
- Preserve full session IDs and agent window/block IDs through selection and mutation. Picker labels and rendered rows are display data; refresh agent metadata before connecting to its pane and tab.
- Keep process arguments separate. Shell templates carry substituted paths as positional data; fzf bindings keep constant shell source and pass query/selection data through the existing environment and socket protocol.
- Test configuration precedence through `Config::load`: it normalizes empty values before picker options are resolved. Cover omitted settings, empty configuration values and explicit CLI overrides; picker prompt/placeholder overrides may intentionally be empty.

## Task references

- **CLI or configuration:** consult [configuration semantics](docs/CONFIGURATION.md), the [editable example](examples/rexue.toml) and [public contract tests](tests/scope.rs) when changing commands, settings or defaults. Keep affected examples and documentation aligned with the resulting behavior.
- **Rex shortcuts or agent navigation:** read [agent picker navigation](docs/VERIFICATION.md#agent-picker-navigation), including the floating picker window-selection requirement, before changing focus or layer placement.

## Verification

- For Rust changes, run the [development checks](README.md#development). Build the current CLI with `cargo build` before script-based acceptance; those scripts execute a binary directly.
- For picker rendering, selection or terminal behavior, run [interactive picker acceptance](docs/VERIFICATION.md#interactive-picker-acceptance). For input decoding or navigation keys, also run [extended keyboard acceptance](docs/VERIFICATION.md#extended-keyboard-acceptance).
- For Worktrunk discovery, create/connect or removal changes, run [Worktrunk acceptance](docs/VERIFICATION.md#worktrunk-acceptance).
- For shell substitution, positional scope or wildcard matching, run the relevant [focused CLI probes](docs/VERIFICATION.md#focused-cli-probes).
- Use the existing isolated fixtures for automated verification. Live Rex checks use the navigation recipe above; record whether evidence comes from fixtures or the live app. Update verification receipts only from checks actually run.
