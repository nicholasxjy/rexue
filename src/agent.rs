//! Agent identity comes from live processes; OSC 7501 adds optional state.
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(crate) struct RunningAgent {
    pub app: &'static str,
    pub state: String,
    pub path: PathBuf,
}

fn app_id(value: &str) -> Option<&'static str> {
    let name = Path::new(value)
        .file_name()?
        .to_str()?
        .trim_end_matches(".exe");
    match name {
        "pi" | "pi-coding-agent" => Some("pi"),
        "claude" | "claude-code" => Some("claude-code"),
        "codex" => Some("codex"),
        "opencode" => Some("opencode"),
        "aider" => Some("aider"),
        "gemini" | "gemini-cli" => Some("gemini"),
        "goose" => Some("goose"),
        "amp" => Some("amp"),
        "cursor-agent" => Some("cursor-agent"),
        "droid" => Some("droid"),
        _ => None,
    }
}

fn process_app(process: &Value) -> Option<&'static str> {
    // Interpreter executable names (node, python) alone do not identify agents.
    for key in ["script", "argv0", "invoked_path", "path", "name"] {
        if let Some(value) = process[key].as_str() {
            if let Some(app) = app_id(value) {
                return Some(app);
            }
            if key == "script" {
                for (package, name) in [
                    ("/pi-coding-agent/", "pi"),
                    ("/@anthropic-ai/claude-code/", "claude-code"),
                    ("/@openai/codex/", "codex"),
                    ("/@google/gemini-cli/", "gemini"),
                    ("/aider/", "aider"),
                ] {
                    if value.contains(package) {
                        return Some(name);
                    }
                }
            }
        }
    }
    None
}

fn owns(record: &Value, process: &Value) -> bool {
    record["owner"]["pid"].as_u64().is_some()
        && record["owner"]["pid"] == process["pid"]
        && record["owner"]["name"].as_str().is_some()
        && record["owner"]["name"] == process["name"]
}

pub(crate) fn running(process: &Value, status: &Value) -> Option<RunningAgent> {
    if !process["last_exit"].is_null() {
        return None;
    }
    let processes: Vec<_> = std::iter::once(&process["foreground"])
        .chain(process["ancestors"].as_array().into_iter().flatten())
        .chain(std::iter::once(&process["child"]))
        .filter(|p| p.is_object())
        .collect();
    let records: Vec<_> = status["records"].as_array().into_iter().flatten().collect();
    let (owner, app) = processes.iter().find_map(|p| {
        let identified = process_app(p).or_else(|| {
            records.iter().find_map(|r| {
                owns(r, p)
                    .then(|| {
                        r["effective_app"]
                            .as_str()
                            .or_else(|| r["app"].as_str())
                            .and_then(app_id)
                    })
                    .flatten()
            })
        });
        identified.map(|app| (*p, app))
    })?;
    let state = records
        .iter()
        .filter(|r| {
            // done/error records may outlive their process. Never borrow state
            // from another process just because it reported the same app name.
            if !r["owner"].is_null() {
                owns(r, owner)
            } else {
                r["effective_app"]
                    .as_str()
                    .or_else(|| r["app"].as_str())
                    .and_then(app_id)
                    == Some(app)
            }
        })
        .filter_map(|r| match r["state"].as_str()? {
            "blocked" => Some((5, "blocked")),
            "working" => Some((4, "working")),
            "error" => Some((3, "error")),
            "done" => Some((2, "done")),
            "idle" => Some((1, "idle")),
            _ => None,
        })
        .max_by_key(|(priority, _)| *priority)
        .map(|(_, state)| state)
        .unwrap_or("running");
    let path = owner["cwd"]
        .as_str()
        .or_else(|| processes.iter().find_map(|p| p["cwd"].as_str()))
        .unwrap_or("");
    Some(RunningAgent {
        app,
        state: state.into(),
        path: path.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn agents_match_binaries_interpreters_and_script_packages() {
        for (identity, expected) in [
            (json!({"argv0": "pi"}), "pi"),
            (json!({"name": "claude"}), "claude-code"),
            (json!({"path": "/opt/bin/codex"}), "codex"),
            (
                json!({"script": "/lib/node_modules/@earendil-works/pi-coding-agent/dist/cli.js"}),
                "pi",
            ),
            (
                json!({"script": "/lib/node_modules/@google/gemini-cli/dist/index.js"}),
                "gemini",
            ),
            (json!({"invoked_path": "/bin/opencode"}), "opencode"),
        ] {
            let value = json!({"last_exit": null, "foreground": identity});
            let agent = running(&value, &Value::Null).unwrap();
            assert_eq!(agent.app, expected);
            assert_eq!(agent.state, "running");
        }
        for identity in [
            json!({"name": "node"}),
            json!({"argv0": "pi-helper"}),
            json!({"name": "zsh"}),
        ] {
            assert!(running(&json!({"foreground": identity}), &Value::Null).is_none());
        }
    }

    #[test]
    fn status_uses_live_owner_and_inherited_app_without_resurrecting_exits() {
        let mut process =
            json!({"last_exit": null, "foreground": {"pid": 4, "name": "node", "cwd": "/project"}});
        let mut status = json!({"records": [
            {"app": "pi", "state": "working", "owner": {"pid": 4, "name": "node"}},
            {"id": "worker", "effective_app": "pi", "state": "blocked", "owner": {"pid": 4, "name": "node"}},
            {"app": "pi", "state": "error", "owner": {"pid": 3, "name": "node"}}
        ]});
        let agent = running(&process, &status).unwrap();
        assert_eq!(agent.state, "blocked");
        assert_eq!(agent.path, Path::new("/project"));
        process["foreground"]["name"] = "zsh".into();
        assert!(running(&process, &status).is_none());
        process["foreground"]["name"] = "node".into();
        process["foreground"]["argv0"] = "pi".into();
        status["records"] =
            json!([{ "app": "pi", "state": "done", "owner": {"pid": 3, "name": "node"}}]);
        assert_eq!(running(&process, &status).unwrap().state, "running");
        process["last_exit"] = json!({"exit_code": 0});
        assert!(running(&process, &status).is_none());
    }
}
