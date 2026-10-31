#![cfg(unix)]

use rexue::{
    backend::Rex,
    config::{Config, FrecencyConfig, SessionConfig, SortGroup, WindowConfig},
    model::Source,
    session::{ConnectOptions, ListOptions, Manager},
};
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};
use tempfile::TempDir;

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn executable(path: &Path, script: &str) {
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

struct FakeRex {
    dir: TempDir,
    rex: Rex,
}

impl FakeRex {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("rex fake ' ; spaces ")
            .tempdir()
            .unwrap();
        let bin = dir.path().join("fake rex ; executable");
        fs::create_dir(dir.path().join("calls")).unwrap();
        let script = format!(
            r#"#!/bin/sh
root={}
n=0
[ ! -f "$root/count" ] || n=$(/bin/cat "$root/count")
n=$((n + 1))
printf '%s' "$n" > "$root/count"
printf '%s\000' "$@" > "$root/calls/$n"
if [ -f "$root/fail.exit" ]; then
    /bin/cat "$root/fail.stderr" >&2
    exit "$(/bin/cat "$root/fail.exit")"
fi
shift 3
if [ "$1" = '-S' ]; then shift 2; fi
case "$1 $2" in
    'whoami --json') /bin/cat "$root/context.json" ;;
    'client ls') /bin/cat "$root/clients.json" ;;
    '-C '*)
        shift 2
        case "$1" in
            actions) /bin/cat "$root/actions.json" ;;
            do) printf '{{}}' ;;
            *) exit 91 ;;
        esac ;;
    'ls --json') /bin/cat "$root/sessions.json" ;;
    'session inspect')
        if [ -f "$root/inspect-$3.exit" ]; then
            printf 'inspect fixture failure' >&2
            exit "$(/bin/cat "$root/inspect-$3.exit")"
        fi
        /bin/cat "$root/inspect-$3.json" ;;
    'block call') /bin/cat "$root/process.json" ;;
    'new '*)
        /bin/cp "$root/after-create.json" "$root/sessions.json"
        /bin/cat "$root/created.json" ;;
    'window new') /bin/cat "$root/window.json" ;;
    'session rename'|'kill '*|'send '*|'send-key '*|'focus '*) printf '{{}}' ;;
    'capture '*) printf 'terminal fixture\n' ;;
    *) printf 'unexpected fake Rex command: %s %s\n' "$1" "$2" >&2; exit 92 ;;
esac
"#,
            shell_quote(dir.path().to_str().unwrap())
        );
        executable(&bin, &script);
        let fake = Self {
            dir,
            rex: Rex {
                bin,
                server: Some("unix:/tmp/server ' ; $socket".into()),
                client: Some("client:desktop-full".into()),
            },
        };
        fake.json("sessions.json", json!({"sessions": []}));
        fake.json(
            "clients.json",
            json!({"clients": [client("client:desktop-full", "Desk", "app", "connected")]}),
        );
        fake.json(
            "actions.json",
            json!({"actions": [{"name": "session.select"}]}),
        );
        fake.json(
            "context.json",
            json!({"session_id": "session:previous-full"}),
        );
        fake.json("created.json", json!({"session_id": "session:created-full", "initial_windows": [{"block_ids": ["block:initial-full"]}]}));
        fake.json("window.json", json!({"window_id": "window:extra-full"}));
        fake.session("session:created-full", "project", Path::new("/unused"));
        fake.json(
            "after-create.json",
            json!({"sessions": [{"session_id": "session:created-full", "label": "project"}]}),
        );
        fake
    }

    fn json(&self, name: &str, value: Value) {
        fs::write(
            self.dir.path().join(name),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    }

    fn session(&self, id: &str, label: &str, path: &Path) -> Value {
        self.json(&format!("inspect-{id}.json"), json!({
            "active_window_id": "window:active-full",
            "windows": [
                {"window_id": "window:first-full", "label": "wrong", "focused_block_id": "block:wrong"},
                {"window_id": "window:active-full", "label": "Editor", "focused_block_id": "block:focused-full"}
            ]
        }));
        self.json("process.json", json!({"foreground": {"cwd": path}}));
        json!({"session_id": id, "label": label})
    }

    fn calls(&self) -> Vec<Vec<String>> {
        let count = fs::read_to_string(self.dir.path().join("count"))
            .unwrap_or_else(|_| "0".into())
            .parse::<usize>()
            .unwrap();
        (1..=count)
            .map(|n| {
                fs::read(self.dir.path().join(format!("calls/{n}")))
                    .unwrap()
                    .split(|b| *b == 0)
                    .filter(|s| !s.is_empty())
                    .map(|s| String::from_utf8(s.to_vec()).unwrap())
                    .collect()
            })
            .collect()
    }

    fn manager(&self, config: Config) -> Manager {
        Manager {
            rex: self.rex.clone(),
            config,
            state_dir: self.dir.path().join("manager state"),
        }
    }
}

fn client(id: &str, label: &str, kind: &str, connection: &str) -> Value {
    json!({"client_id": id, "info": {"label": label, "kind": kind, "platform": "macos"}, "connection_state": connection})
}

#[test]
fn connect_root_preserves_plain_directories_with_and_without_git() {
    for no_git in [false, true] {
        let fake = FakeRex::new();
        let project = fake.dir.path().join("plain $REXUE_ROOT_LITERAL_UNSET");
        fs::create_dir(&project).unwrap();
        fake.session("session:created-full", "project", &project);
        let cfg = fake.dir.path().join("root-connect.toml");
        fs::write(&cfg, "[frecency]\nadd_command='/usr/bin/true'\n").unwrap();
        let tools = fake.dir.path().join("empty-tools");
        fs::create_dir(&tools).unwrap();
        for created in [true, false] {
            let mut cmd = Command::new(env!("CARGO_BIN_EXE_rexue"));
            cmd.args(["--detach", "--rex-bin"])
                .arg(&fake.rex.bin)
                .arg("-C")
                .arg(&cfg)
                .args(["connect", "--root", project.to_str().unwrap(), "--json"])
                .env("REXUE_STATE_DIR", fake.dir.path().join("state"))
                .env_remove("REX_SESSION");
            if no_git {
                cmd.env("PATH", &tools);
            }
            let r = cmd.output().unwrap();
            assert!(
                r.status.success(),
                "missing_git={no_git}: {}",
                String::from_utf8_lossy(&r.stderr)
            );
            let c: Value = serde_json::from_slice(&r.stdout).unwrap();
            assert_eq!(c["created"], created);
            assert_eq!(
                Path::new(c["path"].as_str().unwrap())
                    .canonicalize()
                    .unwrap(),
                project.canonicalize().unwrap()
            );
        }
        let calls = fake.calls();
        let new: Vec<_> = calls
            .iter()
            .filter(|a| a.iter().any(|v| v == "new"))
            .collect();
        assert_eq!(new.len(), 1);
        assert!(new[0].windows(2).any(|p| p[0] == "-c"
            && Path::new(&p[1]).canonicalize().ok() == Some(project.canonicalize().unwrap())));
    }
}
#[test]
fn connect_root_detects_git_jj_and_bare_before_selecting_configuration() {
    for kind in ["git", "jj", "bare"] {
        let fake = FakeRex::new();
        let repo = fake.dir.path().join("repository");
        fs::create_dir(&repo).unwrap();
        let (target, expected) = match kind {
            "git" => {
                let r = Command::new("git")
                    .args(["init", "-b", "main"])
                    .arg(&repo)
                    .output()
                    .unwrap();
                assert!(r.status.success());
                let child = repo.join("child");
                fs::create_dir(&child).unwrap();
                (child, repo.clone())
            }
            "jj" => {
                fs::create_dir(repo.join(".jj")).unwrap();
                let child = repo.join("child");
                fs::create_dir(&child).unwrap();
                (child, repo.clone())
            }
            _ => {
                let bare = repo.join(".bare");
                let r = Command::new("git")
                    .args(["init", "--bare"])
                    .arg(&bare)
                    .output()
                    .unwrap();
                assert!(r.status.success());
                (bare.clone(), bare)
            }
        };
        fake.session("session:created-full", "project", &expected);
        let cfg = fake.dir.path().join("root-connect.toml");
        fs::write(&cfg,format!("[frecency]\nadd_command='/usr/bin/true'\n[[session]]\nname='root-profile'\npath={}\nstartup_command='printf root-profile-startup'\n",serde_json::to_string(expected.to_str().unwrap()).unwrap())).unwrap();
        let r = Command::new(env!("CARGO_BIN_EXE_rexue"))
            .args(["--detach", "--rex-bin"])
            .arg(&fake.rex.bin)
            .arg("-C")
            .arg(&cfg)
            .args(["connect", "--root", target.to_str().unwrap(), "--json"])
            .env("REXUE_STATE_DIR", fake.dir.path().join("state"))
            .env_remove("REX_SESSION")
            .output()
            .unwrap();
        assert!(
            r.status.success(),
            "{kind}: {}",
            String::from_utf8_lossy(&r.stderr)
        );
        let c: Value = serde_json::from_slice(&r.stdout).unwrap();
        assert_eq!(c["name"], "root-profile");
        assert_eq!(
            Path::new(c["path"].as_str().unwrap())
                .canonicalize()
                .unwrap(),
            expected.canonicalize().unwrap()
        );
        assert!(fake.calls().iter().any(|a| a.iter().any(|v| v == "new")
            && a.iter().any(|v| v.contains("root-profile-startup"))));
    }
}
#[test]
fn connect_root_keeps_config_alias_and_exact_live_identity() {
    let fake = FakeRex::new();
    let repo = fake.dir.path().join("repository");
    fs::create_dir(&repo).unwrap();
    let r = Command::new("git")
        .args(["init", "-b", "main"])
        .arg(&repo)
        .output()
        .unwrap();
    assert!(r.status.success());
    let child = repo.join("child");
    fs::create_dir(&child).unwrap();
    fake.session("session:created-full", "project", &child);
    let manager = fake.manager(Config {
        session: vec![SessionConfig {
            name: "configured-child".into(),
            path: child.to_string_lossy().into_owned(),
            alias: Some("root-flag-alias-734129".into()),
            ..Default::default()
        }],
        frecency: FrecencyConfig {
            add_command: Some("/usr/bin/true".into()),
            ..Default::default()
        },
        ..Default::default()
    });
    let opts = ConnectOptions {
        root: true,
        detach: true,
        ..Default::default()
    };
    let first = manager.connect("root-flag-alias-734129", &opts).unwrap();
    assert!(first.created);
    assert_eq!(first.name, "configured-child");
    assert_eq!(
        first.path.canonicalize().unwrap(),
        child.canonicalize().unwrap()
    );
    let again = manager.connect("session:created-full", &opts).unwrap();
    assert!(!again.created);
    assert_eq!(again.session_id, first.session_id);
    assert_eq!(
        again.path.canonicalize().unwrap(),
        child.canonicalize().unwrap()
    );
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|a| a.iter().any(|v| v == "new"))
            .count(),
        1
    );
}

#[test]
fn root_cli_names_plain_context_and_preserves_literal_dollars() {
    let fake = FakeRex::new();
    let path = fake
        .dir
        .path()
        .join("parent/child.$ROOT_LITERAL_UNSET_9183");
    fs::create_dir_all(&path).unwrap();
    let live = fake.session("session:root-full", "live", &path);
    fake.json("sessions.json", json!({"sessions":[live]}));
    fake.json("context.json", json!({"session_id":"session:root-full"}));
    let cfg = fake.dir.path().join("root.toml");
    fs::write(&cfg, "dir_length=2\n").unwrap();
    for args in [
        vec!["root"],
        vec!["root", "session:root-full"],
        vec!["root", "live"],
    ] {
        let r = pane_cli(&fake, &cfg, &args);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        assert_eq!(r.stdout, b"parent/child_$ROOT_LITERAL_UNSET_9183\n");
    }
    assert!(
        !fake
            .calls()
            .iter()
            .any(|args| args.iter().any(|arg| arg == "new"))
    );
}

#[test]
fn root_cli_git_child_substitution_precedes_root_fallback_in_both_modes() {
    let fake = FakeRex::new();
    let repo = fake.dir.path().join("project");
    let child = repo.join("src/pkg");
    fs::create_dir_all(&child).unwrap();
    let init = Command::new("git")
        .args(["init", "--quiet"])
        .arg(&repo)
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    // Colocated JJ metadata must not take precedence over Git.
    fs::create_dir_all(repo.join(".jj")).unwrap();
    fs::write(repo.join(".jj/repo"), "../../different/.jj/repo\n").unwrap();
    let live = fake.session("session:root-full", "live", &child);
    fake.json("sessions.json", json!({"sessions":[live]}));
    let cfg = fake.dir.path().join("root.toml");
    for mode in [false, true] {
        for (rules, expected) in [
            (String::new(), "project\n"),
            (
                format!(
                    "[[name_substitution]]\nfind={}\nreplace='child:chosen name'\n",
                    serde_json::to_string(child.canonicalize().unwrap().to_str().unwrap()).unwrap()
                ),
                "child_chosen_name\n",
            ),
            (
                format!(
                    "[[name_substitution]]\nfind={}\nreplace='  '\n",
                    serde_json::to_string(child.canonicalize().unwrap().to_str().unwrap()).unwrap()
                ),
                "project\n",
            ),
        ] {
            fs::write(&cfg, format!("git_namer_use_worktree_root={mode}\n{rules}")).unwrap();
            let r = pane_cli(&fake, &cfg, &["root", "session:root-full"]);
            assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
            assert_eq!(r.stdout, expected.as_bytes());
        }
    }
    let commit = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "fixture",
        ])
        .output()
        .unwrap();
    assert!(
        commit.status.success(),
        "{}",
        String::from_utf8_lossy(&commit.stderr)
    );
    let tree = repo.join("ws/feature");
    let add = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "worktree",
            "add",
            "--quiet",
            "--detach",
        ])
        .arg(&tree)
        .output()
        .unwrap();
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    let nested = tree.join("src/pkg");
    fs::create_dir_all(&nested).unwrap();
    fake.session("session:root-full", "live", &nested);
    for (mode, expected) in [(false, "project/ws/feature\n"), (true, "feature\n")] {
        fs::write(&cfg, format!("git_namer_use_worktree_root={mode}\n")).unwrap();
        let r = pane_cli(&fake, &cfg, &["root", "session:root-full"]);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        assert_eq!(r.stdout, expected.as_bytes());
    }
}

#[test]
fn root_cli_jj_workspace_fallback_respects_mode_and_anchor_length() {
    let fake = FakeRex::new();
    let repo = fake.dir.path().join("group/project");
    let workspace = repo.join("ws/feature");
    let child = workspace.join("src/pkg");
    fs::create_dir_all(repo.join(".jj/repo")).unwrap();
    fs::create_dir_all(workspace.join(".jj")).unwrap();
    fs::create_dir_all(&child).unwrap();
    fs::write(workspace.join(".jj/repo"), "../../../.jj/repo\n").unwrap();
    let live = fake.session("session:root-full", "live", &child);
    fake.json("sessions.json", json!({"sessions":[live]}));
    let cfg = fake.dir.path().join("root.toml");
    for (mode, expected) in [
        (false, "group/project/ws/feature\n"),
        (true, "ws/feature\n"),
    ] {
        fs::write(
            &cfg,
            format!("git_namer_use_worktree_root={mode}\ngit_dir_length=2\n"),
        )
        .unwrap();
        let r = pane_cli(&fake, &cfg, &["root", "session:root-full"]);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        assert_eq!(r.stdout, expected.as_bytes());
    }
}

#[test]
fn uncreated_worktree_child_through_symlink_keeps_icon_in_cli() {
    let d = tempfile::tempdir().unwrap();
    let actual = d.path().join("actual");
    let alias = d.path().join("alias");
    fs::create_dir(&actual).unwrap();
    std::os::unix::fs::symlink(&actual, &alias).unwrap();
    let cfg = d.path().join("icon.toml");
    let quote = |p: &Path| serde_json::to_string(p.to_str().unwrap()).unwrap();
    fs::write(&cfg,format!("[[worktree]]\nrepo='org/repo'\npath={}\nworktree_dir={}\nicon='W'\n[[session]]\nname='not-created-yet'\npath={}\n",quote(d.path()),quote(&actual),quote(&alias.join("123")))).unwrap();
    let r = Command::new(env!("CARGO_BIN_EXE_rexue"))
        .args(["--rex-bin", "/nonexistent-rex-icon-test"])
        .arg("-C")
        .arg(cfg)
        .args(["list", "--config", "--json"])
        .env("REXUE_STATE_DIR", d.path().join("state"))
        .output()
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let rows: Value = serde_json::from_slice(&r.stdout).unwrap();
    assert_eq!(rows[0]["icon"], "W");
}

#[test]
fn worktree_icon_expansion_is_equivalent_in_config_only_cli_json() {
    let dir = tempfile::tempdir().unwrap();
    let leaf = format!("rexue-cli-icon-fixture-{}", std::process::id());
    let root = dirs::home_dir().unwrap().join(&leaf);
    let cfg = dir.path().join("icons.toml");
    for spelling in [
        root.display().to_string(),
        format!("~/{leaf}"),
        format!("${{HOME}}/{leaf}"),
    ] {
        fs::write(&cfg,format!("[[worktree]]\nrepo='org/repo'\npath={}\nworktree_dir={}\nicon='W'\n[[session]]\nname='tree-project'\npath={}\n",serde_json::to_string(dir.path().to_str().unwrap()).unwrap(),serde_json::to_string(&spelling).unwrap(),serde_json::to_string(root.join("123").to_str().unwrap()).unwrap())).unwrap();
        let r = Command::new(env!("CARGO_BIN_EXE_rexue"))
            .args(["--rex-bin", "/nonexistent-rex-icon-test"])
            .arg("-C")
            .arg(&cfg)
            .args(["list", "--config", "--json"])
            .env("REXUE_STATE_DIR", dir.path().join("state"))
            .output()
            .unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        let rows: Value = serde_json::from_slice(&r.stdout).unwrap();
        assert_eq!(rows[0]["icon"], "W", "{spelling}");
    }
}

#[test]
fn environment_values_with_literal_dollars_survive_list_preview_and_worktree_roots() {
    const PROJECT: &str = "REXUE_LITERAL_PROJECT_734129";
    const ROOT: &str = "REXUE_LITERAL_ROOT_734129";
    const REPO: &str = "REXUE_LITERAL_REPO_734129";
    const UNSET: &str = "REXUE_UNSET_LITERAL_734129";
    let fake = FakeRex::new();
    let repo = fake.dir.path().join(format!("repo-${UNSET}"));
    let root = repo.join(".wk");
    let project = root.join("409");
    fs::create_dir_all(&project).unwrap();
    let cfg = fake.dir.path().join("literal.toml");
    let run = |args: &[&str], project_value: &Path| {
        Command::new(env!("CARGO_BIN_EXE_rexue"))
            .arg("--rex-bin")
            .arg(&fake.rex.bin)
            .arg("-C")
            .arg(&cfg)
            .args(args)
            .env(PROJECT, project_value)
            .env(ROOT, &root)
            .env(REPO, &repo)
            .env_remove(UNSET)
            .env("REXUE_STATE_DIR", fake.dir.path().join("literal-state"))
            .output()
            .unwrap()
    };
    for (repo_value, dir_value) in [
        (
            serde_json::to_string(fake.dir.path().to_str().unwrap()).unwrap(),
            format!("'${{{ROOT}}}'"),
        ),
        (format!("'${{{REPO}}}'"), "'.wk'".into()),
    ] {
        fs::write(&cfg,format!("[[worktree]]\nrepo='org/repo'\npath={repo_value}\nworktree_dir={dir_value}\nicon='W'\n[[session]]\nname='literal-project'\npath='${{{PROJECT}}}'\npreview_command=\"printf '%s' {{}}\"\n")).unwrap();
        for path in [&project, &root.join("not-created")] {
            let r = run(&["list", "--config", "--json"], path);
            assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
            let rows: Value = serde_json::from_slice(&r.stdout).unwrap();
            assert_eq!(rows.as_array().unwrap().len(), 1);
            assert_eq!(rows[0]["path"], path.to_str().unwrap());
            assert_eq!(rows[0]["icon"], "W");
        }
        let r = run(&["preview", "literal-project"], &project);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        assert_eq!(
            String::from_utf8(r.stdout).unwrap(),
            project.to_str().unwrap()
        );
    }
    let r = run(
        &["--detach", "connect", project.to_str().unwrap(), "--json"],
        &project,
    );
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let connection: Value = serde_json::from_slice(&r.stdout).unwrap();
    assert_eq!(connection["name"], "literal-project");
    assert_eq!(
        connection["path"],
        project.canonicalize().unwrap().to_str().unwrap()
    );
    fs::write(
        &cfg,
        format!("[[session]]\nname='bad-project'\npath='${{{UNSET}}}'\n"),
    )
    .unwrap();
    let r = run(&["list", "--config", "--json"], &project);
    assert!(!r.status.success());
    assert!(r.stdout.is_empty());
    assert!(String::from_utf8_lossy(&r.stderr).contains("bad-project"));
}

#[cfg(target_os = "macos")]
#[test]
fn configured_app_activation_respects_foreground_and_background_modes() {
    let fake = FakeRex::new();
    let project = fake.dir.path().join("project");
    fs::create_dir(&project).unwrap();
    fake.session("session:created-full", "project", &project);
    let cfg = fake.dir.path().join("activate.toml");
    let app = "Rex quoted \" name";
    fs::write(
        &cfg,
        format!(
            "terminal={}\n[[session]]\nname='project'\npath={}\n",
            serde_json::to_string(app).unwrap(),
            serde_json::to_string(project.to_str().unwrap()).unwrap()
        ),
    )
    .unwrap();
    let captured = fake.dir.path().join("activation-args");
    let osascript = fake.dir.path().join("fake-osascript");
    executable(
        &osascript,
        &format!(
            "#!/bin/sh\nprintf '%s\\000' \"$@\" > {}\n",
            shell_quote(captured.to_str().unwrap())
        ),
    );
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_rexue"))
            .arg("--rex-bin")
            .arg(&fake.rex.bin)
            .arg("-C")
            .arg(&cfg)
            .args(args)
            .env("REXUE_OPEN", &osascript)
            .env("REXUE_STATE_DIR", fake.dir.path().join("activate-state"))
            .env_remove("REX_SESSION")
            .output()
            .unwrap()
    };
    for args in [
        &["list", "--config", "--json"][..],
        &["--detach", "connect", "project", "--json"][..],
    ] {
        let r = run(args);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        assert!(!captured.exists());
    }
    let r = run(&["connect", "project", "--json"]);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let bytes = fs::read(&captured).unwrap();
    let args: Vec<_> = bytes.split(|b| *b == 0).filter(|b| !b.is_empty()).collect();
    assert_eq!(args.last().copied(), Some(app.as_bytes()));
    assert_eq!(args.len(), 2);
    assert_eq!(args[0], b"-a");
    fs::remove_file(&captured).unwrap();
    let r = run(&[
        "window",
        "connect",
        "--target",
        "session:created-full",
        "Other",
        "--background",
    ]);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    assert!(!captured.exists());
    executable(
        &osascript,
        "#!/bin/sh\necho activation-unavailable >&2\nexit 1\n",
    );
    let r = run(&["connect", "project", "--json"]);
    assert!(r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("could not activate"));
}
#[test]
fn baseline_tui_fallbacks_and_terminal_config_do_not_reject_cli_listing() {
    let d = tempfile::tempdir().unwrap();
    let cfg = d.path().join("fallbacks.toml");
    for fields in [
        "preview_width=0",
        "preview_width=-1",
        "preview_width=1000",
        "preview_min_width=0",
        "preview_min_width=100000",
        "preview_border='unknown'",
        "alias_auto_connect_delay='-1ms'",
        "window_name_format='#{window_name}'",
    ] {
        fs::write(
            &cfg,
            format!(
                "terminal='Rex'\n[tui]\n{fields}\n[[session]]\nname='configured'\npath='/tmp'\n"
            ),
        )
        .unwrap();
        let r = Command::new(env!("CARGO_BIN_EXE_rexue"))
            .arg("--rex-bin")
            .arg("/no-rex-needed")
            .arg("-C")
            .arg(&cfg)
            .args(["list", "--config", "--json"])
            .env("REXUE_STATE_DIR", d.path().join("state"))
            .output()
            .unwrap();
        assert!(
            r.status.success(),
            "{fields}: {}",
            String::from_utf8_lossy(&r.stderr)
        );
        let rows: Value = serde_json::from_slice(&r.stdout).unwrap();
        assert_eq!(rows[0]["name"], "configured");
    }
}

#[test]
fn nested_shell_placeholders_work_in_cli_preview_and_rex_startup_scripts() {
    let fake = FakeRex::new();
    let name = "project '\";$(touch INJECTED) quoted";
    let project = fake.dir.path().join(name);
    fs::create_dir(&project).unwrap();
    fake.session("session:created-full", "project", &project);
    let startup_file = fake.dir.path().join("nested-startup");
    let window_file = fake.dir.path().join("nested-window");
    let preview =
        r#"rexue_path() { printf '%s' "$(basename '{}')"; }; set -- changed; rexue_path other"#;
    let startup = format!(
        "{preview} > {}",
        shell_quote(startup_file.to_str().unwrap())
    );
    let window = format!("{preview} > {}", shell_quote(window_file.to_str().unwrap()));
    let cfg = fake.dir.path().join("nested-shell.toml");
    fs::write(&cfg,format!("[frecency]\nadd_command='/usr/bin/true'\n[[window]]\nname='Nested'\nstartup_script={}\n[[session]]\nname='project'\npath={}\npreview_command={}\nstartup_command={}\nwindows=['Nested']\n",serde_json::to_string(&window).unwrap(),serde_json::to_string(project.to_str().unwrap()).unwrap(),serde_json::to_string(preview).unwrap(),serde_json::to_string(&startup).unwrap())).unwrap();
    let run = |args: &[&str]| {
        let r = Command::new(env!("CARGO_BIN_EXE_rexue"))
            .args(["--detach", "--rex-bin"])
            .arg(&fake.rex.bin)
            .arg("-C")
            .arg(&cfg)
            .args(args)
            .env_remove("REX_SESSION")
            .env("REXUE_STATE_DIR", fake.dir.path().join("state"))
            .output()
            .unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        r
    };
    let previewed = run(&["preview", "project"]);
    run(&["connect", "project", "--json"]);
    let calls = fake.calls();
    let scripts: Vec<_> = calls
        .iter()
        .filter_map(|args| args.iter().position(|a| a == "--").map(|i| &args[i + 1..]))
        .collect();
    assert_eq!(scripts.len(), 2);
    for args in scripts {
        let result = Command::new(&args[0])
            .args(&args[1..])
            .env("SHELL", "/usr/bin/true")
            .current_dir(&project)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    assert_eq!(
        previewed.stdout,
        name.as_bytes(),
        "CLI preview nested substitution"
    );
    assert_eq!(
        fs::read(startup_file).unwrap(),
        name.as_bytes(),
        "Rex session startup nested substitution"
    );
    assert_eq!(
        fs::read(window_file).unwrap(),
        name.as_bytes(),
        "Rex window startup nested substitution"
    );
    assert!(
        !project.join("INJECTED").exists(),
        "path data executed as shell code"
    );
}

#[test]
fn unicode_alias_preview_uses_the_real_cli_resolution_path() {
    let fake = FakeRex::new();
    let cfg = fake.dir.path().join("unicode.toml");
    fs::write(&cfg,format!("[[session]]\nname='project'\npath={}\nalias='ä'\npreview_command='printf unicode-alias-ok'\n",serde_json::to_string(fake.dir.path().to_str().unwrap()).unwrap())).unwrap();
    for alias in ["ä", "Ä"] {
        let result = Command::new(env!("CARGO_BIN_EXE_rexue"))
            .arg("--rex-bin")
            .arg(&fake.rex.bin)
            .arg("-C")
            .arg(&cfg)
            .args(["preview", alias])
            .env_remove("REX_SESSION")
            .env("REXUE_STATE_DIR", fake.dir.path().join("state"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(result.stdout, b"unicode-alias-ok");
    }
}
#[test]
fn pane_icons_and_configured_windows_are_exposed_by_cli() {
    let fake = FakeRex::new();
    let cfg = fake.dir.path().join("pane.toml");
    fs::write(&cfg,"[icons.tmux-pane]\nicon='P'\ncolor='green'\n[[window]]\nname='Logs'\n[[session]]\nname='configured'\npath='/tmp'\nwindows=['Logs']\n").unwrap();
    fake.json("context.json", json!({"session_id":"session:panes-full"}));
    fake.json("inspect-session:panes-full.json",json!({"active_window_id":"window:panes","windows":[{"window_id":"window:panes","label":"Editor","focused_block_id":"block:one","layers":[{"blocks":[{"block_id":"block:one","label":"one"}]}]}]}));
    let run = |args: &[&str]| {
        let result = Command::new(env!("CARGO_BIN_EXE_rexue"))
            .arg("--rex-bin")
            .arg(&fake.rex.bin)
            .arg("-C")
            .arg(&cfg)
            .args(args)
            .env("REXUE_STATE_DIR", fake.dir.path().join("state"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        result.stdout
    };
    let panes: Value = serde_json::from_slice(&run(&["list", "--panes", "--json"])).unwrap();
    assert_eq!(panes[0]["source"], "rex-block");
    assert!(
        String::from_utf8(run(&["list", "--panes", "--icons", "--no-color"]))
            .unwrap()
            .starts_with("P ")
    );
    let configured: Value = serde_json::from_slice(&run(&["list", "--config", "--json"])).unwrap();
    assert_eq!(configured[0]["window_names"], json!(["Logs"]));
}

fn pane_fixture() -> (FakeRex, std::path::PathBuf) {
    let fake = FakeRex::new();
    let cfg = fake.dir.path().join("selector.toml");
    fs::write(
        &cfg,
        "[icons.tmux-pane]\nicon='P'\ncolor='green'\n[[session]]\nname='configured'\npath='/tmp'\n",
    )
    .unwrap();
    fake.json("context.json", json!({"session_id":"session:panes-full"}));
    fake.json(
        "sessions.json",
        json!({"sessions":[{"session_id":"session:panes-full","label":"live"}]}),
    );
    fake.json(
        "process.json",
        json!({"foreground":{"cwd":fake.dir.path()}}),
    );
    fake.json("inspect-session:panes-full.json",json!({"active_window_id":"window:panes","windows":[{"window_id":"window:panes","label":"Editor","focused_block_id":"block:one","layers":[{"blocks":[{"block_id":"block:one","label":"pane one"},{"block_id":"block:two","label":"pane two"}]}]}]}));
    (fake, cfg)
}
#[test]
fn empty_config_fields_inherit_through_executable_cli() {
    let fake = FakeRex::new();
    let cfg = fake.dir.path().join("inherit.toml");
    fs::write(&cfg, format!(
            "[frecency]\nadd_command='true'\n[default_session]\nwindows=['Inherited']\npreview_command='printf inherited-preview'\nstartup_command='printf inherited-startup'\n[[window]]\nname='Inherited'\npath=''\nstartup_script='printf inherited-window'\n[[session]]\nname='project'\npath={}\nwindows=[]\npreview_command=''\nstartup_command=''\nalias='ä'\n",
            serde_json::to_string(fake.dir.path().to_str().unwrap()).unwrap()
        )).unwrap();
    let run = |args: &[&str]| {
        let output = pane_cli(&fake, &cfg, args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };
    let rows: Value = serde_json::from_slice(&run(&["list", "--config", "--json"])).unwrap();
    assert_eq!(rows[0]["window_names"], json!(["Inherited"]));
    assert_eq!(run(&["preview", "Ä"]), b"inherited-preview");
    let created: Value = serde_json::from_slice(&run(&["connect", "Ä", "--json"])).unwrap();
    assert_eq!(created["created"], true);
    let reused: Value = serde_json::from_slice(&run(&["connect", "project", "--json"])).unwrap();
    assert_eq!(reused["created"], false);
    let calls = fake.calls();
    let creates: Vec<_> = calls
        .iter()
        .filter(|args| {
            args.iter().any(|arg| arg == "new") && !args.iter().any(|arg| arg == "window")
        })
        .collect();
    assert_eq!(creates.len(), 1);
    assert!(
        creates[0]
            .iter()
            .any(|arg| arg.contains("printf inherited-startup"))
    );
    let windows: Vec<_> = calls
        .iter()
        .filter(|args| args.windows(2).any(|pair| pair == ["window", "new"]))
        .collect();
    assert_eq!(windows.len(), 1);
    assert!(
        windows[0]
            .iter()
            .any(|arg| arg.contains("printf inherited-window"))
    );
}

#[test]
fn cli_go_wildcard_classes_select_startup_windows_and_preview() {
    for (class, matched, aliased) in [
        ("[^x]", true, false),
        ("[!x]", false, false),
        ("[^x]", true, true),
        ("[!x]", false, true),
    ] {
        let fake = FakeRex::new();
        let actual = fake.dir.path().join("actual");
        fs::create_dir(&actual).unwrap();
        let parent = if aliased {
            let alias = fake.dir.path().join("alias");
            std::os::unix::fs::symlink(&actual, &alias).unwrap();
            alias
        } else {
            actual
        };
        let project = parent.join("rexue");
        fs::create_dir(&project).unwrap();
        let project = if aliased {
            project
        } else {
            project.canonicalize().unwrap()
        };
        fake.session("session:created-full", "project", &project);
        let pattern = format!("{}/rexu{class}", project.parent().unwrap().display());
        let cfg = fake.dir.path().join("go-wildcard.toml");
        fs::write(&cfg,format!("[frecency]\nadd_command='/usr/bin/true'\n[default_session]\nstartup_command='printf default-startup'\npreview_command='printf default-preview'\nwindows=['Default']\n[[window]]\nname='Default'\nstartup_script='printf default-window'\n[[window]]\nname='Wild'\nstartup_script='printf wild-window'\n[[wildcard]]\npattern={}\nstartup_command='printf wild-startup'\npreview_command='printf wild-preview'\nwindows=['Wild']\nicon='W'\n[[session]]\nname='project'\npath={}\n",serde_json::to_string(&pattern).unwrap(),serde_json::to_string(project.to_str().unwrap()).unwrap())).unwrap();
        let run = |args: &[&str]| {
            let r = Command::new(env!("CARGO_BIN_EXE_rexue"))
                .args(["--detach", "--rex-bin"])
                .arg(&fake.rex.bin)
                .arg("-C")
                .arg(&cfg)
                .args(args)
                .env_remove("REX_SESSION")
                .env("REXUE_STATE_DIR", fake.dir.path().join("state"))
                .output()
                .unwrap();
            assert!(
                r.status.success(),
                "{class}: {}",
                String::from_utf8_lossy(&r.stderr)
            );
            r
        };
        let listed: Value =
            serde_json::from_slice(&run(&["list", "--config", "--json"]).stdout).unwrap();
        assert_eq!(
            listed[0]["icon"],
            if matched { json!("W") } else { Value::Null }
        );
        let selected = if matched { "wild" } else { "default" };
        assert_eq!(
            run(&["preview", "project"]).stdout,
            format!("{selected}-preview").as_bytes()
        );
        run(&["connect", "project", "--json"]);
        let calls = fake.calls();
        let creates: Vec<_> = calls
            .iter()
            .filter(|a| a.iter().any(|v| v == "new") && !a.iter().any(|v| v == "window"))
            .collect();
        assert_eq!(creates.len(), 1);
        assert!(
            creates[0]
                .iter()
                .any(|arg| arg.contains(&format!("printf {selected}-startup"))),
            "{class} {calls:#?}"
        );
        let windows: Vec<_> = calls
            .iter()
            .filter(|a| a.windows(2).any(|p| p[0] == "window" && p[1] == "new"))
            .collect();
        assert_eq!(windows.len(), 1);
        assert!(
            windows[0]
                .iter()
                .any(|arg| arg == if matched { "Wild" } else { "Default" })
        );
        assert!(
            windows[0]
                .iter()
                .any(|arg| arg.contains(&format!("printf {selected}-window")))
        );
    }
}

#[test]
fn cli_startup_and_preview_follow_source_strategy_order() {
    for (session, wildcard, expected) in [
        (
            "startup_command='printf session'",
            "startup_command='printf wildcard'\ndisable_startup_command=true",
            "session",
        ),
        (
            "startup_command='printf session'\ndisable_startup_command=true",
            "",
            "session",
        ),
        (
            "startup_command=''\nstartup_script=''",
            "startup_command='printf wildcard'",
            "wildcard",
        ),
        (
            "disable_startup_command=false",
            "startup_command='printf wildcard'\ndisable_startup_command=true",
            "default",
        ),
        (
            "disable_startup_command=true",
            "startup_command='printf wildcard'",
            "wildcard",
        ),
        ("disable_startup_command=true", "startup_command=''", ""),
        (
            "startup_command='printf session'",
            "startup_script='printf wrong-script'",
            "session",
        ),
        (
            "startup_script='printf session-script'",
            "startup_command='printf wildcard'",
            "session-script",
        ),
    ] {
        let fake = FakeRex::new();
        let cfg = fake.dir.path().join("strategies.toml");
        let path = serde_json::to_string(fake.dir.path().canonicalize().unwrap().to_str().unwrap())
            .unwrap();
        fs::write(&cfg, format!(
            "[frecency]\nadd_command='true'\n[default_session]\nstartup_command='printf default'\nstartup_script=''\npreview_command='printf default-preview'\n[[wildcard]]\npattern={path}\npreview_command='printf wildcard-preview'\nicon='W'\n{wildcard}\n[[wildcard]]\npattern={path}\nstartup_command='printf wrong-second'\npreview_command='printf wrong-second'\n[[session]]\nname='project'\npath={path}\npreview_command=''\nicon=''\n{session}\n"
        )).unwrap();
        let output = pane_cli(&fake, &cfg, &["list", "--config", "--json"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rows: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(rows[0]["icon"], "W");
        let preview = pane_cli(&fake, &cfg, &["preview", "project"]);
        assert!(
            preview.status.success(),
            "{}",
            String::from_utf8_lossy(&preview.stderr)
        );
        assert_eq!(preview.stdout, b"wildcard-preview");
        let output = pane_cli(&fake, &cfg, &["connect", "project", "--json"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let calls = fake.calls();
        let create = calls
            .iter()
            .find_map(|args| args.iter().position(|arg| arg == "new").map(|i| &args[i..]))
            .unwrap();
        if expected.is_empty() {
            assert!(!create.iter().any(|arg| arg == "--"));
        } else {
            let separator = create.iter().position(|arg| arg == "--").unwrap();
            let startup = Command::new("/bin/sh")
                .args(["-c", &create[separator + 3], "rexue"])
                .arg(fake.dir.path())
                .env("SHELL", "/usr/bin/true")
                .output()
                .unwrap();
            assert!(
                startup.status.success(),
                "{}",
                String::from_utf8_lossy(&startup.stderr)
            );
            assert_eq!(
                startup.stdout,
                expected.as_bytes(),
                "session={session}; wildcard={wildcard}"
            );
        }
    }
}

#[test]
fn cli_nonempty_fields_win_and_empty_wildcard_fields_continue_to_defaults() {
    let fake = FakeRex::new();
    let path =
        serde_json::to_string(fake.dir.path().canonicalize().unwrap().to_str().unwrap()).unwrap();
    let cfg = fake.dir.path().join("fallbacks.toml");
    for (session, wildcard, window, preview) in [
        (
            "windows=[]\npreview_command=''",
            "windows=[]\npreview_command=''",
            "Default",
            "default-preview",
        ),
        (
            "windows=[]\npreview_command=''",
            "windows=['Wildcard']\npreview_command='printf wildcard-preview'",
            "Wildcard",
            "wildcard-preview",
        ),
        (
            "windows=['Session']\npreview_command='printf session-preview'",
            "windows=['Wildcard']\npreview_command='printf wildcard-preview'",
            "Session",
            "session-preview",
        ),
    ] {
        fs::write(&cfg, format!(
            "[default_session]\nwindows=['Default']\npreview_command='printf default-preview'\n[[window]]\nname='Default'\n[[window]]\nname='Wildcard'\n[[window]]\nname='Session'\n[[wildcard]]\npattern={path}\n{wildcard}\n[[wildcard]]\npattern={path}\nwindows=['Session']\npreview_command='printf wrong-second'\n[[session]]\nname='project'\npath={path}\n{session}\n"
        )).unwrap();
        let output = pane_cli(&fake, &cfg, &["list", "--config", "--json"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rows: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(rows[0]["window_names"], json!([window]));
        let output = pane_cli(&fake, &cfg, &["preview", "project"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, preview.as_bytes());
    }
    // The preview strategy also continues when a nonempty command succeeds
    // with no output, as previewer/previewer.go does.
    fs::write(
        &cfg,
        format!("[[session]]\nname='project'\npath={path}\npreview_command='true'\n"),
    )
    .unwrap();
    fs::write(fake.dir.path().join("directory-preview-marker"), "").unwrap();
    let output = pane_cli(&fake, &cfg, &["preview", "project"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("directory-preview-marker"));
}

fn pane_cli(fake: &FakeRex, cfg: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rexue"))
        .arg("--rex-bin")
        .arg(&fake.rex.bin)
        .args(["--client", "client:desktop-full"])
        .arg("-C")
        .arg(cfg)
        .args(args)
        .env("REXUE_STATE_DIR", fake.dir.path().join("state"))
        .output()
        .unwrap()
}
#[test]
fn pane_selector_names_roundtrip_and_union_obeys_blacklist() {
    let (fake, cfg) = pane_fixture();
    let run = |args: &[&str]| {
        let r = pane_cli(&fake, &cfg, args);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        r.stdout
    };
    let rows: Value = serde_json::from_slice(&run(&["list", "--panes", "--json"])).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 2);
    let name = rows[0]["name"].as_str().unwrap();
    assert_eq!(run(&["preview", name]), b"terminal fixture\n");
    assert_eq!(
        run(&["preview", &format!("P {name}")]),
        b"terminal fixture\n"
    );
    let connected: Value = serde_json::from_slice(&run(&["connect", name, "--json"])).unwrap();
    assert_eq!(connected["session_id"], "session:panes-full");
    assert_eq!(connected["created"], false);
    let rooted: Value =
        serde_json::from_slice(&run(&["connect", "--root", name, "--json"])).unwrap();
    assert_eq!(rooted["session_id"], "session:panes-full");
    assert_eq!(rooted["created"], false);
    let calls = fake.calls();
    assert!(!calls.iter().any(|args| args.iter().any(|a| a == "new")));
    assert!(calls.iter().any(|args| {
        args.iter()
            .any(|a| a.contains("\"window_id\":\"window:panes\""))
    }));
    let union: Value =
        serde_json::from_slice(&run(&["list", "--panes", "--rex", "--config", "--json"])).unwrap();
    for source in ["rex", "config", "rex-block"] {
        assert!(
            union
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["source"] == source)
        );
    }
    assert_eq!(
        serde_json::from_slice::<Value>(&run(&["list", "--panes", "--hide-duplicates", "--json"]))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let text = fs::read_to_string(&cfg).unwrap();
    fs::write(&cfg, format!("blacklist=['.*']\n{text}")).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&run(&["list", "--panes", "--json"])).unwrap(),
        json!([])
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&run(&["list", "--panes", "--blacklisted", "--json"]))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
}
#[test]
fn duplicate_pane_names_require_ids_and_full_ids_remain_selectable() {
    let (fake, cfg) = pane_fixture();
    let file = fake.dir.path().join("inspect-session:panes-full.json");
    let mut view: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    view["windows"][0]["layers"][0]["blocks"][1]["label"] = json!("pane one");
    fs::write(file, serde_json::to_vec(&view).unwrap()).unwrap();
    let r = pane_cli(&fake, &cfg, &["preview", "Editor: pane one"]);
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("ambiguous"));
    let r = pane_cli(&fake, &cfg, &["preview", "block:one"]);
    assert!(r.status.success());
    assert_eq!(r.stdout, b"terminal fixture\n");
}

#[test]
fn cli_alias_expansion_is_case_insensitive_and_precedes_live_names() {
    let fake = FakeRex::new();
    let wrong = fake.session("session:wrong-full", "wp", fake.dir.path());
    fake.json("sessions.json", json!({"sessions":[wrong.clone()]}));
    let cfg = Config {
        session: vec![SessionConfig {
            name: "project".into(),
            path: fake.dir.path().display().to_string(),
            alias: Some("wp".into()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let manager = fake.manager(cfg);
    for query in ["wp", "WP", "Wp"] {
        let entry = manager.resolve_entry(query).unwrap();
        assert_eq!(entry.name, "project");
        assert_eq!(entry.source, Source::Config);
    }
    let correct = fake.session("session:project-full", "project", fake.dir.path());
    fake.json("sessions.json", json!({"sessions":[wrong,correct]}));
    for query in ["wp", "WP"] {
        assert_eq!(
            manager.resolve_entry(query).unwrap().session_id.as_deref(),
            Some("session:project-full")
        );
    }
}

#[test]
fn external_launcher_directory_falls_back_to_live_client_history() {
    let fake = FakeRex::new();
    let session = fake.session("session:remembered-full", "remembered", fake.dir.path());
    fake.json("sessions.json", json!({"sessions":[session]}));
    let cfg = Config {
        frecency: FrecencyConfig {
            list_command: Some("true".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let manager = fake.manager(cfg);
    fs::create_dir_all(&manager.state_dir).unwrap();
    fs::write(
        manager.state_dir.join("state.json"),
        serde_json::to_vec(
            &json!({"paths":{},"history":{"client:desktop-full":["session:remembered-full"]}}),
        )
        .unwrap(),
    )
    .unwrap();
    for context in [json!({}), json!({"session_id":"session:expired-full"})] {
        fake.json("context.json", context);
        assert_eq!(
            manager.current_path().unwrap(),
            fake.dir.path().canonicalize().unwrap()
        );
    }
    assert!(
        !fake
            .calls()
            .iter()
            .any(|args| args.iter().any(|arg| arg == "new"))
    );
}

// Manager invokes zoxide for combined listings. Run those cases
// in a child with a stub on PATH so developer frecency data cannot be touched.
fn isolated(name: &str, test: impl FnOnce()) {
    if std::env::var("REXUE_BACKEND_TEST_CHILD").as_deref() == Ok(name) {
        test();
        return;
    }
    let tools = tempfile::tempdir().unwrap();
    executable(&tools.path().join("zoxide"), "#!/bin/sh\nexit 0\n");
    let mut paths = vec![tools.path().to_owned()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env("REXUE_BACKEND_TEST_CHILD", name)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env_remove("REX_SESSION")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "isolated test failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn connected_app_filter_and_duplicate_client_labels_use_full_ids() {
    let mut fake = FakeRex::new();
    fake.json(
        "clients.json",
        json!({"clients": [
            client("client:desktop-full", "Desk", "app", "connected"),
            client("client:other-full", "Desk", "app", "connected"),
            client("client:offline", "Desk", "app", "disconnected"),
            client("client:cli", "Desk", "cli", "connected"),
            client("client:shadow", "client:desktop-full", "app", "connected")
        ]}),
    );
    assert_eq!(fake.rex.clients().unwrap().len(), 3);
    assert_eq!(
        fake.rex.selected_client().unwrap().id,
        "client:desktop-full"
    );
    fake.rex.client = Some("desk".into());
    assert!(
        fake.rex
            .selected_client()
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
    );
    fake.rex.client = None;
    assert!(
        fake.rex
            .selected_client()
            .unwrap_err()
            .to_string()
            .contains("multiple")
    );
    for args in fake.calls() {
        assert_eq!(
            &args[..5],
            [
                "--autostart=false",
                "--timeout",
                "10s",
                "-S",
                "unix:/tmp/server ' ; $socket"
            ]
        );
    }
}

#[test]
fn process_errors_keep_stderr_exit_codes_and_json_context() {
    for code in [3, 4] {
        let fake = FakeRex::new();
        fs::write(fake.dir.path().join("fail.exit"), code.to_string()).unwrap();
        fs::write(
            fake.dir.path().join("fail.stderr"),
            "fixture service unavailable\n",
        )
        .unwrap();
        let error = format!("{:#}", fake.rex.context().unwrap_err());
        assert!(error.contains(&format!("exit {code}")), "{error}");
        assert!(error.contains("fixture service unavailable"), "{error}");
    }
    let fake = FakeRex::new();
    fs::write(fake.dir.path().join("context.json"), "{broken JSON").unwrap();
    let error = format!("{:#}", fake.rex.context().unwrap_err());
    assert!(
        error.contains("invalid JSON from Rex whoami --json"),
        "{error}"
    );
}

#[test]
fn list_skips_disappeared_sessions_but_reports_service_and_json_errors() {
    let fake = FakeRex::new();
    let live = fake.session(
        "session:live-full",
        "same label",
        Path::new("/tmp/a b; literal"),
    );
    fake.json(
        "sessions.json",
        json!({"sessions": [{"session_id": "session:gone-full", "label": "same label"}, live]}),
    );
    fs::write(fake.dir.path().join("inspect-session:gone-full.exit"), "3").unwrap();
    let entries = fake.rex.list().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].session_id.as_deref(), Some("session:live-full"));
    assert_eq!(entries[0].active_window_name, "Editor");
    assert_eq!(entries[0].path, Path::new("/tmp/a b; literal"));
    let calls = fake.calls();
    assert!(calls.iter().any(|args| args[5..]
        == [
            "block",
            "call",
            "-s",
            "session:live-full",
            "-b",
            "block:focused-full",
            "process"
        ]));
    fs::write(fake.dir.path().join("inspect-session:gone-full.exit"), "4").unwrap();
    assert!(fake.rex.list().unwrap_err().to_string().contains("exit 4"));
    fs::remove_file(fake.dir.path().join("inspect-session:gone-full.exit")).unwrap();
    fs::write(
        fake.dir.path().join("inspect-session:gone-full.json"),
        "not JSON",
    )
    .unwrap();
    assert!(
        fake.rex
            .list()
            .unwrap_err()
            .to_string()
            .contains("invalid JSON")
    );
}

#[test]
fn create_keeps_paths_labels_and_startup_as_separate_arguments() {
    let fake = FakeRex::new();
    let path = fake
        .dir
        .path()
        .join("project ' ; $(touch INJECTED) & spaces");
    fs::create_dir(&path).unwrap();
    let label = "name ' ; $(touch INJECTED)";
    let created = fake
        .rex
        .create(label, &path, Some("printf '%s' {}\nprintf 'startup' "))
        .unwrap();
    assert_eq!(created.session_id, "session:created-full");
    assert_eq!(created.block_id, "block:initial-full");
    let calls = fake.calls();
    let args = &calls[0][5..];
    assert_eq!(
        &args[..7],
        [
            "new",
            label,
            "-c",
            path.to_str().unwrap(),
            "--window",
            "Terminal",
            "--json"
        ]
    );
    assert_eq!(&args[7..10], ["--", "sh", "-c"]);
    let startup = Command::new("/bin/sh")
        .args(["-c", &args[10], "rexue"])
        .arg(&path)
        .env("SHELL", "/usr/bin/true")
        .current_dir(fake.dir.path())
        .output()
        .unwrap();
    assert!(
        startup.status.success(),
        "{}",
        String::from_utf8_lossy(&startup.stderr)
    );
    assert_eq!(
        String::from_utf8(startup.stdout).unwrap(),
        format!("{}startup", path.display())
    );
    assert_eq!(&args[11..], ["rexue", path.to_str().unwrap()]);
    assert!(!fake.dir.path().join("INJECTED").exists());
    assert!(!path.join("INJECTED").exists());
}

#[test]
fn capability_preflight_rejects_before_creating_or_saving() {
    isolated(
        "capability_preflight_rejects_before_creating_or_saving",
        || {
            let fake = FakeRex::new();
            fake.json(
                "actions.json",
                json!({"actions": [{"name": "window.focus"}]}),
            );
            let manager = fake.manager(Config::default());
            let error = manager
                .connect(
                    fake.dir.path().to_str().unwrap(),
                    &ConnectOptions::default(),
                )
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("does not support session.select")
            );
            let calls = fake.calls();
            assert_eq!(calls.len(), 2);
            assert_eq!(&calls[0][5..], ["client", "ls", "--json"]);
            assert_eq!(
                &calls[1][5..],
                ["-C", "client:desktop-full", "actions", "--json"]
            );
            assert!(!manager.state_dir.exists());
        },
    );
}

#[test]
fn connect_creates_selects_then_reuses_without_repeating_startup_or_windows() {
    isolated(
        "connect_creates_selects_then_reuses_without_repeating_startup_or_windows",
        || {
            let fake = FakeRex::new();
            let project = fake.dir.path().join("project ' ; $(touch INJECTED)");
            let window_path = fake.dir.path().join("window cwd ; literal");
            fs::create_dir(&project).unwrap();
            fs::create_dir(&window_path).unwrap();
            fake.session("session:created-full", "project", &project);
            let config = Config {
                session: vec![SessionConfig {
                    name: "project".into(),
                    path: project.to_str().unwrap().into(),
                    startup_command: Some("printf first {}".into()),
                    windows: vec!["Editor ; literal".into()],
                    ..Default::default()
                }],
                window: vec![WindowConfig {
                    name: "Editor ; literal".into(),
                    path: Some(window_path.to_str().unwrap().into()),
                    startup_command: Some("printf 'window:%s' {}".into()),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let manager = fake.manager(config);
            let first = manager
                .connect("project", &ConnectOptions::default())
                .unwrap();
            assert!(first.created);
            let second = manager
                .connect(
                    "project",
                    &ConnectOptions {
                        command: Some("MUST NOT RUN".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert!(!second.created);
            assert_eq!(first.session_id, second.session_id);
            let calls = fake.calls();
            let commands: Vec<_> = calls.iter().map(|a| &a[5..]).collect();
            assert_eq!(commands.iter().filter(|a| a[0] == "new").count(), 1);
            let windows: Vec<_> = commands.iter().filter(|a| a[0] == "window").collect();
            assert_eq!(windows.len(), 1);
            assert_eq!(
                &windows[0][..9],
                [
                    "window",
                    "new",
                    "-s",
                    "session:created-full",
                    "Editor ; literal",
                    "-c",
                    window_path.to_str().unwrap(),
                    "--focus=false",
                    "--json"
                ]
            );
            let startup = Command::new("/bin/sh")
                .args(["-c", &windows[0][12], "rexue"])
                .arg(&window_path)
                .env("SHELL", "/usr/bin/true")
                .current_dir(fake.dir.path())
                .output()
                .unwrap();
            assert!(startup.status.success());
            assert_eq!(
                String::from_utf8(startup.stdout).unwrap(),
                format!("window:{}", window_path.display())
            );
            assert_eq!(&windows[0][13..], ["rexue", window_path.to_str().unwrap()]);
            let selections: Vec<_> = commands
                .iter()
                .enumerate()
                .filter(|(_, a)| a.get(3).map(String::as_str) == Some("session.select"))
                .collect();
            assert_eq!(selections.len(), 2);
            let create_index = commands.iter().position(|a| a[0] == "new").unwrap();
            assert!(create_index < selections[0].0);
            for (_, selection) in selections {
                assert_eq!(
                    &selection[..5],
                    [
                        "-C",
                        "client:desktop-full",
                        "do",
                        "session.select",
                        "--args"
                    ]
                );
                assert_eq!(
                    serde_json::from_str::<Value>(&selection[5]).unwrap(),
                    json!({"session_id": "session:created-full"})
                );
            }
            assert!(
                !commands
                    .iter()
                    .flat_map(|args| args.iter())
                    .any(|a| a.contains("MUST NOT RUN"))
            );
            let state: Value =
                serde_json::from_slice(&fs::read(manager.state_dir.join("state.json")).unwrap())
                    .unwrap();
            assert_eq!(
                Path::new(state["paths"]["session:created-full"].as_str().unwrap())
                    .canonicalize()
                    .unwrap(),
                project.canonicalize().unwrap()
            );
            assert_eq!(
                state["history"]["client:desktop-full"],
                json!(["session:created-full", "session:previous-full"])
            );
        },
    );
}

#[test]
fn duplicate_session_labels_require_full_id_for_selection_and_kill() {
    isolated(
        "duplicate_session_labels_require_full_id_for_selection_and_kill",
        || {
            let fake = FakeRex::new();
            let one = fake.session("session:one-full", "duplicate", fake.dir.path());
            let two = fake.session("session:two-full", "duplicate", fake.dir.path());
            fake.json("sessions.json", json!({"sessions": [one, two]}));
            let manager = fake.manager(Config::default());
            assert!(
                manager
                    .resolve_entry("duplicate")
                    .unwrap_err()
                    .to_string()
                    .contains("ambiguous")
            );
            let chosen = manager
                .connect("session:two-full", &ConnectOptions::default())
                .unwrap();
            assert_eq!(chosen.session_id, "session:two-full");
            assert!(!chosen.created);
            assert!(fake.rex.kill("duplicate").is_err());
            fake.rex.kill("session:two-full").unwrap();
            let calls = fake.calls();
            assert_eq!(&calls.last().unwrap()[5..], ["kill", "session:two-full"]);
            assert!(!calls.iter().any(|a| a[5] == "new"));
        },
    );
}

#[test]
fn failed_window_template_keeps_created_session_discoverable_without_switching() {
    isolated(
        "failed_window_template_keeps_created_session_discoverable_without_switching",
        || {
            let fake = FakeRex::new();
            let project = fake.dir.path().join("recoverable project");
            fs::create_dir(&project).unwrap();
            fake.session("session:created-full", "project", &project);
            fs::write(fake.dir.path().join("window.json"), "malformed window JSON").unwrap();
            let config = Config {
                session: vec![SessionConfig {
                    name: "project".into(),
                    path: project.to_str().unwrap().into(),
                    windows: vec!["Editor".into()],
                    ..Default::default()
                }],
                window: vec![WindowConfig {
                    name: "Editor".into(),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let manager = fake.manager(config);
            let error = format!(
                "{:#}",
                manager
                    .connect("project", &ConnectOptions::default())
                    .unwrap_err()
            );
            assert!(
                error.contains("session session:created-full exists"),
                "{error}"
            );
            assert!(error.contains("invalid Rex window JSON"), "{error}");
            let state: Value =
                serde_json::from_slice(&fs::read(manager.state_dir.join("state.json")).unwrap())
                    .unwrap();
            assert_eq!(
                state["paths"]["session:created-full"],
                json!(project.canonicalize().unwrap())
            );
            assert!(!manager.state_dir.join("list.json").exists());
            let calls = fake.calls();
            assert_eq!(calls.iter().filter(|a| a[5] == "new").count(), 1);
            assert!(
                !calls
                    .iter()
                    .any(|a| a.get(7).map(String::as_str) == Some("do"))
            );
            assert!(!calls.iter().any(|a| a[5] == "kill"));
        },
    );
}

#[test]
fn manager_source_filters_and_dedup_preserve_live_identity() {
    isolated(
        "manager_source_filters_and_dedup_preserve_live_identity",
        || {
            let fake = FakeRex::new();
            let live = fake.session("session:live-full", "project", fake.dir.path());
            fake.json("sessions.json", json!({"sessions": [live]}));
            let config = Config {
                session: vec![
                    SessionConfig {
                        name: "project".into(),
                        path: fake.dir.path().to_str().unwrap().into(),
                        ..Default::default()
                    },
                    SessionConfig {
                        name: "other".into(),
                        path: fake.dir.path().join("other").to_str().unwrap().into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            };
            let manager = fake.manager(config);
            let configs = manager
                .list(&ListOptions {
                    config: true,
                    no_cache: true,
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(configs.len(), 2);
            assert!(configs.iter().all(|e| e.source == Source::Config));
            let live = manager
                .list(&ListOptions {
                    rex: true,
                    no_cache: true,
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(live.len(), 1);
            assert_eq!(live[0].session_id.as_deref(), Some("session:live-full"));
            let dedup = manager
                .list(&ListOptions {
                    rex: true,
                    config: true,
                    hide_duplicates: true,
                    no_cache: true,
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(dedup.len(), 2);
            assert!(
                dedup
                    .iter()
                    .any(|e| e.session_id.as_deref() == Some("session:live-full"))
            );
            assert_eq!(dedup.iter().filter(|e| e.name == "project").count(), 1);
        },
    );
}

#[test]
fn config_source_does_not_contact_unavailable_rex_or_frecency() {
    let fake = FakeRex::new();
    fs::write(fake.dir.path().join("fail.exit"), "4").unwrap();
    fs::write(
        fake.dir.path().join("fail.stderr"),
        "Rex must not be called",
    )
    .unwrap();
    let manager = fake.manager(Config {
        session: vec![SessionConfig {
            name: "available offline".into(),
            path: fake.dir.path().to_str().unwrap().into(),
            ..Default::default()
        }],
        frecency: FrecencyConfig {
            list_command: Some("exit 75".into()),
            ..Default::default()
        },
        ..Default::default()
    });
    let entries = manager
        .list(&ListOptions {
            config: true,
            no_cache: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "available offline");
    assert!(fake.calls().is_empty());
}

#[test]
fn blacklist_inversion_and_source_order_apply_to_selected_sources() {
    let fake = FakeRex::new();
    let live = fake.session("session:live-full", "live", fake.dir.path());
    fake.json("sessions.json", json!({"sessions": [live]}));
    let manager = fake.manager(Config {
        session: vec![
            SessionConfig {
                name: "visible".into(),
                path: fake.dir.path().join("visible").to_str().unwrap().into(),
                ..Default::default()
            },
            SessionConfig {
                name: "scratch-private".into(),
                path: fake.dir.path().join("scratch").to_str().unwrap().into(),
                ..Default::default()
            },
        ],
        blacklist: vec!["[invalid regex".into(), "^scratch-".into()],
        sort_order: vec![
            SortGroup::Source("config".into()),
            SortGroup::Source("rex".into()),
        ],
        ..Default::default()
    });
    let normal = manager
        .list(&ListOptions {
            config: true,
            rex: true,
            no_cache: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        normal.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["visible", "live"]
    );
    assert_eq!(normal[0].source, Source::Config);
    assert_eq!(normal[1].source, Source::Rex);
    let blacklisted = manager
        .list(&ListOptions {
            config: true,
            rex: true,
            blacklisted: true,
            no_cache: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        blacklisted
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["scratch-private"]
    );
}

#[test]
fn configured_frecency_scores_order_merged_sources_before_deduplication() {
    isolated(
        "configured_frecency_scores_order_merged_sources_before_deduplication",
        || {
            let fake = FakeRex::new();
            let low = fake.dir.path().join("low score ; literal");
            let high = fake.dir.path().join("high score ' literal");
            fs::create_dir(&low).unwrap();
            fs::create_dir(&high).unwrap();
            let scores = fake.dir.path().join("frecency scores ' ; data");
            fs::write(
                &scores,
                format!("1.5 {}\n100.25 {}\n", low.display(), high.display()),
            )
            .unwrap();
            let manager = fake.manager(Config {
                session: vec![
                    SessionConfig {
                        name: "low".into(),
                        path: low.to_str().unwrap().into(),
                        ..Default::default()
                    },
                    SessionConfig {
                        name: "high".into(),
                        path: high.to_str().unwrap().into(),
                        ..Default::default()
                    },
                ],
                frecency: FrecencyConfig {
                    list_command: Some(format!(
                        "/bin/cat {}",
                        shell_quote(scores.to_str().unwrap())
                    )),
                    ..Default::default()
                },
                sort_order: vec![SortGroup::Merged(vec!["config".into(), "zoxide".into()])],
                ..Default::default()
            });
            let rows = manager
                .list(&ListOptions {
                    config: true,
                    zoxide: true,
                    hide_duplicates: true,
                    no_cache: true,
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(
                rows.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
                ["high", "low"]
            );
            assert!(rows.iter().all(|e| e.source == Source::Config));
            assert!(fake.calls().is_empty());
        },
    );
}

#[test]
fn empty_frecency_commands_use_all_four_zoxide_defaults() {
    isolated(
        "empty_frecency_commands_use_all_four_zoxide_defaults",
        || {
            let fake = FakeRex::new();
            let project = fake
                .dir
                .path()
                .join("default ' ; $(touch INJECTED) project");
            fs::create_dir(&project).unwrap();
            fake.session("session:created-full", "project", &project);
            let tool = std::env::split_paths(&std::env::var_os("PATH").unwrap())
                .next()
                .unwrap()
                .join("zoxide");
            executable(
                &tool,
                &format!(
                    r#"#!/bin/sh
root={root}
project={project}
case "$1:$2" in
 query:--list) key=list;;
 query:--) key=query;;
 add:--) key=add;;
 remove:--) key=remove;;
 *) exit 92;;
esac
printf '%s\000' "$@" > "$root/$key-args"
case "$key" in
 list) printf '42 %s\n' "$project";;
 query) printf '%s\n' "$project";;
esac
"#,
                    root = shell_quote(fake.dir.path().to_str().unwrap()),
                    project = shell_quote(project.to_str().unwrap())
                ),
            );
            let file = fake.dir.path().join("empty-frecency.toml");
            fs::write(&file,"[frecency]\nlist_command=''\nquery_command=''\nadd_command=''\nremove_command=''\n").unwrap();
            let manager = fake.manager(Config::load(Some(&file)).unwrap());
            let rows = manager
                .list(&ListOptions {
                    zoxide: true,
                    no_cache: true,
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].score, 42.0);
            let query = "query ' ; $(touch INJECTED) literal";
            let c = manager
                .connect(
                    query,
                    &ConnectOptions {
                        detach: true,
                        ..Default::default()
                    },
                )
                .unwrap();
            assert!(c.created);
            manager.remove(&rows[0]).unwrap();
            let args = |name: &str| {
                let bytes = fs::read(fake.dir.path().join(format!("{name}-args"))).unwrap();
                bytes
                    .split(|b| *b == 0)
                    .filter(|v| !v.is_empty())
                    .map(|v| String::from_utf8(v.to_vec()).unwrap())
                    .collect::<Vec<_>>()
            };
            assert_eq!(args("list"), ["query", "--list", "--score"]);
            assert_eq!(args("query"), ["query", "--", query]);
            let add = args("add");
            assert_eq!(&add[..2], ["add", "--"]);
            assert_eq!(
                Path::new(&add[2]).canonicalize().unwrap(),
                project.canonicalize().unwrap()
            );
            let remove = args("remove");
            assert_eq!(&remove[..2], ["remove", "--"]);
            assert_eq!(Path::new(&remove[2]), project);
            assert!(!fake.dir.path().join("INJECTED").exists());
        },
    );
}
#[test]
fn empty_frecency_defaults_allow_missing_optional_zoxide() {
    for fields in [
        "",
        "list_command=''\nquery_command=''\nadd_command=''\nremove_command=''\n",
    ] {
        let fake = FakeRex::new();
        let empty = fake.dir.path().join("empty-tools");
        fs::create_dir(&empty).unwrap();
        let project = fake.dir.path().join("plain-project");
        fs::create_dir(&project).unwrap();
        fake.session("session:created-full", "project", &project);
        let file = fake.dir.path().join("frecency.toml");
        fs::write(&file, format!("[frecency]\n{fields}")).unwrap();
        let run = |args: &[&str]| {
            Command::new(env!("CARGO_BIN_EXE_rexue"))
                .arg("--rex-bin")
                .arg(&fake.rex.bin)
                .arg("-C")
                .arg(&file)
                .args(args)
                .env("PATH", &empty)
                .env("REXUE_STATE_DIR", fake.dir.path().join("state"))
                .env_remove("REX_SESSION")
                .output()
                .unwrap()
        };
        let r = run(&["list", "--zoxide", "--json"]);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        assert_eq!(
            serde_json::from_slice::<Value>(&r.stdout).unwrap(),
            json!([])
        );
        let r = run(&["--detach", "connect", project.to_str().unwrap(), "--json"]);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        assert_eq!(
            serde_json::from_slice::<Value>(&r.stdout).unwrap()["session_id"],
            "session:created-full"
        );
        let unresolved = run(&[
            "--detach",
            "connect",
            "missing-frecency-target-734129",
            "--json",
        ]);
        assert!(!unresolved.status.success());
        let error = String::from_utf8_lossy(&unresolved.stderr);
        assert!(
            error.contains("no session, configuration, or directory matches"),
            "{error}"
        );
        assert!(!error.contains("frecency query failed"));
        for invalid in ["'   '", "'/missing/custom-frecency-executable'"] {
            fs::write(&file, format!("[frecency]\nlist_command={invalid}\n")).unwrap();
            let r = run(&["list", "--zoxide", "--json"]);
            assert!(!r.status.success());
            assert!(
                String::from_utf8_lossy(&r.stderr).contains("could not list frecency directories")
            );
        }
    }
}

#[test]
fn configured_frecency_query_add_remove_keep_metacharacter_paths_as_data() {
    isolated(
        "configured_frecency_query_add_remove_keep_metacharacter_paths_as_data",
        || {
            let fake = FakeRex::new();
            let project = fake
                .dir
                .path()
                .join("project ' ; $(touch INJECTED) & spaces");
            fs::create_dir(&project).unwrap();
            let query = fake.dir.path().join("query argument");
            let added = fake.dir.path().join("added argument");
            let removed = fake.dir.path().join("removed argument");
            let record = |file: &Path, output: Option<&Path>| {
                let bin = file.with_extension("fake executable");
                let mut script = format!(
                    "#!/bin/sh\nprintf '%s' \"$1\" > {}\n",
                    shell_quote(file.to_str().unwrap())
                );
                if let Some(output) = output {
                    script.push_str(&format!(
                        "printf '%s\\n' {}\n",
                        shell_quote(output.to_str().unwrap())
                    ));
                }
                executable(&bin, &script);
                format!("{} {{}}", shell_quote(bin.to_str().unwrap()))
            };
            let manager = fake.manager(Config {
                frecency: FrecencyConfig {
                    list_command: Some("printf ''".into()),
                    query_command: Some(record(&query, Some(&project))),
                    add_command: Some(record(&added, None)),
                    remove_command: Some(record(&removed, None)),
                },
                ..Default::default()
            });
            let target = "query ' ; $(touch INJECTED) & literal";
            let connection = manager
                .connect(
                    target,
                    &ConnectOptions {
                        detach: true,
                        ..Default::default()
                    },
                )
                .unwrap();
            assert!(connection.created);
            assert_eq!(fs::read_to_string(query).unwrap(), target);
            let added_path = fs::read_to_string(added).unwrap();
            assert_eq!(
                Path::new(&added_path).canonicalize().unwrap(),
                project.canonicalize().unwrap()
            );
            let entry = rexue::model::Entry::new("remove", &project, Source::Zoxide);
            manager.remove(&entry).unwrap();
            assert_eq!(
                fs::read_to_string(removed).unwrap(),
                project.to_str().unwrap()
            );
            assert!(!project.join("INJECTED").exists());
            assert!(!fake.dir.path().join("INJECTED").exists());
        },
    );
}

#[test]
fn cli_uses_explicit_fake_rex_and_isolated_xdg_config() {
    let fake = FakeRex::new();
    let config_root = fake.dir.path().join("xdg config");
    fs::create_dir_all(config_root.join("rexue")).unwrap();
    let path = fake.dir.path().join("configured path ; literal");
    let config = format!(
        "[[session]]\nname = 'CLI configured'\npath = {}\n",
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    );
    fs::write(config_root.join("rexue/rexue.toml"), config).unwrap();
    let tools = fake.dir.path().join("tools");
    fs::create_dir(&tools).unwrap();
    executable(&tools.join("zoxide"), "#!/bin/sh\nexit 0\n");
    let output = Command::new(env!("CARGO_BIN_EXE_rexue"))
        .args(["list", "--config", "--json", "--no-cache"])
        .env("REXUE_REX", &fake.rex.bin)
        .env("REXUE_STATE_DIR", fake.dir.path().join("cli state"))
        .env("XDG_CONFIG_HOME", config_root)
        .env("XDG_STATE_HOME", fake.dir.path().join("xdg state"))
        .env("HOME", fake.dir.path())
        .env("PATH", tools)
        .env_remove("REX_CLIENT")
        .env_remove("REX_SERVER")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let entries: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(entries.as_array().unwrap().len(), 1);
    assert_eq!(entries[0]["name"], "CLI configured");
    assert_eq!(entries[0]["path"], json!(path));
    assert!(
        fake.calls().is_empty(),
        "config-only listing must not contact Rex"
    );
}
