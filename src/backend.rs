//! Documented Rex CLI adapter. Labels are for display; mutations use full IDs.
use crate::{
    model::{Agent, Entry, Source},
    process,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::PathBuf, process::Command, time::Duration};

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
        let response = self.json(&["ls", "--json"])?;
        let rows = response["sessions"]
            .as_array()
            .context("Rex response missing sessions")?;
        parallel_entries(rows, |row| {
            self.inspect_entries(row, sessions, agents, None)
        })
    }
    /// Refresh a selected session and, optionally, only its selected agent pane.
    pub fn list_session(&self, id: &str, block: Option<&str>) -> Result<Vec<Entry>> {
        let response = self.json(&["ls", "--json"])?;
        let rows = response["sessions"]
            .as_array()
            .context("Rex response missing sessions")?;
        match rows.iter().find(|row| row["session_id"] == id) {
            Some(row) => self.inspect_entries(row, true, block.is_some(), block),
            None => Ok(Vec::new()),
        }
    }
    fn inspect_entries(
        &self,
        row: &Value,
        sessions: bool,
        agents: bool,
        target_block: Option<&str>,
    ) -> Result<Vec<Entry>> {
        let id = row["session_id"]
            .as_str()
            .context("Rex session missing session_id")?;
        let name = row["label"].as_str().unwrap_or(id);
        // A session can disappear between list and inspect. Skip only that race;
        // malformed or unreachable service responses must remain visible errors.
        let inspect = match self.inspect(id) {
            Ok(v) => v,
            Err(e) if e.to_string().contains("exit 3") => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let focused = active_window(&inspect).and_then(|w| w["focused_block_id"].as_str());
        let focused_process = focused.and_then(|block| self.block_process(id, block).ok());
        let windows: Vec<_> = inspect["windows"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|window| (window, self.tab_title(id, window)))
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
                    if block["creator_name"] != "com.superlogical.terminal" {
                        continue;
                    }
                    pane += 1;
                    if let Some(block_id) = block["block_id"].as_str()
                        && target_block.is_none_or(|target| target == block_id)
                    {
                        blocks.push((window_id, window_name, block_id, pane));
                    }
                }
            }
        }
        // Session rows also need live agent identity to decorate their tabs.
        let mut entries =
            parallel_entries(&blocks, |&(window_id, window_name, block_id, pane)| {
                let process = if focused == Some(block_id) {
                    focused_process.clone()
                } else {
                    self.block_process(id, block_id).ok()
                };
                let Some(process) = process else {
                    return Ok(Vec::new());
                };
                // Status is optional on older Rex versions and on agents that do
                // not implement OSC 7501. Process identity still works.
                let status = self
                    .json(&["block", "call", "-s", id, "-b", block_id, "program_status"])
                    .unwrap_or(Value::Null);
                let Some(running) = crate::agent::running(&process, &status) else {
                    return Ok(Vec::new());
                };
                let mut agent = Entry::new(
                    format!("{} · pane {pane} · {name} / {window_name}", running.app),
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
                Ok(vec![agent])
            })?;
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
                    if let Some(title) = self.block_title(id, &agent.block_id)
                        && !titles.contains(&title)
                    {
                        titles.push(title);
                    }
                }
                if titles.is_empty() {
                    for agent in &tab_agents {
                        if !titles.contains(&agent.app) {
                            titles.push(agent.app.clone());
                        }
                    }
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
        if !agents {
            entries.clear();
        }
        if let Some(session) = session {
            entries.insert(0, session);
        }
        Ok(entries)
    }
    fn tab_title(&self, session: &str, window: &Value) -> Option<String> {
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
        if block["creator_name"] == "com.superlogical.terminal"
            && let Some(title) = self.block_title(session, focused)
        {
            return Some(title);
        }
        block["label"]
            .as_str()
            .filter(|label| !label.is_empty())
            .map(str::to_owned)
    }
    fn block_title(&self, session: &str, block: &str) -> Option<String> {
        self.json(&["block", "call", "-s", session, "-b", block, "title"])
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
fn parallel_entries<T: Sync>(
    items: &[T],
    query: impl Fn(&T) -> Result<Vec<Entry>> + Sync,
) -> Result<Vec<Entry>> {
    if items.len() <= 1 {
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
                scope.spawn(move || -> Result<Vec<Entry>> {
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
