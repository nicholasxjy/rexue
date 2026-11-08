#![cfg(unix)]

use rexue::{
    backend::Rex,
    config::{Config, SessionConfig},
    model::{Entry, Source},
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
if [ "$4" = '-S' ]; then request="$6 $7"; script="$8"; else request="$4 $5"; script="$6"; fi
case "$script" in
    *rexue-connect*)
        if [ ! -f "$root/connect.json" ]; then
            printf 'Error: unknown flag: --eval' >&2
            exit 2
        fi ;;
esac
if [ "$request" = 'do --eval' ] && [ ! -f "$root/connect.json" ] && [ ! -f "$root/snapshot.json" ] && [ ! -f "$root/snapshot.exit" ] && [ ! -f "$root/fail.exit" ]; then
    printf 'Error: unknown flag: --eval' >&2
    exit 2
fi
printf '%s\000' "$@" > "$root/calls/$$"
if [ -f "$root/fail.exit" ]; then
    /bin/cat "$root/fail.stderr" >&2
    exit "$(/bin/cat "$root/fail.exit")"
fi
shift 3
if [ "$1" = '-S' ]; then shift 2; fi
case "$1 $2" in
    'do --eval')
        if [ -f "$root/snapshot.exit" ]; then
            /bin/cat "$root/snapshot.stderr" >&2
            exit "$(/bin/cat "$root/snapshot.exit")"
        fi
        case "$3" in
            *rexue-connect*)
                /bin/cat "$root/connect.json"
                printf '\n'
                IFS= read -r reply || exit 90
                printf '%s' "$reply" > "$root/connect-reply.json"
                if [ -f "$root/connect-fail" ]; then printf 'switch fixture failure' >&2; exit 7; fi
                printf '{{}}' ;;
            *) /bin/cat "$root/snapshot.json" ;;
        esac ;;
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
        if [ -f "$root/check-overlap" ]; then
            if /bin/mkdir "$root/scanning" 2>/dev/null; then
                /bin/sleep 0.1
                /bin/rmdir "$root/scanning"
            else
                : > "$root/overlapped"
            fi
        fi
        if [ -f "$root/inspect-$3.exit" ]; then
            printf 'inspect fixture failure' >&2
            exit "$(/bin/cat "$root/inspect-$3.exit")"
        fi
        /bin/cat "$root/inspect-$3.json" ;;
    'block call')
        if [ -f "$root/$7-$6.exit" ]; then exit 3; fi
        file="$root/$7-$6.json"
        [ -f "$file" ] || file="$root/$7.json"
        /bin/cat "$file" ;;
    'new '*)
        /bin/cp "$root/after-create.json" "$root/sessions.json"
        /bin/cat "$root/created.json" ;;
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
        fake.json("program_status.json", json!({"records": []}));
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
        // Each child owns a log file, so concurrent discovery cannot overwrite calls.
        let mut calls: Vec<_> = fs::read_dir(self.dir.path().join("calls"))
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.metadata().unwrap().modified().unwrap(), entry.path())
            })
            .collect();
        calls.sort();
        calls
            .into_iter()
            .map(|(_, path)| {
                fs::read(path)
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

fn agents_fixture(fake: &FakeRex) {
    let session = fake.session("session:agents-full", "project", fake.dir.path());
    fake.json("sessions.json", json!({"sessions": [session]}));
    fake.json("inspect-session:agents-full.json", json!({
        "active_window_id": "window:shell-full",
        "windows": [
            {"window_id": "window:shell-full", "label": "Shell", "focused_block_id": "block:shell-full", "layers": [{"blocks": [
                {"block_id": "block:shell-full", "creator_name": "com.superlogical.terminal"},
                {"block_id": "block:browser-full", "creator_name": "browser"}
            ]}]},
            {"window_id": "window:agents-full", "label": "Agents", "focused_block_id": "block:pi-full", "layers": [{"blocks": [
                {"block_id": "block:pi-full", "creator_name": "com.superlogical.terminal"},
                {"block_id": "block:codex-full", "creator_name": "com.superlogical.terminal"},
                {"block_id": "block:stale-full", "creator_name": "com.superlogical.terminal"},
                {"block_id": "block:gone-full", "creator_name": "com.superlogical.terminal"}
            ]}]}
        ]
    }));
    fake.json(
        "process.json",
        json!({"last_exit": null, "foreground": {"pid": 1, "name": "zsh", "cwd": fake.dir.path()}}),
    );
    fake.json("process-block:pi-full.json", json!({"last_exit": null, "foreground": {"pid": 2, "name": "node", "argv0": "pi", "cwd": fake.dir.path()}}));
    fake.json("program_status-block:pi-full.json", json!({"records": [{"app": "pi", "state": "blocked", "owner": {"pid": 2, "name": "node"}}]}));
    // Running a tool under an agent keeps that agent in the ancestor chain.
    fake.json("process-block:codex-full.json", json!({"last_exit": null, "foreground": {"pid": 4, "name": "rg"}, "ancestors": [{"pid": 3, "name": "codex", "cwd": fake.dir.path()}]}));
    fs::write(
        fake.dir.path().join("program_status-block:codex-full.exit"),
        "",
    )
    .unwrap();
    // done/error survive shell prompts; neither is evidence of a live agent.
    fake.json(
        "program_status-block:stale-full.json",
        json!({"records": [{"app": "pi", "state": "done", "owner": {"pid": 99, "name": "node"}}]}),
    );
    fs::write(fake.dir.path().join("process-block:gone-full.exit"), "").unwrap();
}

#[test]
fn session_tab_names_resolve_titles_for_generated_window_labels() {
    let fake = FakeRex::new();
    let session = fake.session("session:titles-full", "project", fake.dir.path());
    fake.json("sessions.json", json!({"sessions": [session]}));
    fake.json("inspect-session:titles-full.json", json!({
        "windows": [
            {"window_id": "window:00000000-0000-0000-0000-000027a15bcd", "label": "Window 27a15bcd", "focused_block_id": "block:editor-full", "layers": [{"blocks": [
                {"block_id": "block:other-full", "creator_name": "com.superlogical.terminal", "label": "Other pane"},
                {"block_id": "block:editor-full", "creator_name": "com.superlogical.terminal", "label": "shell"}
            ]}]},
            {"window_id": "window:named-full", "label": "Deploy", "focused_block_id": "block:deploy-full"}
        ]
    }));
    fake.json(
        "title-block:other-full.json",
        json!({"title": "Wrong pane title"}),
    );
    fake.json(
        "title-block:editor-full.json",
        json!({"title": "编辑器 | rexue"}),
    );
    fake.json(
        "title-block:deploy-full.json",
        json!({"title": "A different terminal title"}),
    );

    let entries = fake.rex.list().unwrap();
    assert_eq!(entries[0].tab_names, ["编辑器 | rexue", "Deploy"]);
    assert_eq!(entries[0].target(), "session:titles-full");

    fake.json(
        "title-block:editor-full.json",
        json!({"title": "Renamed editor"}),
    );
    assert_eq!(
        fake.rex.list().unwrap()[0].tab_names,
        ["Renamed editor", "Deploy"]
    );

    fake.json(
        "process-block:editor-full.json",
        json!({"foreground": {"pid": 2, "name": "node", "argv0": "pi", "cwd": fake.dir.path()}}),
    );
    let entries = fake.rex.list_sources(true, true).unwrap();
    assert_eq!(entries[1].name, "project / pi · pane 2 · Renamed editor");
    let agent = entries[1].agent.as_ref().unwrap();
    assert_eq!(
        agent.window_id,
        "window:00000000-0000-0000-0000-000027a15bcd"
    );
    assert_eq!(agent.block_id, "block:editor-full");

    for title in [json!({"title": ""}), json!({})] {
        fake.json("title-block:editor-full.json", title);
        assert_eq!(fake.rex.list().unwrap()[0].tab_names, ["shell", "Deploy"]);
    }
    fs::write(fake.dir.path().join("title-block:editor-full.exit"), "").unwrap();
    assert_eq!(fake.rex.list().unwrap()[0].tab_names, ["shell", "Deploy"]);
}

fn snapshot_fixture(fake: &FakeRex) -> Value {
    let read = |name: &str| -> Value {
        serde_json::from_slice(&fs::read(fake.dir.path().join(name)).unwrap()).unwrap()
    };
    let mut response = read("sessions.json");
    let mut snapshots = serde_json::Map::new();
    for session in response["sessions"].as_array().unwrap() {
        let id = session["session_id"].as_str().unwrap();
        let view = read(&format!("inspect-{id}.json"));
        let mut blocks = serde_json::Map::new();
        for window in view["windows"].as_array().into_iter().flatten() {
            for layer in window["layers"].as_array().into_iter().flatten() {
                for block in layer["blocks"].as_array().into_iter().flatten() {
                    let block = block["block_id"].as_str().unwrap();
                    let mut metadata = serde_json::Map::new();
                    for method in ["process", "program_status", "title"] {
                        let file = format!("{method}-{block}.json");
                        if !fake
                            .dir
                            .path()
                            .join(format!("{method}-{block}.exit"))
                            .exists()
                        {
                            if fake.dir.path().join(&file).exists() {
                                metadata.insert(method.into(), read(&file));
                            } else if fake.dir.path().join(format!("{method}.json")).exists() {
                                metadata.insert(method.into(), read(&format!("{method}.json")));
                            }
                        }
                    }
                    blocks.insert(block.into(), Value::Object(metadata));
                }
            }
        }
        snapshots.insert(id.into(), json!({"view": view, "blocks": blocks}));
    }
    response["snapshots"] = Value::Object(snapshots);
    response
}

#[test]
fn fast_snapshot_preserves_catalog_with_one_process_and_refreshes_metadata() {
    let fake = FakeRex::new();
    agents_fixture(&fake);
    fake.json("title-block:pi-full.json", json!({"title": "Initial task"}));
    let expected = fake.rex.list_sources(true, true).unwrap();
    fake.json("snapshot.json", snapshot_fixture(&fake));
    let before = fake.calls().len();
    let actual = fake.rex.list_sources(true, true).unwrap();
    assert_eq!(
        serde_json::to_value(&actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(fake.calls().len() - before, 1);

    let mut snapshot = snapshot_fixture(&fake);
    snapshot["snapshots"]["session:agents-full"]["blocks"]["block:pi-full"]["title"] =
        json!({"title": "Updated task"});
    snapshot["snapshots"]["session:agents-full"]["blocks"]["block:pi-full"]["program_status"]["records"]
        [0]["state"] = "working".into();
    fake.json("snapshot.json", snapshot);
    let entries = fake.rex.list_sources(true, true).unwrap();
    assert!(entries[0].tab_names[1].contains("Updated task"));
    assert_eq!(entries[1].agent.as_ref().unwrap().state, "working");
    assert_eq!(entries[1].target(), "block:pi-full");
}

fn connection_fixture(fake: &FakeRex) -> Value {
    let mut snapshot = snapshot_fixture(fake);
    let read = |name| {
        serde_json::from_slice::<Value>(&fs::read(fake.dir.path().join(name)).unwrap()).unwrap()
    };
    snapshot["connection"] = json!({
        "clients": read("clients.json")["clients"],
        "inspections": {"client:desktop-full": read("actions.json")},
        "previous": read("context.json")["session_id"],
    });
    snapshot
}

#[test]
fn selected_session_refresh_batches_metadata_without_scanning_other_sessions() {
    let fake = FakeRex::new();
    agents_fixture(&fake);
    for block in [None, Some("block:codex-full")] {
        let expected = fake.rex.list_session("session:agents-full", block).unwrap();
        fake.json("snapshot.json", snapshot_fixture(&fake));
        let before = fake.calls().len();
        let actual = fake.rex.list_session("session:agents-full", block).unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        let calls = fake.calls();
        assert_eq!(
            calls.len() - before,
            1,
            "refresh must use one Rex connection"
        );
        let call = &calls[before];
        let args = call.iter().position(|arg| arg == "--args").unwrap();
        let args: Value = serde_json::from_str(&call[args + 1]).unwrap();
        assert_eq!(args["session_id"], "session:agents-full");
        assert_eq!(args["block_id"], json!(block));
        fs::remove_file(fake.dir.path().join("snapshot.json")).unwrap();
    }
}

#[test]
fn selected_agent_refresh_uses_fresh_window_and_rejects_exited_agents() {
    let fake = FakeRex::new();
    agents_fixture(&fake);
    let manager = fake.manager(Config::default());
    let mut entry = manager.resolve_entry("block:codex-full").unwrap();
    entry.agent.as_mut().unwrap().window_id = "window:old-full".into();
    fake.json("connect.json", connection_fixture(&fake));
    let before = fake.calls().len();
    manager
        .connect_entry(entry.clone(), &ConnectOptions::default())
        .unwrap();
    let calls = fake.calls();
    assert_eq!(
        calls.len() - before,
        1,
        "agent validation and focus must share one Rex connection"
    );
    let args: Value =
        serde_json::from_slice(&fs::read(fake.dir.path().join("connect-reply.json")).unwrap())
            .unwrap();
    assert_eq!(args["session_id"], "session:agents-full");
    assert_eq!(args["window_id"], "window:agents-full");
    assert_eq!(args["block_id"], "block:codex-full");
    let mut snapshot = connection_fixture(&fake);
    snapshot["snapshots"]["session:agents-full"]["blocks"]["block:codex-full"]["process"] =
        json!({"last_exit": {"exit_code": 0}});
    fake.json("connect.json", snapshot);
    fs::remove_file(fake.dir.path().join("connect-reply.json")).unwrap();
    let before = fake.calls().len();
    let error = manager
        .connect_entry(entry, &ConnectOptions::default())
        .unwrap_err();
    assert!(error.to_string().contains("no longer running"));
    assert_eq!(
        fake.calls().len() - before,
        1,
        "reject before focus or switch"
    );
    assert!(!fake.dir.path().join("connect-reply.json").exists());
}

#[test]
fn one_connection_agent_snapshot_needs_only_its_live_process_and_status() {
    let fake = FakeRex::new();
    agents_fixture(&fake);
    let manager = fake.manager(Config::default());
    let entry = manager.resolve_entry("block:codex-full").unwrap();
    let mut snapshot = connection_fixture(&fake);
    let session = &mut snapshot["snapshots"]["session:agents-full"];
    session["target_only"] = json!(true);
    let process = session["blocks"]["block:codex-full"]["process"].clone();
    session["blocks"] =
        json!({"block:codex-full": {"process": process, "program_status": {"records": []}}});
    fake.json("connect.json", snapshot);
    let before = fake.calls().len();
    let result = manager
        .connect_entry(entry, &ConnectOptions::default())
        .unwrap();
    assert_eq!(result.path, fs::canonicalize(fake.dir.path()).unwrap());
    assert_eq!(
        fake.calls().len() - before,
        1,
        "do not query unrelated focused panes or missing titles"
    );
    let reply: Value =
        serde_json::from_slice(&fs::read(fake.dir.path().join("connect-reply.json")).unwrap())
            .unwrap();
    assert_eq!(reply["window_id"], "window:agents-full");
    assert_eq!(reply["block_id"], "block:codex-full");
}

#[test]
#[ignore = "requires the installed Rex CLI with Lua; uses mocked RPCs"]
fn lua_snapshot_only_reads_needed_connection_metadata() {
    let script = concat!(
        r#"
local calls = 0
rex.call = function(method, args)
    calls = calls + 1
    if method == "session.list" then
        return {sessions = {{session_id = "session:target", label = "target"}}}
    elseif method == "session.attach" then
        local function pane(id) return {block_id = id, creator_name = "com.superlogical.terminal"} end
        return {view = {label = "target", active_window_id = "window:one", windows = {
            {window_id = "window:one", focused_block_id = "block:other", layers = {{blocks = {pane("block:other")}}}},
            {window_id = "window:two", focused_block_id = "block:peer", layers = {{blocks = {pane("block:target"), pane("block:peer")}}}},
        }}}
    elseif method == "block.method" then
        if rex.args.connect then
            assert(args.method ~= "title", "selection must not read display titles")
            if rex.args.block_id then assert(args.block_id == rex.args.block_id, "selection scanned an unrelated pane") end
        end
        return {}
    end
    error("unexpected method " .. method)
end
local function snapshot()
"#,
        include_str!("../src/rex_snapshot.lua"),
        r#"
end
local result = snapshot()
return {calls = calls, session = result.snapshots["session:target"]}
"#
    );
    for (args, expected, blocks) in [
        (json!({}), 11, 3),
        (
            json!({"session_id": "session:target", "block_id": "block:target"}),
            10,
            3,
        ),
        (
            json!({"session_id": "session:target", "connect": true}),
            7,
            3,
        ),
        (
            json!({"session_id": "session:target", "block_id": "block:target", "connect": true}),
            3,
            1,
        ),
    ] {
        let result = Rex::default()
            .json(&[
                "do",
                "--eval",
                script,
                "--args",
                &args.to_string(),
                "--json",
            ])
            .unwrap();
        assert_eq!(result["calls"], expected, "{args}");
        assert_eq!(
            result["session"]["blocks"].as_object().unwrap().len(),
            blocks
        );
    }
}

#[test]
fn one_connection_preserves_session_tab_pane_and_detach_targets() {
    for (window, block, expected_window, detach) in [
        (None, None, None, false),
        (
            Some("window:agents-full"),
            None,
            Some("window:agents-full"),
            false,
        ),
        (
            Some("window:old-full"),
            Some("block:stale-full"),
            Some("window:agents-full"),
            false,
        ),
        (
            Some("window:old-full"),
            Some("block:browser-full"),
            Some("window:shell-full"),
            true,
        ),
    ] {
        let fake = FakeRex::new();
        agents_fixture(&fake);
        fake.json("connect.json", connection_fixture(&fake));
        let manager = fake.manager(Config::default());
        let mut entry = Entry::new("old label", fake.dir.path(), Source::Rex);
        entry.session_id = Some("session:agents-full".into());
        entry.window_id = window.map(str::to_owned);
        entry.block_id = block.map(str::to_owned);
        let result = manager
            .connect_entry(
                entry,
                &ConnectOptions {
                    detach,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(!result.created);
        let reply: Value =
            serde_json::from_slice(&fs::read(fake.dir.path().join("connect-reply.json")).unwrap())
                .unwrap();
        assert_eq!(reply["window_id"], json!(expected_window));
        assert_eq!(reply["block_id"], json!(block));
        assert_eq!(
            reply["client_id"],
            if detach {
                Value::Null
            } else {
                json!("client:desktop-full")
            }
        );
        assert_eq!(fake.calls().len(), 1);
    }
}

#[test]
fn one_connection_switch_failure_preserves_history_and_does_not_retry() {
    let fake = FakeRex::new();
    agents_fixture(&fake);
    let manager = fake.manager(Config::default());
    let entry = manager.resolve_entry("block:pi-full").unwrap();
    fake.json("connect.json", connection_fixture(&fake));
    manager
        .connect_entry(entry.clone(), &ConnectOptions::default())
        .unwrap();
    let state_file = manager.state_dir.join("state.json");
    let before = fs::read(&state_file).unwrap();
    let state: Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(
        state["history"]["client:desktop-full"],
        json!(["session:agents-full", "session:previous-full"])
    );
    fs::write(fake.dir.path().join("connect-fail"), "").unwrap();
    let calls = fake.calls().len();
    let error = manager
        .connect_entry(entry, &ConnectOptions::default())
        .unwrap_err();
    assert!(format!("{error:#}").contains("switch fixture failure"));
    assert_eq!(fs::read(state_file).unwrap(), before);
    assert_eq!(fake.calls().len() - calls, 1);
}

#[test]
fn one_connection_preflight_errors_never_submit_focus() {
    for mode in ["unsupported", "ambiguous", "malformed"] {
        let fake = FakeRex::new();
        agents_fixture(&fake);
        let manager = fake.manager(Config::default());
        let entry = manager.resolve_entry("block:pi-full").unwrap();
        let mut snapshot = connection_fixture(&fake);
        let mut manager = manager;
        match mode {
            "unsupported" => {
                snapshot["connection"]["inspections"]["client:desktop-full"] =
                    json!({"actions": []})
            }
            "ambiguous" => {
                manager.rex.client = None;
                snapshot["connection"]["clients"]
                    .as_array_mut()
                    .unwrap()
                    .push(client("client:other-full", "Other", "app", "connected"));
            }
            _ => snapshot["snapshots"] = json!({}),
        }
        fake.json("connect.json", snapshot);
        let calls = fake.calls().len();
        let error = manager
            .connect_entry(entry, &ConnectOptions::default())
            .unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains(match mode {
                "unsupported" => "does not support session.select",
                "ambiguous" => "multiple Rex app clients",
                _ => "missing session metadata",
            }),
            "{message}"
        );
        assert_eq!(fake.calls().len() - calls, 1);
        assert!(!fake.dir.path().join("connect-reply.json").exists());
        assert!(!manager.state_dir.join("state.json").exists());
    }
}

#[test]
fn fast_snapshot_keeps_errors_visible_instead_of_falling_back() {
    let fake = FakeRex::new();
    agents_fixture(&fake);
    fs::write(fake.dir.path().join("snapshot.exit"), "7").unwrap();
    fs::write(
        fake.dir.path().join("snapshot.stderr"),
        "snapshot connection failed",
    )
    .unwrap();
    let error = fake.rex.list().unwrap_err();
    assert!(format!("{error:#}").contains("snapshot connection failed"));
    assert_eq!(fake.calls().len(), 1);
}

#[test]
fn fast_snapshot_rejects_malformed_metadata() {
    let fake = FakeRex::new();
    agents_fixture(&fake);
    for response in [
        json!({}),
        json!({"sessions": [], "snapshots": []}),
        json!({
            "sessions": [{"session_id": "session:agents-full"}], "snapshots": {}
        }),
        json!({
            "sessions": [{"session_id": "session:agents-full"}],
            "snapshots": {"session:agents-full": {"view": null, "blocks": {}}}
        }),
    ] {
        fake.json("snapshot.json", response);
        assert!(fake.rex.list().is_err());
    }
    fs::write(fake.dir.path().join("snapshot.json"), "invalid JSON").unwrap();
    assert!(fake.rex.list().is_err());
}

#[test]
fn fast_snapshot_handles_empty_lua_tables_and_vanished_sessions() {
    let fake = FakeRex::new();
    fake.json("snapshot.json", json!({"sessions": {}, "snapshots": {}}));
    assert!(fake.rex.list().unwrap().is_empty());
    fake.json(
        "snapshot.json",
        json!({
            "sessions": [{"session_id": "session:vanished-full"}],
            "snapshots": {"session:vanished-full": {"missing": true}}
        }),
    );
    assert!(fake.rex.list().unwrap().is_empty());
    assert_eq!(fake.calls().len(), 2);
}

#[test]
fn discovery_reads_each_needed_title_once_per_refresh() {
    let fake = FakeRex::new();
    agents_fixture(&fake);
    let inspect_path = fake.dir.path().join("inspect-session:agents-full.json");
    let mut inspect: Value = serde_json::from_slice(&fs::read(inspect_path).unwrap()).unwrap();
    inspect["windows"][1]["label"] = "Window nts-full".into();
    fake.json("inspect-session:agents-full.json", inspect);
    fake.json("title-block:codex-full.json", json!({"title": "Review"}));

    for (refresh, title) in ["Initial task", "Updated task"].iter().enumerate() {
        fake.json("title-block:pi-full.json", json!({"title": title}));
        let entries = fake.rex.list_sources(true, true).unwrap();
        assert_eq!(
            entries[0].tab_names,
            ["Shell", &format!("{title} · Review")]
        );
        assert!(entries[1].name.ends_with(title));
        for block in ["block:pi-full", "block:codex-full"] {
            let calls = fake
                .calls()
                .iter()
                .filter(|args| {
                    args.windows(7).any(|args| {
                        args == [
                            "block",
                            "call",
                            "-s",
                            "session:agents-full",
                            "-b",
                            block,
                            "title",
                        ]
                    })
                })
                .count();
            assert_eq!(calls, refresh + 1, "read {block}'s title once per refresh");
        }
    }
}

#[test]
fn session_tab_names_preserve_order_duplicates_and_full_identity() {
    let fake = FakeRex::new();
    let session = fake.session("session:tabs-full", "project", fake.dir.path());
    fake.json("sessions.json", json!({"sessions": [session]}));
    fake.json(
        "inspect-session:tabs-full.json",
        json!({
            "windows": [
                {"window_id": "window:shell", "label": "Shell"},
                {"window_id": "window:editor", "label": "编辑器"},
                {"window_id": "window:other-shell", "label": "Shell"},
                {"window_id": "window:unnamed"},
                {"window_id": "window:empty", "label": ""}
            ]
        }),
    );
    let entries = fake.rex.list().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "project");
    assert_eq!(entries[0].target(), "session:tabs-full");
    assert_eq!(
        serde_json::to_value(&entries[0]).unwrap()["tab_names"],
        json!(["Shell", "编辑器", "Shell"])
    );

    for inspect in [json!({}), json!({"windows": []})] {
        fake.json("inspect-session:tabs-full.json", inspect);
        let entries = fake.rex.list().unwrap();
        assert_eq!(entries[0].name, "project");
        assert!(
            serde_json::to_value(&entries[0])
                .unwrap()
                .get("tab_names")
                .is_none()
        );
    }
}

#[test]
fn session_tabs_include_live_agent_titles_even_without_agent_rows() {
    let fake = FakeRex::new();
    agents_fixture(&fake);
    fake.json(
        "title-block:pi-full.json",
        json!({"title": "Fix picker titles"}),
    );
    fake.json(
        "title-block:codex-full.json",
        json!({"title": "Review changes"}),
    );
    fake.json("title-block:stale-full.json", json!({"title": "Old task"}));

    let entries = fake.rex.list().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].target(), "session:agents-full");
    assert_eq!(
        entries[0].tab_names,
        ["Shell", "Agents · Fix picker titles · Review changes"]
    );
    assert_eq!(
        serde_json::to_value(&entries[0]).unwrap()["agent_tab_indices"],
        json!([1])
    );

    // A shell can have focus while an agent runs in another pane of the tab.
    let mut inspect: Value = serde_json::from_slice(
        &fs::read(fake.dir.path().join("inspect-session:agents-full.json")).unwrap(),
    )
    .unwrap();
    inspect["windows"][1]["focused_block_id"] = "block:stale-full".into();
    fake.json("inspect-session:agents-full.json", inspect.clone());
    assert_eq!(fake.rex.list().unwrap()[0].tab_names, entries[0].tab_names);

    // Repeated pane titles appear once within a tab; empty/unavailable titles
    // retain the tab name, and stale status never marks a shell as an agent.
    fake.json(
        "title-block:codex-full.json",
        json!({"title": "Fix picker titles"}),
    );
    assert_eq!(
        fake.rex.list().unwrap()[0].tab_names,
        ["Shell", "Agents · Fix picker titles"]
    );
    fake.json("title-block:pi-full.json", json!({"title": ""}));
    fs::write(fake.dir.path().join("title-block:codex-full.exit"), "").unwrap();
    assert_eq!(fake.rex.list().unwrap()[0].tab_names, ["Shell", "Agents"]);

    // A tab without a name or title still exposes its live agent.
    fake.json("title-block:stale-full.json", json!({"title": ""}));
    inspect["windows"][1]["label"] = "".into();
    fake.json("inspect-session:agents-full.json", inspect);
    assert_eq!(
        fake.rex.list().unwrap()[0].tab_names,
        ["Shell", "pi · codex"]
    );

    for block in ["pi", "codex"] {
        fake.json(
            &format!("process-block:{block}-full.json"),
            json!({"last_exit": {"exit_code": 0}}),
        );
    }
    let entry = &fake.rex.list().unwrap()[0];
    assert_eq!(entry.tab_names, ["Shell"]);
    assert!(
        serde_json::to_value(entry)
            .unwrap()
            .get("agent_tab_indices")
            .is_none()
    );
}

#[test]
fn agents_discover_inactive_tabs_and_panes_without_stale_status_or_dedup_loss() {
    let fake = FakeRex::new();
    agents_fixture(&fake);
    let manager = fake.manager(Config::default());
    let entries = manager
        .list(&ListOptions {
            agents: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|e| e.source == Source::Agent));
    assert_eq!(entries[0].agent.as_ref().unwrap().app, "pi");
    assert_eq!(entries[0].agent.as_ref().unwrap().state, "blocked");
    assert_eq!(entries[0].target(), "block:pi-full");
    assert_eq!(entries[1].agent.as_ref().unwrap().app, "codex");
    assert_eq!(entries[1].agent.as_ref().unwrap().state, "running");
    assert_eq!(entries[1].path, fake.dir.path());
    let all = manager
        .list(&ListOptions {
            rex: true,
            agents: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(
        all.iter().map(|entry| entry.source).collect::<Vec<_>>(),
        [Source::Rex, Source::Agent, Source::Agent]
    );
    let only_sessions = manager
        .list(&ListOptions {
            rex: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(only_sessions.len(), 1);
    fake.json("context.json", json!({"session_id": "session:agents-full"}));
    assert!(
        manager
            .list(&ListOptions {
                agents: true,
                hide_attached: true,
                ..Default::default()
            })
            .unwrap()
            .is_empty()
    );
    let manager = fake.manager(Config {
        blacklist: vec!["^project$".into()],
        ..Default::default()
    });
    assert!(
        manager
            .list(&ListOptions {
                agents: true,
                ..Default::default()
            })
            .unwrap()
            .is_empty()
    );
    assert!(
        !fake
            .calls()
            .iter()
            .any(|args| args.iter().any(|arg| arg == "block:browser-full"))
    );
}

#[test]
fn agent_selection_previews_and_focuses_exact_pane_and_rejects_exited_target() {
    let fake = FakeRex::new();
    agents_fixture(&fake);
    let mut manager = fake.manager(Config::default());
    manager.config.session.push(SessionConfig {
        name: "alias-trap".into(),
        path: fake.dir.path().display().to_string(),
        alias: Some("block:codex-full".into()),
        ..Default::default()
    });
    let entry = manager.resolve_entry("block:codex-full").unwrap();
    assert!(
        manager
            .preview(&entry)
            .unwrap()
            .contains("terminal fixture")
    );
    assert!(manager.remove(&entry).is_err());
    assert!(manager.rename(&entry, "rename").is_err());
    let mut entry = entry;
    // The pane may have moved since the row was fetched; use its refreshed window.
    entry.agent.as_mut().unwrap().window_id = "window:old-full".into();
    let connection = manager
        .connect_entry(entry.clone(), &ConnectOptions::default())
        .unwrap();
    assert_eq!(connection.session_id, "session:agents-full");
    assert_eq!(connection.name, "project");
    assert!(!connection.created);
    let calls = fake.calls();
    assert!(calls.iter().any(|a| a.windows(6).any(|a| a
        == [
            "capture",
            "-s",
            "session:agents-full",
            "-b",
            "block:codex-full",
            "--trim"
        ])));
    let focus = calls
        .iter()
        .position(|a| {
            a.windows(4)
                .any(|a| a == ["focus", "-s", "session:agents-full", "block:codex-full"])
        })
        .unwrap();
    let switch = calls
        .iter()
        .position(|a| a.iter().any(|a| a == "session.select"))
        .unwrap();
    assert!(focus < switch);
    let args_index = calls[switch]
        .iter()
        .position(|arg| arg == "--args")
        .unwrap();
    let selected: Value = serde_json::from_str(&calls[switch][args_index + 1]).unwrap();
    assert_eq!(selected["session_id"], "session:agents-full");
    assert_eq!(
        selected["window_id"], "window:agents-full",
        "the client must select the agent's tab so future picker actions use the visible window"
    );
    assert!(
        !calls
            .iter()
            .any(|a| a.iter().any(|a| a == "new" || a == "kill"))
    );
    fake.json(
        "process-block:codex-full.json",
        json!({"last_exit": {"exit_code": 0}}),
    );
    let error = manager
        .connect_entry(entry, &ConnectOptions::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("no longer running"), "{error}");
}

#[test]
fn session_tree_discovers_all_tabs_and_panes_with_full_ids() {
    let fake = FakeRex::new();
    agents_fixture(&fake);
    let entries = fake.rex.list_sources(true, true).unwrap();
    let session = serde_json::to_value(&entries[0]).unwrap();
    let tabs = session["tabs"].as_array().expect("session tree metadata");
    assert_eq!(tabs.len(), 2);
    assert_eq!(tabs[0]["window_id"], "window:shell-full");
    assert_eq!(tabs[0]["panes"].as_array().unwrap().len(), 2);
    assert_eq!(tabs[0]["panes"][0]["block_id"], "block:shell-full");
    assert_eq!(tabs[0]["panes"][1]["block_id"], "block:browser-full");
    assert_eq!(tabs[1]["panes"].as_array().unwrap().len(), 4);
    assert_eq!(tabs[1]["panes"][0]["agent"]["block_id"], "block:pi-full");
    assert_eq!(tabs[1]["panes"][2]["block_id"], "block:stale-full");
    assert!(tabs[1]["panes"][2]["agent"].is_null());
    let manager = fake.manager(Config {
        session: vec![SessionConfig {
            name: "project".into(),
            path: fake.dir.path().display().to_string(),
            icon: Some("custom".into()),
            ..Default::default()
        }],
        ..Default::default()
    });
    let sessions = manager
        .list(&ListOptions {
            rex: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(sessions[0].tabs[1].panes[0].icon.as_deref(), Some("custom"));
}

#[test]
fn session_tree_selection_focuses_tabs_and_ordinary_panes_and_rejects_missing_targets() {
    for (window, block, current_window) in [
        ("window:agents-full", None, "window:agents-full"),
        (
            "window:old-full",
            Some("block:stale-full"),
            "window:agents-full",
        ),
        (
            "window:old-full",
            Some("block:browser-full"),
            "window:shell-full",
        ),
    ] {
        let fake = FakeRex::new();
        agents_fixture(&fake);
        let manager = fake.manager(Config::default());
        let entry: Entry = serde_json::from_value(json!({
            "name": "old display label", "source": "rex", "session_id": "session:agents-full",
            "window_id": window, "block_id": block
        }))
        .unwrap();
        assert!(manager.rename(&entry, "wrong").is_err());
        assert!(manager.remove(&entry).is_err());
        let connection = manager
            .connect_entry(entry.clone(), &ConnectOptions::default())
            .unwrap();
        assert_eq!(
            fs::canonicalize(connection.path).unwrap(),
            fs::canonicalize(fake.dir.path()).unwrap()
        );
        let calls = fake.calls();
        let target = block.unwrap_or(window);
        assert!(
            calls.iter().any(|a| a
                .windows(4)
                .any(|a| a == ["focus", "-s", "session:agents-full", target])),
            "{calls:?}"
        );
        let switch = calls
            .iter()
            .find(|a| a.iter().any(|arg| arg == "session.select"))
            .unwrap();
        let index = switch.iter().position(|arg| arg == "--args").unwrap();
        let selected: Value = serde_json::from_str(&switch[index + 1]).unwrap();
        assert_eq!(selected["window_id"], current_window);
        assert!(
            manager
                .preview(&entry)
                .unwrap()
                .contains("terminal fixture")
        );
        let calls = fake.calls();
        assert!(calls.iter().any(|a| a.windows(5).any(|a| a
            == [
                "capture",
                "-s",
                "session:agents-full",
                "-b",
                block.unwrap_or("block:pi-full")
            ])));
        fake.json("inspect-session:agents-full.json", json!({"windows": []}));
        let error = manager
            .connect_entry(entry, &ConnectOptions::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("no longer exists"), "{error}");
    }
}

#[test]
fn picker_lists_omit_only_their_own_floating_pane() {
    let fake = FakeRex::new();
    agents_fixture(&fake);
    let inspect_path = fake.dir.path().join("inspect-session:agents-full.json");
    let mut inspect: Value = serde_json::from_slice(&fs::read(&inspect_path).unwrap()).unwrap();
    inspect["active_window_id"] = "window:agents-full".into();
    let window = &mut inspect["windows"][1];
    window["label"] = "".into();
    window["focused_block_id"] = "block:picker-full".into();
    window["layers"].as_array_mut().unwrap().push(json!({
        "kind": "floating", "blocks": [
            {"block_id": "block:picker-full", "creator_name": "com.superlogical.terminal"},
            {"block_id": "block:notes-full", "creator_name": "com.superlogical.terminal"}
        ]
    }));
    fake.json("inspect-session:agents-full.json", inspect);
    fake.json("title-block:picker-full.json", json!({"title": "Picker"}));
    fake.json("title-block:pi-full.json", json!({"title": "Editor"}));
    fake.json(
        "process-block:picker-full.json",
        json!({
            "foreground": {"name": "fzf", "cwd": "/picker-directory"}
        }),
    );
    let manager = fake.manager(Config::default());
    let all = manager
        .list(&ListOptions {
            rex: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(all[0].tabs[1].panes.len(), 6);
    assert!(all[0].tabs[1].name.contains("Picker"));

    let visible = manager
        .list(&ListOptions {
            rex: true,
            agents: true,
            picker_block: Some("block:picker-full".into()),
            ..Default::default()
        })
        .unwrap();
    let session = &visible[0];
    assert_eq!(session.tabs[1].panes.len(), 5);
    assert!(session.tabs[1].name.contains("Editor"));
    assert!(!session.tabs[1].name.contains("Picker"));
    assert_eq!(session.path, fake.dir.path());
    assert!(
        session
            .tabs
            .iter()
            .flat_map(|tab| &tab.panes)
            .all(|pane| pane.block_id() != Some("block:picker-full"))
    );
    let notes = session.tabs[1]
        .panes
        .iter()
        .find(|pane| pane.block_id() == Some("block:notes-full"))
        .unwrap();
    assert!(notes.name.contains("pane 5"), "{}", notes.name);
    assert!(
        visible
            .iter()
            .any(|entry| entry.block_id() == Some("block:pi-full"))
    );

    // A picker running in a normal tiled terminal keeps that terminal listed.
    let tiled = manager
        .list(&ListOptions {
            rex: true,
            picker_block: Some("block:pi-full".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(tiled[0].tabs[1].panes.len(), 6);
    assert!(
        tiled[0].tabs[1]
            .panes
            .iter()
            .any(|pane| pane.block_id() == Some("block:pi-full"))
    );
}

#[test]
fn discovery_overlaps_independent_sessions_and_preserves_order() {
    let fake = FakeRex::new();
    let rows: Vec<_> = (0..8)
        .map(|i| {
            fake.session(
                &format!("session:{i}-full"),
                &format!("session-{i}"),
                fake.dir.path(),
            )
        })
        .collect();
    fake.json("sessions.json", json!({"sessions": rows}));
    fs::write(fake.dir.path().join("check-overlap"), "").unwrap();
    let entries = fake.rex.list().unwrap();
    assert!(
        fake.dir.path().join("overlapped").exists(),
        "independent session queries must overlap"
    );
    assert_eq!(
        entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
        (0..8).map(|i| format!("session-{i}")).collect::<Vec<_>>()
    );
    assert_eq!(
        fake.calls().len(),
        17,
        "one list, one inspect and one process query per session"
    );
}

#[test]
fn picker_live_connect_does_not_inspect_unrelated_sessions() {
    let fake = FakeRex::new();
    let selected = fake.session("session:selected-full", "selected", fake.dir.path());
    let unrelated = fake.session("session:unrelated-full", "unrelated", fake.dir.path());
    fake.json("sessions.json", json!({"sessions": [unrelated, selected]}));
    fs::write(
        fake.dir.path().join("inspect-session:unrelated-full.exit"),
        "4",
    )
    .unwrap();
    let manager = fake.manager(Config::default());
    let mut entry = Entry::new("old label", fake.dir.path(), Source::Rex);
    entry.session_id = Some("session:selected-full".into());
    let result = manager
        .connect_entry(entry, &ConnectOptions::default())
        .unwrap();
    assert_eq!(result.session_id, "session:selected-full");
    assert_eq!(result.name, "selected");
    assert!(!result.created);
    let inspections: Vec<_> = fake
        .calls()
        .into_iter()
        .filter(|args| args.windows(2).any(|args| args == ["session", "inspect"]))
        .collect();
    assert_eq!(
        inspections.len(),
        1,
        "only the selected session needs inspection"
    );
    assert!(
        inspections[0]
            .iter()
            .any(|arg| arg == "session:selected-full")
    );
}

#[test]
fn picker_agent_connect_revalidates_only_the_selected_pane() {
    let fake = FakeRex::new();
    agents_fixture(&fake);
    let entry = fake
        .manager(Config::default())
        .resolve_entry("block:pi-full")
        .unwrap();
    let before = fake.calls().len();
    let manager = fake.manager(Config::default());
    let result = manager
        .connect_entry(entry.clone(), &ConnectOptions::default())
        .unwrap();
    assert_eq!(result.session_id, "session:agents-full");
    let calls = fake.calls();
    let queries: Vec<_> = calls[before..]
        .iter()
        .filter(|args| args.windows(2).any(|args| args == ["block", "call"]))
        .collect();
    assert!(
        queries.iter().all(|args| !args.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "block:codex-full" | "block:stale-full" | "block:gone-full"
            )
        })),
        "connecting must not rescan unrelated panes: {queries:?}"
    );
    assert_eq!(
        calls[before..]
            .iter()
            .filter(|args| args.windows(2).any(|args| args == ["session", "inspect"]))
            .count(),
        1
    );
    fake.json(
        "process-block:pi-full.json",
        json!({"last_exit": {"exit_code": 0}}),
    );
    let error = manager
        .connect_entry(entry, &ConnectOptions::default())
        .unwrap_err()
        .to_string();
    assert!(error.contains("no longer running"), "{error}");
}

#[test]
fn agents_in_the_same_tab_have_distinct_pane_labels() {
    let fake = FakeRex::new();
    agents_fixture(&fake);
    for (block, pid) in [("pi", 2), ("codex", 3)] {
        fake.json(
            &format!("process-block:{block}-full.json"),
            json!({
                "last_exit": null,
                "foreground": {"pid": pid, "name": "node", "argv0": "pi", "cwd": fake.dir.path()}
            }),
        );
        fake.json(
            &format!("program_status-block:{block}-full.json"),
            json!({"records": []}),
        );
    }
    let entries = fake.rex.list_sources(false, true).unwrap();
    assert_eq!(entries.len(), 2);
    assert_ne!(
        entries[0].name, entries[1].name,
        "different panes must not appear as duplicate agents"
    );
    assert_eq!(entries[0].name, "project / pi · pane 1 · Agents");
    assert_eq!(entries[1].name, "project / pi · pane 2 · Agents");
    // An exited terminal and a returned shell must both retire stale OSC records.
    fake.json(
        "process-block:pi-full.json",
        json!({
            "last_exit": {"exit_code": 0},
            "foreground": {"pid": 2, "name": "node", "argv0": "pi"}
        }),
    );
    fake.json(
        "process-block:codex-full.json",
        json!({
            "last_exit": null,
            "foreground": {"pid": 30, "name": "zsh", "cwd": fake.dir.path()}
        }),
    );
    for (block, pid) in [("pi", 2), ("codex", 3)] {
        fake.json(
            &format!("program_status-block:{block}-full.json"),
            json!({
                "records": [{"app": "pi", "state": "done", "owner": {"pid": pid, "name": "node"}}]
            }),
        );
    }
    assert!(
        fake.rex.list_sources(false, true).unwrap().is_empty(),
        "exited agents must disappear even when status records remain"
    );
}

#[test]
fn agents_cli_json_and_source_configuration() {
    let fake = FakeRex::new();
    agents_fixture(&fake);
    let config = fake.dir.path().join("agents.toml");
    fs::write(&config, "strict_mode = true\nsort_order = ['agent', 'rex']\n[icons.agent]\nicon = 'A'\ncolor = 'magenta'\n").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_rexue"))
        .args(["--rex-bin"])
        .arg(&fake.rex.bin)
        .args(["-C"])
        .arg(&config)
        .args(["list", "--agents", "--json"])
        .env("REXUE_STATE_DIR", fake.dir.path().join("cli-state"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let entries: Vec<Value> = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["source"], "agent");
    assert_eq!(entries[0]["agent"]["block_id"], "block:pi-full");
    assert_eq!(entries[0]["agent"]["state"], "blocked");
}

fn client(id: &str, label: &str, kind: &str, connection: &str) -> Value {
    json!({"client_id": id, "info": {"label": label, "kind": kind, "platform": "macos"}, "connection_state": connection})
}

#[test]
fn session_switch_history_preview_and_removal_keep_full_ids() {
    let fake = FakeRex::new();
    let first = fake.session("session:first-full", "same", fake.dir.path());
    let second = fake.session("session:second-full", "same", fake.dir.path());
    fake.json("sessions.json", json!({"sessions": [first, second]}));
    let manager = fake.manager(Config::default());
    manager
        .connect("session:first-full", &ConnectOptions::default())
        .unwrap();
    manager
        .connect("session:second-full", &ConnectOptions::default())
        .unwrap();
    let previous = manager.last().unwrap();
    assert_eq!(previous.session_id, "session:first-full");
    assert!(!previous.created);
    let entry = manager.resolve_entry("session:first-full").unwrap();
    assert_eq!(manager.preview(&entry).unwrap(), "terminal fixture\n");
    manager.remove(&entry).unwrap();
    assert_eq!(
        &fake.calls().last().unwrap()[5..],
        ["kill", "session:first-full"]
    );
}

#[test]
fn rename_cli_validates_names_and_uses_the_resolved_session_id() {
    let fake = FakeRex::new();
    let live = fake.session("session:rename-full", "project", fake.dir.path());
    fake.json("sessions.json", json!({"sessions": [live]}));
    let cfg = fake.dir.path().join("rename.toml");
    fs::write(&cfg, "strict_mode=true").unwrap();
    let invalid = session_cli(&fake, &cfg, &["rename", "project", "--to", "bad\nname"]);
    assert!(!invalid.status.success());
    assert!(
        !fake
            .calls()
            .iter()
            .any(|call| call.windows(2).any(|args| args == ["session", "rename"]))
    );
    let result = session_cli(
        &fake,
        &cfg,
        &["rename", "project", "--to", "renamed ; literal"],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, b"renamed ; literal\n");
    assert!(fake.calls().iter().any(|call| call.ends_with(&[
        "session".into(),
        "rename".into(),
        "session:rename-full".into(),
        "renamed ; literal".into()
    ])));
}

#[test]
fn connect_root_uses_git_checkout_and_matching_session_startup() {
    let fake = FakeRex::new();
    let repo = fake.dir.path().join("repo");
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(&repo)
            .status()
            .unwrap()
            .success()
    );
    let child = repo.join("nested");
    fs::create_dir(&child).unwrap();
    fake.session("session:created-full", "app", &repo);
    let config = Config {
        session: vec![SessionConfig {
            name: "app".into(),
            path: repo.display().to_string(),
            startup_command: Some("printf root-start".into()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let manager = fake.manager(config);
    let result = manager
        .connect(
            child.to_str().unwrap(),
            &ConnectOptions {
                root: true,
                detach: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(result.created);
    assert_eq!(result.name, "app");
    assert_eq!(result.path, repo.canonicalize().unwrap());
    assert!(
        fake.calls()
            .iter()
            .any(|call| call.iter().any(|arg| arg.contains("printf root-start")))
    );
}

#[test]
fn picker_rename_preserves_live_identity_and_rejects_invalid_targets_and_names() {
    let fake = FakeRex::new();
    let manager = fake.manager(Config::default());
    let mut entry = Entry::new("duplicate name", "/project", Source::Rex);
    entry.session_id = Some("session:second-full".into());
    for source in [Source::Config, Source::Directory, Source::Worktree] {
        let mut invalid = entry.clone();
        invalid.source = source;
        assert!(manager.rename(&invalid, "new name").is_err());
    }
    for id in [
        None,
        Some("duplicate name"),
        Some("block:full"),
        Some("session:"),
    ] {
        let mut invalid = entry.clone();
        invalid.session_id = id.map(str::to_owned);
        assert!(manager.rename(&invalid, "new name").is_err());
    }
    for name in ["", " ", "\t", "bad\nname", "bad\0name"] {
        assert!(manager.rename(&entry, name).is_err());
    }
    manager.rename(&entry, &entry.name).unwrap();
    assert!(fake.calls().is_empty());

    let name = " 项目 '; $(touch RENAME_INJECTED) {q} ";
    manager.rename(&entry, name).unwrap();
    let calls = fake.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].ends_with(&[
        "session".into(),
        "rename".into(),
        "session:second-full".into(),
        name.into()
    ]));
    assert!(!fake.dir.path().join("RENAME_INJECTED").exists());
}

#[test]
fn picker_entry_connect_preserves_literal_names_and_paths() {
    for name in [
        "📁 chosen",
        " chosen",
        " chosen ",
        "session:chosen",
        "block:chosen",
    ] {
        let fake = FakeRex::new();
        let wanted = fake.dir.path().join("wanted");
        let wrong = fake.dir.path().join("wrong");
        fs::create_dir(&wanted).unwrap();
        fs::create_dir(&wrong).unwrap();
        let config = Config {
            session: vec![
                SessionConfig {
                    name: name.into(),
                    path: wanted.display().to_string(),
                    ..Default::default()
                },
                SessionConfig {
                    name: "chosen".into(),
                    path: wrong.display().to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let selected = config
            .entries()
            .unwrap()
            .into_iter()
            .find(|e| e.name == name)
            .unwrap();
        let connection = fake
            .manager(config)
            .connect_entry(
                selected,
                &ConnectOptions {
                    detach: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(connection.name, name);
        assert_eq!(connection.path, wanted.canonicalize().unwrap());
        let calls = fake.calls();
        let creation = calls.iter().find(|a| a.iter().any(|v| v == "new")).unwrap();
        let at = creation.iter().position(|v| v == "new").unwrap();
        assert_eq!(creation[at + 1], name);
        let cwd = creation.iter().position(|v| v == "-c").unwrap();
        assert_eq!(
            Path::new(&creation[cwd + 1]),
            wanted.canonicalize().unwrap()
        );
    }
}

#[test]
fn resolved_worktree_path_does_not_expand_a_colliding_alias() {
    let fake = FakeRex::new();
    let wanted = fake.dir.path().join("wanted");
    let wrong = fake.dir.path().join("wrong");
    fs::create_dir(&wanted).unwrap();
    fs::create_dir(&wrong).unwrap();
    let config = Config {
        session: vec![
            SessionConfig {
                name: "wanted".into(),
                path: wanted.display().to_string(),
                ..Default::default()
            },
            SessionConfig {
                name: "wrong".into(),
                path: wrong.display().to_string(),
                alias: Some(wanted.display().to_string()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let connection = fake
        .manager(config)
        .connect_path(
            &wanted,
            &ConnectOptions {
                detach: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(connection.name, "wanted");
    assert_eq!(connection.path, wanted.canonicalize().unwrap());
    let calls = fake.calls();
    let creation = calls.iter().find(|a| a.iter().any(|v| v == "new")).unwrap();
    let at = creation.iter().position(|v| v == "new").unwrap();
    assert_eq!(creation[at + 1], "wanted");
}

#[test]
fn resolved_live_entry_does_not_create_when_id_disappeared() {
    let fake = FakeRex::new();
    let mut selected = rexue::model::Entry::new("project", fake.dir.path(), Source::Rex);
    selected.session_id = Some("session:gone".into());
    let error = fake
        .manager(Config::default())
        .connect_entry(
            selected,
            &ConnectOptions {
                detach: true,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("session:gone no longer exists"));
    assert!(fake.calls().iter().all(|a| !a.iter().any(|v| v == "new")));
}

#[test]
fn connect_root_preserves_plain_directories_with_and_without_git() {
    for no_git in [false, true] {
        let fake = FakeRex::new();
        let project = fake.dir.path().join("plain $REXUE_ROOT_LITERAL_UNSET");
        fs::create_dir(&project).unwrap();
        fake.session("session:created-full", "project", &project);
        let cfg = fake.dir.path().join("root-connect.toml");
        fs::write(&cfg, "").unwrap();
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
fn worktree_switch_does_not_fail_when_repo_name_is_owned_by_another_checkout() {
    let fake = FakeRex::new();
    let repo = fake.dir.path().join("zed-bar.nvim");
    let previous = repo.join(".worktrees/wk-test");
    fs::create_dir_all(&previous).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet", "-b", "main"])
            .arg(&repo)
            .status()
            .unwrap()
            .success()
    );
    let live = fake.session("session:previous-full", "zed-bar_nvim", &previous);
    fake.json("sessions.json", json!({"sessions":[live.clone()]}));
    let wt = fake.dir.path().join("fake wt");
    executable(
        &wt,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'wt v0.81.0\\n'; else printf '%s\\n' {}; fi\n",
            shell_quote(&json!({"action":"existing","path":repo}).to_string())
        ),
    );
    let config = fake.dir.path().join("worktree.toml");
    fs::write(&config, "git_namer_use_worktree_root=true\n").unwrap();
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_rexue"))
            .args(["--detach", "--rex-bin"])
            .arg(&fake.rex.bin)
            .arg("-C")
            .arg(&config)
            .args(["worktree", "connect", "main", "--json", "--path"])
            .arg(&repo)
            .args(extra)
            .env("REXUE_WT", &wt)
            .env("REXUE_STATE_DIR", fake.dir.path().join("state"))
            .output()
            .unwrap()
    };
    let output = run(&[]);
    assert!(
        output.status.success(),
        "worktree switching must create a distinct session instead of failing: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let connection: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        connection["path"],
        repo.canonicalize().unwrap().to_str().unwrap()
    );
    assert!(connection["created"].as_bool().unwrap());
    assert_eq!(connection["name"], "zed-bar_nvim@main");
    assert!(
        !fake
            .calls()
            .iter()
            .any(|args| args.iter().any(|a| a == "kill")
                || args.windows(2).any(|pair| pair == ["session", "rename"]))
    );

    // Reopening the same directory reuses its ID even under the fallback label.
    let created = fake.session("session:created-full", "zed-bar_nvim@main", &repo);
    fake.json("sessions.json", json!({"sessions":[created]}));
    let reused = run(&[]);
    assert!(
        reused.status.success(),
        "{}",
        String::from_utf8_lossy(&reused.stderr)
    );
    let reused: Value = serde_json::from_slice(&reused.stdout).unwrap();
    assert_eq!(reused["session_id"], connection["session_id"]);
    assert_eq!(reused["created"], false);
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|args| args.iter().any(|a| a == "new"))
            .count(),
        1
    );

    // Explicit names and configured session labels still cannot steal a directory.
    fake.session("session:previous-full", "zed-bar_nvim", &previous);
    fake.json("sessions.json", json!({"sessions":[live]}));
    let explicit = run(&["--name", "zed-bar_nvim"]);
    assert!(!explicit.status.success());
    assert!(String::from_utf8_lossy(&explicit.stderr).contains("already belongs"));
    fs::write(
        &config,
        format!(
            "[[session]]\nname='zed-bar_nvim'\npath={}\n",
            serde_json::to_string(repo.to_str().unwrap()).unwrap()
        ),
    )
    .unwrap();
    let configured = run(&[]);
    assert!(!configured.status.success());
    assert!(String::from_utf8_lossy(&configured.stderr).contains("already belongs"));
}

#[test]
fn missing_directory_does_not_gain_a_worktree_icon_by_prefix() {
    let fake = FakeRex::new();
    let d = tempfile::tempdir().unwrap();
    let actual = d.path().join("actual");
    let alias = d.path().join("alias");
    fs::create_dir(&actual).unwrap();
    std::os::unix::fs::symlink(&actual, &alias).unwrap();
    let cfg = d.path().join("icon.toml");
    let quote = |p: &Path| serde_json::to_string(p.to_str().unwrap()).unwrap();
    fs::write(
        &cfg,
        format!(
            "[[worktree]]\npath={}\nicon='W'\n[[session]]\nname='not-created-yet'\npath={}\n",
            quote(&actual),
            quote(&alias.join("feature"))
        ),
    )
    .unwrap();
    let live = fake.session(
        "session:icon-full",
        "not-created-yet",
        &alias.join("feature"),
    );
    fake.json("sessions.json", json!({"sessions": [live]}));
    let r = Command::new(env!("CARGO_BIN_EXE_rexue"))
        .arg("--rex-bin")
        .arg(&fake.rex.bin)
        .arg("-C")
        .arg(cfg)
        .args(["list", "--json"])
        .env("REXUE_STATE_DIR", d.path().join("state"))
        .output()
        .unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let rows: Value = serde_json::from_slice(&r.stdout).unwrap();
    assert!(rows[0]["icon"].is_null());
}

#[test]
fn worktree_repository_path_expansion_is_equivalent_for_live_session_icons() {
    let fake = FakeRex::new();
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repository with spaces");
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(&repo)
            .status()
            .unwrap()
            .success()
    );
    let project = repo.join("feature/sub");
    fs::create_dir_all(&project).unwrap();
    let cfg = dir.path().join("icons.toml");
    for spelling in [
        repo.display().to_string(),
        "${REXUE_ICON_REPO_FIXTURE}".into(),
    ] {
        fs::write(
            &cfg,
            format!(
                "[[worktree]]\npath={}\nicon='W'\n[[session]]\nname='tree-project'\npath={}\n",
                serde_json::to_string(&spelling).unwrap(),
                serde_json::to_string(project.to_str().unwrap()).unwrap()
            ),
        )
        .unwrap();
        let live = fake.session("session:icon-full", "tree-project", &project);
        fake.json("sessions.json", json!({"sessions": [live]}));
        let r = Command::new(env!("CARGO_BIN_EXE_rexue"))
            .arg("--rex-bin")
            .arg(&fake.rex.bin)
            .arg("-C")
            .arg(&cfg)
            .args(["list", "--json"])
            .env("REXUE_ICON_REPO_FIXTURE", &repo)
            .env("REXUE_STATE_DIR", dir.path().join("state"))
            .output()
            .unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        let rows: Value = serde_json::from_slice(&r.stdout).unwrap();
        assert_eq!(rows[0]["icon"], "W", "{spelling}");
    }
}

#[test]
fn environment_values_with_literal_dollars_survive_list_preview_and_repository_icons() {
    const PROJECT: &str = "REXUE_LITERAL_PROJECT_734129";
    const REPO: &str = "REXUE_LITERAL_REPO_734129";
    const UNSET: &str = "REXUE_UNSET_LITERAL_734129";
    let fake = FakeRex::new();
    let repo = fake.dir.path().join(format!("repo-${UNSET}"));
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(&repo)
            .status()
            .unwrap()
            .success()
    );
    let root = repo.join("custom roots");
    let project = root.join("feature");
    fs::create_dir_all(&project).unwrap();
    let cfg = fake.dir.path().join("literal.toml");
    let run = |args: &[&str], path: &Path| {
        Command::new(env!("CARGO_BIN_EXE_rexue"))
            .arg("--rex-bin")
            .arg(&fake.rex.bin)
            .arg("-C")
            .arg(&cfg)
            .args(args)
            .env(PROJECT, path)
            .env(REPO, &repo)
            .env_remove(UNSET)
            .env("REXUE_STATE_DIR", fake.dir.path().join("literal-state"))
            .output()
            .unwrap()
    };
    fs::write(&cfg,format!("[[worktree]]\npath='${{{REPO}}}'\nicon='W'\n[[session]]\nname='literal-project'\npath='${{{PROJECT}}}'\npreview_command=\"printf '%s' {{}}\"\n")).unwrap();
    for (path, icon) in [(&project, Some("W")), (&root.join("not-created"), None)] {
        let live = fake.session("session:literal-full", "literal-project", path);
        fake.json("sessions.json", json!({"sessions": [live]}));
        let r = run(&["list", "--json"], path);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        let rows: Value = serde_json::from_slice(&r.stdout).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["path"], path.to_str().unwrap());
        assert_eq!(rows[0]["icon"].as_str(), icon);
    }
    fake.json("sessions.json", json!({"sessions": []}));
    let r = run(&["preview", "literal-project"], &project);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    assert_eq!(
        String::from_utf8(r.stdout).unwrap(),
        project.to_str().unwrap()
    );
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
    let r = run(&["list", "--json"], &project);
    assert!(!r.status.success());
    assert!(r.stdout.is_empty());
    assert!(String::from_utf8_lossy(&r.stderr).contains("bad-project"));
}

#[test]
fn nested_shell_placeholders_work_in_cli_preview_and_rex_startup_scripts() {
    let fake = FakeRex::new();
    let name = "project '\";$(touch INJECTED) quoted";
    let project = fake.dir.path().join(name);
    fs::create_dir(&project).unwrap();
    fake.session("session:created-full", "project", &project);
    let startup_file = fake.dir.path().join("nested-startup");
    let preview =
        r#"rexue_path() { printf '%s' "$(basename '{}')"; }; set -- changed; rexue_path other"#;
    let startup = format!(
        "{preview} > {}",
        shell_quote(startup_file.to_str().unwrap())
    );
    let cfg = fake.dir.path().join("nested-shell.toml");
    fs::write(
        &cfg,
        format!(
            "[[session]]\nname='project'\npath={}\npreview_command={}\nstartup_command={}\n",
            serde_json::to_string(project.to_str().unwrap()).unwrap(),
            serde_json::to_string(preview).unwrap(),
            serde_json::to_string(&startup).unwrap()
        ),
    )
    .unwrap();
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
    assert_eq!(scripts.len(), 1);
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
        fake.session(
            "session:created-full",
            "project",
            &fake.dir.path().canonicalize().unwrap(),
        );
        let cfg = fake.dir.path().join("strategies.toml");
        let path = serde_json::to_string(fake.dir.path().canonicalize().unwrap().to_str().unwrap())
            .unwrap();
        fs::write(&cfg, format!(
            "[default_session]\nstartup_command='printf default'\nstartup_script=''\npreview_command='printf default-preview'\n[[wildcard]]\npattern={path}\npreview_command='printf wildcard-preview'\nicon='W'\n{wildcard}\n[[wildcard]]\npattern={path}\nstartup_command='printf wrong-second'\npreview_command='printf wrong-second'\n[[session]]\nname='project'\npath={path}\npreview_command=''\nicon=''\n{session}\n"
        )).unwrap();
        let preview = session_cli(&fake, &cfg, &["preview", "project"]);
        assert!(
            preview.status.success(),
            "{}",
            String::from_utf8_lossy(&preview.stderr)
        );
        assert_eq!(preview.stdout, b"wildcard-preview");
        let output = session_cli(&fake, &cfg, &["connect", "project", "--json"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = session_cli(&fake, &cfg, &["list", "--json"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rows: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(rows[0]["icon"], "W");
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

fn session_cli(fake: &FakeRex, cfg: &Path, args: &[&str]) -> std::process::Output {
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
}

#[test]
fn connect_creates_selects_then_reuses_without_repeating_startup() {
    let fake = FakeRex::new();
    let project = fake.dir.path().join("project ' ; $(touch INJECTED)");
    fs::create_dir(&project).unwrap();
    fake.session("session:created-full", "project", &project);
    let config = Config {
        session: vec![SessionConfig {
            name: "project".into(),
            path: project.to_str().unwrap().into(),
            startup_command: Some("printf first {}".into()),
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
        serde_json::from_slice(&fs::read(manager.state_dir.join("state.json")).unwrap()).unwrap();
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
}

#[test]
fn duplicate_session_labels_require_full_id_for_selection_and_kill() {
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
}

#[test]
fn default_list_contains_only_live_sessions_and_retains_configuration_metadata() {
    let fake = FakeRex::new();
    let live = fake.session("session:live-full", "renamed project", fake.dir.path());
    fake.json("sessions.json", json!({"sessions": [live]}));
    let manager = fake.manager(Config {
        session: vec![
            SessionConfig {
                name: "configured project".into(),
                path: fake.dir.path().display().to_string(),
                icon: Some("C".into()),
                alias: Some("p".into()),
                ..Default::default()
            },
            SessionConfig {
                name: "not running".into(),
                path: fake.dir.path().join("other").display().to_string(),
                ..Default::default()
            },
        ],
        worktree: vec![rexue::config::WorktreeConfig {
            path: fake
                .dir
                .path()
                .join("unavailable repository")
                .display()
                .to_string(),
            icon: None,
        }],
        ..Default::default()
    });
    for opts in [
        ListOptions::default(),
        ListOptions {
            rex: true,
            ..Default::default()
        },
    ] {
        let entries = manager.list(&opts).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_session());
        assert_eq!(entries[0].session_id.as_deref(), Some("session:live-full"));
        assert_eq!(entries[0].name, "renamed project");
        assert_eq!(entries[0].icon.as_deref(), Some("C"));
        assert_eq!(entries[0].alias.as_deref(), Some("p"));
    }
}

#[test]
fn blacklist_inversion_applies_to_live_sessions() {
    let fake = FakeRex::new();
    let live = fake.session("session:live-full", "live", fake.dir.path());
    let scratch = fake.session("session:scratch-full", "scratch-private", fake.dir.path());
    fake.json("sessions.json", json!({"sessions": [live, scratch]}));
    let manager = fake.manager(Config {
        blacklist: vec!["[invalid regex".into(), "^scratch-".into()],
        ..Default::default()
    });
    let normal = manager.list(&ListOptions::default()).unwrap();
    assert_eq!(
        normal.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["live"]
    );
    let blacklisted = manager
        .list(&ListOptions {
            blacklisted: true,
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
    let live = fake.session("session:cli-full", "CLI configured", &path);
    fake.json("sessions.json", json!({"sessions": [live]}));
    let output = Command::new(env!("CARGO_BIN_EXE_rexue"))
        .args(["list", "--json"])
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
    assert_eq!(entries[0]["source"], "rex");
    assert_eq!(entries[0]["session_id"], "session:cli-full");
    assert_eq!(entries[0]["name"], "CLI configured");
    assert_eq!(entries[0]["path"], json!(path));
    assert!(!fake.calls().is_empty(), "session listing must contact Rex");
}
