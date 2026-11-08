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
    pub agents: bool,
    pub rex: bool,
    pub hide_attached: bool,
    pub blacklisted: bool,
    pub picker_block: Option<String>,
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
/// Successful removal that needs a session handoff after picker teardown.
#[derive(Debug)]
pub struct WorktreeRemoval {
    session_ids: Vec<String>,
    repository: PathBuf,
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
    fn restore_paths(&self, entries: &mut [Entry], state: &State) {
        for entry in entries {
            if entry.path.as_os_str().is_empty()
                && let Some(path) = entry.session_id.as_ref().and_then(|id| state.paths.get(id))
            {
                entry.path = path.clone();
            }
        }
    }
    fn collect(&self, opts: &ListOptions) -> Result<Vec<Entry>> {
        let configs = self.config.entries()?;
        let mut entries = self.rex.list_sources_excluding_picker(
            opts.rex || !opts.agents,
            opts.agents,
            opts.picker_block.as_deref(),
        )?;
        self.restore_paths(&mut entries, &self.state()?);
        for entry in entries.iter_mut().filter(|entry| entry.is_session()) {
            if let Some(config) = configs.iter().find(|config| {
                config.name == entry.name
                    || (!entry.path.as_os_str().is_empty() && same_path(&config.path, &entry.path))
            }) {
                entry.icon = config.icon.clone();
                entry.alias = config.alias.clone();
                entry.alias_auto_connect = config.alias_auto_connect;
            }
        }
        for entry in &mut entries {
            entry.icon = self.entry_icon(entry)?;
            for tab in &mut entry.tabs {
                for pane in &mut tab.panes {
                    pane.icon = self.entry_icon(pane)?;
                }
            }
        }
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
            let common = if self.config.worktree.iter().any(|w| w.icon.is_some()) {
                git_common_dir(&entry.path)
            } else {
                None
            };
            if let Some(common) = common {
                for worktree in &self.config.worktree {
                    if worktree.icon.as_ref().is_some_and(|s| !s.is_empty())
                        && expand_path(&worktree.path)
                            .ok()
                            .and_then(|p| git_common_dir(&p))
                            .as_ref()
                            == Some(&common)
                    {
                        return Ok(worktree.icon.clone());
                    }
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
        if opts.hide_attached && entries.iter().any(|e| e.session_id.is_some()) {
            let context = self.rex.context()?;
            if let Some(id) = context["session_id"].as_str() {
                entries.retain(|e| e.session_id.as_deref() != Some(id));
            }
        }
        Ok(entries)
    }
    pub fn resolve_entry(&self, target: &str) -> Result<Entry> {
        let configs = self.config.entries()?;
        let mut target = clean_target(target, &configs);
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
        if target.starts_with("block:") {
            return self.resolve_agent(&target);
        }
        if let Some(session) = self.config.session.iter().find(|session| {
            session.alias.as_deref().is_some_and(|alias| {
                crate::model::alias_key(alias) == crate::model::alias_key(&target)
            })
        }) {
            target = session.name.clone();
        }
        let mut entries = self.collect(&ListOptions {
            rex: true,
            ..Default::default()
        })?;
        entries.extend(configs);
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
        bail!("no session, configuration, or directory matches {target:?}")
    }
    fn resolve_agent(&self, block_id: &str) -> Result<Entry> {
        self.rex
            .list_sources(false, true)?
            .into_iter()
            .find(|entry| entry.target() == block_id)
            .with_context(|| format!("agent in {block_id} is no longer running"))
    }
    pub fn connect(&self, target: &str, opts: &ConnectOptions) -> Result<Connection> {
        self.connect_resolved(opts, None, || {
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
        if entry.source == Source::Worktree {
            let prepared = crate::worktree::Worktrees::new(&self.config).prepare(
                &crate::worktree::Selection {
                    path: Some(entry.path.clone()),
                },
                &entry.path.to_string_lossy(),
                false,
                None,
            )?;
            return self.connect_path(&prepared.path, opts);
        }
        self.connect_resolved(opts, Some(&entry), || Ok(entry.clone()))
    }
    /// Connect a known directory without interpreting it as an alias or label.
    pub fn connect_path(&self, path: &Path, opts: &ConnectOptions) -> Result<Connection> {
        self.connect_resolved(opts, None, || {
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
        selected: Option<&Entry>,
        resolve: impl FnOnce() -> Result<Entry>,
    ) -> Result<Connection> {
        let live = selected.filter(|entry| {
            opts.name.is_none()
                && entry
                    .session_id
                    .as_deref()
                    .is_some_and(|id| id.starts_with("session:"))
        });
        // Keep the live snapshot fresh even when another connection holds the state lock.
        let early_lock = live.map(|_| self.lock()).transpose()?;
        let mut prepared = live
            .map(|entry| self.rex.prepare_connection(entry, opts.detach))
            .transpose()?
            .flatten();
        let client = if let Some(prepared) = &prepared {
            prepared.client.clone()
        } else if opts.detach {
            None
        } else {
            let c = self.rex.selected_client()?;
            self.rex.check_switch(&c)?;
            Some(c)
        };
        let _lock = match early_lock {
            Some(lock) => lock,
            None => self.lock()?,
        };
        let mut entry = resolve()?;
        let agent = entry.agent.clone();
        let mut window_id = entry.window_id().map(str::to_owned);
        let block_id = entry.block_id().map(str::to_owned);
        if (window_id.is_some() || block_id.is_some()) && opts.name.is_some() {
            bail!("--name cannot be used when connecting to a tab or pane");
        }
        if let Some(name) = &opts.name {
            if name.trim().is_empty() {
                bail!("session name must not be empty");
            }
            entry.name = name.clone();
        }
        let mut path = normalize(&entry.path);
        let mut state = self.state()?;
        let mut existing = if let Some(prepared) = &mut prepared {
            std::mem::take(&mut prepared.entries)
        } else if opts.name.is_none()
            && let Some(id) = entry
                .session_id
                .as_deref()
                .filter(|id| id.starts_with("session:"))
        {
            self.rex
                .list_session(id, agent.as_ref().map(|agent| agent.block_id.as_str()))?
        } else {
            self.rex.list()?
        };
        self.restore_paths(&mut existing, &state);
        if let Some(selected) = &agent {
            let current = existing
                .iter()
                .find(|candidate| {
                    candidate
                        .agent
                        .as_ref()
                        .is_some_and(|current| current.block_id == selected.block_id)
                })
                .with_context(|| format!("agent in {} is no longer running", selected.block_id))?;
            path = normalize(&current.path);
            window_id = current.window_id().map(str::to_owned);
            existing.retain(|candidate| candidate.is_session());
        } else if let Some(selected_window) = &window_id {
            let session = existing
                .iter()
                .find(|candidate| candidate.is_session())
                .context("session no longer exists")?;
            if let Some(block) = &block_id {
                let current = session
                    .tabs
                    .iter()
                    .flat_map(|tab| &tab.panes)
                    .find(|pane| pane.block_id() == Some(block))
                    .with_context(|| format!("pane {block} no longer exists"))?;
                window_id = current.window_id().map(str::to_owned);
                path = normalize(if current.path.as_os_str().is_empty() {
                    &session.path
                } else {
                    &current.path
                });
            } else {
                let tab = session
                    .tabs
                    .iter()
                    .find(|tab| &tab.window_id == selected_window)
                    .with_context(|| format!("tab {selected_window} no longer exists"))?;
                window_id = Some(tab.window_id.clone());
            }
        }
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
        // A live terminal can move to another checkout without changing its
        // label. Keep that session intact and disambiguate only automatic
        // directory names; explicit/configured names retain conflict checks.
        if opts.name.is_none()
            && entry.source == Source::Directory
            && exact.is_none()
            && same.is_empty()
        {
            entry.name = available_directory_name(&entry.name, &path, &existing);
        }
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
                path: if block_id.is_some() || found.path.as_os_str().is_empty() {
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
            // Keep a created session discoverable even if app selection fails.
            state.paths.insert(created.session_id.clone(), path.clone());
            self.save(&state)?;
            Connection {
                session_id: created.session_id,
                name: entry.name,
                path,
                created: true,
            }
        };
        // Inherited context seeds history only once; later successful switches are authoritative.
        let previous = if let Some(client) = &client
            && state.history.get(&client.id).is_none_or(Vec::is_empty)
        {
            match &prepared {
                Some(prepared) => prepared.previous.clone(),
                None => self
                    .rex
                    .context()
                    .ok()
                    .and_then(|v| v["session_id"].as_str().map(str::to_owned)),
            }
        } else {
            None
        };
        if let Some(prepared) = prepared {
            prepared.finish(
                &result.session_id,
                window_id.as_deref(),
                block_id.as_deref(),
            )?;
        } else {
            if let Some(target) = block_id.as_deref().or(window_id.as_deref()) {
                self.rex.focus(&result.session_id, target)?;
            }
            if let Some(client) = &client {
                self.rex
                    .switch_window(client, &result.session_id, window_id.as_deref())?;
            }
        }
        if let Some(client) = &client {
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
        Ok(result)
    }
    pub fn name(&self, path: &Path) -> Result<String> {
        name_path(&self.config, path)
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
        if let Some(id) = &entry.session_id
            && id.starts_with("session:")
        {
            return if let Some(block) = entry.block_id() {
                self.rex.capture_block(id, block)
            } else if let Some(window) = entry.window_id() {
                let inspect = self.rex.inspect(id)?;
                let block = inspect["windows"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|tab| tab["window_id"] == window)
                    .and_then(|tab| tab["focused_block_id"].as_str())
                    .context("tab has no focused terminal")?;
                self.rex.capture_block(id, block)
            } else {
                self.rex.capture(id)
            };
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
    pub fn rename(&self, entry: &Entry, name: &str) -> Result<()> {
        if !entry.is_session() {
            bail!("only live Rex sessions can be renamed");
        }
        let id = entry
            .session_id
            .as_deref()
            .filter(|id| id.starts_with("session:") && id.len() > "session:".len())
            .context("session has no full session ID")?;
        if name.trim().is_empty() {
            bail!("session name cannot be empty");
        }
        if name.chars().any(char::is_control) {
            bail!("session name contains control characters");
        }
        if name != entry.name {
            self.rex.rename(id, name)?;
        }
        Ok(())
    }
    pub fn remove(&self, entry: &Entry) -> Result<Option<WorktreeRemoval>> {
        if entry.window_id().is_some() || entry.block_id().is_some() {
            bail!("tab and pane rows cannot be removed; select their Rex session to close it");
        }
        let mut removal = None;
        match entry.source {
            Source::Agent => {
                bail!("agent rows cannot be removed; select their Rex session to close it")
            }
            Source::Rex => self
                .rex
                .kill(entry.session_id.as_deref().context("session has no ID")?)?,
            Source::Directory => bail!("directory entries cannot be removed"),
            Source::Worktree => {
                (_, removal) = self.remove_worktree(
                    &crate::worktree::Selection {
                        path: Some(entry.path.clone()),
                    },
                    &entry.path.to_string_lossy(),
                    crate::worktree::RemoveOptions::default(),
                )?;
            }
            _ => bail!("configured sessions cannot be removed here; edit rexue.toml"),
        };
        Ok(removal)
    }
    pub fn remove_worktree(
        &self,
        selection: &crate::worktree::Selection,
        target: &str,
        options: crate::worktree::RemoveOptions,
    ) -> Result<(serde_json::Value, Option<WorktreeRemoval>)> {
        let worktrees = crate::worktree::Worktrees::new(&self.config);
        let entry = worktrees.resolve(selection, target)?;
        let root = normalize(&entry.path);
        let state = self.state()?;
        // Snapshot full IDs and canonical paths before Worktrunk removes the
        // checkout. Include sessions hidden by picker filters and saved paths
        // when the active terminal has moved or cannot report its directory.
        let mut session_ids: Vec<_> = self
            .rex
            .list_sources(true, false)?
            .into_iter()
            .filter_map(|session| {
                let id = session.session_id?;
                let live = !session.path.as_os_str().is_empty()
                    && normalize(&session.path).starts_with(&root);
                let recorded = state.paths.get(&id).is_some_and(|path| {
                    !path.as_os_str().is_empty() && normalize(path).starts_with(&root)
                });
                (live || recorded).then_some(id)
            })
            .collect();
        let current = if session_ids.is_empty() {
            None
        } else {
            self.rex.context()?["session_id"]
                .as_str()
                .map(str::to_owned)
        };
        let repository = if let Some(position) = current
            .as_ref()
            .and_then(|id| session_ids.iter().position(|candidate| candidate == id))
        {
            // Close the launcher's session last, after picker teardown and handoff.
            let id = session_ids.remove(position);
            session_ids.push(id);
            Some(
                git_common_dir(&entry.path)
                    .context("cannot resolve the current worktree's Git directory")?,
            )
        } else {
            None
        };
        let result = worktrees.remove(selection, &entry.path.to_string_lossy(), options)?;
        let pending = match repository {
            Some(repository) => Some(WorktreeRemoval {
                session_ids,
                repository,
            }),
            None => {
                self.close_worktree_sessions(&session_ids)?;
                None
            }
        };
        Ok((result, pending))
    }
    fn close_worktree_sessions(&self, session_ids: &[String]) -> Result<()> {
        for id in session_ids {
            // Rex owns the cascade: killing a session closes every tab and pane
            // and terminates their terminal processes, including coding agents.
            self.rex.kill(id).with_context(|| {
                format!("worktree removed, but could not close Rex session {id}")
            })?;
        }
        Ok(())
    }
    pub fn complete_worktree_removal(
        &self,
        removal: WorktreeRemoval,
        opts: &ConnectOptions,
    ) -> Result<Option<Connection>> {
        let next = crate::worktree::Worktrees::new(&self.config)
            .list(&crate::worktree::Selection {
                path: Some(removal.repository),
            })?
            .into_iter()
            .map(|worktree| worktree.entry())
            // Match the worktree picker's name/path ordering.
            .min_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));
        let next = match next {
            Some(entry) => Some(entry),
            None => self
                .list(&ListOptions {
                    rex: true,
                    ..Default::default()
                })?
                .into_iter()
                .find(|entry| {
                    !entry
                        .session_id
                        .as_ref()
                        .is_some_and(|id| removal.session_ids.contains(id))
                }),
        };
        let connection = next
            .map(|entry| self.connect_entry(entry, opts))
            .transpose()?;
        // Picker teardown restores the terminal before closing its host session.
        // A failed connection leaves that session available for recovery.
        self.close_worktree_sessions(&removal.session_ids)?;
        Ok(connection)
    }
}
fn git_common_dir(path: &Path) -> Option<PathBuf> {
    if !path.is_dir() {
        return None;
    }
    let dir = process::text(Command::new("git").arg("-C").arg(path).args([
        "rev-parse",
        "--path-format=absolute",
        "--git-common-dir",
    ]))
    .ok()?;
    Some(normalize(Path::new(dir.trim())))
}

fn relative_path(input: &str, cwd: &Path) -> Result<PathBuf> {
    let expanded = shellexpand::full(input).context("could not expand startup script path")?;
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
        // Accept shell text or an existing script file relative to the session cwd.
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
// Resolve --root before aliases and session names. A directory outside Git
// keeps its original path.
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
    git_toplevel(&path).ok()
}

pub fn git_root(path: &Path) -> Result<PathBuf> {
    git_toplevel(path)
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
fn live_name_matches<'a>(entries: &'a [Entry], name: &str) -> Vec<&'a Entry> {
    entries
        .iter()
        .filter(|entry| entry.source == Source::Rex && entry.name == name)
        .collect()
}
/// Default names may collide after a terminal changes checkout, or when a
/// template gives unrelated directories the same leaf name. Path-based reuse
/// is handled first; never infer that the occupied label owns this directory.
fn available_directory_name(base: &str, path: &Path, entries: &[Entry]) -> String {
    if live_name_matches(entries, base).is_empty() {
        return base.to_owned();
    }
    let branch = process::text(Command::new("git").arg("-C").arg(path).args([
        "symbolic-ref",
        "--quiet",
        "--short",
        "HEAD",
    ]))
    .ok()
    .map(|value| valid_name(value.trim()))
    .filter(|value| !value.is_empty());
    let suffix = branch.unwrap_or_else(|| fingerprint(&normalize(path))[..8].to_owned());
    let stem = format!("{base}@{suffix}");
    let mut candidate = stem.clone();
    let mut index = 2;
    while !live_name_matches(entries, &candidate).is_empty() {
        candidate = format!("{stem}-{index}");
        index += 1;
    }
    candidate
}

// A naming query only; lifecycle and discovery for managed entries use wt.
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
    // Anchor Git checkout names to the repository unless configured otherwise.
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
    Ok(last_components(&path, config.dir_length))
}
fn fingerprint(value: &impl Hash) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hash);
    format!("{:016x}", hash.finish())
}
fn is_blacklisted(config: &Config, entry: &Entry) -> bool {
    config
        .blacklist
        .iter()
        .filter_map(|pattern| regex::Regex::new(pattern).ok())
        .any(|re| {
            re.is_match(&entry.name)
                || entry
                    .agent
                    .as_ref()
                    .is_some_and(|a| re.is_match(&a.session_name))
        })
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
                "agent" => Some(Source::Agent),
                "rex" => Some(Source::Rex),
                "config" => Some(Source::Config),
                "directory" => Some(Source::Directory),
                "worktree" => Some(Source::Worktree),
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
        Source::Agent,
        Source::Config,
        Source::Directory,
        Source::Worktree,
    ] {
        if seen.insert(source.as_str()) {
            groups.push(vec![source]);
        }
    }
    let mut result = Vec::new();
    for sources in &groups {
        let mut group = Vec::new();
        for source in sources {
            group.extend(entries.iter().filter(|e| e.source == *source).cloned());
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
    for icon in ["󰚩", "", "", "", "", "", "📁"]
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_order_places_agents_after_sessions_and_before_other_sources() {
        let entries = vec![
            Entry::new("pi", "/live", Source::Agent),
            Entry::new("configured", "/config", Source::Config),
            Entry::new("first session", "/live", Source::Rex),
            Entry::new("codex", "/other", Source::Agent),
            Entry::new("second session", "/other", Source::Rex),
        ];
        let ordered = order_entries(entries.clone(), &[]);
        assert_eq!(
            ordered
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            [
                "first session",
                "second session",
                "pi",
                "codex",
                "configured"
            ]
        );

        let explicit = order_entries(entries, &[SortGroup::Source("agent".into())]);
        assert_eq!(explicit[0].source, Source::Agent);
        assert_eq!(explicit[1].source, Source::Agent);
        assert_eq!(explicit[2].source, Source::Rex);
    }

    #[test]
    fn ordering_retains_worktree_sources_and_honors_their_groups() {
        let tree = Entry::new("feature/widgets", "/custom/widgets", Source::Worktree);
        let live = Entry::new("live", "/live", Source::Rex);
        let directory = Entry::new("dir", "/directory", Source::Directory);
        let rows = order_entries(vec![live.clone(), tree.clone(), directory.clone()], &[]);
        assert_eq!(rows.len(), 3);
        let ordered = order_entries(
            vec![live, tree, directory],
            &[
                SortGroup::Source("worktree".into()),
                SortGroup::Source("directory".into()),
            ],
        );
        assert_eq!(ordered[0].source, Source::Worktree);
        assert_eq!(ordered[1].source, Source::Directory);
        assert_eq!(ordered[2].source, Source::Rex);
    }

    #[test]
    fn worktree_icons_follow_repository_identity_and_session_precedence() {
        let d = tempfile::tempdir().unwrap();
        let repo = d.path().join("repo");
        let tree = d.path().join("arbitrary checkout");
        process::text(Command::new("git").args(["init", "-b", "main"]).arg(&repo)).unwrap();
        process::text(Command::new("git").arg("-C").arg(&repo).args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "seed",
        ]))
        .unwrap();
        process::text(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["worktree", "add", "-b", "feature/custom"])
                .arg(&tree),
        )
        .unwrap();
        fs::create_dir(tree.join("sub")).unwrap();
        let mut config = Config::default();
        config.worktree.push(crate::config::WorktreeConfig {
            path: repo.display().to_string(),
            icon: Some("tree".into()),
        });
        config.wildcard.push(crate::config::WildcardConfig {
            pattern: format!("{}/**", d.path().display()),
            icon: Some("wild".into()),
            ..Default::default()
        });
        let mut manager = isolated(config, d.path());
        let entry = Entry::new("custom", tree.join("sub"), Source::Directory);
        assert_eq!(manager.entry_icon(&entry).unwrap().as_deref(), Some("tree"));
        manager.config.session.push(crate::config::SessionConfig {
            name: "custom".into(),
            path: tree.join("sub").display().to_string(),
            icon: Some("session".into()),
            ..Default::default()
        });
        assert_eq!(
            manager.entry_icon(&entry).unwrap().as_deref(),
            Some("session")
        );
        let outside = d.path().join("outside");
        fs::create_dir(&outside).unwrap();
        assert_eq!(
            manager
                .entry_icon(&Entry::new("outside", outside, Source::Directory))
                .unwrap()
                .as_deref(),
            Some("wild")
        );
        #[cfg(unix)]
        {
            let alias = d.path().join("alias");
            std::os::unix::fs::symlink(&tree, &alias).unwrap();
            assert_eq!(git_common_dir(&alias.join("sub")), git_common_dir(&repo));
        }
    }
    #[cfg(unix)]
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
    fn blacklist_uses_regex_and_ignores_invalid_patterns() {
        let config = Config {
            blacklist: vec!["[".into(), "^scratch".into()],
            ..Default::default()
        };
        assert!(is_blacklisted(
            &config,
            &Entry::new("scratch-one", "/scratch", Source::Rex)
        ));
        assert!(!is_blacklisted(
            &config,
            &Entry::new("keep", "/keep", Source::Rex)
        ));
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
    fn relative_startup_scripts_resolve_against_session_cwd() {
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
    --eval) printf 'Error: unknown flag: --eval' >&2; exit 2 ;;
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
    do)
      if [ "$2" = '--eval' ]; then printf 'Error: unknown flag: --eval' >&2; exit 2; fi
      printf '%s' '{}'; exit 0 ;;
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
