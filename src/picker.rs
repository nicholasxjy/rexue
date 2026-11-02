//! System-fzf picker. Display strings never serve as connection identities.
use crate::keybindings::Keybindings;
use crate::keyboard::{Keyboard, copy_size, set_raw};
use crate::model::{Entry, Source, alias_key};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
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
pub struct PickerOptions {
    pub show_icons: bool,
    pub separator_aware: bool,
    pub prompt: String,
    pub placeholder: String,
    pub pointer: String,
    pub query: String,
    pub aliases: BTreeMap<String, Alias>,
    pub alias_icons: BTreeMap<String, String>,
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
    pub group_separator: bool,
    pub keybindings: Keybindings,
    pub worktree: bool,
    pub auto_refresh: bool,
}
impl Default for PickerOptions {
    fn default() -> Self {
        Self {
            show_icons: true,
            separator_aware: false,
            prompt: "> ".into(),
            placeholder: "Filter sessions...".into(),
            pointer: "> ".into(),
            query: String::new(),
            aliases: BTreeMap::new(),
            alias_icons: BTreeMap::new(),
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
            group_separator: false,
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
    next_id: u64,
    last_query: String,
    auto_armed: bool,
    status: String,
}
fn same_entry(a: &Entry, b: &Entry) -> bool {
    if a.source == Source::Agent && b.source == Source::Agent {
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
            next_id: 0,
            auto_armed: false,
            status: String::new(),
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
        if self.options.worktree {
            entries.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));
        } else {
            for entry in &entries {
                if let Some(alias) = &entry.alias {
                    self.options
                        .aliases
                        .entry(alias_key(alias))
                        .or_insert(Alias {
                            alias: alias.clone(),
                            target: entry.name.clone(),
                            auto_connect: entry.alias_auto_connect,
                        });
                }
            }
        }
        self.entries = entries.into_iter().map(|e| self.remember(e)).collect();
        // Alias-only mode can select configured targets excluded by source flags.
        for alias in self.options.aliases.values().cloned().collect::<Vec<_>>() {
            if !self
                .entries
                .iter()
                .any(|id| self.targets[id].name == alias.target)
            {
                let mut entry = Entry::new(&alias.target, "", Source::Config);
                entry.alias = Some(alias.alias);
                entry.alias_auto_connect = alias.auto_connect;
                entry.icon = self.options.alias_icons.get(&alias.target).cloned();
                self.remember(entry);
            }
        }
    }
    fn removed(&mut self, entry: &Entry) {
        self.entries
            .retain(|id| !same_entry(&self.targets[id], entry));
        self.targets.retain(|_, target| !same_entry(target, entry));
        // Preserve an alias while another current row has the same name, then
        // retire it with the last row. Do not resurrect a removed live ID or a
        // synthetic configured target through alias-only mode after deletion.
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
            .or_else(|| {
                self.targets.iter().find_map(|(id, e)| {
                    (e.source == Source::Config && e.session_id.is_none() && e.name == alias.target)
                        .then_some(*id)
                })
            })
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
                            "status" => Ok(state.lock().unwrap().header().into_bytes()),
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
                                        s.auto_armed && !s.options.worktree && !s.options.disable_alias_auto_connect
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
    let (candidates, pattern, fuzzy, options) = {
        let mut s = state.lock().unwrap();
        if query != s.last_query {
            s.auto_armed = true;
            s.last_query = query.into();
        }
        let (ids, query, fuzzy) = s.candidates(query);
        (
            ids.into_iter()
                .map(|id| (id, s.targets[&id].clone()))
                .collect::<Vec<_>>(),
            query,
            fuzzy,
            s.options.clone(),
        )
    };
    let mut input = Vec::new();
    let mut previous_group = None;
    let mut previous_source = None;
    for (id, entry) in &candidates {
        let search = if options.worktree || entry.source == Source::Agent {
            format!(
                "{} {}",
                clean(&entry.display_name()),
                clean(&entry.path.display().to_string())
            )
        } else {
            clean(&entry.name)
        };
        let search = if options.separator_aware {
            normalize(&search)
        } else {
            search
        };
        let mut display = display(entry, &options);
        if query.is_empty()
            && previous_group != Some(entry.group)
            && (options.group_separator
                || entry.source == Source::Agent
                || previous_source == Some(Source::Agent))
        {
            // fzf --read0 displays newlines within one selectable record.
            let label = if entry.source == Source::Agent {
                "Agents"
            } else {
                entry.source.as_str()
            };
            display = format!("── {label} ──\n{display}");
        }
        previous_group = Some(entry.group);
        previous_source = Some(entry.source);
        write!(input, "{id}\t{search}\t{display}\0")?;
    }
    // Use fzf itself for fuzzy matching and ordering. Keeping the visible finder
    // disabled lets alias-only and index modes share exactly the same query box.
    // --filter is a short-lived, noninteractive fzf process, with no terminal use.
    let mut matched = if fuzzy && !pattern.is_empty() {
        let pattern = if options.separator_aware {
            normalize(&pattern)
        } else {
            pattern
        };
        let mut command = Command::new("fzf");
        clean_fzf_env(&mut command);
        let output = pipe_output(
            command.args([
                "--read0",
                "--print0",
                "--ansi",
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
    if !options.worktree && query.starts_with('#') && (options.alias_filter_prefix != "#") {
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
    Ok(matched)
}

fn clean_fzf_env(command: &mut Command) {
    // Protocol and callback integrity cannot depend on personal fzf defaults.
    command
        .env_remove("FZF_DEFAULT_OPTS")
        .env_remove("FZF_DEFAULT_OPTS_FILE")
        .env_remove("FZF_DEFAULT_COMMAND")
        .env_remove("FZF_API_KEY")
        .env_remove("FZF_FILTER")
        .env("SHELL", "/bin/sh");
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
    clean_fzf_env(&mut command);
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
        ])
        .arg(format!("--prompt={}", clean(&options.prompt)))
        .arg(format!("--ghost={}", clean(&options.placeholder)))
        .arg(format!("--pointer={}", clean(&options.pointer)))
        .arg(format!("--query={query}"))
        .arg(format!("--header={header}"))
        .arg(format!("--bind=start:reload({HELPER} rows)"))
        .arg(format!("--bind=change:bg-cancel+reload({HELPER} rows)"))
        .arg(format!("--bind=load:bg-transform({HELPER} loaded)"));
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
    wait_fzf(child, keyboard)
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
    } else if entry.source == Source::Worktree {
        "Remove this worktree using Worktrunk? Its Rex session will remain."
    } else {
        "Remove this directory from history?"
    };
    let mut command = Command::new("fzf");
    clean_fzf_env(&mut command);
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
    clean_fzf_env(&mut command);
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
}

pub fn pick(
    fetch: impl Fn() -> Result<Vec<Entry>> + Send + 'static,
    preview: impl Fn(&Entry) -> Result<String> + Send + Sync + 'static,
    remove: impl Fn(&Entry) -> Result<()> + Send + Sync + 'static,
    rename: impl Fn(&Entry, &str) -> Result<()> + Send + Sync + 'static,
    options: PickerOptions,
) -> Result<Option<Entry>> {
    Ok(pick_with_create(
        fetch,
        preview,
        remove,
        rename,
        |_| bail!("creation is not available in this picker"),
        options,
    )?
    .map(|outcome| match outcome {
        PickOutcome::Selected(entry) | PickOutcome::Created(entry) => entry,
    }))
}

pub fn pick_with_create(
    fetch: impl Fn() -> Result<Vec<Entry>> + Send + 'static,
    preview: impl Fn(&Entry) -> Result<String> + Send + Sync + 'static,
    remove: impl Fn(&Entry) -> Result<()> + Send + Sync + 'static,
    rename: impl Fn(&Entry, &str) -> Result<()> + Send + Sync + 'static,
    create: impl Fn(&str) -> Result<Entry> + Send + Sync + 'static,
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
    let fetch: Fetch = Arc::new(Mutex::new(Box::new(fetch)));
    let mut query = options.query.clone();
    let mut catalog = Catalog::new(options.clone(), entries);
    catalog.status = status;
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
            if entry.source != Source::Rex {
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
            if !matches!(
                entry.source,
                Source::Rex | Source::Zoxide | Source::Directory | Source::Worktree
            ) {
                state.lock().unwrap().status = if entry.source == Source::Agent {
                    "Agent rows cannot be removed; select their Rex session to close it"
                } else {
                    "Configured sessions cannot be removed here; edit rexue.toml"
                }
                .into();
            } else if confirm(&entry, &options.keybindings)? {
                match remove(&entry) {
                    Ok(()) => {
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

fn clean(text: &str) -> String {
    strip_ansi_escapes::strip_str(text)
        .chars()
        .filter(|c| !c.is_control())
        .collect()
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
fn display(entry: &Entry, options: &PickerOptions) -> String {
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
            .unwrap_or(entry.source.glyph());
        let icon = clean(icon);
        let padding = options
            .icon_width
            .saturating_sub(icon.trim_end_matches(' ').width())
            .min(16);
        let color = options
            .glyph_colors
            .get(entry.source.as_str())
            .and_then(|s| crate::cli::ansi_color(&s.to_ascii_lowercase()));
        if let Some(color) = color {
            text.push_str(&format!("\x1b[{color}m{icon}\x1b[0m"));
        } else {
            text.push_str(&icon);
        }
        text.push_str(&" ".repeat(padding + 1));
    }
    if options.worktree {
        text.push_str(&format!(
            "{}  {}",
            clean(&entry.name),
            clean(&entry.path.display().to_string())
        ));
    } else {
        if let Some(alias) = options.aliases.values().find(|a| a.target == entry.name) {
            text.push_str(&format!("[{}] ", clean(&alias.alias)));
        }
        text.push_str(&clean(&entry.display_name()));
    }
    text
}
#[cfg(test)]
mod tests {
    use super::*;
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
    fn group_separators_render_on_separate_lines() {
        let options = PickerOptions {
            group_separator: true,
            show_icons: false,
            ..Default::default()
        };
        let first = Entry::new("live first", "/first", Source::Rex);
        let second = Entry::new("live second", "/second", Source::Rex);
        let mut project = Entry::new("configured project", "/project", Source::Config);
        project.group = 1;
        let state = Mutex::new(Catalog::new(options, vec![first, second, project]));
        let text = String::from_utf8(rows(&state, "").unwrap()).unwrap();
        let records: Vec<_> = text.split('\0').filter(|row| !row.is_empty()).collect();
        // Titles occupy a display line, never a selectable record or search field.
        assert_eq!(records.len(), 3);
        let fields: Vec<_> = records
            .iter()
            .map(|row| row.splitn(3, '\t').collect::<Vec<_>>())
            .collect();
        assert_eq!(fields[0][1], "live first");
        assert_eq!(fields[0][2], "── rex ──\nlive first");
        assert_eq!(fields[1][2], "live second");
        assert_eq!(fields[2][2], "── config ──\nconfigured project");
        for fields in &fields {
            assert_eq!(target(&state, Some(fields[0])).unwrap().name, fields[1]);
        }
        let numbered = String::from_utf8(rows(&state, "#").unwrap()).unwrap();
        assert!(!numbered.contains("──"));
        assert!(numbered.contains("#1 live first"));
        assert!(numbered.contains("#3 configured project"));
    }

    #[test]
    fn agents_have_default_group_and_distinct_stable_pane_identities() {
        use crate::model::Agent;
        let mut first = Entry::new("pi · project / Agents", "/project", Source::Agent);
        first.session_id = Some("session:project".into());
        first.agent = Some(Agent {
            app: "pi".into(),
            state: "working".into(),
            session_name: "project".into(),
            window_id: "window:agents".into(),
            block_id: "block:first".into(),
        });
        let mut second = first.clone();
        second.agent.as_mut().unwrap().block_id = "block:second".into();
        first.group = 1;
        second.group = 1;
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
        assert!(text.contains("── Agents ──\npi · project / Agents [working]"));
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
    fn aliases_keep_unicode_folding_hidden_targets_and_prefix_precedence() {
        let mut opts = PickerOptions::default();
        opts.aliases.insert(
            "Ä".into(),
            Alias {
                alias: "Ä".into(),
                target: "hidden".into(),
                auto_connect: true,
            },
        );
        opts.alias_filter_prefix = "#".into();
        let s = Catalog::new(opts, vec![Entry::new("other", "/tmp", Source::Directory)]);
        assert!(s.index_query("#ä").is_none());
        let (ids, _, fuzzy) = s.candidates("#ä");
        assert!(!fuzzy);
        assert_eq!(s.targets[&ids[0]].name, "hidden");
        assert_eq!(s.candidates("ä").0, ids);
        assert!(!s.auto_armed);
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
