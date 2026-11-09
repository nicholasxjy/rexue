//! System-fzf picker. Display strings never serve as connection identities.
use crate::keybindings::Keybindings;
use crate::keyboard::{Keyboard, copy_size, set_raw};
use crate::model::{Entry, Source, alias_key, clean};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    net::Shutdown,
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};
use tempfile::TempDir;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone)]
pub struct Alias {
    pub alias: String,
    pub target: String,
    pub auto_connect: bool,
}

#[derive(Debug, Clone)]
pub enum InitialFocus {
    Session {
        id: String,
        window_id: Option<String>,
        block_id: Option<String>,
    },
    Worktree(PathBuf),
}

#[derive(Debug, Clone)]
pub struct PickerOptions {
    pub show_icons: bool,
    pub show_tabs: bool,
    pub native_icons: bool,
    pub separator_aware: bool,
    pub prompt: String,
    pub placeholder: String,
    pub pointer: String,
    pub query: String,
    pub aliases: BTreeMap<String, Alias>,
    pub alias_filter_prefix: String,
    pub alias_auto_connect_delay: Duration,
    pub disable_alias_auto_connect: bool,
    pub icon_width: usize,
    pub glyphs: BTreeMap<String, String>,
    pub glyph_colors: BTreeMap<String, String>,
    pub preview: bool,
    pub preview_width: u16,
    pub preview_min_width: usize,
    pub preview_border: String,
    pub keybindings: Keybindings,
    pub worktree: bool,
    pub auto_refresh: bool,
}
impl Default for PickerOptions {
    fn default() -> Self {
        Self {
            show_icons: true,
            show_tabs: false,
            native_icons: false,
            separator_aware: false,
            prompt: "> ".into(),
            placeholder: "Filter sessions...".into(),
            pointer: "> ".into(),
            query: String::new(),
            aliases: BTreeMap::new(),
            alias_filter_prefix: "/".into(),
            alias_auto_connect_delay: Duration::from_millis(150),
            disable_alias_auto_connect: false,
            icon_width: 1,
            glyphs: BTreeMap::new(),
            glyph_colors: BTreeMap::new(),
            preview: false,
            preview_width: 60,
            preview_min_width: 100,
            preview_border: "line".into(),
            keybindings: Keybindings::default(),
            worktree: false,
            auto_refresh: true,
        }
    }
}

// Only constant shell source appears in bindings. fzf quotes {1}; all other
// user data travels in FZF_QUERY or over the private socket, never shell source.
const HELPER: &str = "\"$REXUE_PICKER_EXE\" __picker \"$REXUE_PICKER_SOCKET\"";
const LIMIT: u64 = 32 * 1024 * 1024;
type Fetch = Arc<Mutex<Box<dyn Fn() -> Result<Vec<Entry>> + Send>>>;
type Preview = Arc<dyn Fn(&Entry) -> Result<String> + Send + Sync>;

struct Catalog {
    options: PickerOptions,
    entries: Vec<u64>,
    targets: BTreeMap<u64, Entry>,
    branches: BTreeMap<u64, String>,
    next_id: u64,
    revision: u64,
    rendered_rows: Option<(String, Vec<u8>)>,
    last_query: String,
    auto_armed: bool,
    status: String,
    initial_focus: Option<InitialFocus>,
    loaded_icons: BTreeSet<&'static str>,
}
fn same_entry(a: &Entry, b: &Entry) -> bool {
    if a.session_id.is_some() && b.session_id.is_some() {
        return a.session_id == b.session_id && a.target() == b.target();
    }
    a.source == b.source && a.name == b.name && a.path == b.path && a.session_id == b.session_id
}
impl Catalog {
    fn new(mut options: PickerOptions, entries: Vec<Entry>) -> Self {
        options.aliases = options
            .aliases
            .into_values()
            .map(|a| (alias_key(&a.alias), a))
            .collect();
        let mut state = Self {
            last_query: options.query.clone(),
            options,
            entries: Vec::new(),
            targets: BTreeMap::new(),
            branches: BTreeMap::new(),
            next_id: 0,
            revision: 0,
            rendered_rows: None,
            auto_armed: false,
            status: String::new(),
            initial_focus: None,
            loaded_icons: BTreeSet::new(),
        };
        state.replace(entries);
        state
    }
    fn remember(&mut self, entry: Entry) -> u64 {
        if let Some(id) = self
            .targets
            .iter()
            .find_map(|(id, e)| same_entry(e, &entry).then_some(*id))
        {
            self.targets.insert(id, entry);
            return id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.targets.insert(id, entry);
        id
    }
    fn replace(&mut self, mut entries: Vec<Entry>) {
        self.revision += 1;
        self.rendered_rows = None;
        if self.options.worktree {
            entries.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));
        } else {
            for entry in &entries {
                if let Some(alias) = &entry.alias {
                    self.options.aliases.insert(
                        alias_key(alias),
                        Alias {
                            alias: alias.clone(),
                            target: entry.name.clone(),
                            auto_connect: entry.alias_auto_connect,
                        },
                    );
                }
            }
        }
        let tree_sessions: Vec<_> = entries
            .iter()
            .filter(|entry| !entry.tabs.is_empty())
            .filter_map(|entry| entry.session_id.clone())
            .collect();
        self.entries.clear();
        self.branches.clear();
        for entry in entries {
            // Agents already appear in the session tree when its root is present.
            if entry.agent.is_some()
                && entry
                    .session_id
                    .as_ref()
                    .is_some_and(|id| tree_sessions.contains(id))
            {
                continue;
            }
            let id = self.remember(entry.clone());
            self.entries.push(id);
            let mut remaining_panes = entry.tabs.iter().map(|tab| tab.panes.len()).sum::<usize>();
            for (index, tab) in entry.tabs.iter().enumerate() {
                let last_tab = index + 1 == entry.tabs.len();
                if self.options.show_tabs {
                    let mut child = Entry::new(&tab.name, &entry.path, Source::Rex);
                    child.session_id = entry.session_id.clone();
                    child.window_id = Some(tab.window_id.clone());
                    child.tab_names.push(tab.name.clone());
                    if tab.is_agent {
                        child.agent_tab_indices.push(0);
                    }
                    let id = self.remember(child);
                    self.entries.push(id);
                    self.branches
                        .insert(id, if last_tab { "└── " } else { "├── " }.into());
                }
                for (index, pane) in tab.panes.iter().enumerate() {
                    remaining_panes -= 1;
                    let last_pane = if self.options.show_tabs {
                        index + 1 == tab.panes.len()
                    } else {
                        remaining_panes == 0
                    };
                    let id = self.remember(pane.clone());
                    self.entries.push(id);
                    self.branches.insert(
                        id,
                        format!(
                            "{}{}",
                            if !self.options.show_tabs {
                                ""
                            } else if last_tab {
                                "    "
                            } else {
                                "│   "
                            },
                            if last_pane {
                                "└── "
                            } else {
                                "├── "
                            }
                        ),
                    );
                }
            }
        }
        // Keep current IDs stable and retire targets that disappeared during refresh.
        let reachable: BTreeSet<_> = self.entries.iter().copied().collect();
        self.targets.retain(|id, _| reachable.contains(id));
    }
    fn removed(&mut self, entry: &Entry) {
        self.revision += 1;
        self.rendered_rows = None;
        self.entries
            .retain(|id| !same_entry(&self.targets[id], entry));
        self.targets.retain(|_, target| !same_entry(target, entry));
        // Retire an alias when its last matching row disappears.
        if !self
            .entries
            .iter()
            .any(|id| self.targets[id].name == entry.name)
        {
            self.options
                .aliases
                .retain(|_, alias| alias.target != entry.name);
            self.targets.retain(|_, target| target.name != entry.name);
        }
    }

    fn store_rows(&mut self, revision: u64, query: &str, rows: Vec<u8>) {
        // A slow filter may finish after a refresh or a newer query.
        if self.revision == revision && self.last_query == query {
            self.rendered_rows = Some((query.into(), rows));
        }
    }

    fn alias_query<'a>(&self, query: &'a str) -> Option<&'a str> {
        (!self.options.worktree && !self.options.alias_filter_prefix.is_empty())
            .then(|| query.strip_prefix(&self.options.alias_filter_prefix))
            .flatten()
    }
    fn index_query<'a>(&self, query: &'a str) -> Option<&'a str> {
        (!self.options.worktree && self.alias_query(query).is_none())
            .then(|| query.strip_prefix('#'))
            .flatten()
    }
    fn alias_id(&self, alias: &Alias) -> Option<u64> {
        self.entries
            .iter()
            .copied()
            .find(|id| self.targets[id].name == alias.target)
    }
    fn candidates(&self, query: &str) -> (Vec<u64>, String, bool) {
        if let Some(q) = self.alias_query(query) {
            let q = alias_key(q);
            let mut prefix = Vec::new();
            let mut names = Vec::new();
            for alias in self.options.aliases.values() {
                if let Some(id) = self.alias_id(alias) {
                    if alias_key(&alias.alias).starts_with(&q) {
                        prefix.push(id);
                    } else if !q.is_empty() && alias.target.to_lowercase().contains(&q) {
                        names.push(id);
                    }
                }
            }
            prefix.extend(names);
            return (prefix, String::new(), false);
        }
        if !self.options.worktree
            && let Some(alias) = self.options.aliases.get(&alias_key(query))
        {
            return (
                self.alias_id(alias).into_iter().collect(),
                String::new(),
                false,
            );
        }
        (
            self.entries.clone(),
            self.index_query(query).unwrap_or(query).into(),
            true,
        )
    }
    fn initial_position(&self, focus: InitialFocus) -> Option<usize> {
        match focus {
            InitialFocus::Session {
                id,
                window_id,
                block_id,
            } => {
                let find = |target: &str| {
                    self.entries.iter().position(|row| {
                        let entry = &self.targets[row];
                        entry.session_id.as_deref() == Some(&id) && entry.target() == target
                    })
                };
                block_id
                    .as_deref()
                    .and_then(find)
                    .or_else(|| window_id.as_deref().and_then(find))
                    .or_else(|| find(&id))
            }
            InitialFocus::Worktree(path) => self.entries.iter().position(|id| {
                let entry = &self.targets[id];
                entry.source == Source::Worktree && crate::session::same_path(&entry.path, &path)
            }),
        }
    }

    fn header(&self) -> String {
        let actions: &[(&str, &str)] = if self.options.worktree {
            &[
                ("create_worktree", "create"),
                ("accept", "connect"),
                ("refresh", "refresh"),
                ("remove", "remove"),
                ("toggle_preview", "preview"),
                ("cancel", "cancel"),
            ]
        } else {
            &[
                ("accept", "connect"),
                ("cancel", "cancel"),
                ("toggle_preview", "preview"),
                ("rename", "rename"),
                ("remove", "remove"),
            ]
        };
        let keys = actions
            .iter()
            .filter_map(|(action, label)| self.options.keybindings.hint(action, label))
            .collect::<Vec<_>>()
            .join(" · ");
        let header = if self.options.worktree && self.entries.is_empty() {
            format!("{keys}\nNot a Git repository or no worktrees found")
        } else {
            keys
        };
        if self.status.is_empty() {
            header
        } else {
            format!("{header}\n{}", clean(&self.status))
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Request {
    action: String,
    query: String,
    id: Option<String>,
    matches: usize,
}

/// Hidden CLI callback. Configuration and global CLI defaults are deliberately
/// bypassed: the owning picker already has the resolved config and callbacks.
pub fn helper(socket: &Path, action: &str, id: Option<&str>) -> Result<()> {
    let mut stream =
        UnixStream::connect(socket).context("picker callback is no longer available")?;
    stream.set_read_timeout(Some(Duration::from_secs(35)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    serde_json::to_writer(
        &mut stream,
        &Request {
            action: action.into(),
            query: std::env::var("FZF_QUERY").unwrap_or_default(),
            id: id.map(str::to_owned),
            matches: std::env::var("FZF_MATCH_COUNT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
        },
    )?;
    stream.shutdown(Shutdown::Write)?;
    std::io::copy(&mut stream.take(LIMIT), &mut std::io::stdout().lock())?;
    Ok(())
}

struct Bridge {
    _directory: TempDir,
    socket: PathBuf,
    stop: Arc<AtomicBool>,
    listener: Option<thread::JoinHandle<()>>,
}
impl Bridge {
    fn start(state: Arc<Mutex<Catalog>>, fetch: Fetch, preview: Preview) -> Result<Self> {
        // tempfile creates a 0700 directory, so other users cannot submit callbacks.
        let directory = tempfile::Builder::new().prefix("rexue-").tempdir()?;
        let socket = directory.path().join("picker.sock");
        let listener = UnixListener::bind(&socket).context("open private picker socket")?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let refresh_once = Arc::new(AtomicBool::new(false));
        let preview_lock = Arc::new(Mutex::new(()));
        let preview_seq = Arc::new(AtomicU64::new(0));
        let handle = thread::Builder::new().name("fzf-callbacks".into()).spawn(move || {
            for stream in listener.incoming() {
                if stopped.load(Ordering::Acquire) { break; }
                let Ok(mut stream) = stream else { break; };
                let (state, fetch, preview, refresh_once, preview_lock, preview_seq) =
                    (state.clone(), fetch.clone(), preview.clone(), refresh_once.clone(), preview_lock.clone(), preview_seq.clone());
                let _ = thread::Builder::new().name("fzf-callback".into()).spawn(move || {
                    let result = (|| -> Result<Vec<u8>> {
                        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
                        let request: Request = serde_json::from_reader((&mut stream).take(1024 * 1024))?;
                        match request.action.as_str() {
                            "rows" => rows(&state, &request.query),
                            "icons" | "icons-reset" => {
                                let mut s = state.lock().unwrap();
                                if !s.options.native_icons { return Ok(Vec::new()); }
                                if request.action == "icons-reset" { s.loaded_icons.clear(); }
                                let icons: Vec<_> = s.targets.values().filter_map(|entry| {
                                    entry.program.as_deref().or_else(|| entry.agent.as_ref().map(|a| a.app.as_str()))
                                        .and_then(|program| crate::icons::icon(program).or_else(|| crate::icons::icon("shell")))
                                }).collect();
                                let mut bytes = Vec::new();
                                for icon in icons {
                                    if s.loaded_icons.insert(icon.name) { icon.transmit(&mut bytes); }
                                }
                                Ok(bytes)
                            }
                            "status" => Ok(state.lock().unwrap().header().into_bytes()),
                            "initial" => {
                                let mut s = state.lock().unwrap();
                                // Consume once, even if the user started typing before load.
                                // Subsequent query reloads and dialogs keep normal fzf focus.
                                let focus = s.initial_focus.take();
                                let position = focus.filter(|_| request.query.is_empty())
                                    .and_then(|focus| s.initial_position(focus));
                                Ok(position.map_or_else(Vec::new, |i| format!("pos({})", i + 1).into_bytes()))
                            }
                            "loaded" => {
                                let auto_refresh = { let s = state.lock().unwrap(); s.options.worktree && s.options.auto_refresh };
                                if auto_refresh && !refresh_once.swap(true, Ordering::AcqRel) {
                                    refresh(&state, &fetch);
                                    return Ok(format!("reload({HELPER} rows)+transform-header({HELPER} status)").into_bytes());
                                }
                                let delay = {
                                    let s = state.lock().unwrap();
                                    let q = s.alias_query(&request.query).unwrap_or(&request.query);
                                    s.options.aliases.get(&alias_key(q)).filter(|a| {
                                        s.auto_armed && s.alias_id(a).is_some() && !s.options.worktree && !s.options.disable_alias_auto_connect
                                            && (s.alias_query(&request.query).is_some() || a.auto_connect)
                                    }).map(|_| s.options.alias_auto_connect_delay)
                                };
                                if let Some(delay) = delay { thread::sleep(delay); return Ok(b"pos(1)+print(accept)+accept".to_vec()); }
                                Ok(Vec::new())
                            }
                            "index" => {
                                let digit: usize = request.id.context("missing digit")?.parse()?;
                                if !(1..=9).contains(&digit) { bail!("invalid index shortcut"); }
                                let s = state.lock().unwrap();
                                let action = if s.index_query(&request.query).is_some() {
                                    // pos() clamps, so do not select when the requested row is absent.
                                    let count = request.matches;
                                    if digit <= count { format!("pos({digit})+print(accept)+accept") } else { String::new() }
                                } else { format!("put({digit})") };
                                Ok(action.into_bytes())
                            }
                            "preview" => {
                                let seq = preview_seq.fetch_add(1, Ordering::AcqRel) + 1;
                                let entry = target(&state, request.id.as_deref())?;
                                let _worker = preview_lock.lock().unwrap();
                                if preview_seq.load(Ordering::Acquire) != seq { return Ok(Vec::new()); }
                                let text = preview(&entry).unwrap_or_else(|e| format!("Preview unavailable: {e:#}"));
                                // fzf handles layout; keep arbitrary OSC/control sequences out of previews.
                                Ok(strip_ansi_escapes::strip_str(&text).chars()
                                    .filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).collect::<String>().into_bytes())
                            }
                            _ => bail!("unknown picker callback"),
                        }
                    })();
                    match result {
                        Ok(bytes) => { let _ = stream.write_all(&bytes); }
                        Err(error) => { let _ = stream.write_all(format!("Picker callback failed: {error:#}").as_bytes()); }
                    }
                });
            }
        })?;
        Ok(Self {
            _directory: directory,
            socket,
            stop,
            listener: Some(handle),
        })
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = UnixStream::connect(&self.socket);
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
    }
}
fn target(state: &Mutex<Catalog>, id: Option<&str>) -> Result<Entry> {
    let id: u64 = id
        .context("missing picker row ID")?
        .parse()
        .context("invalid picker row ID")?;
    state
        .lock()
        .unwrap()
        .targets
        .get(&id)
        .cloned()
        .context("unknown picker row ID")
}
fn refresh(state: &Mutex<Catalog>, fetch: &Mutex<Box<dyn Fn() -> Result<Vec<Entry>> + Send>>) {
    let result = (fetch.lock().unwrap())();
    let mut state = state.lock().unwrap();
    state.auto_armed = false;
    match result {
        Ok(entries) => {
            state.replace(entries);
            state.status.clear();
        }
        Err(error) => {
            if state.options.worktree {
                state.replace(Vec::new());
            }
            state.status = format!("Refresh failed: {error:#}");
        }
    }
}

fn rows(state: &Mutex<Catalog>, query: &str) -> Result<Vec<u8>> {
    let (input, pattern, fuzzy, separator_aware, numbered, revision) = {
        let mut s = state.lock().unwrap();
        if query != s.last_query {
            s.auto_armed = true;
            s.last_query = query.into();
        }
        if let Some((rendered_query, rows)) = &s.rendered_rows
            && rendered_query == query
        {
            return Ok(rows.clone());
        }
        let (ids, pattern, fuzzy) = s.candidates(query);
        let options = &s.options;
        let mut input = Vec::new();
        for id in ids {
            let entry = &s.targets[&id];
            let search = if options.worktree || entry.source == Source::Agent {
                format!(
                    "{} {}",
                    clean(&entry.display_name()),
                    clean(&entry.path.display().to_string())
                )
            } else {
                clean(&entry.name)
            };
            let search = match &entry.pane_title {
                Some(title) => format!("{search} {}", clean(title)),
                None => search,
            };
            let search = if options.separator_aware {
                normalize(&search)
            } else {
                search
            };
            let branch = s.branches.get(&id).map(String::as_str).unwrap_or("");
            let display = display(entry, options);
            if branch.is_empty() {
                write!(input, "{id}\t{search}\t{display}\0")?;
            } else {
                write!(input, "{id}\t{search}\t\x1b[2m{branch}\x1b[22m{display}\0")?;
            }
        }
        (
            input,
            pattern,
            fuzzy,
            options.separator_aware,
            !options.worktree && query.starts_with('#') && options.alias_filter_prefix != "#",
            s.revision,
        )
    };
    // Use fzf itself for fuzzy matching and ordering. Keeping the visible finder
    // disabled lets alias-only and index modes share exactly the same query box.
    // --filter is a short-lived, noninteractive fzf process, with no terminal use.
    let mut matched = if fuzzy && !pattern.is_empty() {
        let pattern = if separator_aware {
            normalize(&pattern)
        } else {
            pattern
        };
        let mut command = Command::new("fzf");
        clean_fzf_env(&mut command)?;
        let output = pipe_output(
            command.args([
                "--read0",
                "--print0",
                // Only the cleaned search column is matched; preserve display ANSI.
                "--no-ansi",
                "--delimiter=\t",
                "--nth=2",
                "--ignore-case",
                "--tiebreak=index",
                "--filter",
                &pattern,
            ]),
            input,
        )?;
        match output.status.code() {
            Some(0) | Some(1) => output.stdout,
            _ => bail!(
                "fzf filtering failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        }
    } else {
        input
    };
    // Number the actual filtered order, not backend positions.
    if numbered {
        let mut numbered = Vec::new();
        for (i, row) in matched
            .split(|c| *c == 0)
            .filter(|r| !r.is_empty())
            .enumerate()
        {
            let text = std::str::from_utf8(row)?;
            let mut fields = text.splitn(3, '\t');
            write!(
                numbered,
                "{}\t{}\t#{} {}\0",
                fields.next().unwrap(),
                fields.next().unwrap(),
                i + 1,
                fields.next().unwrap()
            )?;
        }
        matched = numbered;
    }
    state
        .lock()
        .unwrap()
        .store_rows(revision, query, matched.clone());
    Ok(matched)
}

fn clean_fzf_env(command: &mut Command) -> Result<()> {
    // Only inherit colors; other personal defaults can corrupt the row protocol.
    command
        .env_remove("FZF_DEFAULT_OPTS")
        .env_remove("FZF_DEFAULT_OPTS_FILE")
        .env_remove("FZF_DEFAULT_COMMAND")
        .env_remove("FZF_API_KEY")
        .env_remove("FZF_FILTER")
        .env("SHELL", "/bin/sh");
    // Terminal palette entries follow live theme changes; 256-color grays do not.
    if std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty()) {
        command.arg("--color=16");
    }
    if let Some(path) = std::env::var_os("FZF_DEFAULT_OPTS_FILE").filter(|path| !path.is_empty()) {
        let defaults = std::fs::read_to_string(&path)
            .with_context(|| format!("read fzf defaults from {}", Path::new(&path).display()))?;
        command.args(fzf_color_args(&defaults).context("parse FZF_DEFAULT_OPTS_FILE")?);
    }
    command.args(
        fzf_color_args(&std::env::var("FZF_DEFAULT_OPTS").unwrap_or_default())
            .context("parse FZF_DEFAULT_OPTS")?,
    );
    Ok(())
}
fn fzf_color_args(defaults: &str) -> Result<Vec<String>> {
    let mut args = shell_words::split(defaults)?.into_iter();
    let mut colors = Vec::new();
    while let Some(arg) = args.next() {
        if arg == "--color" {
            colors.push(format!(
                "--color={}",
                args.next().context("fzf --color requires a value")?
            ));
        } else if arg.starts_with("--color=") || arg == "--no-color" {
            colors.push(arg);
        }
    }
    Ok(colors)
}
fn pipe_output(command: &mut Command, input: Vec<u8>) -> Result<Output> {
    let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .context("could not start system fzf; install fzf and make it available on PATH (macOS: brew install fzf)")?;
    let mut stdin = child.stdin.take().context("missing fzf stdin")?;
    // Write concurrently: fzf may emit output before consuming a large catalog.
    let writer = thread::spawn(move || stdin.write_all(&input));
    let result = child.wait_with_output();
    let written = writer
        .join()
        .map_err(|_| anyhow!("fzf input writer panicked"))?;
    let output = result?;
    if let Err(error) = written
        && error.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(error.into());
    }
    Ok(output)
}
fn check_fzf() -> Result<()> {
    let output = Command::new("fzf").arg("--version").output()
        .context("system fzf is required; install fzf and make it available on PATH (macOS: brew install fzf)")?;
    if !output.status.success() {
        bail!(
            "fzf --version failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let version = String::from_utf8_lossy(&output.stdout);
    let parts = version
        .split_whitespace()
        .next()
        .unwrap_or("")
        .split('.')
        .take(2)
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("could not read fzf version")?;
    if parts.len() != 2 || (parts[0] == 0 && parts[1] < 63) {
        bail!(
            "fzf 0.63 or newer is required (found {}); upgrade fzf",
            version.trim()
        );
    }
    Ok(())
}
fn finder(options: &PickerOptions, bridge: &Bridge, header: &str, query: &str) -> Result<Output> {
    let mut command = Command::new("fzf");
    clean_fzf_env(&mut command)?;
    command
        .env("REXUE_PICKER_EXE", std::env::current_exe()?)
        .env("REXUE_PICKER_SOCKET", &bridge.socket)
        .args([
            "--read0",
            "--print0",
            "--print-query",
            "--no-multi",
            "--disabled",
            "--no-sort",
            "--ansi",
            "--layout=reverse",
            "--delimiter=\t",
            "--with-nth=3..",
            "--highlight-line",
            "--no-hscroll",
            "--ellipsis=…",
        ])
        .arg(format!("--prompt={}", clean(&options.prompt)))
        .arg(format!("--ghost={}", clean(&options.placeholder)))
        .arg(format!("--pointer={}", clean(&options.pointer)))
        .arg(format!("--query={query}"))
        .arg(format!("--header={header}"))
        .arg(format!(
            "--bind=start:{}reload({HELPER} rows)",
            if options.native_icons {
                format!("execute-silent({HELPER} icons-reset > /dev/tty)+")
            } else {
                String::new()
            }
        ))
        .arg(format!("--bind=change:bg-cancel+reload({HELPER} rows)"))
        .arg(format!(
            "--bind=load:{}transform({HELPER} initial)+bg-transform({HELPER} loaded)",
            if options.native_icons {
                format!("execute-silent({HELPER} icons > /dev/tty)+")
            } else {
                String::new()
            }
        ));
    let bindings = &options.keybindings;
    bindings.reset(&mut command, &Keybindings::picker_actions(options.worktree));
    for (action, fzf_action) in [
        ("accept", "print(accept)+accept"),
        ("cancel", "abort"),
        ("up", "up"),
        ("down", "down"),
        ("half_page_up", "half-page-up"),
        ("half_page_down", "half-page-down"),
        ("toggle_preview", "toggle-preview"),
        ("remove", "print(remove)+accept"),
    ] {
        bindings.bind(&mut command, action, fzf_action);
    }
    if options.worktree {
        bindings.bind(&mut command, "refresh", "print(refresh)+accept");
        bindings.bind(
            &mut command,
            "create_worktree",
            "print(create_worktree)+accept",
        );
    } else {
        bindings.bind(&mut command, "rename", "print(rename)+accept");
        for digit in 1..=9 {
            bindings.bind(
                &mut command,
                &format!("select_{digit}"),
                &format!("transform({HELPER} index {digit})"),
            );
        }
    }
    {
        let border = match options.preview_border.as_str() {
            "none" => "none",
            "thick" => "bold",
            "double" => "double",
            _ => "sharp",
        };
        command
            .arg(format!("--preview={HELPER} preview {{1}}"))
            .arg(format!(
                "--preview-window=right:{}%,border-{border},<{}(hidden){}",
                options.preview_width.clamp(10, 90),
                options.preview_min_width,
                if options.preview { "" } else { ",hidden" }
            ));
    }
    // stdout is exclusively the selection protocol. Keyboard input and terminal
    // rendering use a private PTY so extended keys can be normalized for fzf.
    let keyboard = Keyboard::prepare(&mut command)?;
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("could not start system fzf; is fzf installed and on PATH?")?;
    let result = wait_fzf(child, keyboard);
    if options.native_icons {
        crate::icons::clear();
    }
    result
}
#[derive(Debug, PartialEq, Eq)]
enum ChoiceAction {
    Accept,
    Rename,
    Remove,
    Refresh,
    CreateWorktree,
}
struct Choice {
    query: String,
    action: ChoiceAction,
    id: Option<String>,
}
fn choice(output: Output) -> Result<Option<Choice>> {
    let code = output.status.code();
    match code {
        Some(130) => return Ok(None),
        Some(0) | Some(1) => {}
        code => bail!(
            "fzf exited unexpectedly ({}); check the fzf diagnostics above",
            code.map_or_else(|| "signal".into(), |n| n.to_string())
        ),
    }
    let text = String::from_utf8(output.stdout).context("fzf returned invalid UTF-8")?;
    let mut fields = text.split('\0');
    let query = fields.next().context("fzf omitted query")?.to_owned();
    let action = match fields.next().context("fzf omitted action")? {
        "accept" => ChoiceAction::Accept,
        "rename" => ChoiceAction::Rename,
        "remove" => ChoiceAction::Remove,
        "refresh" => ChoiceAction::Refresh,
        "create_worktree" => ChoiceAction::CreateWorktree,
        "" if code == Some(1) => return Ok(None),
        action => bail!("unexpected fzf action {action:?}"),
    };
    // fzf exits 1 without a selected row. Creation and refresh still apply.
    if code == Some(1)
        && !matches!(
            action,
            ChoiceAction::Rename | ChoiceAction::Refresh | ChoiceAction::CreateWorktree
        )
    {
        return Ok(None);
    }
    let id = fields
        .next()
        .filter(|s| !s.is_empty())
        .and_then(|row| row.split('\t').next())
        .map(str::to_owned);
    Ok(Some(Choice { query, action, id }))
}
// Preserve the incoming terminal configuration across all subprocess exits.
// macOS may independently set PENDIN, a transient kernel input-state flag.
struct TerminalState(std::fs::File, libc::termios);
impl TerminalState {
    fn save() -> Option<Self> {
        use std::os::fd::AsRawFd;
        let terminal = std::fs::File::open("/dev/tty").ok()?;
        let mut saved = std::mem::MaybeUninit::uninit();
        if unsafe { libc::tcgetattr(terminal.as_raw_fd(), saved.as_mut_ptr()) } != 0 {
            return None;
        }
        Some(Self(terminal, unsafe { saved.assume_init() }))
    }
}
impl Drop for TerminalState {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::tcsetattr(self.0.as_raw_fd(), libc::TCSANOW, &self.1);
        }
    }
}

// Keep the real terminal size in sync with fzf's private terminal. Forward
// termination and let fzf restore its display before the keyboard relay stops.
fn wait_fzf(mut child: std::process::Child, mut keyboard: Option<Keyboard>) -> Result<Output> {
    use signal_hook::{
        consts::signal::{SIGCONT, SIGHUP, SIGINT, SIGTERM, SIGTSTP, SIGWINCH},
        iterator::Signals,
    };
    let setup = (|| -> Result<_> {
        let signals = Signals::new([SIGHUP, SIGINT, SIGTERM, SIGWINCH, SIGTSTP, SIGCONT])?;
        let saved = keyboard.as_ref().map(Keyboard::saved_modes);
        let resize = keyboard
            .as_ref()
            .map(Keyboard::resize_handles)
            .transpose()?;
        if let Some(keyboard) = keyboard.as_mut() {
            keyboard.start(child.id())?;
        }
        Ok((signals, resize, saved))
    })();
    let (mut signals, resize, saved) = match setup {
        Ok(setup) => setup,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let handle = signals.handle();
    let pid = child.id() as i32;
    let forwarder = thread::spawn(move || {
        for signal in signals.forever() {
            if signal == SIGTSTP {
                // fzf restores its private PTY before stopping the job. Restore
                // the real terminal too, so the shell retains echo and editing.
                if let (Some((terminal, _)), Some(saved)) = (&resize, &saved) {
                    let _ = set_raw(terminal, saved, false);
                }
                unsafe {
                    libc::kill(pid, SIGTSTP);
                }
                let _ = signal_hook::low_level::emulate_default_handler(SIGTSTP);
                if let (Some((terminal, _)), Some(saved)) = (&resize, &saved) {
                    let _ = set_raw(terminal, saved, true);
                }
                continue;
            }
            if signal == SIGCONT
                && let (Some((terminal, slave)), Some(saved)) = (&resize, &saved)
            {
                let _ = set_raw(terminal, saved, true);
                let _ = copy_size(terminal, slave);
            }
            if signal == SIGWINCH
                && let Some((terminal, slave)) = &resize
            {
                let _ = copy_size(terminal, slave);
            }
            unsafe {
                libc::kill(pid, signal);
            }
        }
    });
    let result = child.wait_with_output();
    handle.close();
    let _ = forwarder.join();
    let relayed = keyboard.as_mut().map(Keyboard::finish).transpose();
    let output = result?;
    relayed?;
    if output.stdout.len() as u64 > LIMIT {
        bail!("fzf returned too much output");
    }
    Ok(output)
}

fn confirm(entry: &Entry, bindings: &Keybindings) -> Result<bool> {
    let question = if entry.source == Source::Rex {
        "Kill this Rex session?"
    } else {
        "Remove this worktree using Worktrunk? This also closes all associated Rex sessions, tabs, panes and agents."
    };
    let mut command = Command::new("fzf");
    clean_fzf_env(&mut command)?;
    command
        .args([
            "--read0",
            "--print0",
            "--no-multi",
            "--no-sort",
            "--layout=reverse",
            "--disabled",
            "--prompt=Confirm > ",
        ])
        .arg(format!(
            "--header={question}\n{}\n{}",
            clean(&entry.name),
            clean(&entry.path.display().to_string())
        ));
    bindings.reset(&mut command, &Keybindings::confirmation_actions());
    for (action, fzf_action) in [
        ("accept", "accept"),
        ("cancel", "abort"),
        ("confirm_yes", "first+accept"),
        ("confirm_no", "abort"),
        ("confirm_up", "up"),
        ("confirm_down", "down"),
    ] {
        bindings.bind(&mut command, action, fzf_action);
    }
    // Confirmation is also rendered by fzf. Cancellation never calls remove.
    let keyboard = Keyboard::prepare(&mut command)?;
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("start fzf confirmation")?;
    child
        .stdin
        .take()
        .context("missing confirmation input")?
        .write_all(b"Yes\0No\0")?;
    let output = wait_fzf(child, keyboard)?;
    match output.status.code() {
        Some(0) => Ok(output.stdout == b"Yes\0"),
        Some(1) | Some(130) => Ok(false),
        code => bail!("fzf confirmation exited unexpectedly ({code:?})"),
    }
}

fn rename_prompt(entry: &Entry, bindings: &Keybindings) -> Result<Option<String>> {
    text_prompt(
        &entry.name,
        "New name > ",
        &format!("Rename Rex session\n{}", clean(entry.target())),
        "save",
        bindings,
    )
}

fn text_prompt(
    initial: &str,
    prompt: &str,
    header: &str,
    accept_label: &str,
    bindings: &Keybindings,
) -> Result<Option<String>> {
    let hints = [("accept", accept_label), ("cancel", "cancel")]
        .iter()
        .filter_map(|(action, label)| bindings.hint(action, label))
        .collect::<Vec<_>>()
        .join(" · ");
    let mut command = Command::new("fzf");
    clean_fzf_env(&mut command)?;
    command
        .args([
            "--read0",
            "--print0",
            "--print-query",
            "--no-multi",
            "--no-sort",
            "--layout=reverse",
            "--disabled",
        ])
        .arg(format!("--prompt={prompt}"))
        .arg(format!("--query={}", clean(initial)))
        .arg(format!("--header={header}\n{hints}"));
    bindings.reset(&mut command, &["accept", "cancel"]);
    bindings.bind(&mut command, "accept", "accept");
    bindings.bind(&mut command, "cancel", "abort");
    let keyboard = Keyboard::prepare(&mut command)?;
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("start fzf text prompt")?;
    child
        .stdin
        .take()
        .context("missing text prompt input")?
        .write_all(b"Confirm\0")?;
    let output = wait_fzf(child, keyboard)?;
    match output.status.code() {
        Some(0) => {
            let text = String::from_utf8(output.stdout).context("fzf returned invalid UTF-8")?;
            let (value, _) = text.split_once('\0').context("fzf omitted input text")?;
            Ok(Some(value.to_owned()))
        }
        Some(1) | Some(130) => Ok(None),
        code => bail!("fzf text prompt exited unexpectedly ({code:?})"),
    }
}

#[derive(Debug)]
pub enum PickOutcome {
    Selected(Entry),
    Created(Entry),
    Removed(crate::session::WorktreeRemoval),
}

pub fn pick(
    fetch: impl Fn() -> Result<Vec<Entry>> + Send + 'static,
    preview: impl Fn(&Entry) -> Result<String> + Send + Sync + 'static,
    remove: impl Fn(&Entry) -> Result<Option<crate::session::WorktreeRemoval>> + Send + Sync + 'static,
    rename: impl Fn(&Entry, &str) -> Result<()> + Send + Sync + 'static,
    initial_focus: impl FnOnce(&[Entry]) -> Option<InitialFocus>,
    options: PickerOptions,
) -> Result<Option<PickOutcome>> {
    pick_with_create(
        fetch,
        preview,
        remove,
        rename,
        |_| bail!("creation is not available in this picker"),
        initial_focus,
        options,
    )
}

pub fn pick_with_create(
    fetch: impl Fn() -> Result<Vec<Entry>> + Send + 'static,
    preview: impl Fn(&Entry) -> Result<String> + Send + Sync + 'static,
    remove: impl Fn(&Entry) -> Result<Option<crate::session::WorktreeRemoval>> + Send + Sync + 'static,
    rename: impl Fn(&Entry, &str) -> Result<()> + Send + Sync + 'static,
    create: impl Fn(&str) -> Result<Entry> + Send + Sync + 'static,
    initial_focus: impl FnOnce(&[Entry]) -> Option<InitialFocus>,
    options: PickerOptions,
) -> Result<Option<PickOutcome>> {
    check_fzf()?;
    let _terminal = TerminalState::save();
    let (entries, status) = match fetch() {
        Ok(entries) => (entries, String::new()),
        Err(error) if options.worktree => {
            (Vec::new(), format!("Could not list worktrees: {error:#}"))
        }
        Err(error) => return Err(error),
    };
    let initial_focus = if options.query.is_empty() && !entries.is_empty() {
        initial_focus(&entries)
    } else {
        None
    };
    let fetch: Fetch = Arc::new(Mutex::new(Box::new(fetch)));
    let mut options = options;
    options.native_icons = options.show_icons && !options.worktree && crate::icons::available();
    let mut query = options.query.clone();
    let mut catalog = Catalog::new(options.clone(), entries);
    catalog.status = status;
    catalog.initial_focus = initial_focus;
    let state = Arc::new(Mutex::new(catalog));
    let bridge = Bridge::start(state.clone(), fetch.clone(), Arc::new(preview))?;
    loop {
        let header = {
            let mut state = state.lock().unwrap();
            state.auto_armed = false;
            state.last_query = query.clone();
            state.header()
        };
        let Some(chosen) = choice(finder(&options, &bridge, &header, &query)?)? else {
            return Ok(None);
        };
        query = chosen.query;
        if chosen.action == ChoiceAction::CreateWorktree && options.worktree {
            if let Some(branch) = text_prompt(
                "",
                "Branch > ",
                "Create worktree",
                "create and connect",
                &options.keybindings,
            )? {
                if branch.trim().is_empty() {
                    state.lock().unwrap().status = "Branch name must not be empty".into();
                    continue;
                }
                match create(&branch) {
                    Ok(entry) => return Ok(Some(PickOutcome::Created(entry))),
                    Err(error) => {
                        // A hook may fail after wt has created the checkout.
                        refresh(&state, &fetch);
                        state.lock().unwrap().status = format!("Create failed: {error:#}");
                    }
                }
            }
            continue;
        }
        if chosen.action == ChoiceAction::Refresh && options.worktree {
            refresh(&state, &fetch);
            continue;
        }
        let Some(id) = chosen.id else {
            if chosen.action == ChoiceAction::Rename {
                continue;
            }
            return Ok(None);
        };
        // Enter may arrive while reload is still showing an earlier query.
        // Revalidate the returned ID against this query before any side effect.
        let current_rows = rows(&state, &query)?;
        let valid = current_rows
            .split(|c| *c == 0)
            .any(|row| row.split(|c| *c == b'\t').next() == Some(id.as_bytes()));
        if !valid {
            return Ok(None);
        }
        let entry = target(&state, Some(&id))?;
        if chosen.action == ChoiceAction::Rename {
            if !entry.is_session() {
                state.lock().unwrap().status = "Only live Rex sessions can be renamed".into();
            } else if let Some(name) = rename_prompt(&entry, &options.keybindings)?
                && name != entry.name
            {
                match rename(&entry, &name) {
                    Ok(()) => refresh(&state, &fetch),
                    Err(error) => {
                        state.lock().unwrap().status = format!("Rename failed: {error:#}");
                    }
                }
            }
            continue;
        }
        if chosen.action == ChoiceAction::Remove {
            if (entry.window_id().is_some() || entry.block_id().is_some())
                || !matches!(entry.source, Source::Rex | Source::Worktree)
            {
                state.lock().unwrap().status = if entry.source == Source::Agent {
                    "Agent rows cannot be removed; select their Rex session to close it"
                } else if entry.window_id().is_some() || entry.block_id().is_some() {
                    "Tab and pane rows cannot be removed; select their Rex session to close it"
                } else {
                    "Configured sessions cannot be removed here; edit rexue.toml"
                }
                .into();
            } else if confirm(&entry, &options.keybindings)? {
                match remove(&entry) {
                    Ok(Some(removal)) => return Ok(Some(PickOutcome::Removed(removal))),
                    Ok(None) => {
                        state.lock().unwrap().removed(&entry);
                        refresh(&state, &fetch);
                    }
                    Err(error) => {
                        state.lock().unwrap().status = format!("Remove failed: {error:#}")
                    }
                }
            }
            continue;
        }
        return Ok(Some(PickOutcome::Selected(entry)));
    }
}

fn normalize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if matches!(c, '-' | '_' | '/' | '\\') {
                ' '
            } else {
                c
            }
        })
        .collect()
}
fn display_icon(
    icon: &str,
    source: Source,
    program: Option<&str>,
    options: &PickerOptions,
) -> String {
    if options.native_icons
        && let Some(icon) = program
            .and_then(|program| crate::icons::icon(program).or_else(|| crate::icons::icon("shell")))
    {
        return icon.placeholder();
    }
    let icon = clean(icon);
    let padding = options
        .icon_width
        .saturating_sub(icon.trim_end_matches(' ').width())
        .min(16);
    let color = options
        .glyph_colors
        .get(source.as_str())
        .and_then(|s| crate::cli::ansi_color(&s.to_ascii_lowercase()));
    let icon = match color {
        Some(color) => format!("\x1b[{color}m{icon}\x1b[39m"),
        None => format!("\x1b[2m{icon}\x1b[22m"),
    };
    format!("{icon}{}", " ".repeat(padding + 2))
}

fn display(entry: &Entry, options: &PickerOptions) -> String {
    if entry.window_id().is_some() && entry.block_id().is_none() {
        return display_tab(&entry.name, options);
    }
    let mut text = String::new();
    if options.show_icons {
        let icon = entry
            .icon
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                options
                    .glyphs
                    .get(entry.source.as_str())
                    .map(String::as_str)
            })
            .unwrap_or(if entry.is_session() {
                ""
            } else {
                entry.source.glyph()
            });
        text.push_str(&display_icon(
            icon,
            entry.source,
            entry
                .program
                .as_deref()
                .or_else(|| entry.agent.as_ref().map(|a| a.app.as_str())),
            options,
        ));
    }
    if options.worktree {
        text.push_str(&format!(
            "{}  \x1b[2m{}\x1b[22m",
            clean(&entry.name),
            clean(&entry.path.display().to_string())
        ));
    } else {
        if let Some(alias) = options.aliases.values().find(|a| a.target == entry.name) {
            text.push_str(&format!("\x1b[2m[{}]\x1b[22m ", clean(&alias.alias)));
        }
        text.push_str(&match &entry.pane_title {
            Some(title) => display_title(title),
            None => clean(&entry.name),
        });
        if let Some(agent) = &entry.agent {
            let status = match agent.state.as_str() {
                "working" => "•••",
                "blocked" => "Blocked",
                "done" => "Done",
                "error" => "Failed",
                _ => "",
            };
            if !status.is_empty() {
                text.push_str(&format!("  \x1b[2m{status}\x1b[22m"));
            }
        }
        if entry.is_session() && entry.tabs.is_empty() {
            for name in &entry.tab_names {
                text.push_str(&format!(" [{}]", display_tab(name, options)));
            }
        }
    }
    text
}
fn display_title(title: &str) -> String {
    let title = clean(title)
        .replace('|', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    match title.split_once(' ') {
        Some((command, detail)) => format!("{command} \x1b[2m{detail}\x1b[22m"),
        None => title,
    }
}

fn display_tab(name: &str, options: &PickerOptions) -> String {
    let name = display_title(name);
    if options.show_icons {
        format!(
            "{}{name}",
            display_icon(Source::Rex.glyph(), Source::Rex, None, options)
        )
    } else {
        name
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personal_fzf_defaults_inherit_only_colors_in_order() {
        assert_eq!(
            fzf_color_args(
                "# theme\n--multi --color 'light,fg+:#123456, bg+:#234567' \\\n                 --bind 'enter:abort' --print-query --color=pointer:red --no-color"
            )
            .unwrap(),
            [
                "--color=light,fg+:#123456, bg+:#234567",
                "--color=pointer:red",
                "--no-color"
            ]
        );
        assert!(fzf_color_args("").unwrap().is_empty());
        assert!(fzf_color_args("--color").is_err());
        assert!(fzf_color_args("--color='unterminated").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn expected_actions_survive_fzf_no_match_exit_status() {
        use std::os::unix::process::ExitStatusExt;
        let result = |code, key| Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: format!("missing\0{key}\0").into_bytes(),
            stderr: Vec::new(),
        };
        for code in [0, 1] {
            for (name, expected) in [
                ("create_worktree", ChoiceAction::CreateWorktree),
                ("refresh", ChoiceAction::Refresh),
                ("rename", ChoiceAction::Rename),
            ] {
                let action = choice(result(code, name)).unwrap().unwrap();
                assert_eq!(action.action, expected);
                assert_eq!(action.query, "missing");
                assert!(action.id.is_none());
            }
        }
        assert!(choice(result(1, "")).unwrap().is_none());
        assert!(choice(result(1, "accept")).unwrap().is_none());
        assert!(choice(result(1, "remove")).unwrap().is_none());
        let selected = Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: b"query\0accept\x0042\trow\0".to_vec(),
            stderr: Vec::new(),
        };
        let selected = choice(selected).unwrap().unwrap();
        assert_eq!(selected.action, ChoiceAction::Accept);
        assert_eq!(selected.id.as_deref(), Some("42"));
        assert!(choice(result(130, "create_worktree")).unwrap().is_none());
        assert!(choice(result(0, "ctrl-q")).is_err());
    }

    #[test]
    fn empty_worktree_catalog_shows_repository_hint_without_selectable_rows() {
        let options = PickerOptions {
            worktree: true,
            ..Default::default()
        };
        let state = Mutex::new(Catalog::new(options, Vec::new()));
        assert!(rows(&state, "").unwrap().is_empty());
        assert!(
            state
                .lock()
                .unwrap()
                .header()
                .contains("Not a Git repository or no worktrees found")
        );
        assert!(
            !Catalog::new(PickerOptions::default(), Vec::new())
                .header()
                .contains("Not a Git repository")
        );
    }

    #[test]
    fn worktree_refresh_failure_empties_rows_and_recovery_clears_hint() {
        let options = PickerOptions {
            worktree: true,
            ..Default::default()
        };
        let entry = Entry::new("main", "/repo", Source::Worktree);
        let state = Mutex::new(Catalog::new(options, vec![entry.clone()]));
        let failure: Fetch = Arc::new(Mutex::new(Box::new(|| bail!("not a git repository"))));
        refresh(&state, &failure);
        assert!(rows(&state, "").unwrap().is_empty());
        let header = state.lock().unwrap().header();
        assert!(header.contains("Not a Git repository or no worktrees found"));
        assert!(header.contains("not a git repository"));

        let success: Fetch = Arc::new(Mutex::new(Box::new(move || Ok(vec![entry.clone()]))));
        refresh(&state, &success);
        assert!(!rows(&state, "").unwrap().is_empty());
        let header = state.lock().unwrap().header();
        assert!(!header.contains("Not a Git repository"));
        assert!(!header.contains("Refresh failed"));
    }

    #[test]
    fn worktree_rows_search_branch_and_path_without_number_assumptions() {
        let options = PickerOptions {
            worktree: true,
            show_icons: false,
            ..Default::default()
        };
        let branch = Entry::new(
            "feature/widgets",
            "/custom checkout/widgets",
            Source::Worktree,
        );
        let detached = Entry::new("(detached abc123)", "/elsewhere/review", Source::Worktree);
        let state = Mutex::new(Catalog::new(options, vec![branch, detached]));
        let text = String::from_utf8(rows(&state, "checkout").unwrap()).unwrap();
        assert!(text.contains("feature/widgets") && text.contains("/custom checkout/widgets"));
        assert!(!text.contains("detached") && !text.contains("#"));
        let detached = String::from_utf8(rows(&state, "review").unwrap()).unwrap();
        assert!(detached.contains("detached abc123"));
    }

    #[test]
    fn rendered_rows_follow_query_refresh_and_removal() {
        let mut entry = Entry::new("alpha", "/project", Source::Rex);
        entry.session_id = Some("session:cached-full".into());
        let state = Mutex::new(Catalog::new(PickerOptions::default(), vec![entry.clone()]));
        let old = rows(&state, "alpha").unwrap();
        assert_eq!(rows(&state, "alpha").unwrap(), old);
        assert!(rows(&state, "beta").unwrap().is_empty());
        assert_eq!(rows(&state, "alpha").unwrap(), old);
        entry.name = "beta".into();
        state.lock().unwrap().replace(vec![entry.clone()]);
        assert!(rows(&state, "alpha").unwrap().is_empty());
        assert!(!rows(&state, "beta").unwrap().is_empty());
        state.lock().unwrap().removed(&entry);
        assert!(rows(&state, "beta").unwrap().is_empty());
    }

    #[test]
    fn late_row_results_cannot_restore_a_replaced_catalog_or_query() {
        let entry = Entry::new("alpha", "/project", Source::Directory);
        let state = Mutex::new(Catalog::new(PickerOptions::default(), vec![entry]));
        let old_revision = state.lock().unwrap().revision;
        let old_rows = rows(&state, "alpha").unwrap();
        {
            let mut catalog = state.lock().unwrap();
            catalog.replace(Vec::new());
            catalog.store_rows(old_revision, "alpha", old_rows.clone());
        }
        assert!(rows(&state, "alpha").unwrap().is_empty());
        let current_revision = state.lock().unwrap().revision;
        rows(&state, "beta").unwrap();
        state
            .lock()
            .unwrap()
            .store_rows(current_revision, "alpha", old_rows);
        assert!(rows(&state, "alpha").unwrap().is_empty());
    }

    #[test]
    fn identity_survives_decorations_duplicates_and_refresh() {
        let name = "同名 '\t\n$(touch /tmp/never)";
        let a = Entry::new(name, "/one", Source::Config);
        let b = Entry::new(name, "/two", Source::Directory);
        let mut s = Catalog::new(PickerOptions::default(), vec![a.clone(), b.clone()]);
        let ids = s.entries.clone();
        assert_ne!(ids[0], ids[1]);
        s.replace(vec![b, a]);
        assert_eq!(s.entries, vec![ids[1], ids[0]]);
        let state = Mutex::new(s);
        let text = String::from_utf8(rows(&state, "").unwrap()).unwrap();
        assert_eq!(text.split('\0').filter(|s| !s.is_empty()).count(), 2);
        assert_eq!(
            target(&state, Some(&ids[1].to_string())).unwrap().path,
            Path::new("/two")
        );
        assert!(target(&state, Some("9999")).is_err());
    }
    #[test]
    fn refresh_retires_targets_without_materializing_configured_aliases() {
        let mut options = PickerOptions::default();
        options.aliases.insert(
            "h".into(),
            Alias {
                alias: "h".into(),
                target: "hidden".into(),
                auto_connect: false,
            },
        );
        let mut keep = Entry::new("keep", "/keep", Source::Rex);
        keep.session_id = Some("session:keep-full".into());
        keep.alias = Some("k".into());
        let state = Mutex::new(Catalog::new(options, vec![keep.clone()]));
        let keep_id = state.lock().unwrap().entries[0];
        let mut retired = None;
        for refresh in 0..32 {
            let current = {
                let mut catalog = state.lock().unwrap();
                let mut temporary =
                    Entry::new(format!("temporary-{refresh}"), "/temporary", Source::Rex);
                temporary.session_id = Some(format!("session:temporary-{refresh}"));
                catalog.replace(vec![keep.clone(), temporary]);
                assert_eq!(catalog.entries[0], keep_id);
                assert!(catalog.candidates("/h").0.is_empty());
                assert_eq!(catalog.candidates("/k").0, [keep_id]);
                assert_eq!(catalog.targets.len(), 2);
                catalog.entries[1]
            };
            if let Some(id) = retired {
                assert!(target(&state, Some(&format!("{id}"))).is_err());
            }
            retired = Some(current);
        }
    }

    #[test]
    fn session_tab_names_render_and_refresh_without_changing_selection() {
        let mut entry: Entry = serde_json::from_value(serde_json::json!({
            "name": "project",
            "source": "rex",
            "session_id": "session:project-full",
            "tab_names": ["Shell", "编辑器", "Shell", "\x1b[31mLogs\x1b[0m\t\n\0"]
        }))
        .unwrap();
        let mut options = PickerOptions {
            show_icons: false,
            ..Default::default()
        };
        options.aliases.insert(
            "p".into(),
            Alias {
                alias: "p".into(),
                target: "project".into(),
                auto_connect: false,
            },
        );
        let state = Mutex::new(Catalog::new(options, vec![entry.clone()]));
        let id = state.lock().unwrap().entries[0];
        let text = String::from_utf8(rows(&state, "").unwrap()).unwrap();
        assert_eq!(
            text,
            format!("{id}\tproject\t\x1b[2m[p]\x1b[22m project [Shell] [编辑器] [Shell] [Logs]\0")
        );
        assert_eq!(
            target(&state, Some(&id.to_string())).unwrap().target(),
            "session:project-full"
        );

        // Tab metadata may change while the session keeps the same picker ID.
        entry = serde_json::from_value(serde_json::json!({
            "name": "project",
            "source": "rex",
            "session_id": "session:project-full",
            "tab_names": ["Renamed"]
        }))
        .unwrap();
        state.lock().unwrap().replace(vec![entry]);
        assert_eq!(state.lock().unwrap().entries, vec![id]);
        let text = String::from_utf8(rows(&state, "").unwrap()).unwrap();
        assert_eq!(
            text,
            format!("{id}\tproject\t\x1b[2m[p]\x1b[22m project [Renamed]\0")
        );
    }

    #[test]
    fn session_tab_icons_are_uniform_and_follow_icon_toggle() {
        let mut entry: Entry = serde_json::from_value(serde_json::json!({
            "name": "project",
            "source": "rex",
            "session_id": "session:project-full",
            "tab_names": [" zsh |  | ~/Projects/rexue  ", "Shell | Agent", "\x1b[31mFix picker\x1b[0m\t\n\0"],
            "agent_tab_indices": [1, 2]
        }))
        .unwrap();
        let mut options = PickerOptions::default();
        assert_eq!(
            strip_ansi_escapes::strip_str(display(&entry, &options)),
            "  project [  zsh ~/Projects/rexue] [  Shell Agent] [  Fix picker]"
        );
        options.show_icons = false;
        assert_eq!(
            display(&entry, &options),
            "project [zsh \x1b[2m~/Projects/rexue\x1b[22m] [Shell \x1b[2mAgent\x1b[22m] [Fix \x1b[2mpicker\x1b[22m]"
        );

        options.show_icons = true;
        let state = Mutex::new(Catalog::new(options, vec![entry.clone()]));
        let id = state.lock().unwrap().entries[0];
        entry = serde_json::from_value(serde_json::json!({
            "name": "project",
            "source": "rex",
            "session_id": "session:project-full",
            "tab_names": ["Shell", "Shell | Agent", "Fix picker"]
        }))
        .unwrap();
        state.lock().unwrap().replace(vec![entry]);
        assert_eq!(state.lock().unwrap().entries, vec![id]);
        let text = String::from_utf8(rows(&state, "").unwrap()).unwrap();
        assert!(
            strip_ansi_escapes::strip_str(&text)
                .contains("project [  Shell] [  Shell Agent] [  Fix picker]")
        );
        assert_eq!(
            target(&state, Some(&id.to_string())).unwrap().target(),
            "session:project-full"
        );
    }

    #[test]
    fn titles_remove_pipes_and_dim_details_after_the_first_word() {
        for (title, expected) in [
            ("", ""),
            (" |  | ", ""),
            (" zsh | | ", "zsh"),
            (
                " zsh |  | ~/Projects/rexue  ",
                "zsh \x1b[2m~/Projects/rexue\x1b[22m",
            ),
            ("pi task title", "pi \x1b[2mtask title\x1b[22m"),
            (
                "\x1b[31m终端\x1b[0m | ~/项目\t\n\0",
                "终端 \x1b[2m~/项目\x1b[22m",
            ),
            ("pi|task||title", "pi \x1b[2mtask title\x1b[22m"),
        ] {
            assert_eq!(display_title(title), expected, "{title:?}");
        }
    }

    #[test]
    fn row_icons_align_wide_glyphs_and_tabs_without_leaking_controls_or_color() {
        let mut options = PickerOptions {
            icon_width: 2,
            ..Default::default()
        };
        options.glyph_colors.insert("rex".into(), "blue".into());
        let mut entry = Entry::new("中文项目", "/project", Source::Rex);
        entry.icon = Some("\x1b[31m界\x1b[0m\t\n".into());
        assert_eq!(display(&entry, &options), "\x1b[34m界\x1b[39m  中文项目");
        assert_eq!(
            display_tab("终端 | ~/项目", &options),
            "\x1b[34m\x1b[39m   终端 \x1b[2m~/项目\x1b[22m"
        );
        options.show_icons = false;
        assert_eq!(display(&entry, &options), "中文项目");
        assert_eq!(
            display_tab("终端 | ~/项目", &options),
            "终端 \x1b[2m~/项目\x1b[22m"
        );
    }

    #[test]
    fn fuzzy_filter_keeps_native_image_placeholder_colors() {
        let mut entry = Entry::new("needle", "/project", Source::Rex);
        entry.program = Some("pi".into());
        let state = Mutex::new(Catalog::new(
            PickerOptions {
                native_icons: true,
                ..Default::default()
            },
            vec![entry],
        ));
        let text = String::from_utf8(rows(&state, "needle").unwrap()).unwrap();
        assert!(text.contains(&crate::icons::icon("pi").unwrap().placeholder()));
        assert!(text.starts_with("0\tneedle\t"));
    }

    #[test]
    fn native_program_icons_use_metadata_and_follow_icon_toggle() {
        let mut options = PickerOptions {
            native_icons: true,
            ..Default::default()
        };
        let mut entry = Entry::new("Claude in a custom tab title", "/project", Source::Rex);
        entry.program = Some("zsh".into());
        entry.window_id = Some("window:tab".into());
        let tab = display(&entry, &options);
        assert_eq!(
            strip_ansi_escapes::strip_str(&tab),
            "  Claude in a custom tab title"
        );
        entry.program = Some("codex".into());
        entry.agent_tab_indices.push(0);
        assert_eq!(display(&entry, &options), tab);
        entry.block_id = Some("block:pane".into());
        entry.program = Some("zsh".into());
        assert!(
            display(&entry, &options)
                .starts_with(&crate::icons::icon("zsh").unwrap().placeholder())
        );
        entry.program = Some("codex".into());
        assert!(
            display(&entry, &options)
                .starts_with(&crate::icons::icon("codex").unwrap().placeholder())
        );
        entry.program = Some("unknown-program".into());
        assert!(
            display(&entry, &options)
                .starts_with(&crate::icons::icon("shell").unwrap().placeholder())
        );
        options.show_icons = false;
        assert_eq!(display(&entry, &options), "Claude in a custom tab title");
    }

    #[test]
    fn pane_rows_show_their_own_title_and_trailing_status_without_search_context() {
        let session = "中文会话名称".repeat(20);
        let mut entry = Entry::new(
            format!("{session} / pi · pane 2 · 相同的长标题"),
            "/project",
            Source::Agent,
        );
        entry.session_id = Some("session:full".into());
        entry.pane_title = Some("相同的 | pane 标题".into());
        entry.agent = Some(crate::model::Agent {
            app: "pi".into(),
            state: "blocked".into(),
            session_name: session.clone(),
            window_id: "window:full".into(),
            block_id: "block:second".into(),
        });
        let options = PickerOptions {
            show_icons: false,
            ..Default::default()
        };
        assert_eq!(
            display(&entry, &options),
            "相同的 \x1b[2mpane 标题\x1b[22m  \x1b[2mBlocked\x1b[22m"
        );
        assert_eq!(entry.target(), "block:second");
        assert!(entry.display_name().starts_with(&session));
    }

    #[test]
    fn sources_render_as_single_rows_with_selection_and_numbering() {
        let options = PickerOptions {
            show_icons: false,
            ..Default::default()
        };
        let first = Entry::new("live first", "/first", Source::Rex);
        let second = Entry::new("live agent", "/second", Source::Agent);
        let project = Entry::new("configured project", "/project", Source::Config);
        let state = Mutex::new(Catalog::new(options, vec![first, second, project]));
        let text = String::from_utf8(rows(&state, "").unwrap()).unwrap();
        let records: Vec<_> = text.split('\0').filter(|row| !row.is_empty()).collect();
        assert_eq!(records.len(), 3);
        let fields: Vec<_> = records
            .iter()
            .map(|row| row.splitn(3, '\t').collect::<Vec<_>>())
            .collect();
        assert_eq!(fields[0][1], "live first");
        assert_eq!(fields[0][2], "live first");
        assert_eq!(fields[1][2], "live agent");
        assert_eq!(fields[2][2], "configured project");
        assert!(!text.contains('\n'));
        for fields in &fields {
            assert_eq!(target(&state, Some(fields[0])).unwrap().name, fields[2]);
        }
        let numbered = String::from_utf8(rows(&state, "#").unwrap()).unwrap();
        assert!(!numbered.contains("──"));
        assert!(numbered.contains("#1 live first"));
        assert!(numbered.contains("#3 configured project"));
    }

    #[test]
    fn agents_have_single_rows_and_distinct_stable_pane_identities() {
        use crate::model::Agent;
        let mut first = Entry::new("project / pi · Agents", "/project", Source::Agent);
        first.session_id = Some("session:project".into());
        first.pane_title = Some("Same | title".into());
        first.agent = Some(Agent {
            app: "pi".into(),
            state: "working".into(),
            session_name: "project".into(),
            window_id: "window:agents".into(),
            block_id: "block:first".into(),
        });
        let mut second = first.clone();
        second.agent.as_mut().unwrap().block_id = "block:second".into();
        let session = Entry::new("project", "/project", Source::Rex);
        let mut catalog = Catalog::new(
            PickerOptions {
                show_icons: false,
                ..Default::default()
            },
            vec![session.clone(), first.clone(), second.clone()],
        );
        let ids = catalog.entries.clone();
        assert_ne!(ids[1], ids[2]);
        first.agent.as_mut().unwrap().state = "blocked".into();
        catalog.replace(vec![session, second, first]);
        assert_eq!(catalog.entries, vec![ids[0], ids[2], ids[1]]);
        let state = Mutex::new(catalog);
        let text = String::from_utf8(rows(&state, "").unwrap()).unwrap();
        assert!(text.contains("\tSame \x1b[2mtitle\x1b[22m  \x1b[2m•••\x1b[22m\0"));
        assert!(!text.contains('\n'));
        assert!(text.starts_with(&format!("{}\tproject\tproject\0", ids[0])));
        let filtered = String::from_utf8(rows(&state, "blocked").unwrap()).unwrap();
        assert_eq!(
            filtered.split('\0').filter(|row| !row.is_empty()).count(),
            1
        );
        assert!(filtered.starts_with(&format!("{}\t", ids[1])));
        assert!(!filtered.contains("──"));
    }

    #[test]
    fn session_tree_keeps_tab_and_pane_order_identity_and_search_context() {
        let session: Entry = serde_json::from_value(serde_json::json!({
            "name": "project", "source": "rex", "session_id": "session:full",
            "tabs": [
                {"name": "Shell | cwd", "window_id": "window:first", "is_agent": false, "panes": [
                    {"name": "project / zsh · pane 1 · Shell", "pane_title": "zsh | cwd", "source": "rex", "session_id": "session:full", "window_id": "window:first", "block_id": "block:shell"}
                ]},
                {"name": "Agents | task", "window_id": "window:second", "is_agent": true, "panes": [
                    {"name": "project / pi · pane 1 · Agents", "pane_title": "Own | task", "source": "agent", "session_id": "session:full", "agent": {"app": "pi", "state": "working", "session_name": "project", "window_id": "window:second", "block_id": "block:agent"}}
                ]}
            ]
        })).unwrap();
        let agent = serde_json::from_value(serde_json::json!({
            "name": "project / pi · pane 1 · Agents", "source": "agent", "session_id": "session:full",
            "agent": {"app": "pi", "state": "working", "session_name": "project", "window_id": "window:second", "block_id": "block:agent"}
        })).unwrap();
        let state = Mutex::new(Catalog::new(
            PickerOptions {
                show_tabs: true,
                ..Default::default()
            },
            vec![agent, session.clone()],
        ));
        let text = String::from_utf8(rows(&state, "").unwrap()).unwrap();
        let fields: Vec<_> = text
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(|s| s.splitn(3, '\t').collect::<Vec<_>>())
            .collect();
        assert_eq!(fields.len(), 5, "{text:?}");
        let visible: Vec<_> = fields
            .iter()
            .map(|row| strip_ansi_escapes::strip_str(row[2]))
            .collect();
        assert_eq!(visible[0], "  project");
        assert_eq!(visible[1], "├──   Shell cwd");
        assert!(visible[2].starts_with("│   └── "));
        assert_eq!(visible[3], "└──   Agents task");
        assert_eq!(visible[4], "    └── 󰚩  Own task  •••");
        assert_eq!(visible[2], "│   └──   zsh cwd");
        for (row, expected) in fields.iter().zip([
            "session:full",
            "window:first",
            "block:shell",
            "window:second",
            "block:agent",
        ]) {
            assert_eq!(target(&state, Some(row[0])).unwrap().target(), expected);
        }
        let ids = state.lock().unwrap().entries.clone();
        let mut renamed = serde_json::to_value(session).unwrap();
        renamed["tabs"][0]["name"] = "Renamed".into();
        state
            .lock()
            .unwrap()
            .replace(vec![serde_json::from_value(renamed).unwrap()]);
        assert_eq!(state.lock().unwrap().entries, ids);
        let filtered = String::from_utf8(rows(&state, "working").unwrap()).unwrap();
        assert_eq!(filtered.split('\0').filter(|s| !s.is_empty()).count(), 1);
        assert!(!filtered.contains("block:agent")); // IDs stay behind the row protocol.
        assert!(filtered.contains("[working]"));
        assert!(filtered.contains("Own | task"));
        let numbered = String::from_utf8(rows(&state, "#").unwrap()).unwrap();
        assert!(strip_ansi_escapes::strip_str(numbered).contains("#4 └──   Agents task"));
    }

    #[test]
    fn default_hidden_tabs_keep_panes_in_order_with_stable_targets_and_tree_connectors() {
        let mut session: Entry = serde_json::from_value(serde_json::json!({
            "name": "project", "source": "rex", "session_id": "session:full",
            "tabs": [
                {"name": "Empty first", "window_id": "window:empty-first"},
                {"name": "Shell", "window_id": "window:first", "panes": [
                    {"name": "project / zsh · pane 1 · Shell", "pane_title": "Shell", "source": "rex", "session_id": "session:full", "window_id": "window:first", "block_id": "block:shell"}
                ]},
                {"name": "Empty middle", "window_id": "window:empty-middle"},
                {"name": "Agents", "window_id": "window:second", "panes": [
                    {"name": "project / pi · pane 1 · Agents", "pane_title": "Same title", "source": "agent", "session_id": "session:full", "agent": {"app": "pi", "state": "working", "session_name": "project", "window_id": "window:second", "block_id": "block:first"}},
                    {"name": "project / pi · pane 2 · Agents", "pane_title": "Same title", "source": "agent", "session_id": "session:full", "agent": {"app": "pi", "state": "blocked", "session_name": "project", "window_id": "window:second", "block_id": "block:second"}}
                ]},
                {"name": "Empty last", "window_id": "window:empty-last"}
            ]
        })).unwrap();
        let agent = session.tabs[3].panes[0].clone();
        let state = Mutex::new(Catalog::new(
            PickerOptions {
                show_icons: false,
                ..Default::default()
            },
            vec![agent, session.clone()],
        ));
        let text = String::from_utf8(rows(&state, "").unwrap()).unwrap();
        let fields: Vec<_> = text
            .split('\0')
            .filter(|row| !row.is_empty())
            .map(|row| row.splitn(3, '\t').collect::<Vec<_>>())
            .collect();
        let visible: Vec<_> = fields
            .iter()
            .map(|row| strip_ansi_escapes::strip_str(row[2]))
            .collect();
        assert_eq!(
            visible,
            [
                "project",
                "├── Shell",
                "├── Same title  •••",
                "└── Same title  Blocked"
            ]
        );
        for (row, (expected, window)) in fields.iter().zip([
            ("session:full", None),
            ("block:shell", Some("window:first")),
            ("block:first", Some("window:second")),
            ("block:second", Some("window:second")),
        ]) {
            let selected = target(&state, Some(row[0])).unwrap();
            assert_eq!(selected.target(), expected);
            assert_eq!(selected.window_id(), window);
        }
        let ids = state.lock().unwrap().entries.clone();
        session.tabs.retain(|tab| !tab.panes.is_empty());
        session.tabs[1].name = "Renamed".into();
        state.lock().unwrap().replace(vec![session.clone()]);
        assert_eq!(state.lock().unwrap().entries, ids);
        let filtered = String::from_utf8(rows(&state, "blocked").unwrap()).unwrap();
        assert_eq!(
            filtered.split('\0').filter(|row| !row.is_empty()).count(),
            1
        );
        assert!(filtered.starts_with(&format!("{}\t", ids[3])));
        session.tabs.clear();
        state.lock().unwrap().replace(vec![session]);
        assert_eq!(state.lock().unwrap().entries, [ids[0]]);
        assert!(target(&state, Some(&ids[3].to_string())).is_err());
    }

    #[test]
    fn aliases_follow_live_sessions_and_retire_with_them() {
        let mut options = PickerOptions {
            alias_filter_prefix: "#".into(),
            ..Default::default()
        };
        let mut entry = Entry::new("project", "/tmp", Source::Rex);
        entry.session_id = Some("session:live-full".into());
        entry.alias = Some("Ä".into());
        entry.alias_auto_connect = true;
        options.aliases.insert(
            "Ä".into(),
            Alias {
                alias: "Ä".into(),
                target: "not running".into(),
                auto_connect: true,
            },
        );
        let mut catalog = Catalog::new(options, vec![entry.clone()]);
        assert!(catalog.index_query("#ä").is_none());
        let (ids, _, fuzzy) = catalog.candidates("#ä");
        assert!(!fuzzy);
        assert_eq!(catalog.targets[&ids[0]].target(), "session:live-full");
        assert_eq!(catalog.candidates("ä").0, ids);
        assert!(!catalog.auto_armed);
        entry.name = "renamed project".into();
        catalog.replace(vec![entry]);
        assert_eq!(catalog.candidates("#ä").0, ids);
        catalog.replace(vec![]);
        assert!(catalog.candidates("#ä").0.is_empty());
        assert!(catalog.candidates("ä").0.is_empty());
        assert!(catalog.targets.is_empty());
    }
    #[test]
    fn removing_duplicate_targets_retires_alias_only_after_last_row() {
        let mut opts = PickerOptions::default();
        opts.aliases.insert(
            "a".into(),
            Alias {
                alias: "a".into(),
                target: "same".into(),
                auto_connect: true,
            },
        );
        let mut first = Entry::new("same", "/tmp", Source::Rex);
        first.session_id = Some("session:first".into());
        let mut second = first.clone();
        second.session_id = Some("session:second".into());
        let mut state = Catalog::new(opts, vec![first.clone(), second.clone()]);
        state.removed(&first);
        state.replace(vec![second.clone()]);
        let alias_id = state.candidates("/a").0[0];
        assert_eq!(state.targets[&alias_id].target(), "session:second");
        state.removed(&second);
        state.replace(vec![]);
        assert!(state.candidates("/a").0.is_empty());
        assert!(state.candidates("a").0.is_empty());
        assert!(!state.options.aliases.contains_key("a"));
        assert!(state.targets.is_empty());
    }
}
