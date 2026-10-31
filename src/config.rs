//! Sesh-compatible configuration for native Rex sessions.
use crate::model::{Entry, Source};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer};
use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub session: Vec<SessionConfig>,
    pub window: Vec<WindowConfig>,
    pub wildcard: Vec<WildcardConfig>,
    pub worktree: Vec<WorktreeConfig>,
    pub tui: TuiConfig,
    pub default_session: SessionConfig,
    pub blacklist: Vec<String>,
    pub sort_order: Vec<SortGroup>,
    pub dir_length: usize,
    pub git_dir_length: usize,
    pub git_namer_use_worktree_root: bool,
    pub separator_aware: bool,
    pub vcs: String,
    pub name_substitution: Vec<NameSubstitution>,
    pub frecency: FrecencyConfig,
    pub cache: bool,
    pub strict_mode: bool,
    pub terminal: String,
    #[serde(rename = "import")]
    pub import_paths: Vec<String>,
    pub icons: BTreeMap<String, SourceIcon>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            session: Vec::new(),
            window: Vec::new(),
            wildcard: Vec::new(),
            worktree: Vec::new(),
            tui: TuiConfig::default(),
            default_session: SessionConfig::default(),
            blacklist: Vec::new(),
            sort_order: Vec::new(),
            dir_length: 1,
            git_dir_length: 1,
            git_namer_use_worktree_root: false,
            separator_aware: false,
            vcs: "git".into(),
            name_substitution: Vec::new(),
            frecency: FrecencyConfig::default(),
            cache: false,
            strict_mode: false,
            terminal: String::new(),
            import_paths: Vec::new(),
            icons: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SourceIcon {
    pub icon: String,
    pub color: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NameSubstitution {
    pub find: String,
    pub replace: String,
    pub regex: bool,
}

impl NameSubstitution {
    pub fn apply(&self, input: &str) -> Result<String> {
        if self.regex {
            Ok(regex::Regex::new(&self.find)
                .context("invalid name_substitution regex")?
                .replace_all(input, self.replace.as_str())
                .into_owned())
        } else {
            Ok(input.replace(&self.find, &self.replace))
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FrecencyConfig {
    pub list_command: Option<String>,
    pub query_command: Option<String>,
    pub add_command: Option<String>,
    pub remove_command: Option<String>,
}

/// A source name, or sources interleaved by score in one group.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum SortGroup {
    Source(String),
    Merged(Vec<String>),
}

#[derive(Debug, Clone, Default)]
pub struct SessionConfig {
    pub name: String,
    pub path: String,
    pub startup_command: Option<String>,
    pub startup_script: Option<String>,
    pub preview_command: Option<String>,
    pub disable_startup_command: bool,
    pub windows: Vec<String>,
    pub icon: Option<String>,
    pub alias: Option<String>,
    pub alias_auto_connect: bool,
    // Raw TOML values are retained for deserialization and public-field edits.
    #[doc(hidden)]
    pub raw: Option<RawSession>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[doc(hidden)]
pub struct RawSession {
    name: Option<String>,
    path: Option<String>,
    startup_command: Option<String>,
    startup_script: Option<String>,
    preview_command: Option<String>,
    disable_startup_command: Option<bool>,
    windows: Option<Vec<String>>,
    icon: Option<String>,
    alias: Option<String>,
    alias_auto_connect: Option<bool>,
}
impl RawSession {
    fn resolved(self) -> SessionConfig {
        SessionConfig {
            name: self.name.unwrap_or_default(),
            path: self.path.unwrap_or_default(),
            startup_command: self.startup_command,
            startup_script: self.startup_script,
            preview_command: self.preview_command,
            disable_startup_command: self.disable_startup_command.unwrap_or_default(),
            windows: self.windows.unwrap_or_default(),
            icon: self.icon,
            alias: self.alias,
            alias_auto_connect: self.alias_auto_connect.unwrap_or_default(),
            raw: None,
        }
    }
}
impl SessionConfig {
    fn overrides(&self) -> RawSession {
        // Public fields can be edited after deserialization. Use their current
        // values, retaining the original presence of false/empty fields.
        let mut raw = self.raw.clone().unwrap_or_default();
        if raw.name.is_some() || !self.name.is_empty() {
            raw.name = Some(self.name.clone());
        }
        if raw.path.is_some() || !self.path.is_empty() {
            raw.path = Some(self.path.clone());
        }
        raw.startup_command = self.startup_command.clone();
        raw.startup_script = self.startup_script.clone();
        raw.preview_command = self.preview_command.clone();
        raw.icon = self.icon.clone();
        raw.alias = self.alias.clone();
        if raw.disable_startup_command.is_some() || self.disable_startup_command {
            raw.disable_startup_command = Some(self.disable_startup_command);
        }
        if raw.windows.is_some() || !self.windows.is_empty() {
            raw.windows = Some(self.windows.clone());
        }
        if raw.alias_auto_connect.is_some() || self.alias_auto_connect {
            raw.alias_auto_connect = Some(self.alias_auto_connect);
        }
        raw
    }
}
impl<'de> Deserialize<'de> for SessionConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = RawSession::deserialize(deserializer)?;
        let mut session = raw.clone().resolved();
        session.raw = Some(raw);
        Ok(session)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    pub name: String,
    pub path: Option<String>,
    pub startup_command: Option<String>,
    pub startup_script: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WildcardConfig {
    pub pattern: String,
    pub startup_command: Option<String>,
    pub startup_script: Option<String>,
    pub preview_command: Option<String>,
    pub disable_startup_command: Option<bool>,
    pub windows: Option<Vec<String>>,
    pub icon: Option<String>,
}
fn nonempty(value: Option<&String>) -> Option<&String> {
    value.filter(|value| !value.is_empty())
}

/// Repository registration for the global picker. Worktrunk owns lifecycle config.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorktreeConfig {
    pub path: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TuiConfig {
    pub show_icons: bool,
    pub show_windows: bool,
    /// Native window labels; ordinary tmux window fields are mapped at load time.
    pub window_name_format: String,
    pub prompt: String,
    pub placeholder: String,
    pub pointer: String,
    pub alias_auto_connect_delay: String,
    pub alias_filter_prefix: String,
    pub preview: bool,
    pub preview_width: u16,
    pub preview_min_width: usize,
    pub preview_border: String,
    pub group_separator: bool,
}
impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            show_icons: true,
            show_windows: false,
            window_name_format: "{window_name}".into(),
            prompt: "> ".into(),
            placeholder: "Filter Sessions...".into(),
            pointer: "> ".into(),
            alias_auto_connect_delay: "150ms".into(),
            alias_filter_prefix: "/".into(),
            preview: false,
            preview_width: 60,
            preview_min_width: 100,
            preview_border: "line".into(),
            group_separator: false,
        }
    }
}

impl TuiConfig {
    fn normalize(&mut self) -> Result<()> {
        let defaults = Self::default();
        for (value, fallback) in [
            (&mut self.prompt, defaults.prompt),
            (&mut self.placeholder, defaults.placeholder),
            (
                &mut self.alias_auto_connect_delay,
                defaults.alias_auto_connect_delay,
            ),
        ] {
            if value.is_empty() {
                *value = fallback;
            }
        }
        if self.pointer.is_empty() || self.pointer.width() > 2 {
            self.pointer = defaults.pointer;
        }
        self.preview_width = match self.preview_width {
            0 => 60,
            width => width.clamp(10, 90),
        };
        if self.preview_min_width == 0 {
            self.preview_min_width = 100;
        }
        if !["none", "line", "thick", "double"].contains(&self.preview_border.as_str()) {
            self.preview_border = defaults.preview_border;
        }
        self.window_name_format = normalize_window_name_format(&self.window_name_format)?;
        Ok(())
    }
}

fn normalize_window_name_format(format: &str) -> Result<String> {
    if format.is_empty() {
        return Ok("{window_name}".into());
    }
    let mut normalized = String::new();
    let mut remaining = format;
    while let Some(index) = remaining.find('#') {
        normalized.push_str(&remaining[..index]);
        remaining = &remaining[index..];
        if remaining.starts_with("##") {
            normalized.push('#');
            remaining = &remaining[2..];
            if let Some(tail) = remaining.strip_prefix('{')
                && let Some(end) = tail.find('}')
            {
                normalized.push_str("{{");
                normalized.push_str(&tail[..end]);
                normalized.push_str("}}");
                remaining = &tail[end + 1..];
            }
            continue;
        }
        let mapped = [
            ("#{session_name}", "{session_name}"),
            ("#S", "{session_name}"),
            ("#{window_name}", "{window_name}"),
            ("#W", "{window_name}"),
            ("#{window_index}", "{window_index}"),
            ("#I", "{window_index}"),
            ("#{window_active}", "{window_active}"),
        ]
        .into_iter()
        .find(|(tmux, _)| remaining.starts_with(tmux));
        if let Some((tmux, native)) = mapped {
            normalized.push_str(native);
            remaining = &remaining[tmux.len()..];
        } else {
            bail!(
                "tui.window_name_format contains unsupported tmux syntax at {remaining:?}; supported fields are #{{window_name}}/#W, #{{window_index}}/#I, and #{{window_active}} (native: {{window_name}}, {{window_index}}, {{window_active}}); tmux expressions and commands cannot be evaluated by Rex"
            );
        }
    }
    normalized.push_str(remaining);
    Ok(normalized)
}

/// Parse Go time.ParseDuration syntax for alias auto-connect. Empty uses the
/// sesh default (150ms); valid negative durations connect immediately.
/// Invalid syntax or values outside Go's signed 64-bit nanosecond range fail.
pub fn parse_alias_delay(input: &str) -> Result<Duration> {
    let invalid = || {
        anyhow::anyhow!(
            "invalid alias_auto_connect_delay {input:?}: expected a Go duration (e.g. 150ms, +1s, -1ms, or 0)"
        )
    };
    if input.is_empty() {
        return Ok(Duration::from_millis(150));
    }
    let (negative, mut remaining) = if let Some(rest) = input.strip_prefix('-') {
        (true, rest)
    } else {
        (false, input.strip_prefix('+').unwrap_or(input))
    };
    if remaining == "0" {
        return Ok(Duration::ZERO);
    }
    if remaining.is_empty() {
        return Err(invalid());
    }
    let limit = (i64::MAX as u128) + u128::from(negative);
    let mut total = 0u128;
    while !remaining.is_empty() {
        let whole_len = remaining.bytes().take_while(u8::is_ascii_digit).count();
        let whole = if whole_len == 0 {
            0
        } else {
            remaining[..whole_len]
                .parse::<u128>()
                .map_err(|_| invalid())?
        };
        remaining = &remaining[whole_len..];
        let mut fraction = 0u128;
        let mut scale = 1u128;
        let mut fraction_len = 0;
        if let Some(rest) = remaining.strip_prefix('.') {
            fraction_len = rest.bytes().take_while(u8::is_ascii_digit).count();
            // Further digits cannot affect nanosecond precision, even for hours.
            for digit in rest.bytes().take(fraction_len.min(19)) {
                fraction = fraction * 10 + u128::from(digit - b'0');
                scale *= 10;
            }
            remaining = &rest[fraction_len..];
        }
        if whole_len == 0 && fraction_len == 0 {
            return Err(invalid());
        }
        let (unit, nanos) = [
            ("ns", 1u128),
            ("us", 1_000),
            ("µs", 1_000),
            ("μs", 1_000),
            ("ms", 1_000_000),
            ("s", 1_000_000_000),
            ("m", 60_000_000_000),
            ("h", 3_600_000_000_000),
        ]
        .into_iter()
        .find(|(unit, _)| remaining.starts_with(unit))
        .ok_or_else(invalid)?;
        remaining = &remaining[unit.len()..];
        let component = whole
            .checked_mul(nanos)
            .and_then(|whole| whole.checked_add(fraction * nanos / scale))
            .ok_or_else(invalid)?;
        total = total
            .checked_add(component)
            .filter(|n| *n <= limit)
            .ok_or_else(invalid)?;
    }
    Ok(if negative {
        Duration::ZERO
    } else {
        Duration::from_nanos(total as u64)
    })
}

// Normalize signed Go integer settings before deserializing into Rust's
// unsigned public fields. Preserve non-integer values so type errors still fail.
fn normalize_config_value(value: &mut toml::Value) {
    for key in ["dir_length", "git_dir_length"] {
        if let Some(raw) = value.get_mut(key)
            && raw.as_integer().is_some_and(|n| n <= 0)
        {
            *raw = toml::Value::Integer(1);
        }
    }
    if let Some(table) = value.get_mut("tui").and_then(toml::Value::as_table_mut) {
        for (key, default) in [("preview_width", 60), ("preview_min_width", 100)] {
            if let Some(raw) = table.get_mut(key)
                && let Some(number) = raw.as_integer()
            {
                *raw = toml::Value::Integer(if number <= 0 {
                    default
                } else if key == "preview_width" {
                    number.clamp(10, 90)
                } else {
                    number
                });
            }
        }
    }
}

fn reject_retired_worktree_keys(value: &toml::Value) -> Result<()> {
    if value.get("browser").is_some() {
        bail!(
            "[browser] was removed with the issue/PR worktree workflow; remove it from rexue config. Use generic branches and Worktrunk configuration instead."
        );
    }
    if let Some(rows) = value.get("worktree").and_then(toml::Value::as_array) {
        for row in rows {
            for key in [
                "repo",
                "worktree_dir",
                "branch_template",
                "base_branch",
                "fetch",
                "create_command",
                "startup_command",
            ] {
                if row.get(key).is_some() {
                    bail!(
                        "[[worktree]].{key} was removed; keep only path/icon in rexue. Move path templates, defaults and hooks to Worktrunk config (worktrunk.dev/config/); use session startup settings for Rex commands."
                    );
                }
            }
        }
    }
    Ok(())
}

impl Config {
    pub fn load(explicit: Option<&Path>) -> Result<Self> {
        let path = match explicit {
            Some(path) => path.to_owned(),
            None => {
                let root = env::var_os("XDG_CONFIG_HOME")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
                    .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
                    .context("cannot determine config directory: set XDG_CONFIG_HOME or HOME")?;
                root.join("rexue/rexue.toml")
            }
        };
        Self::load_path(&path, explicit.is_some())
    }

    fn load_path(path: &Path, explicit: bool) -> Result<Self> {
        Self::load_path_from_cwd(path, explicit, &env::current_dir()?)
    }

    fn load_path_from_cwd(path: &Path, explicit: bool, cwd: &Path) -> Result<Self> {
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            cwd.join(path)
        };
        let path = path.as_path();
        match fs::metadata(path) {
            Ok(_) => {}
            Err(error) if !explicit && error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("could not read config {}", path.display()));
            }
        }
        let value = read_config_value(path, cwd)?;
        Self::from_value(value).with_context(|| format!("invalid config {}", path.display()))
    }

    #[cfg(test)]
    fn parse(text: &str) -> Result<Self> {
        Self::from_value(toml::from_str(text).context("invalid TOML")?)
    }

    fn from_value(mut value: toml::Value) -> Result<Self> {
        migrate_source_names(&mut value)?;
        reject_retired_worktree_keys(&value)?;
        reject_tmux_keys(&value, "")?;
        let strict = value
            .get("strict_mode")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false);
        check_unknown_keys(&mut value, "", strict)?;
        normalize_config_value(&mut value);
        let mut config: Self = value
            .try_into()
            .context("unknown or invalid configuration key")?;
        config.tui.normalize()?;
        config.validate()?;
        Ok(config)
    }

    pub fn resolve(&self, path: &Path, name: Option<&str>) -> Result<SessionConfig> {
        let path = absolute_path(path)?;
        let mut explicit =
            name.and_then(|name| self.session.iter().find(|session| session.name == name));
        if explicit.is_none() {
            for session in &self.session {
                let candidate = expand_path(&session.path).with_context(|| {
                    format!("invalid path for configured session {:?}", session.name)
                })?;
                if crate::session::same_path(&candidate, &path) {
                    explicit = Some(session);
                    break;
                }
            }
        }
        let mut wildcard = None;
        for candidate in &self.wildcard {
            if matches_pattern(&candidate.pattern, &path)? {
                wildcard = Some(candidate);
                break;
            }
        }
        let defaults = &self.default_session;
        let mut resolved = explicit
            .map(|s| s.overrides().resolved())
            .unwrap_or_default();
        // These fields use zero-value fallback, not TOML key presence. Only
        // the first matching wildcard participates (lister/format.go).
        resolved.preview_command = nonempty(resolved.preview_command.as_ref())
            .or_else(|| wildcard.and_then(|w| nonempty(w.preview_command.as_ref())))
            .or_else(|| nonempty(defaults.preview_command.as_ref()))
            .cloned();
        if resolved.windows.is_empty() {
            resolved.windows = wildcard
                .and_then(|w| w.windows.as_ref().filter(|v| !v.is_empty()))
                .unwrap_or(&defaults.windows)
                .clone();
        }
        resolved.icon = nonempty(resolved.icon.as_ref())
            .or_else(|| wildcard.and_then(|w| nonempty(w.icon.as_ref())))
            .cloned();
        // Aliases and auto-connect belong to an explicit session. Defaults
        // have no corresponding fields in the pinned source model.
        resolved.alias = nonempty(resolved.alias.as_ref()).cloned();
        // Startup is a strategy chain, not an independent overlay of command
        // and script: a lower-priority script must never shadow a command.
        // startup_script on a session/default/wildcard is a Rex extension;
        // keep it paired with its layer, with script before command there.
        let session_startup = explicit.filter(|s| {
            nonempty(s.startup_script.as_ref()).is_some()
                || nonempty(s.startup_command.as_ref()).is_some()
        });
        let wildcard_disabled = wildcard.is_some_and(|w| w.disable_startup_command == Some(true));
        let wildcard_startup = wildcard.filter(|w| {
            !wildcard_disabled
                && (nonempty(w.startup_script.as_ref()).is_some()
                    || nonempty(w.startup_command.as_ref()).is_some())
        });
        let session_disabled = explicit.is_some_and(|s| s.disable_startup_command);
        // A wildcard-created session carries its disable flag into the
        // default strategy; a configured session carries its own flag.
        let default_disabled = explicit
            .map(|_| session_disabled)
            .unwrap_or(wildcard_disabled)
            || defaults.disable_startup_command;
        let (command, script) = if let Some(s) = session_startup {
            (&s.startup_command, &s.startup_script)
        } else if let Some(w) = wildcard_startup {
            (&w.startup_command, &w.startup_script)
        } else if !default_disabled {
            (&defaults.startup_command, &defaults.startup_script)
        } else {
            (&None, &None)
        };
        resolved.startup_command = nonempty(command.as_ref()).cloned();
        resolved.startup_script = nonempty(script.as_ref()).cloned();
        resolved.disable_startup_command = session_disabled
            || (session_startup.is_none() && wildcard_disabled)
            || (session_startup.is_none()
                && wildcard_startup.is_none()
                && defaults.disable_startup_command);
        resolved.path = path.to_string_lossy().into_owned();
        if resolved.name.is_empty() {
            resolved.name = name.map(str::to_owned).unwrap_or_else(|| {
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            });
        }
        Ok(resolved)
    }

    pub fn entries(&self) -> Result<Vec<Entry>> {
        self.session
            .iter()
            .map(|session| {
                let path = expand_path(&session.path).with_context(|| {
                    format!("invalid path for configured session {:?}", session.name)
                })?;
                let resolved = self.resolve(&path, Some(&session.name)).with_context(|| {
                    format!("could not resolve configured session {:?}", session.name)
                })?;
                let mut entry = Entry::new(resolved.name, path, Source::Config);
                entry.window_names = resolved.windows.clone();
                entry.icon = resolved.icon;
                entry.alias = resolved.alias;
                entry.alias_auto_connect = resolved.alias_auto_connect;
                Ok(entry)
            })
            .collect()
    }

    pub fn validate(&self) -> Result<()> {
        if !["git", "jj"].contains(&self.vcs.as_str()) {
            bail!("vcs must be git or jj");
        }
        for rule in &self.name_substitution {
            if rule.find.is_empty() {
                bail!("name_substitution find must not be empty");
            }
            if rule.regex {
                regex::Regex::new(&rule.find)
                    .with_context(|| format!("invalid name_substitution regex {:?}", rule.find))?;
            }
        }
        for (source, icon) in &self.icons {
            if ![
                "rex",
                "rex-block",
                "config",
                "zoxide",
                "directory",
                "worktree",
            ]
            .contains(&source.as_str())
            {
                bail!(
                    "unknown icons source {source:?}; use rex, config, zoxide, directory, or worktree"
                );
            }
            let color = icon.color.to_lowercase();
            let base = color.strip_prefix("bright-").unwrap_or(&color);
            if !color.is_empty()
                && color != "gray"
                && color != "grey"
                && ![
                    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
                ]
                .contains(&base)
            {
                bail!("icons.{source}.color: unknown color {:?}", icon.color);
            }
            if icon.icon.chars().any(char::is_control) {
                bail!("icons.{source}.icon contains control characters");
            }
        }
        let mut names = HashSet::new();
        let mut aliases = HashSet::new();
        let mut windows = HashSet::new();
        for window in &self.window {
            if window.name.trim().is_empty() {
                bail!("window name must not be empty");
            }
            if !windows.insert(window.name.as_str()) {
                bail!("duplicate window name {:?}", window.name);
            }
            if let Some(path) = &window.path
                && !path.is_empty()
            {
                expand_path(path).context("invalid window path")?;
            }
        }
        for session in &self.session {
            if session.name.trim().is_empty() {
                bail!("session name must not be empty");
            }
            if !names.insert(session.name.as_str()) {
                bail!("duplicate session name {:?}", session.name);
            }
            expand_path(&session.path)
                .with_context(|| format!("invalid path for session {:?}", session.name))?;
            if let Some(alias) = &session.alias
                && !alias.is_empty()
                && !aliases.insert(crate::model::alias_key(alias))
            {
                bail!("duplicate alias {alias:?}");
            }
        }
        for session in std::iter::once(&self.default_session).chain(&self.session) {
            validate_windows(&session.windows, &windows)?;
        }
        for wildcard in &self.wildcard {
            let pattern = expand_path(&wildcard.pattern).context("invalid wildcard pattern")?;
            if !pattern.to_string_lossy().ends_with("/**") {
                parse_go_path_pattern(&pattern.to_string_lossy())?;
            }
            if let Some(ref list) = wildcard.windows {
                validate_windows(list, &windows)?;
            }
        }
        let mut repos = HashSet::new();
        for worktree in &self.worktree {
            if worktree.path.trim().is_empty() {
                bail!("worktree path must not be empty");
            }
            let path = expand_path(&worktree.path).context("invalid worktree path")?;
            let path = path.canonicalize().unwrap_or(path);
            if !repos.insert(path) {
                bail!("duplicate worktree path {:?}", worktree.path);
            }
        }
        let tui = &self.tui;
        parse_alias_delay(&tui.alias_auto_connect_delay)?;
        if !tui.alias_filter_prefix.is_empty()
            && (tui.alias_filter_prefix.chars().count() != 1
                || tui.alias_filter_prefix.chars().any(char::is_whitespace))
        {
            bail!("alias_filter_prefix must be empty or one non-whitespace character");
        }
        // Window formats are compiled once at load; do not reinterpret literal text.
        for group in &self.sort_order {
            let sources: &[String] = match group {
                SortGroup::Source(source) => std::slice::from_ref(source),
                SortGroup::Merged(sources) => sources,
            };
            if sources.is_empty() {
                bail!("sort_order groups must not be empty");
            }
            for source in sources {
                if ![
                    "rex",
                    "rex-block",
                    "config",
                    "zoxide",
                    "directory",
                    "worktree",
                ]
                .contains(&source.as_str())
                {
                    bail!(
                        "unknown sort_order source {source:?}; use rex, config, zoxide, directory, or worktree"
                    );
                }
            }
        }
        Ok(())
    }
}

// Pinned sesh resolveImports appends only these three lists, once, in
// declaration order. Imported defaults, settings and imports have no effect.
fn merge_import(target: &mut toml::Value, mut imported: toml::Value) {
    for key in ["session", "window", "wildcard"] {
        if let Some(value) = imported.as_table_mut().and_then(|table| table.remove(key)) {
            let target = target.as_table_mut().expect("config is a table");
            if let Some(existing) = target.get_mut(key).and_then(toml::Value::as_array_mut) {
                existing.extend(
                    value
                        .as_array()
                        .expect("validated import list")
                        .iter()
                        .cloned(),
                );
            } else {
                target.insert(key.into(), value);
            }
        }
    }
}

// Match os.ExpandEnv before resolving tilde, as fullImportPath does upstream.
// Missing environment variables expand to empty strings. No glob expansion.
fn import_path(import: &str, cwd: &Path) -> Result<PathBuf> {
    let mut expanded = String::new();
    let mut chars = import.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            expanded.push(ch);
            continue;
        }
        let mut name = String::new();
        match chars.peek().copied() {
            Some('{') => {
                chars.next();
                // os.ExpandEnv consumes only "${" when no closing brace exists.
                if !chars.clone().any(|ch| ch == '}') {
                    continue;
                }
                for ch in chars.by_ref() {
                    if ch == '}' {
                        break;
                    }
                    name.push(ch);
                }
            }
            Some(ch) if "*#$@!?-0123456789".contains(ch) => {
                name.push(chars.next().unwrap());
            }
            Some(ch) if ch.is_ascii_alphabetic() || ch == '_' => {
                while let Some(ch) = chars.peek().copied() {
                    if !ch.is_ascii_alphanumeric() && ch != '_' {
                        break;
                    }
                    name.push(chars.next().unwrap());
                }
            }
            _ => {
                expanded.push('$');
                continue;
            }
        }
        expanded.push_str(&env::var(&name).unwrap_or_default());
    }
    if let Some(suffix) = expanded.strip_prefix('~') {
        let home = dirs::home_dir().context("cannot determine home directory for import")?;
        // Go filepath.Join retains home even when the suffix starts with '/'.
        return Ok(home.join(suffix.trim_start_matches('/')));
    }
    let path = PathBuf::from(expanded);
    Ok(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

fn read_config_value(path: &Path, cwd: &Path) -> Result<toml::Value> {
    fn read(path: &Path, imported: bool) -> Result<toml::Value> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("could not read config {}", path.display()))?;
        let mut value: toml::Value =
            toml::from_str(&text).with_context(|| format!("invalid TOML in {}", path.display()))?;
        migrate_source_names(&mut value)?;
        if !imported {
            reject_retired_worktree_keys(&value)
                .with_context(|| format!("in config {}", path.display()))?;
        }
        reject_tmux_keys(&value, "").with_context(|| format!("in config {}", path.display()))?;
        let strict = !imported
            && value
                .get("strict_mode")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);
        check_unknown_keys(&mut value, "", strict)
            .with_context(|| format!("in config {}", path.display()))?;
        // Like upstream Unmarshal, decode the entire file for type errors,
        // but apply semantic validation only after the selected lists merge.
        normalize_config_value(&mut value);
        let _: Config = value
            .clone()
            .try_into()
            .with_context(|| format!("invalid configuration in {}", path.display()))?;
        Ok(value)
    }

    let mut value = read(path, false)?;
    let imports = value
        .get("import")
        .map(|v| v.clone().try_into::<Vec<String>>())
        .transpose()
        .context("import must be a list of paths")?
        .unwrap_or_default();
    for import in imports {
        let imported_path = import_path(&import, cwd)?;
        let imported = read(&imported_path, true)
            .with_context(|| format!("import {import:?} from {}", path.display()))?;
        merge_import(&mut value, imported);
    }
    Ok(value)
}

/// Unknown fields are diagnosed even in permissive mode. Type errors are
/// always fatal; strict_mode promotes unknown-field diagnostics to errors.
fn check_unknown_keys(value: &mut toml::Value, kind: &str, strict: bool) -> Result<()> {
    if let toml::Value::Array(values) = value {
        for value in values {
            check_unknown_keys(value, kind, strict)?;
        }
        return Ok(());
    }
    let Some(table) = value.as_table_mut() else {
        return Ok(());
    };
    let known: &[&str] = match kind {
        "" => &[
            "session",
            "window",
            "wildcard",
            "worktree",
            "tui",
            "default_session",
            "blacklist",
            "sort_order",
            "dir_length",
            "git_dir_length",
            "git_namer_use_worktree_root",
            "separator_aware",
            "vcs",
            "name_substitution",
            "frecency",
            "cache",
            "strict_mode",
            "terminal",
            "import",
            "icons",
        ],
        "session" | "default_session" => &[
            "name",
            "path",
            "startup_command",
            "startup_script",
            "preview_command",
            "disable_startup_command",
            "windows",
            "icon",
            "alias",
            "alias_auto_connect",
        ],
        "window" => &["name", "path", "startup_command", "startup_script"],
        "wildcard" => &[
            "pattern",
            "startup_command",
            "startup_script",
            "preview_command",
            "disable_startup_command",
            "windows",
            "icon",
        ],
        "worktree" => &["path", "icon"],
        "tui" => &[
            "show_icons",
            "show_windows",
            "window_name_format",
            "prompt",
            "placeholder",
            "pointer",
            "alias_auto_connect_delay",
            "alias_filter_prefix",
            "preview",
            "preview_width",
            "preview_min_width",
            "preview_border",
            "group_separator",
        ],
        "name_substitution" => &["find", "replace", "regex"],
        "frecency" => &[
            "list_command",
            "query_command",
            "add_command",
            "remove_command",
        ],
        "source_icon" => &["icon", "color"],
        "icons" => {
            for (_, value) in table.iter_mut() {
                check_unknown_keys(value, "source_icon", strict)?;
            }
            return Ok(());
        }
        _ => return Ok(()),
    };
    let unknown: Vec<_> = table
        .keys()
        .filter(|key| !known.contains(&key.as_str()))
        .cloned()
        .collect();
    for key in unknown {
        let full = if kind.is_empty() {
            key.clone()
        } else {
            format!("{kind}.{key}")
        };
        if strict {
            bail!("unknown field {full:?} (strict_mode = true)");
        }
        eprintln!(
            "rexue: unknown field {full:?}; ignored (set strict_mode = true to reject unknown fields)"
        );
        table.remove(&key);
    }
    for (key, value) in table {
        check_unknown_keys(value, key, strict)?;
    }
    Ok(())
}

fn validate_windows(list: &[String], known: &HashSet<&str>) -> Result<()> {
    for name in list {
        if !known.contains(name.as_str()) {
            bail!("unknown window {name:?}; define a matching [[window]]");
        }
    }
    Ok(())
}

/// Expand only tilde and environment references; never execute shell syntax.
/// Relative paths are made absolute without requiring the target to exist.
pub fn expand_path(input: &str) -> Result<PathBuf> {
    if input.trim().is_empty() {
        bail!("path must not be empty");
    }
    if input.starts_with('~') && input != "~" && !input.starts_with("~/") {
        bail!("named-user tilde expansion is unsupported: {input:?}");
    }
    let expanded =
        shellexpand::full(input).with_context(|| format!("could not expand path {input:?}"))?;
    absolute_path(Path::new(expanded.as_ref()))
}

/// Normalize resolved path data without interpreting ~ or environment variables.
pub fn absolute_path(path: &Path) -> Result<PathBuf> {
    let path = path.to_path_buf();
    if path.as_os_str().is_empty() {
        bail!("resolved path must not be empty");
    }
    let path = if path.is_absolute() {
        path
    } else {
        env::current_dir()?.join(path)
    };
    // Lexical cleanup also makes path-based config matching independent of ./.
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                cleaned.pop();
            }
            other => cleaned.push(other.as_os_str()),
        }
    }
    Ok(cleaned)
}

fn matches_pattern(pattern: &str, path: &Path) -> Result<bool> {
    let pattern = expand_path(pattern)?.to_string_lossy().into_owned();
    let candidate = path.to_string_lossy();
    // lister/config_wildcard.go treats every trailing /** as a literal
    // descendant prefix, even if that prefix contains wildcard characters.
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return Ok(candidate
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/') && suffix.len() > 1));
    }
    go_path_match(&pattern, &candidate)
}

#[derive(Debug)]
enum PathToken {
    Star,
    Any,
    Literal(char),
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
}

// Go path.Match uses ^ (not !) for class negation, backslash escapes,
// and non-recursive stars. Parse the entire pattern before matching so
// malformed suffixes are rejected even when an earlier term cannot match.
fn parse_go_path_pattern(pattern: &str) -> Result<Vec<PathToken>> {
    fn class_char(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<char> {
        match chars.next()? {
            '\\' => chars.next(),
            '-' | ']' => None,
            c => Some(c),
        }
    }
    let bad = || anyhow::anyhow!("invalid wildcard pattern {pattern:?}");
    let mut chars = pattern.chars().peekable();
    let mut tokens = Vec::new();
    while let Some(c) = chars.next() {
        tokens.push(match c {
            '*' => {
                if matches!(tokens.last(), Some(PathToken::Star)) {
                    continue;
                }
                PathToken::Star
            }
            '?' => PathToken::Any,
            '\\' => PathToken::Literal(chars.next().ok_or_else(bad)?),
            '[' => {
                let negated = chars.next_if_eq(&'^').is_some();
                let mut ranges = Vec::new();
                loop {
                    if chars.peek() == Some(&']') && !ranges.is_empty() {
                        chars.next();
                        break;
                    }
                    let lo = class_char(&mut chars).ok_or_else(bad)?;
                    let hi = if chars.next_if_eq(&'-').is_some() {
                        class_char(&mut chars).ok_or_else(bad)?
                    } else {
                        lo
                    };
                    ranges.push((lo, hi));
                }
                PathToken::Class { negated, ranges }
            }
            c => PathToken::Literal(c),
        });
    }
    Ok(tokens)
}

fn go_path_match(pattern: &str, name: &str) -> Result<bool> {
    // Match byte positions: Go's star scans bytes, while ? and classes
    // decode runes (a continuation-byte position decodes as RuneError).
    fn rune(bytes: &[u8]) -> (char, usize) {
        let width = match bytes[0] {
            0..=0x7f => 1,
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => 1,
        };
        if let Some(head) = bytes.get(..width)
            && let Ok(text) = std::str::from_utf8(head)
        {
            return (text.chars().next().expect("nonempty rune"), width);
        }
        ('\u{fffd}', 1)
    }
    let tokens = parse_go_path_pattern(pattern)?;
    let bytes = name.as_bytes();
    let mut matched = vec![false; bytes.len() + 1];
    matched[0] = true;
    for token in tokens {
        let mut next = vec![false; bytes.len() + 1];
        if matches!(token, PathToken::Star) {
            next[0] = matched[0];
            for i in 0..bytes.len() {
                next[i + 1] = matched[i + 1] || (next[i] && bytes[i] != b'/');
            }
        } else {
            for i in 0..bytes.len() {
                if !matched[i] {
                    continue;
                }
                match &token {
                    PathToken::Literal(c) => {
                        let mut buffer = [0; 4];
                        let literal = c.encode_utf8(&mut buffer).as_bytes();
                        if bytes[i..].starts_with(literal) {
                            next[i + literal.len()] = true;
                        }
                    }
                    PathToken::Any => {
                        if bytes[i] != b'/' {
                            let (_, width) = rune(&bytes[i..]);
                            next[i + width] = true;
                        }
                    }
                    PathToken::Class { negated, ranges } => {
                        let (c, width) = rune(&bytes[i..]);
                        if ranges.iter().any(|&(lo, hi)| lo <= c && c <= hi) != *negated {
                            next[i + width] = true;
                        }
                    }
                    PathToken::Star => unreachable!(),
                }
            }
        }
        matched = next;
    }
    Ok(matched[bytes.len()])
}

fn migrate_source_names(value: &mut toml::Value) -> Result<()> {
    if let Some(table) = value.get_mut("icons").and_then(toml::Value::as_table_mut)
        && let Some(icon) = table.remove("tmux")
    {
        eprintln!("rexue: icons.tmux migrated to icons.rex");
        table.entry("rex").or_insert(icon);
    }
    if let Some(table) = value.get_mut("icons").and_then(toml::Value::as_table_mut) {
        for legacy in ["tmux-pane", "rex-pane"] {
            if let Some(icon) = table.remove(legacy) {
                eprintln!("rexue: icons.{legacy} migrated to icons.rex-block");
                table.entry("rex-block").or_insert(icon);
            }
        }
    }
    fn migrate(value: &mut toml::Value) {
        match value {
            toml::Value::String(source) if source == "tmux" => {
                eprintln!("rexue: sort_order source tmux migrated to rex");
                *source = "rex".into();
            }
            toml::Value::String(source) if source == "tmux-pane" || source == "rex-pane" => {
                *source = "rex-block".into();
            }
            toml::Value::Array(values) => {
                for value in values {
                    migrate(value);
                }
            }
            _ => {}
        }
    }
    if let Some(order) = value.get_mut("sort_order") {
        migrate(order);
    }
    Ok(())
}

fn reject_tmux_keys(value: &toml::Value, prefix: &str) -> Result<()> {
    match value {
        toml::Value::Table(table) => {
            for (key, value) in table {
                let full = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                if key.starts_with("tmux") {
                    bail!(
                        "{full} is a tmux-specific setting unsupported by Rex; migrate to [[window]] and startup_command/startup_script"
                    );
                }
                reject_tmux_keys(value, &full)?;
            }
        }
        toml::Value::Array(values) => {
            for value in values {
                reject_tmux_keys(value, prefix)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_worktree_keys_require_migration_even_in_permissive_mode() {
        for strict in [false, true] {
            for setting in [
                "repo='owner/repo'",
                "worktree_dir=''",
                "branch_template=''",
                "base_branch=''",
                "fetch=false",
                "create_command='true'",
                "startup_command='true'",
            ] {
                let error = Config::parse(&format!(
                    "strict_mode={strict}\n[[worktree]]\npath='/repo'\n{setting}"
                ))
                .unwrap_err()
                .to_string();
                assert!(
                    error.contains("was removed") && error.contains("Worktrunk"),
                    "{error}"
                );
            }
            assert!(
                Config::parse(&format!(
                    "strict_mode={strict}\n[browser]\napplication='Safari'"
                ))
                .unwrap_err()
                .to_string()
                .contains("[browser] was removed")
            );
        }
        assert!(Config::parse("[[worktree]]\npath='/repo'\nicon='W'").is_ok());
        assert!(
            Config::parse("[[worktree]]\npath='/repo'\n[[worktree]]\npath='/repo'")
                .unwrap_err()
                .to_string()
                .contains("duplicate worktree path")
        );
    }

    #[test]
    fn file_loader_rejects_retired_keys_before_permissive_unknown_key_filtering() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.toml");
        for strict in [false, true] {
            for body in [
                "[[worktree]]\npath='/repo'\nrepo='owner/repo'",
                "[[worktree]]\npath='/repo'\nworktree_dir='.wk'",
                "[browser]\napplication='Safari'",
            ] {
                fs::write(&path, format!("strict_mode={strict}\n{body}\n")).unwrap();
                let error = format!("{:#}", Config::load(Some(&path)).unwrap_err());
                assert!(error.contains("was removed"), "{error}");
            }
        }
    }

    #[test]
    fn defaults_and_worktree_defaults() {
        let config = Config::parse("").unwrap();
        assert!(
            config.session.is_empty()
                && config.window.is_empty()
                && config.wildcard.is_empty()
                && config.worktree.is_empty()
        );
        assert!(config.blacklist.is_empty() && config.sort_order.is_empty());
        assert_eq!(config.tui.alias_auto_connect_delay, "150ms");
        assert_eq!(config.tui.preview_width, 60);
        let config = Config::parse("[[worktree]]\npath='/repo'").unwrap();
        assert_eq!(config.worktree[0].path, "/repo");
        assert!(config.worktree[0].icon.is_none());
    }

    #[test]
    fn precedence_uses_nonempty_fields_and_first_wildcard() {
        let config = Config::parse(
            r#"
[default_session]
startup_command = "default"
preview_command = "default preview"
disable_startup_command = true
windows = ["editor"]
icon = "default"
[[window]]
name = "editor"
[[wildcard]]
pattern = "/projects/**"
startup_command = "first"
preview_command = "first preview"
icon = "first"
[[wildcard]]
pattern = "/projects/**"
startup_command = "second"
[[session]]
name = "app"
path = "/projects/app"
startup_command = ""
disable_startup_command = false
windows = []
icon = ""
"#,
        )
        .unwrap();
        let app = config.resolve(Path::new("/projects/app"), None).unwrap();
        assert_eq!(app.startup_command.as_deref(), Some("first"));
        assert!(!app.disable_startup_command);
        assert_eq!(app.windows, ["editor"]);
        assert_eq!(app.icon.as_deref(), Some("first"));
        assert_eq!(app.preview_command.as_deref(), Some("first preview"));
        let other = config.resolve(Path::new("/projects/other"), None).unwrap();
        assert_eq!(other.startup_command.as_deref(), Some("first"));
        assert!(!other.disable_startup_command);
        assert_eq!(other.windows, ["editor"]);
        let outside = config.resolve(Path::new("/elsewhere"), None).unwrap();
        assert!(outside.startup_command.is_none());
    }

    #[test]
    fn wildcard_false_does_not_clear_default_disable_extension() {
        let config = Config::parse(
            r#"
[default_session]
disable_startup_command = true
startup_command = "default"
[[wildcard]]
pattern = "/projects/**"
disable_startup_command = false
startup_command = ""
windows = []
"#,
        )
        .unwrap();
        let session = config.resolve(Path::new("/projects/app"), None).unwrap();
        assert!(session.disable_startup_command);
        assert!(session.startup_command.is_none());
    }

    #[test]
    fn startup_strategies_match_pinned_source_guards() {
        // startup/{config,config_wildcard,defaultconfig}.go: disable is a
        // strategy guard, not a blanket veto over an earlier command.
        for (session, wildcard, expected) in [
            (
                "startup_command='session'",
                "startup_command='wildcard'\ndisable_startup_command=true",
                Some("session"),
            ),
            (
                "startup_command='session'\ndisable_startup_command=true",
                "",
                Some("session"),
            ),
            (
                "startup_command=''",
                "startup_command='wildcard'",
                Some("wildcard"),
            ),
            (
                "disable_startup_command=false",
                "startup_command='wildcard'\ndisable_startup_command=true",
                Some("default"),
            ),
            (
                "disable_startup_command=true",
                "startup_command='wildcard'",
                Some("wildcard"),
            ),
            ("disable_startup_command=true", "startup_command=''", None),
            ("", "startup_command=''", Some("default")),
        ] {
            let config = Config::parse(&format!(
                "[default_session]\nstartup_command='default'\n[[wildcard]]\npattern='/projects/**'\n{wildcard}\n[[session]]\nname='app'\npath='/projects/app'\n{session}\n"
            )).unwrap();
            let resolved = config.resolve(Path::new("/projects/app"), None).unwrap();
            assert_eq!(
                resolved.startup_command.as_deref(),
                expected,
                "session={session}; wildcard={wildcard}"
            );
        }
        for disabled in [false, true] {
            let config = Config::parse(&format!(
                "[default_session]\nstartup_command='default'\n[[wildcard]]\npattern='/projects/**'\ndisable_startup_command={disabled}\nstartup_command=''\n"
            )).unwrap();
            assert_eq!(
                config
                    .resolve(Path::new("/projects/app"), None)
                    .unwrap()
                    .startup_command
                    .as_deref(),
                if disabled { None } else { Some("default") }
            );
        }
    }

    #[test]
    fn startup_extensions_stay_with_the_selected_layer_and_aliases_are_local() {
        for (session, wildcard, command, script) in [
            (
                "startup_command='session'",
                "startup_script='wildcard script'",
                Some("session"),
                None,
            ),
            (
                "startup_script='session script'",
                "startup_command='wildcard'",
                None,
                Some("session script"),
            ),
            (
                "startup_command=''\nstartup_script=''",
                "startup_command='wildcard'",
                Some("wildcard"),
                None,
            ),
            (
                "",
                "startup_command=''\nstartup_script=''",
                Some("default"),
                Some("default script"),
            ),
        ] {
            let config = Config::parse(&format!(
                "[default_session]\nstartup_command='default'\nstartup_script='default script'\nalias='default alias'\nalias_auto_connect=true\nicon='default icon'\n[[wildcard]]\npattern='/projects/**'\n{wildcard}\n[[session]]\nname='app'\npath='/projects/app'\nalias=''\nalias_auto_connect=false\n{session}\n"
            )).unwrap();
            let resolved = config.resolve(Path::new("/projects/app"), None).unwrap();
            assert_eq!(resolved.startup_command.as_deref(), command);
            assert_eq!(resolved.startup_script.as_deref(), script);
            assert!(resolved.alias.is_none());
            assert!(!resolved.alias_auto_connect);
            assert!(resolved.icon.is_none());
            let unconfigured = config.resolve(Path::new("/outside"), None).unwrap();
            assert!(unconfigured.alias.is_none());
            assert!(!unconfigured.alias_auto_connect);
        }
    }

    #[test]
    fn wildcard_go_character_groups_control_all_inherited_fields() {
        for (pattern, matched) in [("/projects/rexu[^x]", true), ("/projects/rexu[!x]", false)] {
            let cfg=Config::parse(&format!("[default_session]\nstartup_command='default-start'\npreview_command='default-preview'\nwindows=['default']\n[[window]]\nname='default'\n[[window]]\nname='wild'\n[[wildcard]]\npattern='{pattern}'\nstartup_command='wild-start'\npreview_command='wild-preview'\nwindows=['wild']\nicon='W'\n[[session]]\nname='app'\npath='/projects/rexue'\n")).unwrap();
            let resolved = cfg
                .resolve(Path::new("/projects/rexue"), Some("app"))
                .unwrap();
            assert_eq!(
                resolved.icon.as_deref(),
                matched.then_some("W"),
                "{pattern}"
            );
            assert_eq!(
                resolved.startup_command.as_deref(),
                Some(if matched {
                    "wild-start"
                } else {
                    "default-start"
                }),
                "{pattern}"
            );
            assert_eq!(
                resolved.preview_command.as_deref(),
                Some(if matched {
                    "wild-preview"
                } else {
                    "default-preview"
                }),
                "{pattern}"
            );
            assert_eq!(
                resolved.windows,
                vec![if matched { "wild" } else { "default" }],
                "{pattern}"
            );
            assert_eq!(
                cfg.entries().unwrap()[0].icon.as_deref(),
                matched.then_some("W")
            );
        }
    }

    #[test]
    fn wildcard_go_syntax_and_unicode_match_reference() {
        for (pattern, path, expected) in [
            ("/p/[^x]", "/p/r", true),
            ("/p/[^x]", "/p/x", false),
            ("/p/[!x]", "/p/r", false),
            ("/p/[!x]", "/p/!", true),
            ("/p/[!x]", "/p/x", true),
            ("/p/[a-c]", "/p/b", true),
            ("/p/[z-a]", "/p/r", false),
            ("/p/?", "/p/日", true),
            ("/p/[日-語]", "/p/本", true),
            (r"/p/\*", "/p/*", true),
            (r"/p/[\-]", "/p/-", true),
            ("/p[a/]q", "/p/q", true),
            ("/p[^a]q", "/p/q", true),
            ("/p/*/src", "/p/a/b/src", false),
            ("/p/**/src", "/p/a/b/src", false),
            ("/p/**/src", "/p/src", false),
            ("/p/**/src", "/p/a/src", true),
            ("/p/*", "/p/.hidden", true),
            ("/p/?", "/p/ab", false),
        ] {
            assert_eq!(
                matches_pattern(pattern, Path::new(path)).unwrap(),
                expected,
                "{pattern:?} {path:?}"
            );
        }
        for (pattern, name, expected) in [
            ("", "", true),
            ("", "/", false),
            ("*", "", true),
            ("**", "a/b", false),
            ("??", "日", false),
            ("*[�]", "ä", true),
            ("?[�]", "ä", false),
            (r"[\]]", "]", true),
            (r"[\^]", "^", true),
            ("[^z-a]", "z", true),
        ] {
            assert_eq!(
                go_path_match(pattern, name).unwrap(),
                expected,
                "{pattern:?} {name:?}"
            );
        }
    }

    #[test]
    fn wildcard_go_recursive_suffix_uses_literal_prefix() {
        for (pattern, path, expected) in [
            ("/p/*/**", "/p/app/deep", false),
            ("/p/*/**", "/p/*/deep", true),
            ("/p/**", "/p", false),
            ("/p/**", "/p/a/b", true),
        ] {
            assert_eq!(
                matches_pattern(pattern, Path::new(path)).unwrap(),
                expected,
                "{pattern} {path}"
            );
        }
    }

    #[test]
    fn glob_depth_and_directory_boundaries() {
        for (pattern, path, expected) in [
            ("/projects/*", "/projects/app", true),
            ("/projects/*", "/projects/org/app", false),
            ("/projects/**", "/projects/org/app", true),
            ("/projects/**", "/projects", false),
            ("/projects/**", "/projects-other/app", false),
            ("/projects/**/src", "/projects/org/app/src", false),
            ("/projects/**/src", "/projects/src", false),
            ("/projects/[ab]?", "/projects/a1", true),
        ] {
            assert_eq!(
                matches_pattern(pattern, Path::new(path)).unwrap(),
                expected,
                "{pattern} {path}"
            );
        }
    }

    #[test]
    fn wildcard_go_rejects_malformed_classes_and_accepts_empty_ranges() {
        for pattern in ["[", "[]", "[^]", "[a-]", "[-a]", "[a-b-c]", r"abc\"] {
            assert!(
                Config::parse(&format!("[[wildcard]]\npattern='{pattern}'\n")).is_err(),
                "{pattern}"
            );
            assert!(go_path_match(pattern, "unrelated").is_err(), "{pattern}");
        }
        assert!(Config::parse("[[wildcard]]\npattern='/p/[z-a]'\n").is_ok());
        // The special recursive-prefix branch never parses its prefix as glob.
        assert!(Config::parse("[[wildcard]]\npattern='/p/[/**'\n").is_ok());
        assert!(matches_pattern("/p/[/**", Path::new("/p/[/child")).unwrap());
    }

    #[test]
    fn expansion_is_lexical_and_never_executes_commands() {
        assert_eq!(
            expand_path("~/repo").unwrap(),
            dirs::home_dir().unwrap().join("repo")
        );
        assert_eq!(
            expand_path("${HOME}/repo").unwrap(),
            dirs::home_dir().unwrap().join("repo")
        );
        assert_eq!(
            expand_path("./one/../two").unwrap(),
            env::current_dir().unwrap().join("two")
        );
        assert!(expand_path("${REXUE_TEST_MISSING_ENV_69F8}/repo").is_err());
        assert!(expand_path("").is_err());
        assert!(expand_path("~someone/repo").is_err());
        assert!(
            expand_path("$(touch should-not-exist)")
                .unwrap()
                .ends_with("$(touch should-not-exist)")
        );
    }

    #[test]
    fn validation_reports_bad_configuration() {
        for (text, message) in [
            ("strict_mode=true\nmystery=true", "unknown field"),
            ("strict_mode=true\n[tui]\nprevew=true", "unknown field"),
            ("[default_session]\ntmuxp='x'", "tmux-specific"),
            ("tmux_command='tmux'", "tmux-specific"),
            (
                "[tui]\nwindow_name_format='#{pane_title}'",
                "unsupported tmux syntax",
            ),
            (
                "[tui]\nwindow_name_format='#(echo nope)'",
                "cannot be evaluated",
            ),
            (
                "[tui]\nwindow_name_format='#{?window_active,yes,no}'",
                "unsupported tmux syntax",
            ),
            ("[tui]\nalias_filter_prefix='ab'", "alias_filter_prefix"),
            ("[tui]\nalias_filter_prefix=' '", "alias_filter_prefix"),
            (
                "[tui]\nalias_auto_connect_delay='later'",
                "alias_auto_connect_delay",
            ),
            ("[[wildcard]]\npattern='['", "wildcard pattern"),
            ("[[session]]\nname='x'\npath=''", "path"),
            (
                "[[session]]\nname='x'\npath='/x'\nwindows=['missing']",
                "unknown window",
            ),
            (
                "[[session]]\nname='x'\npath='/x'\nalias='a'\n[[session]]\nname='y'\npath='/y'\nalias='A'",
                "duplicate alias",
            ),
            ("[[worktree]]\npath=''", "worktree path"),
        ] {
            let error = format!("{:#}", Config::parse(text).unwrap_err());
            assert!(error.contains(message), "{text}: {error}");
        }
        Config::parse("[tui]\nalias_filter_prefix=''\nalias_auto_connect_delay='0s'\npointer='🦀'")
            .unwrap();
    }

    #[test]
    fn loaded_config_normalizes_pinned_go_fallbacks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rexue.toml");
        fs::write(
            &path,
            r#"
strict_mode = true
terminal = 'Rex'
dir_length = -2
git_dir_length = 0
[tui]
prompt = ''
placeholder = ''
pointer = 'too wide'
window_name_format = '#{window_index}: #W #{window_active}'
alias_auto_connect_delay = '-1ms'
preview_width = 0
preview_min_width = -1
preview_border = 'unknown'
[[worktree]]
path = '/repo'
"#,
        )
        .unwrap();
        let config = Config::load(Some(&path)).unwrap();
        assert_eq!(config.terminal, "Rex");
        assert_eq!((config.dir_length, config.git_dir_length), (1, 1));
        let tui = &config.tui;
        assert_eq!(tui.prompt, "> ");
        assert_eq!(tui.placeholder, "Filter Sessions...");
        assert_eq!(tui.pointer, "> ");
        assert_eq!(
            tui.window_name_format,
            "{window_index}: {window_name} {window_active}"
        );
        assert_eq!(
            parse_alias_delay(&tui.alias_auto_connect_delay).unwrap(),
            Duration::ZERO
        );
        assert_eq!((tui.preview_width, tui.preview_min_width), (60, 100));
        assert_eq!(tui.preview_border, "line");
        assert_eq!(config.worktree[0].path, "/repo");
        for (raw, expected) in [
            (-10, 60),
            (0, 60),
            (1, 10),
            (9, 10),
            (10, 10),
            (60, 60),
            (90, 90),
            (91, 90),
            (100_000, 90),
        ] {
            fs::write(
                &path,
                format!("[tui]\npreview_width={raw}\npreview_min_width=0"),
            )
            .unwrap();
            let loaded = Config::load(Some(&path)).unwrap();
            assert_eq!(loaded.tui.preview_width, expected, "{raw}");
            assert_eq!(loaded.tui.preview_min_width, 100);
        }
        assert!(Config::default().terminal.is_empty());
    }

    #[test]
    fn native_window_fields_and_picker_normalization() {
        for (raw, expected) in [
            ("", "{window_name}"),
            ("#{window_name}", "{window_name}"),
            ("#W", "{window_name}"),
            ("#I #W", "{window_index} {window_name}"),
            ("#{window_active}", "{window_active}"),
            ("{window_name}", "{window_name}"),
        ] {
            let config = Config::parse(&format!("[tui]\nwindow_name_format='{raw}'")).unwrap();
            assert_eq!(config.tui.window_name_format, expected);
        }
        for border in ["", "unknown", "line", "none", "thick", "double"] {
            let mut tui = TuiConfig {
                pointer: "wide".into(),
                preview_width: 1,
                preview_min_width: 0,
                preview_border: border.into(),
                ..Default::default()
            };
            tui.normalize().unwrap();
            assert_eq!(tui.pointer, "> ");
            assert_eq!((tui.preview_width, tui.preview_min_width), (10, 100));
            assert_eq!(
                tui.preview_border,
                if border.is_empty() || border == "unknown" {
                    "line"
                } else {
                    border
                }
            );
        }
        let config = Config::parse("[tui]\npointer=''\nprompt=''\nplaceholder=''\nalias_auto_connect_delay=''\nalias_filter_prefix=''").unwrap();
        assert_eq!(config.tui.pointer, "> ");
        assert_eq!(config.tui.alias_auto_connect_delay, "150ms");
        assert!(config.tui.alias_filter_prefix.is_empty());
    }

    #[test]
    fn alias_delay_matches_go_duration_grammar_and_signed_limits() {
        for (raw, nanos) in [
            ("", 150_000_000),
            ("0", 0),
            ("+0", 0),
            ("-0", 0),
            ("-1ms", 0),
            ("-1h2m3s", 0),
            ("+150ms", 150_000_000),
            ("1h2m3.5s", 3_723_500_000_000),
            (".5s", 500_000_000),
            ("1.s", 1_000_000_000),
            ("1us", 1_000),
            ("1µs", 1_000),
            ("1μs", 1_000),
            ("0.9ns", 0),
            ("9223372036854775807ns", i64::MAX as u64),
            ("-9223372036854775808ns", 0),
        ] {
            assert_eq!(
                parse_alias_delay(raw).unwrap(),
                Duration::from_nanos(nanos),
                "{raw}"
            );
        }
        for raw in [
            "later",
            "1",
            "00",
            "1d",
            "1 sec",
            " 1s",
            "1s ",
            "+",
            "--1ms",
            ".s",
            "1e3s",
            "9223372036854775808ns",
            "-9223372036854775809ns",
            "9999999999999999999999999999999999999999h",
        ] {
            assert!(parse_alias_delay(raw).is_err(), "{raw}");
            assert!(
                Config::parse(&format!("[tui]\nalias_auto_connect_delay='{raw}'")).is_err(),
                "{raw}"
            );
        }
    }

    #[test]
    fn explicit_missing_and_malformed_files_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(Config::load(Some(&path)).is_err());
        assert!(Config::load_path(&path, false).unwrap().session.is_empty());
        fs::write(&path, "[[session").unwrap();
        assert!(Config::load(Some(&path)).is_err());
        fs::write(&path, "[tui]\npreview=true").unwrap();
        assert!(Config::load(Some(&path)).unwrap().tui.preview);
    }

    #[test]
    fn unicode_aliases_cannot_duplicate_by_simple_fold() {
        for (a, b) in [("ä", "Ä"), ("Σ", "ς"), ("S", "ſ")] {
            let text = format!(
                "[[session]]\nname='one'\npath='/one'\nalias='{a}'\n[[session]]\nname='two'\npath='/two'\nalias='{b}'\n"
            );
            assert!(
                Config::parse(&text)
                    .unwrap_err()
                    .to_string()
                    .contains("duplicate alias")
            );
        }
    }
    #[test]
    fn legacy_pane_icon_maps_to_native_block_source() {
        let cfg = Config::parse("[icons.tmux-pane]\nicon='P'\ncolor='green'\n").unwrap();
        assert_eq!(cfg.icons["rex-block"].icon, "P");
        let cfg = Config::parse("[icons.rex-block]\nicon='B'\ncolor='green'\n").unwrap();
        assert_eq!(cfg.icons["rex-block"].icon, "B");
    }

    #[test]
    fn resolved_path_data_is_not_expanded_and_entry_errors_are_not_dropped() {
        let data = Path::new("/tmp/literal-$REXUE_UNSET_LITERAL_734129/409");
        assert_eq!(absolute_path(data).unwrap(), data);
        let resolved = Config::default().resolve(data, None).unwrap();
        assert_eq!(resolved.path, data.to_str().unwrap());
        let cfg = Config {
            session: vec![
                SessionConfig {
                    name: "valid".into(),
                    path: "/tmp".into(),
                    ..Default::default()
                },
                SessionConfig {
                    name: "broken".into(),
                    path: String::new(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let error = cfg.entries().unwrap_err();
        assert!(error.to_string().contains("broken"), "{error:#}");
    }

    #[test]
    fn entries_carry_resolved_metadata_and_name_lookup() {
        let config = Config::parse(
            r#"
[default_session]
icon = "default"
windows = ["Logs"]
[[window]]
name="Logs"
[[session]]
name = "app"
path = "/projects/app"
alias = "a"
alias_auto_connect = true
[[wildcard]]
pattern = "/projects/**"
icon = "project"
"#,
        )
        .unwrap();
        let entries = config.entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, Source::Config);
        assert_eq!(entries[0].window_names, vec!["Logs"]);
        assert_eq!(entries[0].icon.as_deref(), Some("project"));
        assert_eq!(entries[0].alias.as_deref(), Some("a"));
        assert!(entries[0].alias_auto_connect);
        let resolved = config
            .resolve(Path::new("/different"), Some("app"))
            .unwrap();
        assert_eq!(resolved.name, "app");
        assert_eq!(resolved.alias.as_deref(), Some("a"));
    }

    #[test]
    fn xdg_config_location_and_home_fallback() {
        const CHILD: &str = "REXUE_CONFIG_LOCATION_TEST_CHILD";
        if env::var_os(CHILD).is_some() {
            let config = Config::load(None).unwrap();
            assert_eq!(config.tui.prompt, "from fixture");
            return;
        }
        // Child processes isolate environment changes from other test threads.
        let dir = tempfile::tempdir().unwrap();
        for use_xdg in [true, false] {
            let root = if use_xdg {
                dir.path().join("xdg")
            } else {
                dir.path().join(".config")
            };
            fs::create_dir_all(root.join("rexue")).unwrap();
            fs::write(
                root.join("rexue/rexue.toml"),
                "[tui]\nprompt='from fixture'",
            )
            .unwrap();
            let mut command = std::process::Command::new(env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "config::tests::xdg_config_location_and_home_fallback",
                ])
                .env(CHILD, "1")
                .env("HOME", dir.path());
            if use_xdg {
                command.env("XDG_CONFIG_HOME", &root);
            } else {
                command.env_remove("XDG_CONFIG_HOME");
            }
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }

    #[test]
    fn imports_append_only_three_lists_from_cwd_in_declaration_order() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("cwd");
        let configdir = dir.path().join("configdir");
        fs::create_dir(&cwd).unwrap();
        fs::create_dir(&configdir).unwrap();
        // A config-directory decoy makes the resolution base observable.
        fs::write(configdir.join("first.toml"), "this is invalid TOML").unwrap();
        fs::write(
            cwd.join("first.toml"),
            r#"
strict_mode = true
terminal = "Imported terminal"
unknown_root = true
import = ["missing-nested.toml"]
cache = true
blacklist = ["imported"]
sort_order = ["directory"]
dir_length = 8
git_dir_length = 9
vcs = "unsupported-but-not-inherited"
separator_aware = true
git_namer_use_worktree_root = true
[default_session]
startup_command = "imported default"
preview_command = "imported preview"
windows = ["missing-but-not-inherited"]
[tui]
preview = true
prompt = "imported prompt"
preview_width = 1
[icons.config]
icon = "C"
[browser]
application = "imported"
[frecency]
list_command = "imported"
[[worktree]]
repo = "invalid-but-not-inherited"
path = "/imported"
[[name_substitution]]
find = "imported"
replace = "leaked"
[[window]]
name = "first-window"
unknown_window = true
[[wildcard]]
pattern = "/projects/**"
startup_command = "first wildcard"
unknown_wildcard = true
[[session]]
name = "first"
path = "/projects/first"
windows = ["second-window"]
startup_command = ""
disable_startup_command = false
unknown_session = true
"#,
        )
        .unwrap();
        fs::write(
            cwd.join("second.toml"),
            r#"
[[window]]
name = "second-window"
[[wildcard]]
pattern = "/projects/**"
startup_command = "second wildcard"
[[session]]
name = "second"
path = "/projects/second"
"#,
        )
        .unwrap();
        let path = configdir.join("rexue.toml");
        fs::write(
            &path,
            r#"
strict_mode = true
import = ["first.toml", "second.toml"]
[[session]]
name = "local"
path = "/local"
[[window]]
name = "local-window"
[[wildcard]]
pattern = "/local/**"
startup_command = "local wildcard"
[default_session]
startup_command = "primary default"
disable_startup_command = true
"#,
        )
        .unwrap();
        let config = Config::load_path_from_cwd(&path, true, &cwd).unwrap();
        assert_eq!(
            config
                .session
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["local", "first", "second"]
        );
        assert_eq!(
            config
                .window
                .iter()
                .map(|w| w.name.as_str())
                .collect::<Vec<_>>(),
            ["local-window", "first-window", "second-window"]
        );
        assert_eq!(
            config
                .wildcard
                .iter()
                .map(|w| w.pattern.as_str())
                .collect::<Vec<_>>(),
            ["/local/**", "/projects/**", "/projects/**"]
        );
        assert!(config.strict_mode);
        assert!(config.terminal.is_empty());
        assert!(!config.cache && !config.separator_aware && !config.git_namer_use_worktree_root);
        assert_eq!((config.dir_length, config.git_dir_length), (1, 1));
        assert_eq!(config.vcs, "git");
        assert!(config.blacklist.is_empty() && config.sort_order.is_empty());
        assert!(config.worktree.is_empty() && config.name_substitution.is_empty());
        assert!(config.icons.is_empty());
        assert!(config.frecency.list_command.is_none());
        assert!(!config.tui.preview);
        assert_eq!(config.tui.prompt, TuiConfig::default().prompt);
        assert_eq!(config.tui.preview_width, 60);
        assert!(config.default_session.preview_command.is_none());
        assert!(config.default_session.windows.is_empty());
        let first = config.resolve(Path::new("/projects/first"), None).unwrap();
        assert_eq!(first.startup_command.as_deref(), Some("first wildcard"));
        assert!(!first.disable_startup_command);
        let other = config.resolve(Path::new("/projects/other"), None).unwrap();
        assert_eq!(other.startup_command.as_deref(), Some("first wildcard"));
        assert!(!other.disable_startup_command);
        let outside = config.resolve(Path::new("/outside"), None).unwrap();
        assert!(outside.startup_command.is_none());
    }

    #[test]
    fn imports_are_exact_nonrecursive_and_independently_permissive() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();
        let a = cwd.join("a.toml");
        let b = cwd.join("b.toml");
        let load = || Config::load_path_from_cwd(&a, true, cwd);
        fs::write(&a, "strict_mode=true\nimport=['b.toml']").unwrap();
        // A nested cycle and missing nested file both remain unvisited.
        fs::write(
            &b,
            "strict_mode=true\nimport=['a.toml', 'missing.toml']\nunknown_child=true",
        )
        .unwrap();
        load().unwrap();
        fs::write(
            &a,
            "strict_mode=true\nunknown_primary=true\nimport=['b.toml']",
        )
        .unwrap();
        assert!(format!("{:#}", load().unwrap_err()).contains("unknown_primary"));
        fs::write(&a, "import=['b.toml']").unwrap();
        load().unwrap();
        fs::write(&b, "[tui]\npreview='yes'").unwrap();
        assert!(format!("{:#}", load().unwrap_err()).contains("b.toml"));
        fs::write(&b, "[[session]]\nname='x'\npath='/x'\ntmuxp='unsupported'").unwrap();
        let error = format!("{:#}", load().unwrap_err());
        assert!(
            error.contains("b.toml")
                && error.contains("session.tmuxp")
                && error.contains("tmux-specific"),
            "{error}"
        );
        fs::write(&b, "tmux_command='tmux'").unwrap();
        assert!(format!("{:#}", load().unwrap_err()).contains("tmux-specific"));
        fs::write(&a, "import=['missing.toml']").unwrap();
        let error = format!("{:#}", load().unwrap_err());
        assert!(
            error.contains("missing.toml") && error.contains("could not read config"),
            "{error}"
        );
        // A matching file cannot turn an exact filename into a glob.
        fs::write(
            cwd.join("part-01.toml"),
            "[[session]]\nname='globbed'\npath='/globbed'",
        )
        .unwrap();
        fs::write(&a, "import=['part-*.toml']").unwrap();
        assert!(load().is_err());
        fs::write(
            cwd.join("part-*.toml"),
            "[[session]]\nname='literal'\npath='/literal'",
        )
        .unwrap();
        assert_eq!(load().unwrap().session[0].name, "literal");
        fs::write(&b, "[[session").unwrap();
        fs::write(&a, "import=['b.toml']").unwrap();
        assert!(format!("{:#}", load().unwrap_err()).contains("invalid TOML"));
    }

    #[test]
    fn public_load_uses_process_cwd_and_upstream_import_expansion() {
        const CHILD: &str = "REXUE_IMPORT_CWD_TEST_CHILD";
        if let Some(path) = env::var_os(CHILD) {
            let config = Config::load(Some(Path::new(&path))).unwrap();
            assert_eq!(
                config
                    .session
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>(),
                ["relative", "env", "tilde", "tilde-prefix", "missing-env"]
            );
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("cwd");
        let configdir = dir.path().join("configdir");
        let home = dir.path().join("home");
        for path in [&cwd, &configdir, &home] {
            fs::create_dir(path).unwrap();
        }
        for (path, name) in [
            (cwd.join("relative.toml"), "relative"),
            (cwd.join("env.toml"), "env"),
            (home.join("tilde.toml"), "tilde"),
            (home.join("prefix.toml"), "tilde-prefix"),
            (cwd.join("missing-env.toml"), "missing-env"),
        ] {
            fs::write(path, format!("[[session]]\nname='{name}'\npath='/{name}'")).unwrap();
        }
        let path = configdir.join("config.toml");
        fs::write(&path, r#"import = ["relative.toml", "$REXUE_IMPORT_ROOT/env.toml", "${HOME}/tilde.toml", "~prefix.toml", "${REXUE_UNSET_IMPORT_TEST}missing-env.toml"]"#).unwrap();
        let result = std::process::Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "config::tests::public_load_uses_process_cwd_and_upstream_import_expansion",
            ])
            .current_dir(&cwd)
            .env(CHILD, &path)
            .env("HOME", &home)
            .env("REXUE_IMPORT_ROOT", &cwd)
            .env_remove("REXUE_UNSET_IMPORT_TEST")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            import_path("~/tilde.toml", &cwd).unwrap(),
            dirs::home_dir().unwrap().join("tilde.toml")
        );
        for (input, literal) in [
            ("${unterminated.toml", "unterminated.toml"),
            ("${}empty.toml", "empty.toml"),
            ("$%literal.toml", "$%literal.toml"),
            ("trailing$", "trailing$"),
        ] {
            assert_eq!(import_path(input, &cwd).unwrap(), cwd.join(literal));
        }
    }

    #[test]
    fn permissive_unknown_fields_still_reject_bad_types() {
        Config::parse("mystery=true\n[tui]\nprevew=true").unwrap();
        assert!(Config::parse("[tui]\npreview='yes'").is_err());
        assert!(Config::parse("strict_mode=true\n[tui]\nprevew=true").is_err());
    }

    #[test]
    fn regex_substitution_and_source_icons() {
        let config = Config::parse(
            r#"
sort_order = ["tmux", ["config", "zoxide"]]
[[name_substitution]]
find = '^~/projects/(.*)$'
replace = '$1'
regex = true
[icons.tmux]
icon = "R"
color = "Bright-Blue"
[icons.config]
color = "cyan"
"#,
        )
        .unwrap();
        assert_eq!(
            config.name_substitution[0].apply("~/projects/app").unwrap(),
            "app"
        );
        assert_eq!(config.icons["rex"].icon, "R");
        assert!(matches!(&config.sort_order[0], SortGroup::Source(source) if source == "rex"));
        assert!(Config::parse("[[name_substitution]]\nfind='['\nregex=true").is_err());
        assert!(Config::parse("[icons.config]\ncolor='purple'").is_err());
        assert!(Config::parse("[icons.unknown]\nicon='x'").is_err());
    }

    #[test]
    fn example_is_valid() {
        Config::parse(include_str!("../examples/rexue.toml")).unwrap();
    }
}
