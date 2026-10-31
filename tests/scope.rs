use clap::{CommandFactory, Parser};
use rexue::{cli::Cli, config::Config};
use std::fs;

fn load(text: &str) -> anyhow::Result<Config> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("rexue.toml");
    fs::write(&path, text)?;
    Config::load(Some(&path))
}

#[test]
fn public_commands_cover_only_sessions_worktrees_and_completion() {
    let command = Cli::command();
    let names: Vec<_> = command
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set())
        .map(|sub| sub.get_name())
        .collect();
    assert_eq!(
        names,
        [
            "list",
            "connect",
            "picker",
            "preview",
            "last",
            "worktree",
            "rename",
            "completion"
        ]
    );
    for removed in [
        "root", "r", "window", "w", "clone", "cl", "mkdir", "md", "doctor", "cache",
    ] {
        assert!(
            Cli::try_parse_from(["rexue", removed]).is_err(),
            "{removed}"
        );
    }
    for args in [
        vec!["list", "--panes"],
        vec!["list", "--tmux"],
        vec!["connect", "project", "--switch"],
        vec!["rename", "project", "--enrich"],
    ] {
        assert!(Cli::try_parse_from(std::iter::once("rexue").chain(args)).is_err());
    }
    assert!(Cli::try_parse_from(["rexue", "rename", "project", "--to", "new name"]).is_ok());
}

#[test]
fn removed_configuration_is_rejected_without_compatibility_mapping() {
    for setting in [
        "vcs='git'",
        "terminal='Rex'",
        "[[window]]\nname='editor'",
        "[default_session]\nwindows=['editor']",
        "[tui]\nshow_windows=true",
        "[tui]\nwindow_name_format='{name}'",
        "[icons.tmux]\nicon='X'",
        "sort_order=['tmux']",
        "[icons.rex-block]\nicon='X'",
        "[[worktree]]\npath='/project'\nbase_branch='main'",
    ] {
        assert!(
            load(&format!("strict_mode=true\n{setting}")).is_err(),
            "{setting}"
        );
    }
    let ignored = load("vcs='git'\nterminal='Rex'\n[[window]]\nname='editor'\n[default_session]\nwindows=['editor']").unwrap();
    assert!(ignored.session.is_empty());
}

#[test]
fn session_wildcard_metadata_and_startup_inherit_without_window_templates() {
    let cfg = load(
        r#"
strict_mode=true
[default_session]
startup_command='default'
preview_command='default-preview'
[[wildcard]]
pattern='/projects/**'
startup_command='wildcard'
preview_command='wildcard-preview'
icon='G'
[[wildcard]]
pattern='/projects/**'
startup_command='wrong-second'
[[session]]
name='app'
path='/projects/app'
startup_command=''
preview_command=''
alias='a'
alias_auto_connect=true
"#,
    )
    .unwrap();
    let resolved = cfg.resolve("/projects/app".as_ref(), Some("app")).unwrap();
    assert_eq!(resolved.startup_command.as_deref(), Some("wildcard"));
    assert_eq!(
        resolved.preview_command.as_deref(),
        Some("wildcard-preview")
    );
    let entries = cfg.entries().unwrap();
    assert_eq!(entries[0].icon.as_deref(), Some("G"));
    assert_eq!(entries[0].alias.as_deref(), Some("a"));
    assert!(entries[0].alias_auto_connect);
    let json = serde_json::to_value(&entries[0]).unwrap();
    assert!(json.get("window_names").is_none());
    assert!(json.get("active_window_name").is_none());
}

#[test]
fn imports_append_session_wildcard_and_worktree_definitions_once() {
    let dir = tempfile::tempdir().unwrap();
    let imported = dir.path().join("projects.toml");
    fs::write(&imported, "import=['missing.toml']\n[default_session]\nstartup_command='ignored'\n[[session]]\nname='imported'\npath='/imported'\n[[wildcard]]\npattern='/imported/**'\nicon='I'\n[[worktree]]\npath='/imported/repository'\n").unwrap();
    let config = dir.path().join("rexue.toml");
    fs::write(&config, format!("strict_mode=true\nimport=[{}]\n[default_session]\nstartup_command='main'\n[[session]]\nname='main'\npath='/main'\n", serde_json::to_string(&imported).unwrap())).unwrap();
    let cfg = Config::load(Some(&config)).unwrap();
    assert_eq!(
        cfg.session
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["main", "imported"]
    );
    assert_eq!(cfg.default_session.startup_command.as_deref(), Some("main"));
    assert_eq!(cfg.wildcard.len(), 1);
    assert_eq!(cfg.worktree[0].path, "/imported/repository");
}

#[test]
fn picker_defaults_and_numeric_normalization_survive_config_loading() {
    let cfg = load("strict_mode=true\ndir_length=-1\ngit_dir_length=0\n[tui]\nprompt=''\npointer='too wide'\npreview_width=1000\npreview_min_width=-1\nalias_auto_connect_delay='-1ms'").unwrap();
    assert_eq!((cfg.dir_length, cfg.git_dir_length), (1, 1));
    assert_eq!(cfg.tui.prompt, "> ");
    assert_eq!(cfg.tui.pointer, "> ");
    assert_eq!(
        (cfg.tui.preview_width, cfg.tui.preview_min_width),
        (90, 100)
    );
}
