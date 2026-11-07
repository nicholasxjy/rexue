//! Documented Rex CLI adapter. Labels are for display; mutations use full IDs.
use crate::{
    model::{Agent, Entry, Source, Tab},
    process,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, process::Command, time::Duration};

#[derive(Debug, Clone)]
pub struct Rex {
    pub bin: PathBuf,
    pub server: Option<String>,
    pub client: Option<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppClient {
    pub id: String,
    pub label: String,
    pub platform: String,
}
#[derive(Deserialize)]
struct Snapshot {
    sessions: Vec<Value>,
    snapshots: BTreeMap<String, SessionSnapshot>,
}
#[derive(Deserialize)]
struct SessionSnapshot {
    #[serde(default)]
    missing: bool,
    #[serde(default)]
    view: Value,
    #[serde(default)]
    blocks: BTreeMap<String, BTreeMap<String, Value>>,
}
#[derive(Debug, Clone)]
pub struct Created {
    pub session_id: String,
    pub block_id: String,
}
impl Default for Rex {
    fn default() -> Self {
        Self {
            bin: std::env::var_os("REXUE_REX")
                .map(PathBuf::from)
                .unwrap_or_else(|| "rex".into()),
            server: None,
            client: std::env::var("REX_CLIENT").ok().filter(|x| !x.is_empty()),
        }
    }
}
impl Rex {
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(&self.bin);
        cmd.args(["--autostart=false", "--timeout", "10s"]);
        if let Some(s) = &self.server {
            cmd.args(["-S", s]);
        }
        cmd
    }
    pub fn text(&self, args: &[&str]) -> Result<String> {
        process::text(self.command().args(args))
    }
    pub fn json(&self, args: &[&str]) -> Result<Value> {
        let output = self.text(args)?;
        serde_json::from_str(&output)
            .with_context(|| format!("invalid JSON from Rex {}", args.join(" ")))
    }
    pub fn context(&self) -> Result<Value> {
        self.json(&["whoami", "--json"])
    }
    pub fn inspect(&self, id: &str) -> Result<Value> {
        self.json(&["session", "inspect", id, "--json"])
    }
    pub fn clients(&self) -> Result<Vec<AppClient>> {
        let value = self.json(&["client", "ls", "--json"])?;
        let rows = value["clients"]
            .as_array()
            .context("Rex response missing clients")?;
        Ok(rows
            .iter()
            .filter(|c| c["info"]["kind"] == "app" && c["connection_state"] == "connected")
            .filter_map(|c| {
                Some(AppClient {
                    id: c["client_id"].as_str()?.into(),
                    label: c["info"]["label"].as_str().unwrap_or("").into(),
                    platform: c["info"]["platform"].as_str().unwrap_or("").into(),
                })
            })
            .collect())
    }
    pub fn selected_client(&self) -> Result<AppClient> {
        select_client(&self.clients()?, self.client.as_deref())
    }
    pub fn check_switch(&self, client: &AppClient) -> Result<()> {
        let value = self.json(&["-C", &client.id, "actions", "--json"])?;
        if !value["actions"]
            .as_array()
            .is_some_and(|a| a.iter().any(|a| a["name"] == "session.select"))
        {
            bail!(
                "Rex client {} does not support session.select",
                client.label
            );
        }
        Ok(())
    }
    pub fn switch(&self, client: &AppClient, id: &str) -> Result<()> {
        self.switch_window(client, id, None)
    }
    pub fn switch_window(&self, client: &AppClient, id: &str, window: Option<&str>) -> Result<()> {
        let mut args = serde_json::json!({"session_id": id});
        if let Some(window) = window {
            args["window_id"] = window.into();
        }
        self.json(&[
            "-C",
            &client.id,
            "do",
            "session.select",
            "--args",
            &args.to_string(),
            "--json",
        ])?;
        Ok(())
    }
    pub fn list(&self) -> Result<Vec<Entry>> {
        self.list_sources(true, false)
    }
    pub fn list_sources(&self, sessions: bool, agents: bool) -> Result<Vec<Entry>> {
        self.list_sources_excluding_picker(sessions, agents, None)
    }
    pub fn list_sources_excluding_picker(
        &self,
        sessions: bool,
        agents: bool,
        picker_block: Option<&str>,
    ) -> Result<Vec<Entry>> {
        let (rows, snapshots) = match self.snapshot(picker_block)? {
            Some(snapshot) => (snapshot.sessions, Some(snapshot.snapshots)),
            None => {
                let response = self.json(&["ls", "--json"])?;
                (
                    response["sessions"]
                        .as_array()
                        .context("Rex response missing sessions")?
                        .clone(),
                    None,
                )
            }
        };
        parallel_entries(&rows, snapshots.is_none(), |row| {
            let snapshot = snapshots
                .as_ref()
                .map(|snapshots| &snapshots[row["session_id"].as_str().unwrap()]);
            self.inspect_entries(row, sessions, agents, None, picker_block, snapshot)
        })
    }
    fn snapshot(&self, picker_block: Option<&str>) -> Result<Option<Snapshot>> {
        let args = serde_json::json!({"picker_block": picker_block}).to_string();
        let mut response = match self.json(&[
            "do",
            "--eval",
            include_str!("rex_snapshot.lua"),
            "--args",
            &args,
            "--json",
        ]) {
            Ok(response) => response,
            Err(error) => {
                let message = format!("{error:#}");
                // Older Rex CLIs reject Lua before connecting. Preserve their
                // existing discovery path, but keep service/JSON errors visible.
                if message.contains("unknown flag: --eval")
                    || message.contains("unknown command \"do\"")
                {
                    return Ok(None);
                }
                return Err(error);
            }
        };
        // Lua encodes an empty table as an object, including an empty session list.
        if response["sessions"]
            .as_object()
            .is_some_and(|sessions| sessions.is_empty())
        {
            response["sessions"] = Value::Array(Vec::new());
        }
        let snapshot: Snapshot =
            serde_json::from_value(response).context("invalid Rex snapshot")?;
        for row in &snapshot.sessions {
            let id = row["session_id"]
                .as_str()
                .context("Rex session missing session_id")?;
            let session = snapshot
                .snapshots
                .get(id)
                .context("Rex snapshot missing session metadata")?;
            if !session.missing && !session.view.is_object() {
                bail!("Rex snapshot missing session view");
            }
        }
        Ok(Some(snapshot))
    }
    /// Refresh a selected session and, optionally, only its selected agent pane.
    pub fn list_session(&self, id: &str, block: Option<&str>) -> Result<Vec<Entry>> {
        let response = self.json(&["ls", "--json"])?;
        let rows = response["sessions"]
            .as_array()
            .context("Rex response missing sessions")?;
        match rows.iter().find(|row| row["session_id"] == id) {
            Some(row) => self.inspect_entries(row, true, block.is_some(), block, None, None),
            None => Ok(Vec::new()),
        }
    }
    fn inspect_entries(
        &self,
        row: &Value,
        sessions: bool,
        agents: bool,
        target_block: Option<&str>,
        picker_block: Option<&str>,
        snapshot: Option<&SessionSnapshot>,
    ) -> Result<Vec<Entry>> {
        let id = row["session_id"]
            .as_str()
            .context("Rex session missing session_id")?;
        let name = row["label"].as_str().unwrap_or(id);
        // A session can disappear between list and inspect. Skip only that race;
        // malformed or unreachable service responses must remain visible errors.
        let mut inspect = match snapshot {
            Some(snapshot) if snapshot.missing => return Ok(Vec::new()),
            Some(snapshot) => snapshot.view.clone(),
            None => match self.inspect(id) {
                Ok(v) => v,
                Err(e) if e.to_string().contains("exit 3") => return Ok(Vec::new()),
                Err(e) => return Err(e),
            },
        };
        if let Some(picker) = picker_block {
            for window in inspect["windows"].as_array_mut().into_iter().flatten() {
                let mut removed = false;
                for layer in window["layers"].as_array_mut().into_iter().flatten() {
                    if layer["kind"] == "floating"
                        && let Some(blocks) = layer["blocks"].as_array_mut()
                    {
                        blocks.retain(|block| {
                            if block["block_id"] == picker {
                                removed = true;
                                false
                            } else {
                                true
                            }
                        });
                    }
                }
                if removed && window["focused_block_id"] == picker {
                    // The transient overlay must not supply the tab title or cwd.
                    window["focused_block_id"] = window["layers"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .flat_map(|layer| layer["blocks"].as_array().into_iter().flatten())
                        .find_map(|block| block.get("block_id"))
                        .cloned()
                        .unwrap_or(Value::Null);
                }
            }
        }
        let focused = active_window(&inspect).and_then(|w| w["focused_block_id"].as_str());
        let focused_process =
            focused.and_then(|block| self.block_metadata(id, block, "process", snapshot).ok());
        let mut titles = BTreeMap::new();
        let windows: Vec<_> = inspect["windows"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|window| (window, self.tab_title(id, window, &mut titles, snapshot)))
            .collect();
        let mut blocks = Vec::new();
        for (window, title) in &windows {
            let Some(window_id) = window["window_id"].as_str() else {
                continue;
            };
            let window_name = title.as_deref().unwrap_or(window_id);
            let mut pane = 0;
            for layer in window["layers"].as_array().into_iter().flatten() {
                for block in layer["blocks"].as_array().into_iter().flatten() {
                    pane += 1;
                    if let Some(block_id) = block["block_id"].as_str()
                        && target_block.is_none_or(|target| target == block_id)
                    {
                        blocks.push((window_id, window_name, block, pane));
                    }
                }
            }
        }
        // Session rows also need live agent identity to decorate their tabs.
        let inspected = parallel_entries(
            &blocks,
            snapshot.is_none(),
            |&(window_id, window_name, block, pane)| {
                let block_id = block["block_id"].as_str().unwrap();
                let terminal = block["creator_name"] == "com.superlogical.terminal";
                let process = if !terminal {
                    None
                } else if focused == Some(block_id) {
                    focused_process.clone()
                } else {
                    self.block_metadata(id, block_id, "process", snapshot).ok()
                };
                // Status is optional; process identity remains authoritative.
                let running = process.as_ref().and_then(|process| {
                    let status = self
                        .block_metadata(id, block_id, "program_status", snapshot)
                        .unwrap_or(Value::Null);
                    crate::agent::running(process, &status)
                });
                let Some(running) = running else {
                    let label = process
                        .as_ref()
                        .and_then(|p| p["foreground"]["name"].as_str())
                        .or_else(|| block["label"].as_str().filter(|label| !label.is_empty()))
                        .unwrap_or(if terminal { "terminal" } else { "pane" });
                    let mut entry = Entry::new(
                        format!("{name} / {label} · pane {pane} · {window_name}"),
                        process.as_ref().and_then(process_path).unwrap_or_default(),
                        Source::Rex,
                    );
                    entry.session_id = Some(id.into());
                    entry.window_id = Some(window_id.into());
                    entry.block_id = Some(block_id.into());
                    return Ok(vec![(entry, None)]);
                };

                let mut agent = Entry::new(
                    format!("{name} / {} · pane {pane} · {window_name}", running.app),
                    running.path,
                    Source::Agent,
                );
                agent.session_id = Some(id.into());
                agent.agent = Some(Agent {
                    app: running.app.into(),
                    state: running.state,
                    session_name: name.into(),
                    window_id: window_id.into(),
                    block_id: block_id.into(),
                });
                // Reuse the focused title fetched for an unnamed tab, including an
                // empty/failed result. Fetch other agent titles in the bounded pane
                // workers instead of serially after all process queries complete.
                let title = if sessions {
                    titles
                        .get(block_id)
                        .cloned()
                        .unwrap_or_else(|| self.block_title(id, block_id, snapshot))
                } else {
                    None
                };
                Ok(vec![(agent, title)])
            },
        )?;
        let agent_titles: BTreeMap<_, _> = inspected
            .iter()
            .filter_map(|(entry, title)| {
                entry
                    .agent
                    .as_ref()
                    .map(|agent| (agent.block_id.clone(), title.clone()))
            })
            .collect();
        let mut entries: Vec<_> = inspected.into_iter().map(|(entry, _)| entry).collect();
        let mut session = None;
        if sessions {
            let mut entry = Entry::new(name, PathBuf::new(), Source::Rex);
            entry.session_id = Some(id.into());
            for (window, title) in &windows {
                let tab_agents: Vec<_> = entries
                    .iter()
                    .filter_map(|entry| entry.agent.as_ref())
                    .filter(|agent| window["window_id"] == agent.window_id)
                    .collect();
                let mut titles: Vec<_> = title.iter().cloned().collect();
                for agent in &tab_agents {
                    if let Some(Some(title)) = agent_titles.get(&agent.block_id)
                        && !titles.contains(title)
                    {
                        titles.push(title.clone());
                    }
                }
                if titles.is_empty() {
                    for agent in &tab_agents {
                        if !titles.contains(&agent.app) {
                            titles.push(agent.app.clone());
                        }
                    }
                }
                if let Some(window_id) = window["window_id"].as_str() {
                    entry.tabs.push(Tab {
                        name: if titles.is_empty() {
                            window_id.into()
                        } else {
                            titles.join(" · ")
                        },
                        window_id: window_id.into(),
                        is_agent: !tab_agents.is_empty(),
                        panes: entries
                            .iter()
                            .filter(|pane| pane.window_id() == Some(window_id))
                            .cloned()
                            .collect(),
                    });
                }
                if !titles.is_empty() {
                    if !tab_agents.is_empty() {
                        entry.agent_tab_indices.push(entry.tab_names.len());
                    }
                    entry.tab_names.push(titles.join(" · "));
                }
            }
            if let Some(process) = &focused_process {
                entry.path = process_path(process).unwrap_or_default();
            }
            if entry.path.as_os_str().is_empty()
                && let Some(block) = focused
            {
                entry.path = self.reported_path(id, block).unwrap_or_default();
            }
            session = Some(entry);
        }
        entries.retain(|entry| agents && entry.source == Source::Agent);
        if let Some(session) = session {
            entries.insert(0, session);
        }
        Ok(entries)
    }
    fn tab_title(
        &self,
        session: &str,
        window: &Value,
        titles: &mut BTreeMap<String, Option<String>>,
        snapshot: Option<&SessionSnapshot>,
    ) -> Option<String> {
        let label = window["label"].as_str().unwrap_or("");
        // Rex assigns unnamed tabs "Window <last eight ID characters>".
        // Their visible title comes from the focused terminal instead.
        let generated = label.strip_prefix("Window ").is_some_and(|suffix| {
            suffix.len() == 8
                && window["window_id"]
                    .as_str()
                    .is_some_and(|id| id.ends_with(suffix))
        });
        if !label.is_empty() && !generated {
            return Some(label.into());
        }
        let focused = window["focused_block_id"].as_str()?;
        let block = window["layers"]
            .as_array()?
            .iter()
            .flat_map(|layer| layer["blocks"].as_array().into_iter().flatten())
            .find(|block| block["block_id"] == focused)?;
        if block["creator_name"] == "com.superlogical.terminal" {
            let title = self.block_title(session, focused, snapshot);
            titles.insert(focused.into(), title.clone());
            if title.is_some() {
                return title;
            }
        }
        block["label"]
            .as_str()
            .filter(|label| !label.is_empty())
            .map(str::to_owned)
    }
    fn block_metadata(
        &self,
        session: &str,
        block: &str,
        method: &str,
        snapshot: Option<&SessionSnapshot>,
    ) -> Result<Value> {
        if let Some(snapshot) = snapshot {
            return snapshot
                .blocks
                .get(block)
                .and_then(|metadata| metadata.get(method))
                .cloned()
                .context("terminal metadata unavailable in Rex snapshot");
        }
        self.json(&["block", "call", "-s", session, "-b", block, method])
    }
    fn block_title(
        &self,
        session: &str,
        block: &str,
        snapshot: Option<&SessionSnapshot>,
    ) -> Option<String> {
        self.block_metadata(session, block, "title", snapshot)
            .ok()?["title"]
            .as_str()
            .filter(|title| !title.is_empty())
            .map(str::to_owned)
    }
    pub fn block_process(&self, session: &str, block: &str) -> Result<Value> {
        self.json(&["block", "call", "-s", session, "-b", block, "process"])
    }
    pub fn block_path(&self, session: &str, block: &str) -> Result<PathBuf> {
        let value = self.block_process(session, block)?;
        if let Some(path) = process_path(&value) {
            return Ok(path);
        }
        self.reported_path(session, block)
    }
    fn reported_path(&self, session: &str, block: &str) -> Result<PathBuf> {
        let value = self.json(&["block", "call", "-s", session, "-b", block, "pwd"])?;
        Ok(value["path"]
            .as_str()
            .or_else(|| value["cwd"].as_str())
            .unwrap_or("")
            .into())
    }
    pub fn create(
        &self,
        label: &str,
        path: &std::path::Path,
        startup: Option<&str>,
    ) -> Result<Created> {
        let mut cmd = self.command();
        cmd.args(["new", label, "-c"])
            .arg(path)
            .args(["--window", "Terminal", "--json"]);
        if let Some(script) = startup.filter(|s| !s.is_empty()) {
            // The final login shell keeps the project terminal usable after setup.
            let script = format!(
                "{}\nrexue_status=$?\nif [ \"$rexue_status\" -ne 0 ]; then printf 'Startup exited %s\\n' \"$rexue_status\"; fi\nexec \"${{SHELL:-/bin/sh}}\" -l",
                process::path_script(script)
            );
            cmd.args(["--", "sh", "-c", &script, "rexue"]).arg(path);
        }
        let response: Value =
            serde_json::from_slice(&process::output(&mut cmd, Duration::from_secs(30))?.stdout)
                .context("invalid Rex create JSON")?;
        let id = response["session_id"]
            .as_str()
            .context("Rex new response missing session_id")?;
        let block = response["initial_windows"][0]["block_ids"][0]
            .as_str()
            .context("Rex new response missing block ID")?;
        Ok(Created {
            session_id: id.into(),
            block_id: block.into(),
        })
    }
    pub fn rename(&self, id: &str, name: &str) -> Result<()> {
        self.text(&["session", "rename", id, name])?;
        Ok(())
    }
    pub fn kill(&self, id: &str) -> Result<()> {
        if !id.starts_with("session:") {
            bail!("refusing to close a session without a full session ID");
        }
        self.text(&["kill", id])?;
        Ok(())
    }
    pub fn capture(&self, id: &str) -> Result<String> {
        let detail = self.inspect(id)?;
        let block = active_window(&detail)
            .and_then(|w| w["focused_block_id"].as_str())
            .context("session has no focused terminal")?;
        self.capture_block(id, block)
    }
    pub fn capture_block(&self, session: &str, block: &str) -> Result<String> {
        self.text(&["capture", "-s", session, "-b", block, "--trim", "--unwrap"])
    }
    pub fn focus(&self, session: &str, block: &str) -> Result<()> {
        self.text(&["focus", "-s", session, block])?;
        Ok(())
    }
}
/// Preserve catalog order while bounding discovery to four sessions and four panes
/// per session at a time. Small/targeted queries need no worker thread.
fn parallel_entries<T: Sync, U: Send>(
    items: &[T],
    parallel: bool,
    query: impl Fn(&T) -> Result<Vec<U>> + Sync,
) -> Result<Vec<U>> {
    if !parallel || items.len() <= 1 {
        return items.iter().try_fold(Vec::new(), |mut entries, item| {
            entries.extend(query(item)?);
            Ok(entries)
        });
    }
    std::thread::scope(|scope| {
        let workers: Vec<_> = items
            .chunks(items.len().div_ceil(4))
            .map(|chunk| {
                let query = &query;
                scope.spawn(move || -> Result<Vec<U>> {
                    chunk.iter().try_fold(Vec::new(), |mut entries, item| {
                        entries.extend(query(item)?);
                        Ok(entries)
                    })
                })
            })
            .collect();
        workers
            .into_iter()
            .try_fold(Vec::new(), |mut entries, worker| {
                entries.extend(worker.join().expect("Rex discovery worker panicked")?);
                Ok(entries)
            })
    })
}

fn process_path(value: &Value) -> Option<PathBuf> {
    value["foreground"]["cwd"]
        .as_str()
        .or_else(|| {
            value["ancestors"]
                .as_array()
                .and_then(|a| a.iter().find_map(|v| v["cwd"].as_str()))
        })
        .map(PathBuf::from)
}
pub fn active_window(value: &Value) -> Option<&Value> {
    let windows = value["windows"].as_array()?;
    windows
        .iter()
        .find(|w| w["window_id"] == value["active_window_id"])
        .or_else(|| windows.first())
}
pub fn select_client(clients: &[AppClient], target: Option<&str>) -> Result<AppClient> {
    if let Some(target) = target {
        let full: Vec<_> = clients.iter().filter(|c| c.id == target).collect();
        let matching = if !full.is_empty() {
            full
        } else {
            let labels: Vec<_> = clients
                .iter()
                .filter(|c| crate::model::alias_key(&c.label) == crate::model::alias_key(target))
                .collect();
            if !labels.is_empty() {
                labels
            } else {
                clients
                    .iter()
                    .filter(|c| target.len() >= 4 && c.id.ends_with(target))
                    .collect()
            }
        };
        return match matching.as_slice() {
            [client] => Ok((*client).clone()),
            [] => bail!("Rex app client {target:?} was not found"),
            _ => bail!("Rex app client {target:?} is ambiguous; use a full client ID"),
        };
    }
    match clients {
        [client] => Ok(client.clone()),
        [] => bail!("no connected Rex app client; open Rex or use --detach"),
        _ => bail!(
            "multiple Rex app clients: {}; choose --client LABEL or set REX_CLIENT",
            clients
                .iter()
                .map(|c| format!("{} ({})", c.label, c.id))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn c(id: &str, label: &str) -> AppClient {
        AppClient {
            id: id.into(),
            label: label.into(),
            platform: "macos".into(),
        }
    }
    #[test]
    fn exact_id_precedes_label() {
        let cs = vec![c("client:aaaa", "desk"), c("client:bbbb", "client:aaaa")];
        assert_eq!(
            select_client(&cs, Some("client:aaaa")).unwrap().label,
            "desk"
        );
    }
    #[test]
    fn ambiguity_requires_choice() {
        let cs = vec![c("client:aaaa", "desk"), c("client:bbbb", "desk")];
        assert!(select_client(&cs, None).is_err());
        assert!(select_client(&cs, Some("desk")).is_err());
        assert_eq!(select_client(&cs, Some("bbbb")).unwrap().id, "client:bbbb");
    }
    #[test]
    fn active_window_is_not_first_window() {
        let v = serde_json::json!({"active_window_id":"two","windows":[{"window_id":"one"},{"window_id":"two"}]});
        assert_eq!(active_window(&v).unwrap()["window_id"], "two");
    }
}
