//! Configuration for Rex sessions, directory history and Worktrunk repositories.
use crate::keybindings::{DEFAULT_BINDINGS, Keybindings};
use crate::model::{Entry, Source};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
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
    pub name_substitution: Vec<NameSubstitution>,
    pub frecency: FrecencyConfig,
    pub cache: bool,
    pub strict_mode: bool,
    #[serde(rename = "import")]
    pub import_paths: Vec<String>,
    pub icons: BTreeMap<String, SourceIcon>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            session: Vec::new(),
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
            name_substitution: Vec::new(),
            frecency: FrecencyConfig::default(),
            cache: false,
            strict_mode: false,
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

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionConfig {
    pub name: String,
    pub path: String,
    pub startup_command: Option<String>,
    pub startup_script: Option<String>,
    pub preview_command: Option<String>,
    pub disable_startup_command: bool,
    pub icon: Option<String>,
    pub alias: Option<String>,
    pub alias_auto_connect: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WildcardConfig {
    pub pattern: String,
    pub startup_command: Option<String>,
    pub startup_script: Option<String>,
    pub preview_command: Option<String>,
    pub disable_startup_command: Option<bool>,
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
    pub prompt: String,
    pub placeholder: String,
    pub pointer: String,
    pub alias_auto_connect_delay: String,
    pub alias_filter_prefix: String,
    pub preview: bool,
    pub preview_width: u16,
    pub preview_min_width: usize,
    pub preview_border: String,
    pub keybindings: Keybindings,
}
impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            show_icons: true,
            prompt: "> ".into(),
            placeholder: "Filter Sessions...".into(),
            pointer: "> ".into(),
            alias_auto_connect_delay: "150ms".into(),
            alias_filter_prefix: "/".into(),
            preview: false,
            preview_width: 60,
            preview_min_width: 100,
            preview_border: "line".into(),
            keybindings: Keybindings::default(),
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
        Ok(())
    }
}

/// Parse alias auto-connect durations. Empty uses 150ms; negative values connect immediately.
/// Invalid syntax or values outside Go's signed 64-bit nanosecond range fail.
pub fn parse_alias_delay(input: &str) -> Result<Duration> {
    let invalid = || {
        anyhow::anyhow!(
            "invalid alias_auto_connect_delay {input:?}: expected a duration (e.g. 150ms, +1s, -1ms, or 0)"
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

// Normalize signed integer settings before deserializing into Rust's
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
        let mut resolved = explicit.cloned().unwrap_or_default();
        // Empty values fall through; only the first matching wildcard participates.
        resolved.preview_command = nonempty(resolved.preview_command.as_ref())
            .or_else(|| wildcard.and_then(|w| nonempty(w.preview_command.as_ref())))
            .or_else(|| nonempty(defaults.preview_command.as_ref()))
            .cloned();
        resolved.icon = nonempty(resolved.icon.as_ref())
            .or_else(|| wildcard.and_then(|w| nonempty(w.icon.as_ref())))
            .cloned();
        // Aliases and auto-connect belong to an explicit session.
        resolved.alias = nonempty(resolved.alias.as_ref()).cloned();
        // Startup is a strategy chain, not an independent overlay of command
        // and script: a lower-priority script must never shadow a command.
        // Keep scripts and commands paired with their configuration layer.
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
                entry.icon = resolved.icon;
                entry.alias = resolved.alias;
                entry.alias_auto_connect = resolved.alias_auto_connect;
                Ok(entry)
            })
            .collect()
    }

    pub fn validate(&self) -> Result<()> {
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
            if !["agent", "rex", "config", "zoxide", "directory", "worktree"]
                .contains(&source.as_str())
            {
                bail!(
                    "unknown icons source {source:?}; use agent, rex, config, zoxide, directory, or worktree"
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
        for wildcard in &self.wildcard {
            let pattern = expand_path(&wildcard.pattern).context("invalid wildcard pattern")?;
            if !pattern.to_string_lossy().ends_with("/**") {
                parse_path_pattern(&pattern.to_string_lossy())?;
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
        for group in &self.sort_order {
            let sources: &[String] = match group {
                SortGroup::Source(source) => std::slice::from_ref(source),
                SortGroup::Merged(sources) => sources,
            };
            if sources.is_empty() {
                bail!("sort_order groups must not be empty");
            }
            for source in sources {
                if !["agent", "rex", "config", "zoxide", "directory", "worktree"]
                    .contains(&source.as_str())
                {
                    bail!(
                        "unknown sort_order source {source:?}; use agent, rex, config, zoxide, directory, or worktree"
                    );
                }
            }
        }
        Ok(())
    }
}

// Append session, wildcard and repository definitions once, in declaration order.
fn merge_import(target: &mut toml::Value, mut imported: toml::Value) {
    for key in ["session", "wildcard", "worktree"] {
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

// Imports are exact filenames resolved from the process working directory.
fn import_path(input: &str, cwd: &Path) -> Result<PathBuf> {
    let expanded =
        shellexpand::full(input).with_context(|| format!("could not expand import {input:?}"))?;
    let path = PathBuf::from(expanded.as_ref());
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
        let strict = !imported
            && value
                .get("strict_mode")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);
        check_unknown_keys(&mut value, "", strict)
            .with_context(|| format!("in config {}", path.display()))?;
        // Decode the entire file for type errors,
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
    let binding_names: Vec<_>;
    let known: &[&str] = match kind {
        "" => &[
            "session",
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
            "name_substitution",
            "frecency",
            "cache",
            "strict_mode",
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
            "icon",
            "alias",
            "alias_auto_connect",
        ],
        "wildcard" => &[
            "pattern",
            "startup_command",
            "startup_script",
            "preview_command",
            "disable_startup_command",
            "icon",
        ],
        "worktree" => &["path", "icon"],
        "tui" => &[
            "show_icons",
            "prompt",
            "placeholder",
            "pointer",
            "alias_auto_connect_delay",
            "alias_filter_prefix",
            "preview",
            "preview_width",
            "preview_min_width",
            "preview_border",
            "keybindings",
        ],
        "keybindings" => {
            binding_names = DEFAULT_BINDINGS.iter().map(|(name, _)| *name).collect();
            &binding_names
        }
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
    // A trailing /** matches literal descendants of the prefix.
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return Ok(candidate
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/') && suffix.len() > 1));
    }
    path_match(&pattern, &candidate)
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

// Wildcards use ^ for class negation, backslash escapes,
// and non-recursive stars. Parse the entire pattern before matching so
// malformed suffixes are rejected even when an earlier term cannot match.
fn parse_path_pattern(pattern: &str) -> Result<Vec<PathToken>> {
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

fn path_match(pattern: &str, name: &str) -> Result<bool> {
    // Stars scan bytes; ? and character classes decode Unicode characters.
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
    let tokens = parse_path_pattern(pattern)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_strategies_respect_disable_flags() {
        // Disable flags guard fallback strategies while explicit startup takes priority.
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
    fn wildcard_syntax_handles_unicode() {
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
                path_match(pattern, name).unwrap(),
                expected,
                "{pattern:?} {name:?}"
            );
        }
    }

    #[test]
    fn wildcard_recursive_suffix_uses_literal_prefix() {
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
    fn wildcard_rejects_malformed_classes_and_accepts_empty_ranges() {
        for pattern in ["[", "[]", "[^]", "[a-]", "[-a]", "[a-b-c]", r"abc\"] {
            assert!(
                Config::parse(&format!("[[wildcard]]\npattern='{pattern}'\n")).is_err(),
                "{pattern}"
            );
            assert!(path_match(pattern, "unrelated").is_err(), "{pattern}");
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
    fn alias_delay_accepts_duration_grammar_and_signed_limits() {
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
    fn permissive_unknown_fields_still_reject_bad_types() {
        Config::parse("mystery=true\n[tui]\nprevew=true").unwrap();
        assert!(Config::parse("[tui]\npreview='yes'").is_err());
        assert!(Config::parse("strict_mode=true\n[tui]\nprevew=true").is_err());
    }

    #[test]
    fn regex_substitution_and_source_icons() {
        let config = Config::parse(
            r#"
sort_order = ["rex", ["config", "zoxide"]]
[[name_substitution]]
find = '^~/projects/(.*)$'
replace = '$1'
regex = true
[icons.rex]
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
