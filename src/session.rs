use crate::{
    backend::Rex,
    config::{Config, SortGroup, expand_path},
    model::{Entry, Source},
    process,
};
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Default)]
pub struct ListOptions {
    pub rex: bool,
    pub config: bool,
    pub zoxide: bool,
    pub hide_attached: bool,
    pub hide_duplicates: bool,
    pub blacklisted: bool,
    pub no_cache: bool,
    pub panes: bool,
}
#[derive(Debug, Clone, Default)]
pub struct ConnectOptions {
    pub detach: bool,
    pub command: Option<String>,
    pub root: bool,
    pub name: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Connection {
    pub session_id: String,
    pub name: String,
    pub path: PathBuf,
    pub created: bool,
}
#[derive(Debug, Clone)]
pub struct Manager {
    pub rex: Rex,
    pub config: Config,
    pub state_dir: PathBuf,
}
#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    paths: HashMap<String, PathBuf>,
    #[serde(default)]
    history: HashMap<String, Vec<String>>,
}
#[derive(Serialize, Deserialize)]
struct Cache {
    time: u64,
    scope: String,
    entries: Vec<Entry>,
}
impl Manager {
    pub fn new(rex: Rex, config: Config) -> Self {
        let base = std::env::var_os("REXUE_STATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::state_dir()
                    .or_else(dirs::data_local_dir)
                    .unwrap_or_else(std::env::temp_dir)
                    .join("rexue")
            });
        let server_env = std::env::var("REX_SERVER").ok();
        let key = rex
            .server
            .as_deref()
            .or(server_env.as_deref())
            .unwrap_or("local");
        let scope = fingerprint(&(key, &rex.bin));
        Self {
            rex,
            config,
            state_dir: base.join(scope),
        }
    }
    fn lock(&self) -> Result<File> {
        fs::create_dir_all(&self.state_dir)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.state_dir.join("state.lock"))?;
        lock.lock_exclusive()?;
        Ok(lock)
    }
    fn state(&self) -> Result<State> {
        let path = self.state_dir.join("state.json");
        match fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b)
                .with_context(|| format!("invalid state {}; restore or remove it", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(e.into()),
        }
    }
    fn save(&self, state: &State) -> Result<()> {
        atomic_json(&self.state_dir.join("state.json"), state)
    }
    pub fn invalidate(&self) {
        let _ = fs::remove_file(self.state_dir.join("list.json"));
    }
    fn restore_paths(&self, entries: &mut [Entry], state: &State) {
        for entry in entries {
            if entry.path.as_os_str().is_empty()
                && let Some(path) = entry.session_id.as_ref().and_then(|id| state.paths.get(id))
            {
                entry.path = path.clone();
            }
        }
    }
    fn frecency_override(&self, operation: &str) -> Result<Option<&str>> {
        let script = match operation {
            "list" => self.config.frecency.list_command.as_deref(),
            "query" => self.config.frecency.query_command.as_deref(),
            "add" => self.config.frecency.add_command.as_deref(),
            "remove" => self.config.frecency.remove_command.as_deref(),
            _ => bail!("unknown frecency operation {operation}"),
        };
        Ok(script.filter(|command| !command.is_empty()))
    }
    fn frecency(&self, operation: &str, value: &str) -> Result<String> {
        if let Some(script) = self.frecency_override(operation)? {
            return frecency_command(script, value);
        }
        let mut cmd = Command::new("zoxide");
        match operation {
            "list" => {
                cmd.args(["query", "--list", "--score"]);
            }
            "query" => {
                cmd.args(["query", "--", value]);
            }
            _ => {
                cmd.args([operation, "--", value]);
            }
        }
        process::text(&mut cmd)
    }
    fn frecency_entries(&self, no_cache: bool) -> Result<Vec<Entry>> {
        // Only cache frecency. Rex sessions and resolved config must remain live.
        let scope = fingerprint(&format!("{:?}", self.config));
        if self.config.cache
            && !no_cache
            && let Ok(bytes) = fs::read(self.state_dir.join("list.json"))
            && let Ok(cache) = serde_json::from_slice::<Cache>(&bytes)
            && cache.scope == scope
            && now().saturating_sub(cache.time) < 5
        {
            return Ok(cache.entries);
        }
        let entries = match self.frecency("list", "") {
            Ok(output) => parse_frecency(&output),
            Err(error)
                if self.frecency_override("list")?.is_none()
                    && format!("{error:#}").contains("could not start") =>
            {
                Vec::new()
            }
            Err(error) => return Err(error).context("could not list frecency directories"),
        };
        if self.config.cache {
            atomic_json(
                &self.state_dir.join("list.json"),
                &Cache {
                    time: now(),
                    scope,
                    entries: entries.clone(),
                },
            )?;
        }
        Ok(entries)
    }
    fn collect(&self, opts: &ListOptions) -> Result<Vec<Entry>> {
        let selected = opts.rex || opts.config || opts.zoxide || opts.panes;
        let mut entries = Vec::new();
        let configs = self.config.entries()?;
        if !selected || opts.rex {
            let mut live = self.rex.list()?;
            self.restore_paths(&mut live, &self.state()?);
            for entry in &mut live {
                if let Some(config) = configs.iter().find(|c| {
                    c.name == entry.name
                        || (!entry.path.as_os_str().is_empty() && same_path(&c.path, &entry.path))
                }) {
                    entry.icon = config.icon.clone();
                    entry.alias = config.alias.clone();
                    entry.alias_auto_connect = config.alias_auto_connect;
                }
            }
            entries.extend(live);
        }
        if !selected || opts.config {
            entries.extend(configs.iter().cloned());
        }
        if !selected || opts.zoxide {
            entries.extend(self.frecency_entries(opts.no_cache)?);
        }
        if opts.panes {
            entries.extend(self.panes()?);
        }
        for entry in &mut entries {
            entry.icon = self.entry_icon(entry)?;
        }
        // TUI resolves source icons when no custom icon applies.
        Ok(entries)
    }
    fn entry_icon(&self, entry: &Entry) -> Result<Option<String>> {
        let explicit = self
            .config
            .session
            .iter()
            .find(|s| s.name == entry.name && s.icon.as_ref().is_some_and(|s| !s.is_empty()))
            .or_else(|| {
                self.config.session.iter().find(|s| {
                    s.icon.as_ref().is_some_and(|s| !s.is_empty())
                        && !s.path.is_empty()
                        && expand_path(&s.path).is_ok_and(|p| same_path(&p, &entry.path))
                })
            });
        if let Some(session) = explicit {
            return Ok(session.icon.clone());
        }
        if !entry.path.as_os_str().is_empty() {
            for worktree in &self.config.worktree {
                if worktree.path.is_empty()
                    || !worktree.icon.as_ref().is_some_and(|s| !s.is_empty())
                {
                    continue;
                }
                let root = normalize(&crate::worktree::worktree_root(worktree)?);
                let path = normalize(&entry.path);
                if path != root && path.starts_with(&root) {
                    return Ok(worktree.icon.clone());
                }
            }
            return Ok(self.config.resolve(&entry.path, Some(&entry.name))?.icon);
        }
        Ok(entry.icon.clone())
    }
    pub fn list(&self, opts: &ListOptions) -> Result<Vec<Entry>> {
        let mut entries = self.collect(opts)?;
        entries = order_entries(entries, &self.config.sort_order);
        entries.retain(|e| is_blacklisted(&self.config, e) == opts.blacklisted);
        if opts.hide_attached && entries.iter().any(|e| e.source == Source::Rex) {
            let context = self.rex.context()?;
            if let Some(id) = context["session_id"].as_str() {
                entries.retain(|e| e.source != Source::Rex || e.session_id.as_deref() != Some(id));
            }
        }
        if opts.hide_duplicates {
            entries = dedup_entries(entries);
        }
        Ok(entries)
    }
    pub fn panes(&self) -> Result<Vec<Entry>> {
        let id = self.rex.context()?["session_id"]
            .as_str()
            .context("not inside a Rex session; use a session target")?
            .to_owned();
        let detail = self.rex.inspect(&id)?;
        let mut entries = Vec::new();
        for w in detail["windows"].as_array().into_iter().flatten() {
            for layer in w["layers"].as_array().into_iter().flatten() {
                for b in layer["blocks"].as_array().into_iter().flatten() {
                    if let Some(block) = b["block_id"].as_str() {
                        let mut e = Entry::new(
                            format!(
                                "{}: {}",
                                w["label"].as_str().unwrap_or(""),
                                b["label"].as_str().unwrap_or(block)
                            ),
                            self.rex.block_path(&id, block).unwrap_or_default(),
                            Source::RexBlock,
                        );
                        e.session_id = Some(block.into());
                        e.active_window_name = w["label"].as_str().unwrap_or("").into();
                        entries.push(e);
                    }
                }
            }
        }
        Ok(entries)
    }
    pub fn resolve_entry(&self, target: &str) -> Result<Entry> {
        let mut target = clean_target(target, &self.config.entries()?);
        for icon in self
            .config
            .icons
            .values()
            .map(|i| i.icon.as_str())
            .filter(|s| !s.is_empty())
        {
            if let Some(rest) = target.strip_prefix(&format!("{icon} ")) {
                target = rest.trim_start().to_owned();
                break;
            }
        }
        if let Some(session) = self.config.session.iter().find(|session| {
            session.alias.as_deref().is_some_and(|alias| {
                crate::model::alias_key(alias) == crate::model::alias_key(&target)
            })
        }) {
            target = session.name.clone();
        }
        if let Ok(panes) = self.panes() {
            let matches: Vec<_> = panes
                .into_iter()
                .filter(|entry| {
                    entry.session_id.as_deref() == Some(&target) || entry.name == target
                })
                .collect();
            match matches.as_slice() {
                [entry] => return Ok(entry.clone()),
                [] => {}
                _ => bail!("pane name {target:?} is ambiguous; use a full block ID"),
            };
        }
        let entries = self.collect(&ListOptions {
            rex: true,
            config: true,
            no_cache: true,
            ..Default::default()
        })?;
        let exact: Vec<_> = entries
            .iter()
            .filter(|e| e.session_id.as_deref() == Some(&target))
            .collect();
        if let [e] = exact.as_slice() {
            return Ok((*e).clone());
        }
        let live = live_name_matches(&entries, &target);
        if live.len() > 1 {
            bail!("session {target:?} is ambiguous; use a full session ID");
        }
        if let Some(entry) = live.first() {
            return Ok((*entry).clone());
        }
        let matches: Vec<_> = entries.iter().filter(|e| e.name == target).collect();
        if !matches.is_empty() {
            let active: Vec<_> = matches.iter().filter(|e| e.source == Source::Rex).collect();
            if active.len() > 1 {
                bail!("session {target:?} is ambiguous; use a full session ID");
            }
            return Ok(active
                .first()
                .map(|e| (**e).clone())
                .unwrap_or_else(|| matches[0].clone()));
        }
        let literal = Path::new(&target);
        let literal_absolute = crate::config::absolute_path(literal)?;
        if let Some(config) = entries
            .iter()
            .find(|e| e.source == Source::Config && same_path(&e.path, &literal_absolute))
        {
            return Ok(config.clone());
        }
        let path = if literal.is_dir() {
            crate::config::absolute_path(literal)?
        } else {
            expand_path(&target)?
        };
        if let Some(config) = entries
            .iter()
            .find(|e| e.source == Source::Config && same_path(&e.path, &path))
        {
            return Ok(config.clone());
        }
        if path.is_dir() {
            return Ok(Entry::new(
                self.name(&path)?,
                normalize(&path),
                Source::Directory,
            ));
        }
        if target.starts_with("session:") {
            bail!("session {target} no longer exists");
        }
        if target.starts_with("block:") {
            return self
                .panes()?
                .into_iter()
                .find(|e| e.session_id.as_deref() == Some(&target))
                .context("pane no longer exists in the current session");
        }
        match self.frecency("query", &target) {
            Ok(output) => {
                if let Some(entry) = parse_frecency(&output).into_iter().next()
                    && entry.path.is_dir()
                {
                    return Ok(Entry::new(
                        self.name(&entry.path)?,
                        entry.path,
                        Source::Directory,
                    ));
                }
            }
            Err(error) if self.frecency_override("query")?.is_some() => {
                return Err(error).context("frecency query failed");
            }
            Err(_) => {}
        }
        bail!("no session, configuration, or directory matches {target:?}")
    }
    pub fn connect(&self, target: &str, opts: &ConnectOptions) -> Result<Connection> {
        self.connect_resolved(opts, || {
            let rooted = if opts.root {
                connect_root_target(target)
            } else {
                None
            };
            let rooted = rooted
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned());
            self.resolve_entry(rooted.as_deref().unwrap_or(target))
        })
    }
    /// Connect a resolved picker row without treating its name as decorated text.
    /// Root conversion belongs to text-target resolution, before calling this API.
    pub fn connect_entry(&self, entry: Entry, opts: &ConnectOptions) -> Result<Connection> {
        self.connect_resolved(opts, || {
            // Alias-only rows can be synthesized outside the fetched sources.
            // Complete their typed config identity by exact key, never by the
            // human-target parser that strips decoration or expands aliases.
            if entry.source == Source::Config
                && entry.session_id.is_none()
                && entry.path.as_os_str().is_empty()
            {
                return self
                    .config
                    .entries()?
                    .into_iter()
                    .find(|configured| configured.name == entry.name)
                    .context("configured picker target no longer exists");
            }
            Ok(entry)
        })
    }
    /// Connect a known directory without interpreting it as an alias or label.
    pub fn connect_path(&self, path: &Path, opts: &ConnectOptions) -> Result<Connection> {
        self.connect_resolved(opts, || {
            if let Some(entry) = self
                .config
                .entries()?
                .into_iter()
                .find(|e| same_path(&e.path, path))
            {
                return Ok(entry);
            }
            if !path.is_dir() {
                bail!("session directory does not exist: {}", path.display());
            }
            Ok(Entry::new(
                self.name(path)?,
                normalize(path),
                Source::Directory,
            ))
        })
    }

    fn connect_resolved(
        &self,
        opts: &ConnectOptions,
        resolve: impl FnOnce() -> Result<Entry>,
    ) -> Result<Connection> {
        // Preflight and lock both text and resolved-entry workflows identically.
        let client = if opts.detach {
            None
        } else {
            let c = self.rex.selected_client()?;
            self.rex.check_switch(&c)?;
            Some(c)
        };
        let _lock = self.lock()?;
        let mut entry = resolve()?;
        if entry
            .session_id
            .as_deref()
            .is_some_and(|id| id.starts_with("block:"))
        {
            if opts.name.is_some() || opts.command.is_some() {
                bail!("pane selection does not accept --name or --command");
            }
            let session = self.rex.context()?["session_id"]
                .as_str()
                .context("pane selection requires a Rex session")?
                .to_owned();
            self.rex.text(&[
                "focus",
                "-s",
                &session,
                entry.session_id.as_deref().unwrap(),
            ])?;
            if let Some(client) = client {
                let view = self.rex.inspect(&session)?;
                let block = entry.session_id.as_deref().unwrap();
                let window = view["windows"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|window| {
                        window["layers"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|layer| {
                                layer["blocks"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .any(|b| b["block_id"] == block)
                            })
                    })
                    .and_then(|w| w["window_id"].as_str())
                    .context("pane window no longer exists")?;
                self.rex.switch_window(&client, &session, window)?;
                let mut state = self.state()?;
                let history = state.history.entry(client.id).or_default();
                history.retain(|id| id != &session);
                history.insert(0, session.clone());
                history.truncate(20);
                self.save(&state)?;
                self.activate_terminal();
            }
            return Ok(Connection {
                session_id: session,
                name: entry.name,
                path: entry.path,
                created: false,
            });
        }
        if let Some(name) = &opts.name {
            if name.trim().is_empty() {
                bail!("session name must not be empty");
            }
            entry.name = name.clone();
        }
        let path = normalize(&entry.path);
        let mut state = self.state()?;
        let mut existing = self.rex.list()?;
        self.restore_paths(&mut existing, &state);
        let exact = entry.session_id.as_deref().and_then(|id| {
            existing
                .iter()
                .find(|e| e.session_id.as_deref() == Some(id))
        });
        if entry
            .session_id
            .as_deref()
            .is_some_and(|id| id.starts_with("session:"))
            && exact.is_none()
        {
            bail!(
                "session {} no longer exists",
                entry.session_id.as_deref().unwrap()
            );
        }
        let same: Vec<_> = existing
            .iter()
            .filter(|e| !path.as_os_str().is_empty() && same_path(&e.path, &path))
            .collect();
        let named = if opts.name.is_some() {
            existing.iter().filter(|e| e.name == entry.name).collect()
        } else {
            live_name_matches(&existing, &entry.name)
        };
        if named.len() > 1 && exact.is_none() {
            bail!(
                "session {:?} is ambiguous; use its full session ID",
                entry.name
            );
        }
        if opts.name.is_some()
            && let Some(conflict) = named
                .iter()
                .find(|e| e.path.as_os_str().is_empty() || !same_path(&e.path, &path))
        {
            bail!(
                "session {:?} already belongs to {}; choose a different --name",
                entry.name,
                if conflict.path.as_os_str().is_empty() {
                    "an unknown directory".into()
                } else {
                    conflict.path.display().to_string()
                }
            );
        }
        let found = if opts.name.is_some() {
            named.first().copied().or_else(|| same.first().copied())
        } else {
            exact
                .or_else(|| same.first().copied())
                .or_else(|| named.first().copied())
        };
        let result = if let Some(found) = found {
            let forced_name = if opts.name.is_some() && found.name != entry.name {
                self.rex.rename(
                    found
                        .session_id
                        .as_deref()
                        .context("existing session has no ID")?,
                    &entry.name,
                )?;
                Some(entry.name.clone())
            } else {
                None
            };
            if exact.is_none()
                && same.is_empty()
                && !path.as_os_str().is_empty()
                && (found.path.as_os_str().is_empty() || !same_path(&path, &found.path))
            {
                bail!(
                    "session {:?} already belongs to {}; choose a different --name",
                    entry.name,
                    if found.path.as_os_str().is_empty() {
                        "an unknown directory".into()
                    } else {
                        found.path.display().to_string()
                    }
                );
            }
            Connection {
                session_id: found
                    .session_id
                    .clone()
                    .context("existing session has no ID")?,
                name: forced_name.unwrap_or_else(|| found.name.clone()),
                path: if found.path.as_os_str().is_empty() {
                    path
                } else {
                    found.path.clone()
                },
                created: false,
            }
        } else {
            if !path.is_dir() {
                bail!("session directory does not exist: {}", path.display());
            }
            // Resolve configuration against the selected path before the Rex
            // cwd is canonicalized, just as list and preview do.
            let cfg = self.config.resolve(&entry.path, Some(&entry.name))?;
            let startup = if let Some(c) = opts.command.as_ref().filter(|c| !c.is_empty()) {
                Some(c.clone())
            } else {
                startup_script_at(
                    cfg.startup_command.as_deref(),
                    cfg.startup_script.as_deref(),
                    &path,
                )?
            };
            let created = self.rex.create(&entry.name, &path, startup.as_deref())?;
            // If a configured window fails, keep the created session discoverable and
            // report its full ID rather than silently deleting user processes.
            state.paths.insert(created.session_id.clone(), path.clone());
            self.save(&state)?;
            self.invalidate();
            for name in &cfg.windows {
                let window = self
                    .config
                    .window
                    .iter()
                    .find(|w| &w.name == name)
                    .with_context(|| {
                        format!(
                            "undefined window template {name:?}; session {} exists",
                            created.session_id
                        )
                    })?;
                let cwd = window
                    .path
                    .as_deref()
                    .filter(|p| !p.is_empty())
                    .map(|p| relative_path(p, &path))
                    .transpose()?
                    .unwrap_or_else(|| path.clone());
                let script = startup_script_at(
                    window.startup_command.as_deref(),
                    window.startup_script.as_deref(),
                    &cwd,
                )?;
                self.rex
                    .window(&created.session_id, name, &cwd, script.as_deref())
                    .with_context(|| {
                        format!(
                            "window {name:?} failed; session {} exists",
                            created.session_id
                        )
                    })?;
            }
            Connection {
                session_id: created.session_id,
                name: entry.name,
                path,
                created: true,
            }
        };
        if let Some(client) = &client {
            // whoami describes the launcher process, whose inherited session
            // can remain stale after switching the selected app client. Once
            // recorded, successful switches are the authority for that client.
            let initial_history = state.history.get(&client.id).is_none_or(Vec::is_empty);
            let previous = if initial_history {
                self.rex
                    .context()
                    .ok()
                    .and_then(|v| v["session_id"].as_str().map(str::to_owned))
            } else {
                None
            };
            self.rex.switch(client, &result.session_id)?;
            let history = state.history.entry(client.id.clone()).or_default();
            if let Some(previous) = previous.filter(|id| id != &result.session_id) {
                history.retain(|id| id != &previous);
                history.insert(0, previous);
            }
            history.retain(|id| id != &result.session_id);
            history.insert(0, result.session_id.clone());
            history.truncate(20);
        }
        state
            .paths
            .insert(result.session_id.clone(), result.path.clone());
        self.save(&state)?;
        self.invalidate();
        if !result.path.as_os_str().is_empty()
            && let Err(error) = self.frecency("add", &result.path.to_string_lossy())
        {
            eprintln!("rexue: could not record directory: {error:#}");
        }
        if client.is_some() {
            self.activate_terminal();
        }
        Ok(result)
    }
    pub fn activate_terminal(&self) {
        if let Err(error) = process::activate_application(&self.config.terminal) {
            eprintln!(
                "rexue: could not activate configured application {:?}: {error:#}",
                self.config.terminal
            );
        }
    }
    pub fn note_client_switch(&self, client: &str, id: &str) -> Result<()> {
        let _lock = self.lock()?;
        let mut state = self.state()?;
        let history = state.history.entry(client.into()).or_default();
        if history.is_empty()
            && let Ok(previous) = std::env::var("REX_SESSION")
            && previous != id
        {
            history.push(previous);
        }
        history.retain(|old| old != id);
        history.insert(0, id.into());
        history.truncate(20);
        self.save(&state)
    }

    pub fn name(&self, path: &Path) -> Result<String> {
        name_path(&self.config, path)
    }
    pub fn root_name(&self, path: &Path) -> Result<String> {
        let path = normalize(path);
        if let Some(name) = substituted_name(&self.config, &path)? {
            return Ok(valid_name(&name));
        }
        // Root naming has its own strategies: substitutions see the original
        // path, while only VCS strategies collapse it to a worktree/workspace.
        let git_name = if self.config.git_namer_use_worktree_root {
            git_toplevel(&path)
                .ok()
                .map(|top| last_components(&top, self.config.git_dir_length))
        } else {
            git_anchor(&path).ok().map(|anchor| {
                let top = git_toplevel(&path).unwrap_or_else(|_| anchor.clone());
                anchored_root_name(&self.config, &anchor, &top)
            })
        };
        let name = git_name
            .or_else(|| {
                jj_roots(&path).map(|(workspace, repo)| {
                    if self.config.git_namer_use_worktree_root {
                        last_components(&workspace, self.config.git_dir_length)
                    } else {
                        anchored_root_name(&self.config, &repo, &workspace)
                    }
                })
            })
            .unwrap_or_else(|| last_components(&path, self.config.dir_length));
        Ok(valid_name(&name))
    }
    /// Calling terminal first, then this client's last successful switch for external launchers.
    pub fn current_path(&self) -> Result<PathBuf> {
        if let Ok(context) = self.rex.context()
            && let Some(id) = context["session_id"].as_str()
            && let Ok(entry) = self.resolve_entry(id)
            && !entry.path.as_os_str().is_empty()
        {
            return Ok(normalize(&entry.path));
        }
        let client = self.rex.selected_client()?;
        let state = self.state()?;
        let history = state
            .history
            .get(&client.id)
            .context("no session history for the selected Rex client")?;
        for id in history {
            if let Ok(entry) = self.resolve_entry(id)
                && !entry.path.as_os_str().is_empty()
            {
                return Ok(normalize(&entry.path));
            }
        }
        bail!("no running session path for the selected Rex client")
    }
    pub fn last(&self) -> Result<Connection> {
        let client = self.rex.selected_client()?;
        let state = self.state()?;
        let history = state
            .history
            .get(&client.id)
            .context("no previous session recorded for this Rex client")?;
        let live = self.rex.list()?;
        let id = history
            .iter()
            .skip(1)
            .find(|id| live.iter().any(|e| e.session_id.as_ref() == Some(id)))
            .context("previous sessions are no longer running")?;
        self.connect(id, &ConnectOptions::default())
    }
    pub fn preview(&self, entry: &Entry) -> Result<String> {
        if let Some(id) = &entry.session_id {
            if id.starts_with("session:") {
                return self.rex.capture(id);
            }
            if id.starts_with("block:") {
                return self.rex.text(&["capture", "-b", id, "--trim", "--unwrap"]);
            }
        }
        let cfg = self.config.resolve(&entry.path, Some(&entry.name))?;
        if let Some(script) = cfg.preview_command.filter(|s| !s.is_empty()) {
            let output = process::shell_script(&script, &entry.path)?;
            if !output.is_empty() {
                return Ok(output);
            }
        }
        directory_preview(&entry.path)
    }
    pub fn remove(&self, entry: &Entry) -> Result<()> {
        match entry.source {
            Source::Rex => self
                .rex
                .kill(entry.session_id.as_deref().context("session has no ID")?)?,
            Source::Zoxide | Source::Directory => {
                self.frecency("remove", &entry.path.to_string_lossy())?;
            }
            _ => bail!("configured sessions cannot be removed here; edit rexue.toml"),
        };
        self.invalidate();
        Ok(())
    }
}
fn relative_path(input: &str, cwd: &Path) -> Result<PathBuf> {
    let expanded = shellexpand::full(input).context("could not expand window path")?;
    let path = PathBuf::from(expanded.as_ref());
    Ok(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}
fn startup_script_at(
    command: Option<&str>,
    script: Option<&str>,
    cwd: &Path,
) -> Result<Option<String>> {
    if let Some(script) = script.filter(|s| !s.is_empty()) {
        // Window startup_script is shell text upstream. Retain support for
        // an existing script file, resolving relative files against its cwd.
        if let Ok(p) = relative_path(script, cwd)
            && p.is_file()
        {
            return Ok(Some(fs::read_to_string(&p).with_context(|| {
                format!("read startup script {}", p.display())
            })?));
        }
        return Ok(Some(script.to_owned()));
    }
    Ok(command.filter(|c| !c.is_empty()).map(str::to_owned))
}
pub fn normalize(path: &Path) -> PathBuf {
    if path.as_os_str().is_empty() {
        return PathBuf::new();
    }
    if let Ok(path) = path.canonicalize() {
        return path;
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    // A configured project may not exist yet; still resolve its existing
    // parent aliases (/tmp, symlinked worktree roots) before comparing paths.
    for parent in absolute.ancestors().skip(1) {
        if let Ok(mut resolved) = parent.canonicalize()
            && let Ok(suffix) = absolute.strip_prefix(parent)
        {
            for component in suffix.components() {
                match component {
                    std::path::Component::ParentDir => {
                        resolved.pop();
                    }
                    std::path::Component::CurDir => {}
                    _ => resolved.push(component.as_os_str()),
                }
            }
            return resolved;
        }
    }
    absolute
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    normalize(a) == normalize(b)
}
pub fn abbreviate(path: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(home)
    {
        return if rest.as_os_str().is_empty() {
            "~".into()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.display().to_string()
}
pub fn smart_name(path: &Path) -> String {
    name_path(&Config::default(), path).unwrap_or_else(|_| last_components(path, 1))
}
// sesh applies --root to an input directory before aliases/session resolution.
// Finding no Git/JJ root is an optional miss, never a connection failure.
fn connect_root_target(target: &str) -> Option<PathBuf> {
    let literal = Path::new(target);
    let path = if literal.is_dir() {
        crate::config::absolute_path(literal).ok()?
    } else {
        expand_path(target).ok().filter(|path| path.is_dir())?
    };
    if let Ok(common) = process::text(
        Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(["rev-parse", "--git-common-dir"]),
    ) {
        let common = PathBuf::from(common.trim_end());
        let common = normalize(&if common.is_absolute() {
            common
        } else {
            path.join(common)
        });
        if common.file_name().and_then(|n| n.to_str()) == Some(".bare")
            && let Some(top) = common.parent()
            && let Ok(relative) = path.strip_prefix(top)
        {
            return Some(
                relative
                    .components()
                    .next()
                    .map(|first| top.join(first.as_os_str()))
                    .unwrap_or_else(|| top.to_owned()),
            );
        }
    }
    git_toplevel(&path)
        .ok()
        .or_else(|| jj_roots(&path).map(|(workspace, _)| workspace))
}

pub fn git_root(path: &Path) -> Result<PathBuf> {
    match git_toplevel(path) {
        Ok(root) => Ok(root),
        Err(error) => jj_roots(path).map(|(workspace, _)| workspace).ok_or(error),
    }
}
fn git_toplevel(path: &Path) -> Result<PathBuf> {
    let output = process::text(
        Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "--show-toplevel"]),
    )?;
    Ok(normalize(Path::new(output.trim_end())))
}
// Read JJ's workspace metadata directly, as upstream does. Ordinary directories
// require neither a JJ process nor an installed optional executable.
fn jj_roots(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let path = normalize(path);
    let workspace = path.ancestors().find(|p| p.join(".jj").is_dir())?;
    let repo = fs::read_to_string(workspace.join(".jj/repo"))
        .ok()
        .and_then(|pointer| {
            let pointer = PathBuf::from(pointer.trim());
            let repo_dir = if pointer.is_absolute() {
                pointer
            } else {
                workspace.join(".jj").join(pointer)
            };
            repo_dir.parent()?.parent().map(normalize)
        })
        .unwrap_or_else(|| workspace.to_path_buf());
    Some((workspace.to_path_buf(), repo))
}
fn live_name_matches<'a>(entries: &'a [Entry], base: &str) -> Vec<&'a Entry> {
    let exact: Vec<_> = entries
        .iter()
        .filter(|e| e.source == Source::Rex && e.name == base)
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    let prefix = format!("{base} — ");
    entries
        .iter()
        .filter(|e| e.source == Source::Rex && e.name.starts_with(&prefix))
        .collect()
}
fn dedup_entries(entries: Vec<Entry>) -> Vec<Entry> {
    let live_names: HashSet<_> = entries
        .iter()
        .filter(|e| e.source == Source::Rex)
        .map(|e| e.name.clone())
        .collect();
    let kept_config = |e: &Entry| e.source == Source::Config && !live_names.contains(&e.name);
    let paths: HashSet<_> = entries
        .iter()
        .filter(|e| e.source == Source::Rex || kept_config(e))
        .filter(|e| !e.path.as_os_str().is_empty())
        .map(|e| normalize(&e.path))
        .collect();
    entries
        .into_iter()
        .filter(|e| match e.source {
            Source::Config => !live_names.contains(&e.name),
            Source::Zoxide => e.path.as_os_str().is_empty() || !paths.contains(&normalize(&e.path)),
            _ => true,
        })
        .collect()
}
fn git_anchor(path: &Path) -> Result<PathBuf> {
    let output = process::text(Command::new("git").arg("-C").arg(path).args([
        "worktree",
        "list",
        "--porcelain",
    ]))?;
    let first = output.split("\n\n").next().unwrap_or("");
    let root = first
        .lines()
        .find_map(|l| l.strip_prefix("worktree "))
        .context("git worktree list has no root")?;
    let mut root = PathBuf::from(root);
    if first.lines().any(|l| l == "bare")
        && matches!(
            root.file_name().and_then(|s| s.to_str()),
            Some(".git" | ".bare")
        )
    {
        root.pop();
    }
    Ok(normalize(&root))
}
fn last_components(path: &Path, length: usize) -> String {
    let parts: Vec<_> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        return if path.is_absolute() {
            "/".into()
        } else {
            ".".into()
        };
    }
    parts[parts.len().saturating_sub(length.max(1))..].join("/")
}
fn anchored_root_name(config: &Config, anchor: &Path, root: &Path) -> String {
    // Match upstream's string-prefix semantics, including external workspaces.
    let root = root.to_string_lossy();
    let anchor_path = anchor.to_string_lossy();
    format!(
        "{}{}",
        last_components(anchor, config.git_dir_length),
        root.strip_prefix(anchor_path.as_ref()).unwrap_or(&root)
    )
}
fn valid_name(name: &str) -> String {
    name.replace(['.', ':'], "_")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("_")
}
fn name_path(config: &Config, path: &Path) -> Result<String> {
    let name = valid_name(&raw_name_path(config, path)?);
    if name.is_empty() {
        bail!("could not determine session name for {}", path.display());
    }
    Ok(name)
}
fn substituted_name(config: &Config, path: &Path) -> Result<Option<String>> {
    let abbreviated = abbreviate(path);
    let mut substituted = abbreviated.clone();
    for rule in &config.name_substitution {
        if !rule.find.is_empty() {
            substituted = rule.apply(&substituted)?;
        }
    }
    if substituted != abbreviated && !substituted.trim().is_empty() {
        return Ok(Some(substituted));
    }
    Ok(None)
}
fn raw_name_path(config: &Config, path: &Path) -> Result<String> {
    let path = normalize(path);
    if let Some(name) = substituted_name(config, &path)? {
        return Ok(name);
    }
    // Naming strategies are independent of the VCS selected for cloning.
    {
        let anchor = if config.git_namer_use_worktree_root {
            git_root(&path)
        } else {
            git_anchor(&path)
        };
        if let Ok(anchor) = anchor {
            let base = last_components(&anchor, config.git_dir_length);
            if let Ok(rest) = path.strip_prefix(&anchor) {
                return Ok(if rest.as_os_str().is_empty() {
                    base
                } else {
                    format!("{base}/{}", rest.display())
                });
            }
            // Sibling worktrees have no path relative to the main checkout.
            // Anchor them by the repo and their own directory, then retain subdirs.
            if let Ok(top) = git_root(&path) {
                let suffix = path.strip_prefix(&top).unwrap_or(Path::new(""));
                let tree = last_components(&top, 1);
                return Ok(if suffix.as_os_str().is_empty() {
                    format!("{base}/{tree}")
                } else {
                    format!("{base}/{tree}/{}", suffix.display())
                });
            }
        }
    }
    if let Some((workspace, repo)) = jj_roots(&path) {
        let anchor = if config.git_namer_use_worktree_root {
            workspace
        } else {
            repo
        };
        let base = last_components(&anchor, config.git_dir_length);
        // Upstream retains the full workspace path when it is outside the repo.
        let suffix = path.to_string_lossy();
        let anchor = anchor.to_string_lossy();
        return Ok(format!(
            "{base}{}",
            suffix.strip_prefix(anchor.as_ref()).unwrap_or(&suffix)
        ));
    }
    Ok(last_components(&path, config.dir_length))
}
fn fingerprint(value: &impl Hash) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hash);
    format!("{:016x}", hash.finish())
}
fn frecency_command(script: &str, value: &str) -> Result<String> {
    // Parse first, substitute second: a query/path is always one argument,
    // even when it contains whitespace, quotes, or shell metacharacters.
    let mut args = shell_words::split(script)
        .context("invalid frecency command quoting")?
        .into_iter();
    let executable = args.next().context("frecency command is empty")?;
    let mut command = Command::new(executable);
    command.args(args.map(|arg| arg.replace("{}", value)));
    process::text(&mut command)
}
fn parse_frecency(output: &str) -> Vec<Entry> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (score, path) = match line.split_once(char::is_whitespace) {
                Some((prefix, rest)) => match prefix.parse::<f64>() {
                    Ok(score) if score.is_finite() => (score, rest.trim_start()),
                    _ => (0., line),
                },
                None => (0., line),
            };
            if path.is_empty() {
                return None;
            }
            let mut entry = Entry::new(abbreviate(Path::new(path)), path, Source::Zoxide);
            entry.score = score;
            Some(entry)
        })
        .collect()
}
fn is_blacklisted(config: &Config, entry: &Entry) -> bool {
    config
        .blacklist
        .iter()
        .filter_map(|pattern| regex::Regex::new(pattern).ok())
        .any(|re| re.is_match(&entry.name))
}
fn order_entries(entries: Vec<Entry>, order: &[SortGroup]) -> Vec<Entry> {
    let mut groups: Vec<Vec<Source>> = Vec::new();
    let mut seen = HashSet::new();
    for group in order {
        let names = match group {
            SortGroup::Source(name) => vec![name],
            SortGroup::Merged(names) => names.iter().collect(),
        };
        let sources: Vec<_> = names
            .into_iter()
            .filter_map(|n| match n.to_ascii_lowercase().as_str() {
                "rex" => Some(Source::Rex),
                "rex-block" => Some(Source::RexBlock),
                "config" => Some(Source::Config),
                "zoxide" => Some(Source::Zoxide),
                _ => None,
            })
            .filter(|s| seen.insert(s.as_str()))
            .collect();
        if !sources.is_empty() {
            groups.push(sources);
        }
    }
    for source in [
        Source::Rex,
        Source::Config,
        Source::Zoxide,
        Source::RexBlock,
    ] {
        if seen.insert(source.as_str()) {
            groups.push(vec![source]);
        }
    }
    let scores: HashMap<_, _> = entries
        .iter()
        .filter(|e| e.source == Source::Zoxide)
        .map(|e| (normalize(&e.path), e.score))
        .collect();
    let mut result = Vec::new();
    for (index, sources) in groups.iter().enumerate() {
        let mut group = Vec::new();
        for source in sources {
            group.extend(entries.iter().filter(|e| e.source == *source).cloned());
        }
        if sources.len() > 1 {
            for entry in &mut group {
                if entry.score == 0. {
                    entry.score = scores.get(&normalize(&entry.path)).copied().unwrap_or(0.);
                }
            }
            group.sort_by(|a, b| b.score.total_cmp(&a.score));
        }
        for entry in &mut group {
            entry.group = index;
        }
        result.extend(group);
    }
    result
}
pub fn directory_preview(path: &Path) -> Result<String> {
    let mut items = fs::read_dir(path)
        .with_context(|| format!("cannot preview {}", path.display()))?
        .map(|e| {
            let e = e?;
            let suffix = if e.file_type()?.is_dir() { "/" } else { "" };
            Ok(format!("{}{}", e.file_name().to_string_lossy(), suffix))
        })
        .collect::<Result<Vec<_>>>()?;
    items.sort();
    Ok(format!("{}\n\n{}", abbreviate(path), items.join("\n")))
}
pub fn clean_target(target: &str, configs: &[Entry]) -> String {
    let clean = String::from_utf8_lossy(&strip_ansi_escapes::strip(target))
        .trim()
        .to_owned();
    for icon in ["", "", "", "", "", "📁"]
        .into_iter()
        .chain(configs.iter().filter_map(|c| c.icon.as_deref()))
    {
        if let Some(rest) = clean.strip_prefix(&format!("{icon} ")) {
            return rest.trim_start().into();
        }
    }
    clean
}
pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("state path has no parent")?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}-{}-{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(tmp);
        return Err(e.into());
    }
    Ok(())
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn root_naming_plain_directories_and_substitutions_use_the_input_path() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("parent/child.$UNSET_ROOT_NAME");
        fs::create_dir_all(&path).unwrap();
        let mut manager = isolated(Config::default(), d.path());
        manager.config.dir_length = 2;
        assert_eq!(
            manager.root_name(&path).unwrap(),
            "parent/child_$UNSET_ROOT_NAME"
        );
        assert_eq!(
            manager.name(&path).unwrap(),
            manager.root_name(&path).unwrap()
        );
        process::text(Command::new("git").args(["init", "--quiet"]).arg(d.path())).unwrap();
        manager
            .config
            .name_substitution
            .push(crate::config::NameSubstitution {
                find: abbreviate(&normalize(&path)),
                replace: "child:chosen name".into(),
                regex: false,
            });
        assert_eq!(manager.root_name(&path).unwrap(), "child_chosen_name");
        assert_eq!(manager.name(&path).unwrap(), "child_chosen_name");
        for replacement in ["", " \t "] {
            manager.config.name_substitution[0].replace = replacement.into();
            assert_eq!(
                manager.root_name(&path).unwrap(),
                last_components(d.path(), 1).replace('.', "_")
            );
        }
    }

    #[test]
    fn root_naming_does_not_apply_rules_to_the_collapsed_repository_root() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("repo");
        process::text(Command::new("git").args(["init", "--quiet"]).arg(&repo)).unwrap();
        let child = repo.join("child");
        fs::create_dir(&child).unwrap();
        let mut manager = isolated(Config::default(), d.path());
        manager
            .config
            .name_substitution
            .push(crate::config::NameSubstitution {
                find: format!("^{}$", regex::escape(&abbreviate(&normalize(&repo)))),
                replace: "root-only".into(),
                regex: true,
            });
        assert_eq!(manager.root_name(&child).unwrap(), "repo");
        assert_eq!(manager.name(&child).unwrap(), "repo/child");
        assert_eq!(manager.root_name(&repo).unwrap(), "root-only");
    }

    #[test]
    fn root_naming_bare_git_and_external_jj_workspaces_follow_reference_modes() {
        let d = tempfile::tempdir().unwrap();
        let bare = d.path().join("project/.bare");
        process::text(
            Command::new("git")
                .args(["init", "--quiet", "--bare"])
                .arg(&bare),
        )
        .unwrap();
        let mut manager = isolated(Config::default(), d.path());
        assert_eq!(manager.root_name(&bare).unwrap(), "project");
        manager.config.git_namer_use_worktree_root = true;
        assert_eq!(manager.root_name(&bare).unwrap(), "_bare");

        let repo = d.path().join("jj-project");
        let workspace = d.path().join("external");
        fs::create_dir_all(repo.join(".jj/repo")).unwrap();
        fs::create_dir_all(workspace.join(".jj")).unwrap();
        fs::create_dir_all(workspace.join("child")).unwrap();
        fs::write(
            workspace.join(".jj/repo"),
            repo.join(".jj/repo").to_str().unwrap(),
        )
        .unwrap();
        for mode in [false, true] {
            manager.config.git_namer_use_worktree_root = mode;
            let expected = if mode {
                "external".into()
            } else {
                valid_name(&format!("jj-project{}", normalize(&workspace).display()))
            };
            assert_eq!(
                manager.root_name(&workspace.join("child")).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn jj_metadata_names_workspaces_and_roots_without_a_cli() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("project");
        let workspace = repo.join("ws/feature");
        let nested = workspace.join("src/pkg");
        fs::create_dir_all(repo.join(".jj/repo")).unwrap();
        fs::create_dir_all(workspace.join(".jj")).unwrap();
        fs::create_dir_all(&nested).unwrap();
        // Relative pointers are relative to the workspace's .jj directory.
        fs::write(workspace.join(".jj/repo"), "../../../.jj/repo\n").unwrap();
        let mut manager = isolated(Config::default(), d.path());
        assert_eq!(manager.name(&repo).unwrap(), "project");
        assert_eq!(manager.name(&nested).unwrap(), "project/ws/feature/src/pkg");
        assert_eq!(manager.root_name(&nested).unwrap(), "project/ws/feature");
        assert!(same_path(&git_root(&nested).unwrap(), &workspace));
        manager.config.git_dir_length = 2;
        assert!(
            manager
                .name(&nested)
                .unwrap()
                .ends_with("/project/ws/feature/src/pkg")
        );
        manager.config.git_namer_use_worktree_root = true;
        manager.config.git_dir_length = 1;
        assert_eq!(manager.name(&nested).unwrap(), "feature/src/pkg");
        assert_eq!(manager.root_name(&nested).unwrap(), "feature");
        // Absolute workspace pointers resolve to the same roots.
        fs::write(
            workspace.join(".jj/repo"),
            repo.join(".jj/repo").to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(jj_roots(&nested).unwrap().1, normalize(&repo));
        let ordinary = d.path().join("ordinary");
        fs::create_dir(&ordinary).unwrap();
        assert_eq!(manager.name(&ordinary).unwrap(), "ordinary");
        assert!(jj_roots(&ordinary).is_none());
        assert!(git_root(&ordinary).is_err());
    }
    #[test]
    fn dedup_priority_is_independent_of_display_order_and_preserves_live_rows() {
        let entries = vec![
            Entry::new("live", "/live", Source::Rex),
            Entry::new("live", "/live", Source::Rex),
            Entry::new("live", "/dropped-config", Source::Config),
            Entry::new("different", "/live", Source::Config),
            Entry::new("kept", "/config", Source::Config),
            Entry::new("free", "/dropped-config", Source::Zoxide),
            Entry::new("live-path", "/live", Source::Zoxide),
            Entry::new("config-path", "/config", Source::Zoxide),
        ];
        for order in [
            vec![],
            vec![
                SortGroup::Source("zoxide".into()),
                SortGroup::Source("config".into()),
            ],
            vec![SortGroup::Merged(vec![
                "zoxide".into(),
                "config".into(),
                "rex".into(),
            ])],
        ] {
            let kept = dedup_entries(order_entries(entries.clone(), &order));
            assert_eq!(kept.len(), 5);
            assert_eq!(kept.iter().filter(|e| e.source == Source::Rex).count(), 2);
            assert!(kept.iter().any(|e| e.name == "free"));
            assert!(kept.iter().any(|e| e.name == "different"));
            assert!(
                !kept
                    .iter()
                    .any(|e| e.name == "config-path" || e.name == "live-path")
            );
        }
        // Once the attached live row is hidden, its config row is eligible.
        let kept = dedup_entries(
            entries
                .into_iter()
                .filter(|e| e.source != Source::Rex)
                .collect(),
        );
        assert!(
            kept.iter()
                .any(|e| e.source == Source::Config && e.name == "live")
        );
        assert!(!kept.iter().any(|e| e.name == "free"));
    }
    #[test]
    fn worktree_icon_root_expansion_matches_workflow_for_absolute_home_and_env() {
        let d = tempfile::tempdir().unwrap();
        let home = dirs::home_dir().unwrap();
        let leaf = format!("rexue-icon-fixture-{}", std::process::id());
        let root = home.join(&leaf);
        let child = root.join("123");
        for spelling in [
            root.display().to_string(),
            format!("~/{leaf}"),
            format!("${{HOME}}/{leaf}"),
            format!("$HOME/{leaf}"),
        ] {
            let cfg = Config {
                session: vec![crate::config::SessionConfig {
                    name: "tree".into(),
                    path: child.display().to_string(),
                    ..Default::default()
                }],
                worktree: vec![crate::config::WorktreeConfig {
                    repo: "org/repo".into(),
                    path: d.path().display().to_string(),
                    worktree_dir: spelling.clone(),
                    icon: Some("W".into()),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let rows = isolated(cfg, d.path())
                .list(&ListOptions {
                    config: true,
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(rows[0].icon.as_deref(), Some("W"), "{spelling}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn missing_worktree_child_resolves_existing_symlink_ancestors() {
        let d = tempfile::tempdir().unwrap();
        let actual = d.path().join("actual");
        let alias = d.path().join("alias");
        fs::create_dir(&actual).unwrap();
        std::os::unix::fs::symlink(&actual, &alias).unwrap();
        let child = alias.join("123");
        assert!(!child.exists());
        assert_eq!(
            normalize(&child),
            actual.canonicalize().unwrap().join("123")
        );
        let cfg = Config {
            session: vec![crate::config::SessionConfig {
                name: "tree".into(),
                path: child.display().to_string(),
                ..Default::default()
            }],
            worktree: vec![crate::config::WorktreeConfig {
                repo: "org/repo".into(),
                path: d.path().display().to_string(),
                worktree_dir: actual.display().to_string(),
                icon: Some("W".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let rows = isolated(cfg, d.path())
            .list(&ListOptions {
                config: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows[0].icon.as_deref(), Some("W"));
    }

    #[test]
    fn worktree_icons_precede_wildcards_but_follow_explicit_session_icons() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("trees");
        fs::create_dir_all(root.join("42/sub")).unwrap();
        let mut config = Config::default();
        config.worktree.push(crate::config::WorktreeConfig {
            path: d.path().to_string_lossy().into_owned(),
            worktree_dir: "trees".into(),
            icon: Some("tree".into()),
            ..Default::default()
        });
        config.wildcard.push(crate::config::WildcardConfig {
            pattern: format!("{}/**", d.path().display()),
            icon: Some("wild".into()),
            ..Default::default()
        });
        config.session.push(crate::config::SessionConfig {
            name: "saved".into(),
            path: root.join("42/sub").to_string_lossy().into_owned(),
            ..Default::default()
        });
        let mut manager = isolated(config, d.path());
        let opts = ListOptions {
            config: true,
            ..Default::default()
        };
        assert_eq!(
            manager.list(&opts).unwrap()[0].icon.as_deref(),
            Some("tree")
        );
        manager.config.session[0].icon = Some("session".into());
        assert_eq!(
            manager.list(&opts).unwrap()[0].icon.as_deref(),
            Some("session")
        );
        let sibling = Entry::new("sibling", d.path().join("trees-other/42"), Source::Zoxide);
        assert_eq!(
            manager.entry_icon(&sibling).unwrap().as_deref(),
            Some("wild")
        );
        manager.config.worktree[0].worktree_dir = root.to_string_lossy().into_owned();
        manager.config.session[0].icon = None;
        assert_eq!(
            manager.list(&opts).unwrap()[0].icon.as_deref(),
            Some("tree")
        );
    }
    #[cfg(unix)]
    #[test]
    fn enriched_live_base_reuses_session_and_rejects_ambiguity() {
        let d = tempfile::tempdir().unwrap();
        let mut manager = isolated(Config::default(), d.path());
        manager.rex.bin = fake_rex(d.path());
        manager.config.frecency.add_command = Some("printf ''".into());
        let script = fs::read_to_string(&manager.rex.bin).unwrap();
        fs::write(
            &manager.rex.bin,
            script.replace("already-running", "project — issue"),
        )
        .unwrap();
        let entry = manager.resolve_entry("project").unwrap();
        assert_eq!(entry.name, "project — issue");
        let connection = manager
            .connect(
                "project",
                &ConnectOptions {
                    detach: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(!connection.created);
        assert_eq!(connection.session_id, "session:existing");
        let script = fs::read_to_string(&manager.rex.bin).unwrap();
        fs::write(&manager.rex.bin, script.replace(
            "\"label\":\"project — issue\"}",
            "\"label\":\"project — issue\"},{\"session_id\":\"session:second\",\"label\":\"project — another\"}"
        )).unwrap();
        assert!(
            manager
                .resolve_entry("project")
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
        let entries = vec![
            Entry::new("project-other — issue", "", Source::Rex),
            Entry::new("project—issue", "", Source::Rex),
        ];
        assert!(live_name_matches(&entries, "project").is_empty());
        let mut entries = vec![
            Entry::new("project — issue", "", Source::Rex),
            Entry::new("project", "", Source::Rex),
        ];
        assert_eq!(live_name_matches(&entries, "project")[0].name, "project");
        entries.push(Entry::new("project", "", Source::Rex));
        assert_eq!(live_name_matches(&entries, "project").len(), 2);
    }
    fn isolated(config: Config, root: &Path) -> Manager {
        Manager {
            rex: Rex {
                bin: root.join("must-not-run"),
                server: None,
                client: None,
            },
            config,
            state_dir: root.join("state"),
        }
    }
    #[test]
    fn config_only_never_queries_rex_or_frecency() {
        let d = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.session.push(crate::config::SessionConfig {
            name: "saved".into(),
            path: d.path().display().to_string(),
            ..Default::default()
        });
        config.frecency.list_command = Some("must-not-run".into());
        let manager = isolated(config, d.path());
        let entries = manager
            .list(&ListOptions {
                config: true,
                hide_attached: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "saved");
        assert!(!manager.state_dir.exists());
    }
    #[test]
    fn blacklist_is_regex_and_flag_inverts_selection() {
        let d = tempfile::tempdir().unwrap();
        let mut config = Config {
            blacklist: vec!["[".into(), "^scratch".into()],
            ..Default::default()
        };
        for name in ["scratch-one", "keep"] {
            config.session.push(crate::config::SessionConfig {
                name: name.into(),
                path: d.path().display().to_string(),
                ..Default::default()
            });
        }
        let manager = isolated(config, d.path());
        let normal = manager
            .list(&ListOptions {
                config: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(normal[0].name, "keep");
        assert_eq!(normal.len(), 1);
        let blocked = manager
            .list(&ListOptions {
                config: true,
                blacklisted: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(blocked[0].name, "scratch-one");
        assert_eq!(blocked.len(), 1);
    }
    #[test]
    fn optional_scores_preserve_paths_with_spaces() {
        let entries = parse_frecency("  42.5 /tmp/a b\n/tmp/plain name\n/tmp/single\n\n");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].score, 42.5);
        assert_eq!(entries[0].path, Path::new("/tmp/a b"));
        assert_eq!(entries[1].path, Path::new("/tmp/plain name"));
        assert_eq!(entries[1].score, 0.);
    }
    #[test]
    fn merged_groups_borrow_scores_and_keep_stable_ties() {
        let mut hot = Entry::new("hot", "/hot", Source::Zoxide);
        hot.score = 90.;
        let cold = Entry::new("cold", "/cold", Source::Zoxide);
        let entries = vec![
            Entry::new("live", "/live", Source::Rex),
            Entry::new("notes", "/hot", Source::Config),
            Entry::new("unknown", "/unknown", Source::Config),
            hot,
            cold,
        ];
        let merged = order_entries(
            entries.clone(),
            &[SortGroup::Merged(vec!["config".into(), "zoxide".into()])],
        );
        assert_eq!(
            merged.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["notes", "hot", "unknown", "cold", "live"]
        );
        assert_eq!(merged[0].group, merged[3].group);
        assert_ne!(merged[3].group, merged[4].group);
        let flat = order_entries(entries, &[SortGroup::Source("zoxide".into())]);
        assert_eq!(flat[0].name, "hot");
        assert_eq!(flat[2].name, "live");
        assert_eq!(flat[3].name, "notes");
    }
    #[test]
    fn command_arguments_are_data_even_for_non_directory_queries() {
        let d = tempfile::tempdir().unwrap();
        let marker = d.path().join("injected");
        let query = format!("it's a query; $(touch {})", marker.display());
        let output = frecency_command("printf '%s' '{}'", &query).unwrap();
        assert_eq!(output, query);
        assert!(!marker.exists());
        assert!(frecency_command("", "").is_err());
        assert!(frecency_command("printf '", "").is_err());
    }
    #[test]
    fn cache_is_opt_in_and_scoped_to_config() {
        let d = tempfile::tempdir().unwrap();
        let mut manager = isolated(Config::default(), d.path());
        manager.config.frecency.list_command = Some("printf '/first\\n'".into());
        let opts = ListOptions {
            zoxide: true,
            ..Default::default()
        };
        assert_eq!(manager.list(&opts).unwrap()[0].path, Path::new("/first"));
        assert!(!manager.state_dir.join("list.json").exists());
        manager.config.cache = true;
        manager.list(&opts).unwrap();
        assert!(manager.state_dir.join("list.json").exists());
        manager.config.frecency.list_command = Some("printf '/second\\n'".into());
        assert_eq!(manager.list(&opts).unwrap()[0].path, Path::new("/second"));
        manager.config.cache = false;
        atomic_json(
            &manager.state_dir.join("list.json"),
            &Cache {
                time: now(),
                scope: fingerprint(&format!("{:?}", manager.config)),
                entries: vec![Entry::new("stale", "/stale", Source::Zoxide)],
            },
        )
        .unwrap();
        assert_eq!(manager.list(&opts).unwrap()[0].path, Path::new("/second"));
    }
    #[test]
    fn saved_paths_restore_unknown_cwd_and_canonicalize() {
        let d = tempfile::tempdir().unwrap();
        let manager = isolated(Config::default(), d.path());
        let mut state = State::default();
        state.paths.insert("session:one".into(), d.path().into());
        let mut entries = vec![Entry::new("one", "", Source::Rex)];
        entries[0].session_id = Some("session:one".into());
        manager.restore_paths(&mut entries, &state);
        assert!(same_path(&entries[0].path, &d.path().join(".")));
    }
    #[test]
    fn naming_substitution_precedes_directory_length() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("parent/child");
        fs::create_dir_all(&path).unwrap();
        let mut config = Config {
            dir_length: 2,
            ..Default::default()
        };
        assert_eq!(name_path(&config, &path).unwrap(), "parent/child");
        config
            .name_substitution
            .push(crate::config::NameSubstitution {
                find: abbreviate(&normalize(d.path())),
                replace: "workspace".into(),
                regex: false,
            });
        assert_eq!(name_path(&config, &path).unwrap(), "workspace/parent/child");
        config
            .name_substitution
            .push(crate::config::NameSubstitution {
                find: "workspace/(.*)".into(),
                replace: "project:$1".into(),
                regex: true,
            });
        assert_eq!(name_path(&config, &path).unwrap(), "project_parent/child");
    }
    #[test]
    fn git_worktrees_and_subdirectories_get_distinct_names() {
        let d = tempfile::tempdir().unwrap();
        let main = d.path().join("repo");
        let other = d.path().join("feature");
        process::text(Command::new("git").args(["init", "--quiet"]).arg(&main)).unwrap();
        process::text(Command::new("git").arg("-C").arg(&main).args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "init",
        ]))
        .unwrap();
        process::text(
            Command::new("git")
                .arg("-C")
                .arg(&main)
                .args(["worktree", "add", "--quiet", "-b", "feature"])
                .arg(&other),
        )
        .unwrap();
        fs::create_dir(main.join("sub")).unwrap();
        fs::create_dir(other.join("sub")).unwrap();
        let mut config = Config::default();
        assert_eq!(name_path(&config, &main).unwrap(), "repo");
        assert_eq!(name_path(&config, &main.join("sub")).unwrap(), "repo/sub");
        assert_eq!(name_path(&config, &other).unwrap(), "repo/feature");
        assert_eq!(
            name_path(&config, &other.join("sub")).unwrap(),
            "repo/feature/sub"
        );
        assert!(same_path(&git_root(&other.join("sub")).unwrap(), &other));
        let manager = isolated(config.clone(), d.path());
        // RootName retains upstream's complete suffix for sibling worktrees;
        // ordinary Name keeps the established repo/feature/sub spelling.
        assert_eq!(
            manager.root_name(&other.join("sub")).unwrap(),
            valid_name(&format!("repo{}", normalize(&other).display()))
        );
        assert_eq!(manager.root_name(&main.join("sub")).unwrap(), "repo");
        let mut root_manager = manager.clone();
        root_manager.config.git_namer_use_worktree_root = true;
        assert_eq!(
            root_manager.root_name(&other.join("sub")).unwrap(),
            "feature"
        );
        root_manager.config.git_dir_length = 2;
        assert_eq!(
            root_manager.root_name(&other.join("sub")).unwrap(),
            valid_name(&last_components(&normalize(&other), 2))
        );
        config.vcs = "jj".into();
        assert_eq!(name_path(&config, &main.join("sub")).unwrap(), "repo/sub");
        config.git_namer_use_worktree_root = true;
        assert_eq!(
            name_path(&config, &other.join("sub")).unwrap(),
            "feature/sub"
        );
        config.git_dir_length = 2;
        assert!(name_path(&config, &other).unwrap().ends_with("/feature"));
    }
    #[test]
    fn relative_window_scripts_resolve_against_window_cwd() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("setup.sh"), "printf ready").unwrap();
        assert_eq!(
            startup_script_at(None, Some("setup.sh"), d.path())
                .unwrap()
                .as_deref(),
            Some("printf ready")
        );
        assert_eq!(
            startup_script_at(None, Some("printf inline"), d.path())
                .unwrap()
                .as_deref(),
            Some("printf inline")
        );
    }
    #[cfg(unix)]
    fn fake_rex(root: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let executable = root.join("rex-test");
        fs::write(&executable,r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
    ls) printf '%s' '{"sessions":[{"session_id":"session:existing","label":"already-running"}]}'; exit 0 ;;
    inspect) printf '%s' '{"active_window_id":"window:one","windows":[{"window_id":"window:one","label":"Terminal","focused_block_id":"block:one","layers":[{"blocks":[{"block_id":"block:one","label":"shell"}]}]}]}'; exit 0 ;;
    process|pwd) printf '%s' '{}'; exit 0 ;;
    whoami) printf '%s' '{"session_id":"session:existing"}'; exit 0 ;;
    focus) exit 0 ;;
    new|rename) echo 'unexpected mutation' >&2; exit 91 ;;
  esac
  shift
done
exit 92
"#).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        executable
    }
    #[cfg(unix)]
    #[test]
    fn connect_reuses_saved_canonical_path_when_rex_cwd_is_unknown() {
        let d = tempfile::tempdir().unwrap();
        let project = d.path().join("project");
        fs::create_dir(&project).unwrap();
        let mut manager = isolated(Config::default(), d.path());
        manager.rex.bin = fake_rex(d.path());
        manager.config.frecency.add_command = Some("printf ''".into());
        let mut state = State::default();
        state
            .paths
            .insert("session:existing".into(), project.clone());
        manager.save(&state).unwrap();
        let connection = manager
            .connect(
                &project.join(".").to_string_lossy(),
                &ConnectOptions {
                    detach: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(!connection.created);
        assert_eq!(connection.session_id, "session:existing");
        assert_eq!(connection.name, "already-running");
        let other = d.path().join("other");
        fs::create_dir(&other).unwrap();
        let error = manager
            .connect(
                &other.to_string_lossy(),
                &ConnectOptions {
                    detach: true,
                    name: Some("already-running".into()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("already belongs"));
    }
    #[cfg(unix)]
    #[test]
    fn pane_target_focuses_without_session_creation() {
        let d = tempfile::tempdir().unwrap();
        let mut manager = isolated(Config::default(), d.path());
        manager.rex.bin = fake_rex(d.path());
        let result = manager
            .connect(
                "block:one",
                &ConnectOptions {
                    detach: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(result.session_id, "session:existing");
        assert!(!result.created);
    }
    #[cfg(unix)]
    #[test]
    fn configured_source_emoji_is_stripped_when_resolving() {
        let d = tempfile::tempdir().unwrap();
        let mut manager = isolated(Config::default(), d.path());
        manager.rex.bin = fake_rex(d.path());
        manager.config.icons.insert(
            "rex".into(),
            crate::config::SourceIcon {
                icon: "🦀".into(),
                color: "cyan".into(),
            },
        );
        let entry = manager
            .resolve_entry("\u{1b}[36m🦀 already-running\u{1b}[0m")
            .unwrap();
        assert_eq!(entry.session_id.as_deref(), Some("session:existing"));
        assert!(entry.icon.is_none());
    }
    #[cfg(unix)]
    #[test]
    fn stale_launcher_environment_does_not_override_client_history() {
        // Run the complete regression with a stale inherited environment in
        // its own process. Parallel tests never observe an environment change.
        const CHILD: &str = "REXUE_SESSION_HISTORY_TEST_CHILD";
        if std::env::var(CHILD).ok().as_deref() != Some("1") {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "session::tests::stale_launcher_environment_does_not_override_client_history",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("REX_SESSION", "session:launcher")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let executable = d.path().join("rex-history");
        fs::write(&executable,r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in
    client) printf '%s' '{"clients":[{"client_id":"client:desk","info":{"kind":"app","label":"desk"},"connection_state":"connected"}]}'; exit 0 ;;
    actions) printf '%s' '{"actions":[{"name":"session.select"}]}'; exit 0 ;;
    do) printf '%s' '{}'; exit 0 ;;
    whoami) printf '{"session_id":"%s"}' "$REX_SESSION"; exit 0 ;;
    ls) printf '%s' '{"sessions":[{"session_id":"session:launcher","label":"launcher"},{"session_id":"session:one","label":"one"},{"session_id":"session:two","label":"two"}]}'; exit 0 ;;
    inspect) printf '%s' '{"windows":[]}'; exit 0 ;;
    new) echo 'unexpected creation' >&2; exit 91 ;;
  esac
  shift
done
exit 92
"#).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let mut manager = isolated(Config::default(), d.path());
        manager.rex.bin = executable;
        manager.rex.client = Some("client:desk".into());
        manager
            .connect("session:one", &ConnectOptions::default())
            .unwrap();
        manager
            .connect("session:two", &ConnectOptions::default())
            .unwrap();
        assert_eq!(
            manager.state().unwrap().history["client:desk"],
            ["session:two", "session:one", "session:launcher"]
        );
        assert_eq!(manager.last().unwrap().session_id, "session:one");
        assert_eq!(manager.last().unwrap().session_id, "session:two");
    }
    #[test]
    fn decorated_target_round_trip() {
        let mut e = Entry::new("project", "/tmp", Source::Config);
        e.icon = Some("🚀".into());
        assert_eq!(clean_target("\x1b[32m🚀 project\x1b[0m", &[e]), "project");
    }
    #[test]
    fn file_names_are_data_in_preview() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("$(touch should-not-exist)"), "").unwrap();
        let p = directory_preview(dir.path()).unwrap();
        assert!(p.contains("$(touch should-not-exist)"));
        assert!(!dir.path().join("should-not-exist").exists());
    }
    #[test]
    fn atomic_state_replaces_existing() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("state");
        atomic_json(&p, &vec![1]).unwrap();
        atomic_json(&p, &vec![2]).unwrap();
        assert_eq!(
            serde_json::from_slice::<Vec<u8>>(&fs::read(p).unwrap()).unwrap(),
            vec![2]
        );
    }
}
