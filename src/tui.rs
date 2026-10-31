//! Native sesh-style picker. Configuration is deliberately independent of the
//! application's config module; the caller maps its configuration into options.
use std::{
    io::{self, Stdout},
    panic::{self, AssertUnwindSafe},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use crossterm::{
    cursor::{Hide, SetCursorStyle, Show},
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use nucleo_matcher::{
    Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::model::{Entry, Source};

/// A configured alias. Targets absent from the fetched list remain selectable.
#[derive(Debug, Clone)]
pub struct Alias {
    pub alias: String,
    pub target: String,
    pub auto_connect: bool,
}

/// Values are concrete resolved options, not configuration overrides.
/// Aliases and glyphs are keyed by lowercase alias and `Source::as_str()`.
#[derive(Debug, Clone)]
pub struct PickerOptions {
    pub show_icons: bool,
    pub show_windows: bool,
    /// Supports {name}, {window_name}, and {active} ("*" for the active window).
    pub window_name_format: String,
    pub separator_aware: bool,
    pub prompt: String,
    pub placeholder: String,
    pub pointer: String,
    pub query: String,
    pub aliases: std::collections::BTreeMap<String, Alias>,
    /// Configured target icons for aliases synthesized outside fetched sources.
    pub alias_icons: std::collections::BTreeMap<String, String>,
    pub alias_filter_prefix: String,
    pub alias_auto_connect_delay: Duration,
    pub disable_alias_auto_connect: bool,
    pub icon_width: usize,
    pub glyphs: std::collections::BTreeMap<String, String>,
    /// Source color names (including bright-*), #RGB/#RRGGBB, or ANSI codes.
    pub glyph_colors: std::collections::BTreeMap<String, String>,
    pub preview: bool,
    /// Percentage, clamped to 10..=90; zero uses 60.
    pub preview_width: u16,
    /// Zero uses 100 columns.
    pub preview_min_width: usize,
    /// "none", "line" (default), "thick", or "double".
    pub preview_border: String,
    /// Draw boundaries between consecutive Entry.group values.
    pub group_separator: bool,
    /// Specialized full-width worktree layout and bindings.
    pub worktree: bool,
    /// Worktrees refresh once after the initial cached list. Disable when the
    /// caller already forces a fresh initial fetch (for example --refresh).
    pub auto_refresh: bool,
}

impl Default for PickerOptions {
    fn default() -> Self {
        Self {
            show_icons: true,
            show_windows: true,
            window_name_format: "{window_name}".into(),
            separator_aware: false,
            prompt: "> ".into(),
            placeholder: "Filter sessions...".into(),
            pointer: "> ".into(),
            query: String::new(),
            aliases: Default::default(),
            alias_icons: Default::default(),
            alias_filter_prefix: "/".into(),
            alias_auto_connect_delay: Duration::from_millis(150),
            disable_alias_auto_connect: false,
            icon_width: 1,
            glyphs: Default::default(),
            glyph_colors: Default::default(),
            preview: false,
            preview_width: 60,
            preview_min_width: 100,
            preview_border: "line".into(),
            group_separator: false,
            worktree: false,
            auto_refresh: true,
        }
    }
}

const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(60);
const TICK: Duration = Duration::from_millis(20);
const CURSOR_BLINK: Duration = Duration::from_millis(530);
// OSC 112 restores the terminal's configured cursor color.
const RESET_CURSOR_COLOR: &str = "\x1b]112\x07";

#[derive(Clone)]
struct Match {
    entry: Entry,
    indices: Vec<u32>,
    chip_match: usize,
}

struct Confirm {
    entry: Entry,
    yes: bool,
}

struct State {
    options: PickerOptions,
    entries: Vec<Entry>,
    filtered: Vec<Match>,
    rows: Vec<Option<usize>>,
    row_of: Vec<usize>,
    query: String,
    /// Character index, never a byte index.
    caret: usize,
    cursor_blink_started: Instant,
    cursor: usize,
    offset: usize,
    loading: bool,
    fetching: bool,
    refresh_pending: bool,
    explicit_refresh: bool,
    pasting: bool,
    status: String,
    confirm: Option<Confirm>,
    viewport: (u16, u16),
    removing: bool,
    preview_seq: u64,
    preview_due: Option<Instant>,
    preview_entry: Option<Entry>,
    preview_text: Option<String>,
    alias_due: Option<Instant>,
    matcher: Matcher,
}

fn same_entry(a: &Entry, b: &Entry) -> bool {
    a.source == b.source && a.name == b.name && a.path == b.path && a.session_id == b.session_id
}

fn normalize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if matches!(c, '-' | '_' | '/' | '\\') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

impl State {
    fn new(mut options: PickerOptions) -> Self {
        // Normalize lookup keys, retaining deterministic ordering for missing targets.
        options.aliases = options
            .aliases
            .into_values()
            .map(|a| (crate::model::alias_key(&a.alias), a))
            .collect();
        let pointer_width = UnicodeWidthStr::width(options.pointer.as_str());
        if pointer_width == 0 || pointer_width > 2 || options.pointer.chars().any(char::is_control)
        {
            options.pointer = "> ".into();
        }
        let query = options.query.clone();
        let caret = query.chars().count();
        Self {
            options,
            entries: Vec::new(),
            filtered: Vec::new(),
            rows: Vec::new(),
            row_of: Vec::new(),
            query,
            caret,
            cursor_blink_started: Instant::now(),
            cursor: 0,
            offset: 0,
            loading: true,
            fetching: true,
            refresh_pending: false,
            explicit_refresh: false,
            pasting: false,
            status: String::new(),
            confirm: None,
            viewport: (80, 24),
            removing: false,
            preview_seq: 0,
            preview_due: None,
            preview_entry: None,
            preview_text: None,
            alias_due: None,
            matcher: Matcher::new(nucleo_matcher::Config::DEFAULT),
        }
    }

    fn loaded(&mut self, entries: Vec<Entry>, now: Instant) {
        for entry in &entries {
            if let Some(alias) = &entry.alias {
                self.options
                    .aliases
                    .entry(crate::model::alias_key(alias))
                    .or_insert_with(|| Alias {
                        alias: alias.clone(),
                        target: entry.name.clone(),
                        auto_connect: entry.alias_auto_connect,
                    });
            }
        }
        self.entries = entries;
        if self.entries.iter().all(|e| e.source == Source::Worktree) && !self.entries.is_empty() {
            self.options.worktree = true;
        }
        if self.options.worktree {
            self.entries
                .sort_by_key(|e| worktree_parts(e).0.parse::<u64>().unwrap_or(u64::MAX));
        }
        self.refresh_pending = self.loading
            && self.options.worktree
            && self.options.auto_refresh
            && !self.entries.is_empty();
        self.explicit_refresh = false;
        self.loading = false;
        self.fetching = false;
        self.status.clear();
        self.filter();
        self.preview_entry = None;
        self.schedule_preview(now);
        // A prefilled query never arms auto-connect.
    }

    fn start_refresh(&mut self, explicit: bool) {
        self.fetching = true;
        self.explicit_refresh = explicit;
        self.status = "Refreshing worktrees...".into();
        self.alias_due = None;
    }

    fn refresh_failed(&mut self, error: &impl std::fmt::Display) {
        self.fetching = false;
        self.status = if self.explicit_refresh {
            format!("Refresh failed: {error}")
        } else {
            String::new()
        };
        self.explicit_refresh = false;
    }

    fn alias_query(&self) -> Option<&str> {
        if self.options.worktree {
            return None;
        }
        let prefix = &self.options.alias_filter_prefix;
        if prefix.is_empty() {
            None
        } else {
            self.query.strip_prefix(prefix)
        }
    }

    fn index_query(&self) -> Option<&str> {
        if self.options.worktree || self.alias_query().is_some() {
            None
        } else {
            self.query.strip_prefix('#')
        }
    }

    fn exact_alias(&self) -> Option<&Alias> {
        if self.options.worktree {
            return None;
        }
        let query = self.alias_query().unwrap_or(&self.query);
        let alias = self.options.aliases.get(&crate::model::alias_key(query))?;
        (self.alias_query().is_some() || alias.auto_connect).then_some(alias)
    }

    fn alias_entry(&self, alias: &Alias) -> Entry {
        self.entries
            .iter()
            .find(|e| e.name == alias.target)
            .cloned()
            .unwrap_or_else(|| {
                let mut e = Entry::new(&alias.target, "", Source::Config);
                e.alias = Some(alias.alias.clone());
                e.alias_auto_connect = alias.auto_connect;
                e.icon = self.options.alias_icons.get(&alias.target).cloned();
                e
            })
    }

    fn alias_for(&self, name: &str) -> Option<&Alias> {
        if self.options.worktree {
            return None;
        }
        self.options.aliases.values().find(|a| a.target == name)
    }

    fn filter(&mut self) {
        let raw = self.query.clone();
        let mut found = Vec::new();
        if let Some(query) = self.alias_query() {
            let q = crate::model::alias_key(query);
            let mut candidates: Vec<Entry> = self
                .entries
                .iter()
                .filter(|e| self.alias_for(&e.name).is_some())
                .cloned()
                .collect();
            for alias in self.options.aliases.values() {
                if !candidates.iter().any(|e| e.name == alias.target) {
                    candidates.push(self.alias_entry(alias));
                }
            }
            let mut by_name = Vec::new();
            for entry in candidates {
                let alias = self.alias_for(&entry.name).expect("alias candidate");
                let prefix = crate::model::alias_key(&alias.alias).starts_with(&q);
                let m = Match {
                    entry: entry.clone(),
                    indices: vec![],
                    chip_match: if prefix { query.chars().count() } else { 0 },
                };
                if prefix {
                    found.push(m);
                } else if !q.is_empty() && entry.name.to_lowercase().contains(&q) {
                    by_name.push(m);
                }
            }
            found.extend(by_name);
        } else {
            let query_owned = self.index_query().unwrap_or(&raw).to_owned();
            let query = query_owned.as_str();
            if query.is_empty() {
                found = self
                    .entries
                    .iter()
                    .cloned()
                    .map(|entry| Match {
                        entry,
                        indices: vec![],
                        chip_match: 0,
                    })
                    .collect();
            } else if let Some(alias) = self
                .options
                .aliases
                .get(&crate::model::alias_key(query))
                .filter(|_| !self.options.worktree)
            {
                found.push(Match {
                    entry: self.alias_entry(alias),
                    indices: vec![],
                    chip_match: query.chars().count(),
                });
            } else {
                let pattern = if self.options.separator_aware {
                    normalize(query)
                } else {
                    query.into()
                };
                let pattern = Pattern::new(
                    &pattern,
                    CaseMatching::Ignore,
                    Normalization::Smart,
                    nucleo_matcher::pattern::AtomKind::Fuzzy,
                );
                let mut ranked = Vec::new();
                let mut buffer = Vec::new();
                for entry in &self.entries {
                    let display_name = if self.options.worktree {
                        let (number, title) = worktree_parts(entry);
                        format!("{number} {title}")
                    } else {
                        clean(&entry.name)
                    };
                    let name = if self.options.separator_aware {
                        normalize(&display_name)
                    } else {
                        display_name
                    };
                    let mut indices = Vec::new();
                    // Use scalar indexes consistently in matching and rendering.
                    buffer.clear();
                    buffer.extend(name.chars());
                    if let Some(score) =
                        pattern.indices(Utf32Str::Unicode(&buffer), &mut self.matcher, &mut indices)
                    {
                        indices.sort_unstable();
                        indices.dedup();
                        let chip_match = self
                            .alias_for(&entry.name)
                            .filter(|a| {
                                crate::model::alias_key(&a.alias)
                                    .starts_with(&crate::model::alias_key(query))
                            })
                            .map_or(0, |_| query.chars().count());
                        ranked.push((
                            score,
                            Match {
                                entry: entry.clone(),
                                indices,
                                chip_match,
                            },
                        ));
                    }
                }
                // Stable ties preserve the backend's source/frecency ordering.
                ranked.sort_by_key(|item| std::cmp::Reverse(item.0));
                found = ranked.into_iter().map(|(_, m)| m).collect();
            }
        }
        self.filtered = found;
        self.rows.clear();
        self.row_of.clear();
        for (i, m) in self.filtered.iter().enumerate() {
            if !self.options.worktree
                && self.options.group_separator
                && self.query.is_empty()
                && i > 0
                && m.entry.group != self.filtered[i - 1].entry.group
            {
                self.rows.push(None);
            }
            self.row_of.push(self.rows.len());
            self.rows.push(Some(i));
        }
        self.cursor = self.cursor.min(self.filtered.len().saturating_sub(1));
    }

    fn selected(&self) -> Option<&Entry> {
        self.filtered.get(self.cursor).map(|m| &m.entry)
    }

    fn schedule_preview(&mut self, now: Instant) {
        if self.options.worktree {
            self.preview_due = None;
            return;
        }
        let entry = self.selected().cloned();
        if self.options.preview
            && entry
                .as_ref()
                .zip(self.preview_entry.as_ref())
                .is_some_and(|(a, b)| same_entry(a, b))
        {
            return;
        }
        self.preview_seq = self.preview_seq.wrapping_add(1);
        self.preview_entry = entry;
        if !self.options.preview || self.preview_entry.is_none() {
            self.preview_text = None;
        }
        self.preview_due = (self.options.preview && self.preview_entry.is_some())
            .then_some(now + PREVIEW_DEBOUNCE);
    }

    fn preview_loaded(&mut self, seq: u64, result: Result<String>) {
        if seq == self.preview_seq && self.options.preview {
            self.preview_text =
                Some(result.unwrap_or_else(|e| format!("Preview unavailable: {e}")));
        }
    }

    fn changed_query(&mut self, now: Instant) {
        self.cursor_blink_started = now;
        self.cursor = 0;
        self.offset = 0;
        self.filter();
        self.schedule_preview(now);
        self.alias_due = if !self.options.disable_alias_auto_connect && self.exact_alias().is_some()
        {
            Some(now + self.options.alias_auto_connect_delay)
        } else {
            None
        };
    }

    fn scroll(&mut self, visible: usize) {
        let Some(&row) = self.row_of.get(self.cursor) else {
            self.offset = 0;
            return;
        };
        if row < self.offset {
            self.offset = row;
        }
        if row >= self.offset + visible.max(1) {
            self.offset = row + 1 - visible.max(1);
        }
    }

    fn move_cursor(&mut self, delta: isize, now: Instant) {
        self.cursor = self
            .cursor
            .saturating_add_signed(delta)
            .min(self.filtered.len().saturating_sub(1));
        self.schedule_preview(now);
    }

    fn insert_text(&mut self, text: &str, now: Instant) {
        if self.confirm.is_some() {
            return;
        }
        let text = clean(text);
        if text.is_empty() {
            return;
        }
        self.query
            .insert_str(byte_at(&self.query, self.caret), &text);
        self.caret += text.chars().count();
        self.changed_query(now);
    }

    fn delete_range(&mut self, start: usize, end: usize, now: Instant) {
        if start == end {
            return;
        }
        let range = byte_at(&self.query, start)..byte_at(&self.query, end);
        self.query.replace_range(range, "");
        self.caret = start;
        self.changed_query(now);
    }

    fn key(&mut self, key: KeyEvent, visible: usize, now: Instant) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }
        if let Some(confirm) = &mut self.confirm {
            if key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                return Action::None;
            }
            let decision = match key.code {
                KeyCode::Esc | KeyCode::Char('n' | 'N' | 'q') => Some(false),
                KeyCode::Char('y' | 'Y') => Some(true),
                KeyCode::Enter => Some(confirm.yes),
                KeyCode::Left
                | KeyCode::Right
                | KeyCode::Tab
                | KeyCode::BackTab
                | KeyCode::Char('h' | 'l') => {
                    confirm.yes = !confirm.yes;
                    None
                }
                _ => None,
            };
            if let Some(yes) = decision {
                if yes && !confirmation_fits(self.viewport.0, self.viewport.1) {
                    self.status = "Confirmation disabled: resize terminal; Esc cancels".into();
                    return Action::None;
                }
                let c = self.confirm.take().expect("dialog exists");
                if yes {
                    self.removing = true;
                    self.status = "Removing...".into();
                    return Action::Remove(c.entry);
                }
            }
            return Action::None;
        }
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if !self.options.worktree {
            self.status.clear();
        }
        self.cursor_blink_started = now;
        match key.code {
            KeyCode::Left if alt || ctrl => {
                self.caret = word_left(&self.query, self.caret);
                return Action::None;
            }
            KeyCode::Right if alt || ctrl => {
                self.caret = word_right(&self.query, self.caret);
                return Action::None;
            }
            KeyCode::Char('b') if alt => {
                self.caret = word_left(&self.query, self.caret);
                return Action::None;
            }
            KeyCode::Char('f') if alt => {
                self.caret = word_right(&self.query, self.caret);
                return Action::None;
            }
            KeyCode::Backspace if alt || ctrl => {
                self.delete_range(word_left(&self.query, self.caret), self.caret, now);
                return Action::None;
            }
            KeyCode::Char('w') if ctrl => {
                self.delete_range(word_left(&self.query, self.caret), self.caret, now);
                return Action::None;
            }
            KeyCode::Delete if alt || ctrl => {
                self.delete_range(self.caret, word_right(&self.query, self.caret), now);
                return Action::None;
            }
            KeyCode::Char('d') if alt => {
                self.delete_range(self.caret, word_right(&self.query, self.caret), now);
                return Action::None;
            }
            _ => {}
        }
        match key.code {
            KeyCode::Esc => return Action::Quit,
            KeyCode::Enter if !self.loading => {
                return self
                    .selected()
                    .cloned()
                    .map_or(Action::Quit, Action::Choose);
            }
            KeyCode::Enter => return Action::None,
            KeyCode::Up => self.move_cursor(-1, now),
            KeyCode::Down => self.move_cursor(1, now),
            KeyCode::PageUp => self.move_cursor(-(visible.max(1) as isize), now),
            KeyCode::PageDown => self.move_cursor(visible.max(1) as isize, now),
            KeyCode::Char('k' | 'p') if ctrl => self.move_cursor(-1, now),
            KeyCode::Char('j' | 'n') if ctrl => self.move_cursor(1, now),
            KeyCode::Char('b') if ctrl => self.caret = self.caret.saturating_sub(1),
            KeyCode::Char('f') if ctrl => {
                self.caret = (self.caret + 1).min(self.query.chars().count())
            }
            KeyCode::Char('o') if ctrl && !self.options.worktree => {
                self.options.preview = !self.options.preview;
                // Invalidate even if toggled back onto the same entry.
                self.preview_entry = None;
                self.schedule_preview(now);
            }
            KeyCode::Char('r') if ctrl && self.options.worktree => {
                if !self.fetching {
                    self.start_refresh(true);
                    return Action::Refresh;
                }
            }
            KeyCode::Char('x') if ctrl && !self.options.worktree => {
                if !self.loading
                    && !self.removing
                    && let Some(entry) = self.selected()
                {
                    if matches!(entry.source, Source::Rex | Source::Zoxide) {
                        self.confirm = Some(Confirm {
                            entry: entry.clone(),
                            yes: true,
                        });
                    } else {
                        self.status = "Only Rex sessions and zoxide entries can be removed".into();
                    }
                }
            }
            KeyCode::Char('v') if ctrl && !self.pasting => {
                self.pasting = true;
                return Action::Paste;
            }
            KeyCode::Char('h') if ctrl && self.caret > 0 => {
                self.delete_range(self.caret - 1, self.caret, now);
            }
            KeyCode::Char('d') if ctrl => self.move_cursor((visible / 2) as isize, now),
            KeyCode::Char('u') if ctrl => self.move_cursor(-((visible / 2) as isize), now),
            KeyCode::Left => self.caret = self.caret.saturating_sub(1),
            KeyCode::Right => self.caret = (self.caret + 1).min(self.query.chars().count()),
            KeyCode::Home => self.caret = 0,
            KeyCode::End => self.caret = self.query.chars().count(),
            KeyCode::Char('a') if ctrl => self.caret = 0,
            KeyCode::Char('e') if ctrl => self.caret = self.query.chars().count(),
            KeyCode::Backspace if self.caret > 0 => {
                let end = byte_at(&self.query, self.caret);
                self.caret -= 1;
                let start = byte_at(&self.query, self.caret);
                self.query.replace_range(start..end, "");
                self.changed_query(now);
            }
            KeyCode::Delete if self.caret < self.query.chars().count() => {
                let start = byte_at(&self.query, self.caret);
                let end = byte_at(&self.query, self.caret + 1);
                self.query.replace_range(start..end, "");
                self.changed_query(now);
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                if self.index_query().is_some() && ('1'..='9').contains(&c) {
                    return self
                        .filtered
                        .get((c as u8 - b'1') as usize)
                        .map_or(Action::None, |m| Action::Choose(m.entry.clone()));
                }
                if !c.is_control() {
                    self.query.insert(byte_at(&self.query, self.caret), c);
                    self.caret += 1;
                    self.changed_query(now);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn removed(&mut self, entry: &Entry, result: Result<()>, now: Instant) {
        self.removing = false;
        match result {
            Err(e) => self.status = removal_failed(entry.source, &e),
            Ok(()) => {
                if let Some(index) = self.entries.iter().position(|e| same_entry(e, entry)) {
                    self.entries.remove(index);
                }
                // Do not resurrect a removed target via a configured alias.
                if !self.entries.iter().any(|e| e.name == entry.name) {
                    self.options.aliases.retain(|_, a| a.target != entry.name);
                }
                self.filter();
                self.status.clear();
                self.preview_entry = None;
                self.schedule_preview(now);
            }
        }
    }
}

fn removal_failed(source: Source, error: &impl std::fmt::Display) -> String {
    if source == Source::Rex {
        format!("Couldn't kill session: {error}")
    } else {
        format!("Couldn't remove entry: {error}")
    }
}

fn word_left(text: &str, caret: usize) -> usize {
    let chars: Vec<_> = text.chars().collect();
    let mut pos = caret.min(chars.len());
    while pos > 0 && chars[pos - 1].is_whitespace() {
        pos -= 1;
    }
    while pos > 0 && !chars[pos - 1].is_whitespace() {
        pos -= 1;
    }
    pos
}
fn word_right(text: &str, caret: usize) -> usize {
    let chars: Vec<_> = text.chars().collect();
    let mut pos = caret.min(chars.len());
    while pos < chars.len() && !chars[pos].is_whitespace() {
        pos += 1;
    }
    while pos < chars.len() && chars[pos].is_whitespace() {
        pos += 1;
    }
    pos
}

fn byte_at(s: &str, index: usize) -> usize {
    s.char_indices().nth(index).map_or(s.len(), |(i, _)| i)
}

enum Action {
    None,
    Quit,
    Choose(Entry),
    Remove(Entry),
    Refresh,
    Paste,
}

enum Work {
    Remove(Box<Entry>),
    Paste,
}

struct PreviewJob {
    seq: u64,
    entry: Entry,
}
enum Message {
    Loaded(Result<Vec<Entry>>),
    Preview(u64, Result<String>),
    Removed(Entry, Result<()>),
    Clipboard(Result<String>),
}

fn callback<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    panic::catch_unwind(AssertUnwindSafe(f))
        .unwrap_or_else(|_| Err(anyhow!("picker callback panicked")))
}

/// Opens the alternate screen until selection or cancellation. Fetch runs on
/// startup and Ctrl+R; fetch, preview and removal each have one worker and a
/// bounded one-slot queue.
/// Callbacks should set deadlines on subprocesses: arbitrary Rust closures cannot
/// be cancelled, so a running callback may finish after the picker has closed.
pub fn pick(
    fetch: impl Fn() -> Result<Vec<Entry>> + Send + 'static,
    preview: impl Fn(&Entry) -> Result<String> + Send + Sync + 'static,
    remove: impl Fn(&Entry) -> Result<()> + Send + Sync + 'static,
    options: PickerOptions,
) -> Result<Option<Entry>> {
    // The terminal, panic hook and signal dispositions are process-global.
    static PICKER_LOCK: Mutex<()> = Mutex::new(());
    let _lock = PICKER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let guard = TerminalGuard::enter()?;
    let result = panic::catch_unwind(AssertUnwindSafe(|| -> Result<Option<Entry>> {
        let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        terminal.clear()?;
        if !options.worktree {
            execute!(io::stdout(), SetCursorStyle::SteadyBlock)?;
        }
        let mut state = State::new(options);
        let (tx, rx) = mpsc::channel();
        let load_tx = tx.clone();
        let (fetch_jobs, fetch_rx) = mpsc::sync_channel::<()>(1);
        thread::Builder::new()
            .name("picker-fetch".into())
            .spawn(move || {
                while fetch_rx.recv().is_ok() {
                    if load_tx.send(Message::Loaded(callback(&fetch))).is_err() {
                        break;
                    }
                }
            })?;
        fetch_jobs.try_send(())?;
        let sequence = Arc::new(AtomicU64::new(0));
        // Invalidate queued work and stop accepting callbacks when pick returns.
        let _worker_guard = WorkerGuard(sequence.clone());
        let preview_tx = tx.clone();
        let worker_seq = sequence.clone();
        let (preview_jobs, preview_rx) = mpsc::sync_channel::<PreviewJob>(1);
        thread::Builder::new()
            .name("picker-preview".into())
            .spawn(move || {
                while let Ok(job) = preview_rx.recv() {
                    if worker_seq.load(Ordering::Acquire) != job.seq {
                        continue;
                    }
                    let result = callback(|| preview(&job.entry));
                    if preview_tx.send(Message::Preview(job.seq, result)).is_err() {
                        break;
                    }
                }
            })?;
        let (remove_jobs, remove_rx) = mpsc::sync_channel::<Work>(1);
        thread::Builder::new()
            .name("picker-remove".into())
            .spawn(move || {
                while let Ok(work) = remove_rx.recv() {
                    let message = match work {
                        Work::Remove(entry) => {
                            let result = callback(|| remove(&entry));
                            Message::Removed(*entry, result)
                        }
                        Work::Paste => Message::Clipboard(callback(clipboard_text)),
                    };
                    if tx.send(message).is_err() {
                        break;
                    }
                }
            })?;
        run(
            &mut terminal,
            &mut state,
            &rx,
            &fetch_jobs,
            &preview_jobs,
            &remove_jobs,
            &sequence,
        )
    }));
    // Restore the hook as well as the terminal before propagating a panic.
    drop(guard);
    match result {
        Ok(result) => result,
        Err(payload) => panic::resume_unwind(payload),
    }
}

struct WorkerGuard(Arc<AtomicU64>);
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.0.store(u64::MAX, Ordering::Release);
    }
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut State,
    rx: &Receiver<Message>,
    fetches: &SyncSender<()>,
    previews: &SyncSender<PreviewJob>,
    removals: &SyncSender<Work>,
    sequence: &AtomicU64,
) -> Result<Option<Entry>> {
    loop {
        if INTERRUPTED.load(Ordering::Relaxed) != 0 {
            return Ok(None);
        }
        let now = Instant::now();
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Message::Loaded(result) => match result {
                    Ok(entries) => state.loaded(entries, now),
                    Err(e) if state.loading => return Err(e.context("loading picker entries")),
                    Err(e) => state.refresh_failed(&e),
                },
                Message::Preview(seq, result) => state.preview_loaded(seq, result),
                Message::Removed(entry, result) => state.removed(&entry, result, now),
                Message::Clipboard(result) => {
                    state.pasting = false;
                    match result {
                        Ok(text) => state.insert_text(&text, now),
                        Err(e) => state.status = format!("Clipboard unavailable: {e}"),
                    }
                }
            }
        }
        if state.refresh_pending && !state.fetching {
            state.refresh_pending = false;
            if fetches.try_send(()).is_ok() {
                state.start_refresh(false);
            }
        }
        sequence.store(state.preview_seq, Ordering::Release);
        if state.preview_due.is_some_and(|due| due <= now)
            && let Some(entry) = &state.preview_entry
        {
            match previews.try_send(PreviewJob {
                seq: state.preview_seq,
                entry: entry.clone(),
            }) {
                Ok(()) => state.preview_due = None,
                Err(TrySendError::Full(_)) => {} // Retry on the next bounded tick; no new worker.
                Err(TrySendError::Disconnected(_)) => {
                    state.preview_due = None;
                    state.preview_text = Some("Preview unavailable: worker stopped".into());
                }
            }
        }
        if state.alias_due.is_some_and(|due| due <= now) {
            state.alias_due = None;
            if !state.options.disable_alias_auto_connect
                && let Some(alias) = state.exact_alias()
            {
                return Ok(Some(state.alias_entry(alias)));
            }
        }
        terminal.draw(|frame| render(frame, state))?;
        if event::poll(TICK)? {
            match event::read()? {
                Event::Key(key) => {
                    let size = terminal.size()?;
                    state.viewport = (size.width, size.height);
                    let visible = size.height.saturating_sub(2) as usize;
                    match state.key(key, visible, Instant::now()) {
                        Action::Quit => return Ok(None),
                        Action::Choose(entry) => return Ok(Some(entry)),
                        Action::Refresh => {
                            if let Err(e) = fetches.try_send(()) {
                                state.fetching = false;
                                state.status = format!("Couldn't refresh: {e}");
                            }
                        }
                        Action::Remove(entry) => {
                            let source = entry.source;
                            if let Err(e) = removals.try_send(Work::Remove(Box::new(entry))) {
                                state.removing = false;
                                state.status = removal_failed(source, &e);
                            }
                        }
                        Action::Paste => {
                            if removals.try_send(Work::Paste).is_err() {
                                state.pasting = false;
                                state.status = "Clipboard worker busy".into();
                            }
                        }
                        Action::None => {}
                    }
                    sequence.store(state.preview_seq, Ordering::Release);
                }
                Event::Paste(text) => state.insert_text(&text, Instant::now()),
                Event::Resize(width, height) => {
                    state.viewport = (width, height);
                }
                _ => {}
            }
        }
    }
}

fn clipboard_text() -> Result<String> {
    #[cfg(target_os = "macos")]
    let commands: &[(&str, &[&str])] = &[("pbpaste", &[])];
    #[cfg(target_os = "windows")]
    let commands: &[(&str, &[&str])] =
        &[("powershell", &["-NoProfile", "-Command", "Get-Clipboard"])];
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let commands: &[(&str, &[&str])] = &[
        ("wl-paste", &["--no-newline"]),
        ("xclip", &["-selection", "clipboard", "-o"]),
        ("xsel", &["--clipboard", "--output"]),
    ];
    let mut last = anyhow!("no clipboard command available");
    for (program, args) in commands {
        match crate::process::output(
            std::process::Command::new(program).args(*args),
            Duration::from_secs(2),
        ) {
            Ok(output) => return Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn source_style(source: Source) -> Style {
    Style::default().fg(match source {
        Source::Rex => Color::Blue,
        Source::RexBlock => Color::Green,
        Source::Config => Color::DarkGray,
        Source::Zoxide | Source::Directory => Color::Cyan,
        Source::Worktree => Color::Magenta,
    })
}

fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim().to_ascii_lowercase();
    if let Some(hex) = value.strip_prefix('#') {
        if !hex.is_ascii() {
            return None;
        }
        return match hex.len() {
            6 => Some(Color::Rgb(
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
            )),
            3 => Some(Color::Rgb(
                u8::from_str_radix(&hex[0..1], 16).ok()? * 17,
                u8::from_str_radix(&hex[1..2], 16).ok()? * 17,
                u8::from_str_radix(&hex[2..3], 16).ok()? * 17,
            )),
            _ => None,
        };
    }
    // Explicit ansi:N/indexed:N selects palette entries, even for 30..37.
    if let Some(index) = value
        .strip_prefix("ansi:")
        .or_else(|| value.strip_prefix("indexed:"))
    {
        return index.parse().ok().map(Color::Indexed);
    }
    if let Ok(code) = value.parse::<u16>() {
        return if matches!(code, 30..=37 | 90..=97) {
            Some(ansi_color(code))
        } else {
            u8::try_from(code).ok().map(Color::Indexed)
        };
    }
    Some(match value.replace('_', "-").as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::Gray,
        "gray" | "grey" | "bright-black" => Color::DarkGray,
        "bright-red" => Color::LightRed,
        "bright-green" => Color::LightGreen,
        "bright-yellow" => Color::LightYellow,
        "bright-blue" => Color::LightBlue,
        "bright-magenta" => Color::LightMagenta,
        "bright-cyan" => Color::LightCyan,
        "bright-white" => Color::White,
        "default" | "reset" => Color::Reset,
        _ => return None,
    })
}

fn render_window_format(
    format: &str,
    entry: &Entry,
    name: &str,
    index: usize,
    active: bool,
) -> String {
    let index = index.to_string();
    let mut remaining = format;
    let mut result = String::new();
    while !remaining.is_empty() {
        if let Some(rest) = remaining.strip_prefix("{{") {
            result.push('{');
            remaining = rest;
            continue;
        }
        if let Some(rest) = remaining.strip_prefix("}}") {
            result.push('}');
            remaining = rest;
            continue;
        }
        let found = [
            ("{window_name}", name),
            ("{name}", name),
            ("{session_name}", entry.name.as_str()),
            ("{active}", if active { "*" } else { "" }),
            ("{window_index}", index.as_str()),
            ("{window_active}", if active { "1" } else { "0" }),
        ]
        .into_iter()
        .find(|(key, _)| remaining.starts_with(key));
        if let Some((key, value)) = found {
            result.push_str(value);
            remaining = &remaining[key.len()..];
        } else {
            let ch = remaining.chars().next().unwrap();
            result.push(ch);
            remaining = &remaining[ch.len_utf8()..];
        }
    }
    result
}

fn window_names_text(entry: &Entry, format: &str, budget: usize) -> String {
    let fallback;
    let names = if entry.window_names.is_empty() && !entry.active_window_name.is_empty() {
        fallback = vec![entry.active_window_name.clone()];
        &fallback
    } else {
        &entry.window_names
    };
    let format = if format.is_empty() {
        "{window_name}"
    } else {
        format
    };
    let mut text = String::new();
    let mut used = 0;
    let mut shown = 0;
    for (i, name) in names.iter().enumerate() {
        let active = entry
            .active_window_index
            .map(|active| active == i + 1)
            .unwrap_or_else(|| name == &entry.active_window_name);
        let name = clean(name);
        let window = clean(&render_window_format(format, entry, &name, i + 1, active));
        let piece = format!(" {window}");
        let cells = UnicodeWidthStr::width(piece.as_str());
        let remaining = names.len() - i - 1;
        let reserve = if remaining > 0 {
            format!(" +{remaining}").len()
        } else {
            0
        };
        if used + cells + reserve > budget {
            break;
        }
        text.push_str(&piece);
        used += cells;
        shown += 1;
    }
    if shown < names.len() {
        let label = format!(" +{}", names.len() - shown);
        if used + label.len() <= budget {
            text.push_str(&label);
        }
    }
    text
}

fn clean(s: &str) -> String {
    // Entry names/config strings are untrusted terminal text too.
    strip_ansi_escapes::strip_str(s)
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

fn worktree_parts(entry: &Entry) -> (String, String) {
    let name = clean(&entry.name);
    let from_path = entry
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    let from_name = name.split_whitespace().find_map(|word| {
        let word = word.trim_start_matches('#');
        (!word.is_empty() && word.bytes().all(|b| b.is_ascii_digit())).then_some(word)
    });
    let number = from_path.or(from_name).unwrap_or("").to_owned();
    let title = if number.is_empty() {
        name.clone()
    } else if let Some((_, title)) = name.split_once(&format!("#{number}")) {
        title.trim().to_owned()
    } else if let Some(title) = name.strip_prefix(&number) {
        title.trim().to_owned()
    } else {
        name.clone()
    };
    (number, title)
}

fn truncate_cells(text: &str, width: usize) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut end = text.len();
    while end > 0 && UnicodeWidthStr::width(&text[..end]) + 1 > width {
        end = text[..end].char_indices().next_back().map_or(0, |(i, _)| i);
    }
    format!("{}…", &text[..end])
}

fn render_worktree_row(
    frame: &mut Frame<'_>,
    state: &State,
    m: &Match,
    mut spans: Vec<Span<'static>>,
    number_width: usize,
    area: Rect,
) {
    let (number, title) = worktree_parts(&m.entry);
    let red = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
    spans.push(Span::raw(
        " ".repeat(number_width.saturating_sub(number.len())),
    ));
    spans.extend(highlight_spans(&number, &m.indices, red));
    spans.push(Span::raw("  "));
    let color = match m.entry.active_window_name.as_str() {
        "OPEN" => Some(Color::Green),
        "CLOSED" => Some(Color::Red),
        "MERGED" => Some(Color::Magenta),
        _ => None,
    };
    if let Some(color) = color {
        let (left, right) = if state.options.show_icons {
            ("", "")
        } else {
            (" ", " ")
        };
        let badge = &m.entry.active_window_name;
        let caps = if state.options.show_icons {
            Style::default().fg(color)
        } else {
            Style::default().fg(color).add_modifier(Modifier::REVERSED)
        };
        spans.push(Span::styled(left, caps));
        spans.push(Span::styled(
            badge.clone(),
            Style::default().fg(color).add_modifier(Modifier::REVERSED),
        ));
        spans.push(Span::styled(right, caps));
        spans.push(Span::raw(
            " ".repeat(9usize.saturating_sub(badge.len() + 2)),
        ));
    } else {
        spans.push(Span::raw("         "));
    }
    let used: usize = spans.iter().map(Span::width).sum();
    let title = truncate_cells(&title, (area.width as usize).saturating_sub(used));
    let offset = number.chars().count() as u32 + 1;
    let title_indices: Vec<_> = m
        .indices
        .iter()
        .filter_map(|&i| i.checked_sub(offset))
        .collect();
    spans.extend(highlight_spans(&title, &title_indices, red));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn widths(state: &State, width: u16) -> (u16, u16) {
    if state.options.worktree {
        return (width, 0);
    }
    let min = if state.options.preview_min_width == 0 {
        100
    } else {
        state.options.preview_min_width
    };
    if !state.options.preview || usize::from(width) < min || width <= 42 {
        return (width.min(60), 0);
    }
    let pct = if state.options.preview_width == 0 {
        60
    } else {
        state.options.preview_width.clamp(10, 90)
    };
    let cols = ((u32::from(width) * u32::from(pct) / 100) as u16)
        .max(width.saturating_sub(60))
        .min(width.saturating_sub(40));
    (width - cols, cols)
}

fn highlight_spans(name: &str, indices: &[u32], highlight: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut matched = false;
    for (i, c) in clean(name).chars().enumerate() {
        // Keep combining marks/variation selectors with their styled base.
        let next = if c.width() == Some(0) && !run.is_empty() {
            matched
        } else {
            indices.binary_search(&(i as u32)).is_ok()
        };
        if next != matched && !run.is_empty() {
            spans.push(Span::styled(
                std::mem::take(&mut run),
                if matched { highlight } else { Style::default() },
            ));
        }
        matched = next;
        run.push(c);
    }
    if !run.is_empty() {
        spans.push(Span::styled(
            run,
            if matched { highlight } else { Style::default() },
        ));
    }
    spans
}

fn render(frame: &mut Frame<'_>, state: &mut State) {
    let area = frame.area();
    state.viewport = (area.width, area.height);
    if area.is_empty() {
        return;
    }
    let (list_width, preview_width) = widths(state, area.width);
    let list = Rect::new(area.x, area.y, list_width, area.height);
    let visible = area.height.saturating_sub(2) as usize;
    state.scroll(visible);
    let dim = Style::default().fg(Color::DarkGray);
    let faint = Style::default().add_modifier(Modifier::DIM);
    let prompt_style = Style::default().fg(Color::Indexed(7));
    let placeholder_style = Style::default().fg(Color::Indexed(240));
    let green = Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD);
    // Horizontally scroll the input at character boundaries and measure cells,
    // so Unicode editing never slices UTF-8 or places the cursor out of bounds.
    let prompt = clean(&state.options.prompt);
    let input_width = list_width.saturating_sub(2) as usize;
    let prompt_width = UnicodeWidthStr::width(prompt.as_str()).min(input_width);
    let query_budget = input_width.saturating_sub(prompt_width);
    let mut start = 0;
    let caret_byte = byte_at(&state.query, state.caret);
    while start < caret_byte
        && UnicodeWidthStr::width(&state.query[start..caret_byte]) >= query_budget.max(1)
    {
        start += state.query[start..]
            .chars()
            .next()
            .map_or(1, char::len_utf8);
    }
    let input = if state.query.is_empty() {
        Line::from(vec![
            Span::styled(prompt, prompt_style),
            Span::styled(clean(&state.options.placeholder), placeholder_style),
        ])
    } else {
        Line::from(vec![
            Span::styled(prompt, prompt_style),
            Span::raw(clean(&state.query[start..])),
        ])
    };
    frame.render_widget(
        Paragraph::new(input),
        Rect::new(
            list.x.saturating_add(2).min(list.right()),
            list.y,
            list_width.saturating_sub(2),
            1,
        ),
    );
    if list.height > 1 {
        let status = if state.options.worktree {
            truncate_cells(&format!("  {}", clean(&state.status)), list_width as usize)
        } else {
            format!("  {}", clean(&state.status))
        };
        frame.render_widget(
            Paragraph::new(status).style(if state.options.worktree {
                faint
            } else {
                Style::default().fg(Color::Yellow)
            }),
            Rect::new(
                list.x,
                list.y + 1,
                if preview_width == 0 {
                    area.width
                } else {
                    list_width
                },
                1,
            ),
        );
    }
    if state.loading && visible > 0 {
        frame.render_widget(
            Paragraph::new(if state.options.worktree {
                "  Loading worktrees..."
            } else {
                "  Loading sessions..."
            })
            .style(faint),
            Rect::new(list.x, list.y + 2, list.width, 1),
        );
    } else if state.options.worktree && state.entries.is_empty() && visible > 0 {
        frame.render_widget(
            Paragraph::new("  No worktrees found").style(faint),
            Rect::new(list.x, list.y + 2, list.width, 1),
        );
    } else if state.alias_query().is_some() && state.options.aliases.is_empty() && visible > 0 {
        frame.render_widget(
            Paragraph::new("  No aliases configured").style(faint),
            Rect::new(list.x, list.y + 2, list.width, 1),
        );
    } else {
        let index_mode = state.index_query().is_some();
        let icon_width = state
            .options
            .glyphs
            .values()
            .map(|i| UnicodeWidthStr::width(clean(i).trim_end_matches(' ')))
            .max()
            .unwrap_or(1)
            .max(state.options.icon_width.max(1))
            .min(usize::from(list_width));
        for (line, row) in state
            .rows
            .iter()
            .skip(state.offset)
            .take(visible)
            .enumerate()
        {
            let row_width = if preview_width == 0 && !state.options.worktree {
                area.width
            } else {
                list.width
            };
            let row_area = Rect::new(list.x, list.y + 2 + line as u16, row_width, 1);
            let Some(index) = row else {
                frame.render_widget(
                    Paragraph::new(format!(
                        "  {}",
                        "─".repeat(list.width.saturating_sub(2) as usize)
                    ))
                    .style(dim),
                    row_area,
                );
                continue;
            };
            let m = &state.filtered[*index];
            let pointer = if *index == state.cursor {
                let p = &state.options.pointer;
                Span::styled(
                    format!(
                        "{p}{}",
                        " ".repeat(2usize.saturating_sub(UnicodeWidthStr::width(p.as_str())))
                    ),
                    green,
                )
            } else {
                Span::raw("  ")
            };
            let mut spans = vec![pointer];
            if state.options.worktree {
                let number_width = state
                    .entries
                    .iter()
                    .map(|e| worktree_parts(e).0.len())
                    .max()
                    .unwrap_or(1);
                render_worktree_row(frame, state, m, spans, number_width, row_area);
                continue;
            }
            if index_mode {
                spans.push(Span::styled(
                    if *index < 9 {
                        format!("{} ", index + 1)
                    } else {
                        "  ".into()
                    },
                    Style::default().fg(Color::Blue),
                ));
            }
            if state.options.show_icons {
                let custom = m.entry.icon.as_deref().filter(|s| !s.is_empty());
                let icon = clean(custom.unwrap_or_else(|| {
                    state
                        .options
                        .glyphs
                        .get(m.entry.source.as_str())
                        .filter(|icon| !icon.is_empty())
                        .map_or_else(
                            || match m.entry.source {
                                Source::Zoxide | Source::Directory => "",
                                _ => m.entry.source.glyph(),
                            },
                            String::as_str,
                        )
                }));
                let cells = UnicodeWidthStr::width(if custom.is_some() {
                    icon.trim_end_matches(' ')
                } else {
                    &icon
                });
                let style = if custom.is_some() {
                    Style::default()
                } else {
                    state
                        .options
                        .glyph_colors
                        .get(m.entry.source.as_str())
                        .and_then(|color| parse_color(color))
                        .map_or_else(
                            || source_style(m.entry.source),
                            |color| Style::default().fg(color),
                        )
                };
                spans.push(Span::styled(
                    format!("{icon}{} ", " ".repeat(icon_width.saturating_sub(cells))),
                    style,
                ));
            }
            if m.entry.source == Source::Worktree
                && matches!(
                    m.entry.active_window_name.as_str(),
                    "OPEN" | "MERGED" | "CLOSED"
                )
            {
                let color = match m.entry.active_window_name.as_str() {
                    "OPEN" => Color::Green,
                    "MERGED" => Color::Magenta,
                    _ => Color::Red,
                };
                spans.push(Span::styled(
                    format!("[{}] ", m.entry.active_window_name),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ));
            }
            if let Some(alias) = state.alias_for(&m.entry.name) {
                let base = Style::default().add_modifier(Modifier::REVERSED);
                let (left, right) = if state.options.show_icons {
                    ("", "")
                } else {
                    ("[", "]")
                };
                let caps = if state.options.show_icons {
                    Style::default()
                } else {
                    base
                };
                spans.push(Span::styled(
                    left,
                    if m.chip_match > 0 {
                        caps.fg(Color::Green)
                    } else {
                        caps
                    },
                ));
                for (i, c) in clean(&alias.alias).chars().enumerate() {
                    spans.push(Span::styled(
                        c.to_string(),
                        if i < m.chip_match {
                            base.fg(Color::Green)
                        } else {
                            base
                        },
                    ));
                }
                spans.push(Span::styled(
                    right,
                    if m.chip_match > 0 && m.chip_match >= alias.alias.chars().count() {
                        caps.fg(Color::Green)
                    } else {
                        caps
                    },
                ));
                spans.push(Span::raw(" "));
            }
            spans.extend(highlight_spans(&m.entry.name, &m.indices, green));
            if state.options.show_windows
                && !(m.entry.source == Source::Worktree
                    && matches!(
                        m.entry.active_window_name.as_str(),
                        "OPEN" | "MERGED" | "CLOSED"
                    ))
            {
                let used: usize = spans.iter().map(Span::width).sum();
                let windows = window_names_text(
                    &m.entry,
                    &state.options.window_name_format,
                    (list_width as usize).saturating_sub(used),
                );
                if !windows.is_empty() {
                    spans.push(Span::styled(windows, dim.add_modifier(Modifier::DIM)));
                }
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), row_area);
        }
    }
    if preview_width > 0 {
        let pane = Rect::new(area.x + list_width, area.y, preview_width, area.height);
        let divider = match state.options.preview_border.as_str() {
            "none" => None,
            "thick" => Some("┃"),
            "double" => Some("║"),
            _ => Some("│"),
        };
        if let Some(symbol) = divider {
            for y in pane.y..pane.bottom() {
                frame.render_widget(
                    Paragraph::new(symbol).style(dim),
                    Rect::new(pane.x, y, 1, 1),
                );
            }
        }
        let chrome = if divider.is_some() { 2 } else { 1 };
        let body = Rect::new(
            pane.x + chrome,
            pane.y + 2.min(pane.height),
            pane.width.saturating_sub(chrome),
            pane.height.saturating_sub(2),
        );
        let text = state
            .preview_text
            .as_deref()
            .unwrap_or("Loading preview...");
        let text = if text.trim().is_empty() {
            "No preview"
        } else {
            text
        };
        frame.render_widget(
            Paragraph::new(ansi_preview(text, body.height as usize)).style(
                if state.preview_text.as_ref().is_none_or(|value| {
                    value.trim().is_empty() || value.starts_with("Preview unavailable:")
                }) {
                    faint
                } else {
                    Style::default()
                },
            ),
            body,
        );
    }
    if let Some(confirm) = &state.confirm {
        render_confirm(frame, confirm, &state.status);
    } else if list_width > 2 && query_budget > 0 {
        let cells = UnicodeWidthStr::width(&state.query[start..caret_byte]);
        let x = (2 + prompt_width + cells).min(list_width.saturating_sub(1) as usize);
        if state.options.worktree {
            if virtual_cursor_visible(state, Instant::now()) {
                let position = (list.x + x as u16, list.y);
                let cell = &mut frame.buffer_mut()[position];
                cell.set_style(
                    Style::reset()
                        .fg(Color::Indexed(7))
                        .add_modifier(Modifier::REVERSED),
                );
            }
        } else {
            frame.set_cursor_position((list.x + x as u16, list.y));
        }
    }
}

fn virtual_cursor_visible(state: &State, now: Instant) -> bool {
    (now.saturating_duration_since(state.cursor_blink_started)
        .as_millis()
        / CURSOR_BLINK.as_millis())
    .is_multiple_of(2)
}

fn confirmation_fits(width: u16, height: u16) -> bool {
    width >= 12 && height >= 3
}

fn confirm_prompt_lines(prompt: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in prompt.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn render_confirm(frame: &mut Frame<'_>, confirm: &Confirm, status: &str) {
    let area = frame.area();
    if !confirmation_fits(area.width, area.height) {
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(if status.starts_with("Confirmation disabled") {
                status
            } else {
                "Resize terminal to confirm; Esc cancels"
            }),
            area,
        );
        return;
    }
    let prompt = if confirm.entry.source == Source::Rex {
        "Do you want to kill this Rex session?"
    } else {
        "Do you want to remove this directory from zoxide?"
    };
    let path_text = clean(&confirm.entry.path.to_string_lossy());
    let name_text = clean(&confirm.entry.name);
    let show_path =
        confirm.entry.source != Source::Rex && !path_text.is_empty() && path_text != name_text;
    let inner = area.width.saturating_sub(6).clamp(40, 60);
    let prompt_lines = confirm_prompt_lines(prompt, inner as usize);
    let body_height = prompt_lines.len() as u16 + 4 + u16::from(show_path);
    let full_width = inner + 6;
    let full_height = body_height + 4;
    let full = area.width >= full_width && area.height >= full_height;
    let selected = Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD);
    let faint = Style::default().add_modifier(Modifier::DIM);
    let buttons = Line::from(vec![
        Span::styled(" Yes ", if confirm.yes { selected } else { faint }),
        Span::raw("   "),
        Span::styled(" No ", if confirm.yes { faint } else { selected }),
    ]);
    let content_width = if full { inner } else { area.width.min(66) };
    let name = Line::styled(
        truncate_cells(&name_text, content_width as usize),
        Style::default().add_modifier(Modifier::BOLD),
    );
    let mut text = if full {
        prompt_lines.into_iter().map(Line::raw).collect::<Vec<_>>()
    } else {
        vec![Line::raw(truncate_cells(prompt, content_width as usize))]
    };
    if full {
        text.push(Line::raw(""));
    }
    text.push(name);
    if show_path && (full || area.height >= 4) {
        text.push(Line::styled(
            truncate_cells(&path_text, content_width as usize),
            faint,
        ));
    }
    if full {
        text.push(Line::raw(""));
    }
    text.push(buttons);
    let width = if full { full_width } else { content_width };
    let height = if full { full_height } else { text.len() as u16 };
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let body = if full {
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Blue)),
            popup,
        );
        Rect::new(popup.x + 3, popup.y + 2, inner, body_height)
    } else {
        popup
    };
    frame.render_widget(Paragraph::new(text).alignment(Alignment::Center), body);
}

/// Parse only SGR styling into ratatui spans. Never pass raw terminal escape
/// sequences to the backend. OSC (including clipboard/hyperlink), DCS, cursor
/// motion and other controls are discarded. Each preview line resets its style.
fn ansi_preview(text: &str, max_lines: usize) -> Text<'static> {
    let mut lines = Vec::new();
    let mut spans = Vec::new();
    let mut plain = String::new();
    let mut style = Style::default();
    let mut chars = text.chars().peekable();
    fn flush(plain: &mut String, spans: &mut Vec<Span<'static>>, style: Style) {
        if !plain.is_empty() {
            spans.push(Span::styled(std::mem::take(plain), style));
        }
    }
    while let Some(c) = chars.next() {
        if lines.len() >= max_lines {
            break;
        }
        match c {
            '\n' => {
                flush(&mut plain, &mut spans, style);
                lines.push(Line::from(std::mem::take(&mut spans)));
                style = Style::default();
            }
            '\t' => plain.push_str("    "),
            '\u{1b}' => {
                match chars.next() {
                    Some('[') => {
                        let mut params = String::new();
                        for next in chars.by_ref() {
                            if ('\u{40}'..='\u{7e}').contains(&next) {
                                if next == 'm' {
                                    flush(&mut plain, &mut spans, style);
                                    sgr(&params, &mut style);
                                }
                                break;
                            }
                            // Bound allocation even for malformed/hostile escape sequences.
                            if params.len() < 128 {
                                params.push(next);
                            }
                        }
                    }
                    Some(']' | 'P' | 'X' | '^' | '_') => {
                        let mut escape = false;
                        for next in chars.by_ref() {
                            if next == '\u{7}' || (escape && next == '\\') {
                                break;
                            }
                            escape = next == '\u{1b}';
                        }
                    }
                    Some('\u{20}'..='\u{2f}') => {
                        for next in chars.by_ref() {
                            if ('\u{30}'..='\u{7e}').contains(&next) {
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
            c if c.is_control() => {}
            c => plain.push(c),
        }
    }
    if lines.len() < max_lines {
        flush(&mut plain, &mut spans, style);
        lines.push(Line::from(spans));
    }
    Text::from(lines)
}

fn sgr(params: &str, style: &mut Style) {
    let values: Vec<u16> = if params.is_empty() {
        vec![0]
    } else {
        let Some(values) = params
            .split(';')
            .map(|p| {
                if p.is_empty() {
                    Some(0)
                } else {
                    p.parse().ok()
                }
            })
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        values
    };
    let mut i = 0;
    while i < values.len() {
        let n = values[i];
        match n {
            0 => *style = Style::default(),
            1 => *style = style.add_modifier(Modifier::BOLD),
            2 => *style = style.add_modifier(Modifier::DIM),
            3 => *style = style.add_modifier(Modifier::ITALIC),
            4 => *style = style.add_modifier(Modifier::UNDERLINED),
            7 => *style = style.add_modifier(Modifier::REVERSED),
            9 => *style = style.add_modifier(Modifier::CROSSED_OUT),
            22 => *style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => *style = style.remove_modifier(Modifier::ITALIC),
            24 => *style = style.remove_modifier(Modifier::UNDERLINED),
            27 => *style = style.remove_modifier(Modifier::REVERSED),
            29 => *style = style.remove_modifier(Modifier::CROSSED_OUT),
            30..=37 | 90..=97 => *style = style.fg(ansi_color(n)),
            40..=47 | 100..=107 => *style = style.bg(ansi_color(n - 10)),
            39 => *style = style.fg(Color::Reset),
            49 => *style = style.bg(Color::Reset),
            38 | 48 => {
                let mut color = None;
                if values.get(i + 1) == Some(&5) {
                    if let Some(&value) = values.get(i + 2) {
                        color = u8::try_from(value).ok().map(Color::Indexed);
                        i += 2;
                    }
                } else if values.get(i + 1) == Some(&2) && i + 4 < values.len() {
                    if let (Ok(r), Ok(g), Ok(b)) = (
                        u8::try_from(values[i + 2]),
                        u8::try_from(values[i + 3]),
                        u8::try_from(values[i + 4]),
                    ) {
                        color = Some(Color::Rgb(r, g, b));
                    }
                    i += 4;
                }
                if let Some(color) = color {
                    *style = if n == 38 {
                        style.fg(color)
                    } else {
                        style.bg(color)
                    };
                }
            }
            _ => {}
        }
        i += 1;
    }
}

fn ansi_color(n: u16) -> Color {
    match n {
        30 => Color::Black,
        31 => Color::Red,
        32 => Color::Green,
        33 => Color::Yellow,
        34 => Color::Blue,
        35 => Color::Magenta,
        36 => Color::Cyan,
        37 => Color::Gray,
        90 => Color::DarkGray,
        91 => Color::LightRed,
        92 => Color::LightGreen,
        93 => Color::LightYellow,
        94 => Color::LightBlue,
        95 => Color::LightMagenta,
        96 => Color::LightCyan,
        _ => Color::White,
    }
}

static INTERRUPTED: AtomicU64 = AtomicU64::new(0);

// POSIX handlers do nothing except an atomic store; cleanup happens on the main
// thread, never inside a signal handler. SIGKILL/SIGSTOP cannot be intercepted.
#[cfg(unix)]
mod signals {
    use super::*;
    extern "C" fn interrupted(number: i32) {
        INTERRUPTED.store(number as u64, Ordering::Relaxed);
    }
    pub struct Guard(Vec<(i32, libc::sigaction)>);
    impl Guard {
        pub fn new() -> Result<Self> {
            let mut guard = Self(Vec::new());
            for number in [libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM] {
                // SAFETY: zeroed sigaction is initialized before use. The handler
                // has C ABI, only stores an atomic flag, and never accesses IO.
                let previous = unsafe {
                    let mut action: libc::sigaction = std::mem::zeroed();
                    let mut previous: libc::sigaction = std::mem::zeroed();
                    action.sa_sigaction = interrupted as *const () as usize;
                    libc::sigemptyset(&mut action.sa_mask);
                    action.sa_flags = libc::SA_RESTART;
                    if libc::sigaction(number, &action, &mut previous) != 0 {
                        return Err(io::Error::last_os_error().into());
                    }
                    previous
                };
                guard.0.push((number, previous));
            }
            Ok(guard)
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            for (number, previous) in self.0.iter().rev() {
                // SAFETY: restore the complete disposition, including mask and
                // flags. Unlike unregistering signal-hook actions this does not
                // leave termination signals ignored after a library call.
                unsafe {
                    libc::sigaction(*number, previous, std::ptr::null_mut());
                }
            }
        }
    }
}

fn restore_terminal(active: &AtomicBool) {
    if active.swap(false, Ordering::AcqRel) {
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            SetCursorStyle::DefaultUserShape,
            crossterm::style::Print(RESET_CURSOR_COLOR),
            Show,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}

type PanicHook = Box<dyn Fn(&panic::PanicHookInfo<'_>) + Send + Sync + 'static>;
struct TerminalGuard {
    active: Arc<AtomicBool>,
    old_hook: Option<Arc<PanicHook>>,
    #[cfg(unix)]
    _signals: signals::Guard,
}
impl TerminalGuard {
    fn enter() -> Result<Self> {
        INTERRUPTED.store(0, Ordering::Relaxed);
        #[cfg(unix)]
        let signals = signals::Guard::new()?;
        let active = Arc::new(AtomicBool::new(true));
        let old_hook = Arc::new(panic::take_hook());
        let hook_active = active.clone();
        let hook_previous = old_hook.clone();
        // Worker callback panics are caught and reported as errors. Only a panic
        // on the owning thread should tear down the terminal before printing.
        let owner = thread::current().id();
        panic::set_hook(Box::new(move |info| {
            if thread::current().id() == owner {
                restore_terminal(&hook_active);
                hook_previous(info);
            } else if !thread::current()
                .name()
                .is_some_and(|name| name.starts_with("picker-"))
            {
                hook_previous(info);
            }
        }));
        let guard = Self {
            active,
            old_hook: Some(old_hook),
            #[cfg(unix)]
            _signals: signals,
        };
        enable_raw_mode()?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            crossterm::style::Print(RESET_CURSOR_COLOR),
            Hide
        )?;
        Ok(guard)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal(&self.active);
        // set_hook is forbidden while unwinding. catch_unwind around the event
        // loop below ensures this guard normally drops outside an unwind.
        if !thread::panicking()
            && let Some(old) = self.old_hook.take()
        {
            let _ = panic::take_hook();
            match Arc::try_unwrap(old) {
                Ok(hook) => panic::set_hook(hook),
                Err(old) => panic::set_hook(Box::new(move |info| old(info))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn entries() -> Vec<Entry> {
        vec![
            Entry::new("alpha", "/a", Source::Rex),
            Entry::new("日本語-project", "/b", Source::Config),
            Entry::new("gamma", "/c", Source::Zoxide),
        ]
        .into_iter()
        .enumerate()
        .map(|(group, mut entry)| {
            entry.group = group;
            entry
        })
        .collect()
    }
    fn state(options: PickerOptions) -> State {
        let mut s = State::new(options);
        s.loaded(entries(), Instant::now());
        s
    }
    fn key(s: &mut State, code: KeyCode, ctrl: bool) -> Action {
        s.key(
            KeyEvent::new(
                code,
                if ctrl {
                    KeyModifiers::CONTROL
                } else {
                    KeyModifiers::NONE
                },
            ),
            5,
            Instant::now(),
        )
    }
    fn snapshot(s: &mut State, width: u16, height: u16) -> (String, ratatui::buffer::Buffer) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render(f, s)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let text = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        (text, buffer)
    }

    #[test]
    fn blank_query_order_separators_and_navigation() {
        let mut s = state(PickerOptions {
            group_separator: true,
            ..Default::default()
        });
        assert_eq!(s.rows, vec![Some(0), None, Some(1), None, Some(2)]);
        key(&mut s, KeyCode::Char('j'), true);
        assert_eq!(s.cursor, 1);
        s.scroll(2);
        assert_eq!(s.offset, 1);
        key(&mut s, KeyCode::Char('n'), true);
        assert_eq!(s.cursor, 2);
        key(&mut s, KeyCode::Char('k'), true);
        assert_eq!(s.cursor, 1);
        key(&mut s, KeyCode::Char('p'), true);
        assert_eq!(s.cursor, 0);
        key(&mut s, KeyCode::PageDown, false);
        assert_eq!(s.cursor, 2);
        key(&mut s, KeyCode::PageUp, false);
        assert_eq!(s.cursor, 0);
    }

    #[test]
    fn unicode_filter_edit_and_highlight() {
        let mut s = state(PickerOptions::default());
        key(&mut s, KeyCode::Char('日'), false);
        assert_eq!(s.filtered.len(), 1);
        assert_eq!(s.filtered[0].indices, vec![0]);
        let (_, buffer) = snapshot(&mut s, 80, 24);
        assert!(
            buffer
                .content
                .iter()
                .any(|cell| cell.symbol() == "日" && cell.fg == Color::Green)
        );
        key(&mut s, KeyCode::Left, false);
        key(&mut s, KeyCode::Delete, false);
        assert!(s.query.is_empty());
        key(&mut s, KeyCode::Char('語'), false);
        key(&mut s, KeyCode::Backspace, false);
        assert!(s.query.is_empty());
    }

    #[test]
    fn index_mode_and_alias_precedence() {
        let mut s = state(PickerOptions::default());
        key(&mut s, KeyCode::Char('#'), false);
        assert_eq!(s.query, "#");
        assert!(
            matches!(key(&mut s, KeyCode::Char('2'), false), Action::Choose(e) if e.name == "日本語-project")
        );
        assert!(matches!(
            key(&mut s, KeyCode::Char('9'), false),
            Action::None
        ));
        assert_eq!(s.query, "#");
        key(&mut s, KeyCode::Backspace, false);
        assert!(s.query.is_empty());
        s.options.alias_filter_prefix = "#".into();
        key(&mut s, KeyCode::Char('#'), false);
        assert!(s.index_query().is_none());
    }

    #[test]
    fn alias_modes_missing_targets_and_debounce() {
        let mut options = PickerOptions {
            query: "a".into(),
            ..Default::default()
        };
        options.aliases.insert(
            "A".into(),
            Alias {
                alias: "A".into(),
                target: "absent".into(),
                auto_connect: true,
            },
        );
        options.aliases.insert(
            "b".into(),
            Alias {
                alias: "b".into(),
                target: "gamma".into(),
                auto_connect: false,
            },
        );
        let mut s = state(options);
        assert_eq!(s.filtered[0].entry.name, "absent");
        assert!(s.alias_due.is_none());
        key(&mut s, KeyCode::Backspace, false);
        key(&mut s, KeyCode::Char('A'), false);
        assert!(s.alias_due.is_some());
        key(&mut s, KeyCode::Char('x'), false);
        assert!(s.alias_due.is_none());
        s.query = "/b".into();
        s.caret = 2;
        s.changed_query(Instant::now());
        assert_eq!(s.exact_alias().unwrap().target, "gamma");
        assert!(s.alias_due.is_some());
        s.options.disable_alias_auto_connect = true;
        s.changed_query(Instant::now());
        assert!(s.alias_due.is_none());
    }

    #[test]
    fn session_cursor_is_real_and_unicode_cell_positioned() {
        let mut s = state(PickerOptions {
            query: "a日本e\u{301}".into(),
            ..Default::default()
        });
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| render(frame, &mut s)).unwrap();
        assert_eq!(terminal.get_cursor_position().unwrap(), (10, 0).into());
        assert!(
            !terminal.backend().buffer()[(10, 0)]
                .modifier
                .contains(Modifier::REVERSED)
        );
        key(&mut s, KeyCode::Left, false);
        key(&mut s, KeyCode::Left, false);
        terminal.draw(|frame| render(frame, &mut s)).unwrap();
        assert_eq!(terminal.get_cursor_position().unwrap(), (9, 0).into());
        assert_eq!(format!("{}", SetCursorStyle::SteadyBlock), "\x1b[2 q");
        assert_eq!(format!("{}", SetCursorStyle::DefaultUserShape), "\x1b[0 q");
        assert_eq!(RESET_CURSOR_COLOR, "\x1b]112\x07");
    }

    #[test]
    fn worktree_virtual_cursor_blinks_on_placeholder_and_wide_query() {
        let mut s = state(PickerOptions {
            worktree: true,
            ..Default::default()
        });
        let now = Instant::now();
        s.cursor_blink_started = now;
        assert!(virtual_cursor_visible(&s, now));
        assert!(!virtual_cursor_visible(&s, now + CURSOR_BLINK));
        assert!(virtual_cursor_visible(&s, now + CURSOR_BLINK * 2));
        let (_, b) = snapshot(&mut s, 80, 24);
        assert_eq!(b[(4, 0)].symbol(), "F");
        assert_eq!(b[(4, 0)].fg, Color::Indexed(7));
        assert!(b[(4, 0)].modifier.contains(Modifier::REVERSED));
        assert_eq!(b[(5, 0)].fg, Color::Indexed(240));
        s.cursor_blink_started = Instant::now() - CURSOR_BLINK;
        let (_, b) = snapshot(&mut s, 80, 24);
        assert_eq!(b[(4, 0)].fg, Color::Indexed(240));
        assert!(!b[(4, 0)].modifier.contains(Modifier::REVERSED));
        s.insert_text("a日本", Instant::now());
        key(&mut s, KeyCode::Left, false);
        let (_, b) = snapshot(&mut s, 80, 24);
        assert_eq!(b[(7, 0)].symbol(), "本");
        assert_eq!(b[(7, 0)].fg, Color::Indexed(7));
        assert!(b[(7, 0)].modifier.contains(Modifier::REVERSED));
        assert_eq!(b[(5, 0)].fg, Color::Reset);
        assert!(!b[(5, 0)].modifier.contains(Modifier::REVERSED));
        // Long Unicode input must scroll without splitting UTF-8 or exceeding the input.
        s.query = "日本".repeat(40);
        s.caret = s.query.chars().count();
        snapshot(&mut s, 12, 3);
    }

    #[test]
    fn worktree_square_badge_caps_are_filled_and_number_padding_is_normal() {
        let mut s = State::new(PickerOptions {
            worktree: true,
            show_icons: false,
            ..Default::default()
        });
        let mut rows = Vec::new();
        for (number, label) in [(1, "OPEN"), (22, "CLOSED"), (333, "MERGED")] {
            let mut row = Entry::new(
                format!("#{number} title"),
                format!("/wk/{number}"),
                Source::Worktree,
            );
            row.active_window_name = label.into();
            rows.push(row);
        }
        s.loaded(rows, Instant::now());
        let (_, b) = snapshot(&mut s, 80, 24);
        for (y, label, color) in [
            (2, "OPEN", Color::Green),
            (3, "CLOSED", Color::Red),
            (4, "MERGED", Color::Magenta),
        ] {
            for x in 7..7 + label.len() as u16 + 2 {
                assert_eq!(b[(x, y)].fg, color);
                assert!(b[(x, y)].modifier.contains(Modifier::REVERSED));
            }
            assert_eq!(b[(7, y)].symbol(), " ");
            assert_eq!(b[(8 + label.len() as u16, y)].symbol(), " ");
            assert_eq!(b[(6, y)].fg, Color::Reset);
            assert!(b[(6, y)].modifier.is_empty());
        }
        assert_eq!(b[(2, 2)].fg, Color::Reset);
        assert!(b[(2, 2)].modifier.is_empty());
    }

    #[test]
    fn worktree_empty_catalog_differs_from_unmatched_query() {
        let mut s = State::new(PickerOptions {
            worktree: true,
            ..Default::default()
        });
        s.loaded(Vec::new(), Instant::now());
        assert!(!s.refresh_pending);
        assert!(snapshot(&mut s, 80, 24).0.contains("No worktrees found"));
        s.loaded(
            vec![Entry::new("#1 title", "/wk/1", Source::Worktree)],
            Instant::now(),
        );
        s.insert_text("unmatched", Instant::now());
        assert!(s.filtered.is_empty());
        assert!(!snapshot(&mut s, 80, 24).0.contains("No worktrees found"));
    }

    #[test]
    fn worktree_refresh_header_and_errors_keep_cached_rows_and_survive_keys() {
        let mut s = State::new(PickerOptions {
            worktree: true,
            ..Default::default()
        });
        let rows = vec![Entry::new("#1 title", "/wk/1", Source::Worktree)];
        s.loaded(rows.clone(), Instant::now());
        assert!(s.refresh_pending);
        s.start_refresh(false);
        let (text, b) = snapshot(&mut s, 80, 24);
        assert!(
            text.lines()
                .nth(1)
                .unwrap()
                .contains("Refreshing worktrees...")
        );
        assert!(text.contains("title"));
        assert_eq!(b[(2, 1)].fg, Color::Reset);
        assert!(b[(2, 1)].modifier.contains(Modifier::DIM));
        s.refresh_failed(&"offline");
        assert!(s.status.is_empty());
        assert_eq!(s.entries.len(), 1);
        assert!(matches!(
            key(&mut s, KeyCode::Char('r'), true),
            Action::Refresh
        ));
        s.refresh_failed(&"network 日本 unavailable");
        let error = s.status.clone();
        key(&mut s, KeyCode::Down, false);
        key(&mut s, KeyCode::Char('t'), false);
        key(&mut s, KeyCode::Left, false);
        assert_eq!(s.status, error);
        let (text, _) = snapshot(&mut s, 18, 6);
        assert_eq!(text.lines().nth(1).unwrap(), "  Refresh failed:…");
        assert_eq!(s.entries.len(), 1);
        assert!(snapshot(&mut s, 80, 6).0.contains("title"));
        key(&mut s, KeyCode::Char('r'), true);
        assert_eq!(s.status, "Refreshing worktrees...");
        s.loaded(rows, Instant::now());
        assert!(s.status.is_empty());
    }

    #[test]
    fn displayed_preview_stays_during_debounce_and_fetch_but_not_empty_selection() {
        let mut s = state(PickerOptions {
            preview: true,
            ..Default::default()
        });
        s.preview_loaded(s.preview_seq, Ok("last displayed".into()));
        let old_seq = s.preview_seq;
        let now = Instant::now();
        s.move_cursor(1, now);
        assert_eq!(s.preview_due, Some(now + PREVIEW_DEBOUNCE));
        assert!(snapshot(&mut s, 120, 24).0.contains("last displayed"));
        s.preview_due = None; // Worker accepted the fetch; the event loop is still free.
        s.preview_loaded(old_seq, Ok("stale".into()));
        assert!(snapshot(&mut s, 120, 24).0.contains("last displayed"));
        s.preview_loaded(s.preview_seq, Ok("next displayed".into()));
        assert!(snapshot(&mut s, 120, 24).0.contains("next displayed"));
        s.insert_text("unmatched", Instant::now());
        assert!(s.preview_text.is_none());
    }

    #[test]
    fn long_session_names_use_physical_width_without_preview_split() {
        let name = format!("{}日本-tail", "a".repeat(61));
        let mut s = State::new(PickerOptions {
            show_icons: false,
            show_windows: true,
            ..Default::default()
        });
        let mut entry = Entry::new(&name, "/a", Source::Rex);
        entry.window_names = vec!["WINDOW_MUST_BE_ELIDED".into()];
        s.loaded(vec![entry], Instant::now());
        assert_eq!(widths(&s, 100), (60, 0));
        let (text, b) = snapshot(&mut s, 100, 8);
        assert!(text.contains("-tail"));
        assert!(!text.contains("WINDOW_MUST_BE_ELIDED"));
        assert_eq!(b[(63, 2)].symbol(), "日");
        assert_eq!(b[(65, 2)].symbol(), "本");
        s.options.preview = true;
        let (text, b) = snapshot(&mut s, 120, 8);
        assert_eq!(b[(48, 2)].symbol(), "│");
        assert!(!text.contains("日本-tail"));
    }

    #[test]
    fn icon_compensation_spaces_and_unicode_whitespace_keep_their_text() {
        let mut s = State::new(PickerOptions {
            show_icons: true,
            icon_width: 4,
            ..Default::default()
        });
        let mut custom = Entry::new("custom", "/c", Source::Config);
        custom.icon = Some("日  ".into());
        let mut unicode_space = Entry::new("unicode", "/u", Source::Config);
        unicode_space.icon = Some("x\u{3000}".into());
        s.options.glyphs.insert("rex".into(), "R  ".into());
        s.loaded(
            vec![
                custom,
                unicode_space,
                Entry::new("source", "/s", Source::Rex),
            ],
            Instant::now(),
        );
        let (_, b) = snapshot(&mut s, 80, 8);
        assert_eq!(b[(2, 2)].symbol(), "日");
        for x in 4..9 {
            assert_eq!(b[(x, 2)].symbol(), " ");
        }
        assert_eq!(b[(9, 2)].symbol(), "c");
        assert_eq!(b[(3, 3)].symbol(), "\u{3000}");
        assert_eq!(b[(5, 3)].symbol(), " ");
        assert_eq!(b[(6, 3)].symbol(), " ");
        assert_eq!(b[(7, 3)].symbol(), "u");
        assert_eq!(b[(7, 4)].symbol(), "s");
        assert_eq!(b[(2, 2)].fg, Color::Reset);
    }

    #[test]
    fn removal_errors_name_the_action_and_success_preserves_scrolled_viewport() {
        let mut s = State::new(PickerOptions {
            show_icons: false,
            ..Default::default()
        });
        s.loaded(
            (0..20)
                .map(|i| Entry::new(format!("session-{i}"), format!("/{i}"), Source::Rex))
                .collect(),
            Instant::now(),
        );
        s.cursor = 12;
        s.offset = 10;
        snapshot(&mut s, 80, 7);
        let target = s.selected().unwrap().clone();
        s.removed(&target, Err(anyhow!("denied")), Instant::now());
        assert_eq!(s.status, "Couldn't kill session: denied");
        assert_eq!(s.offset, 10);
        let directory = Entry::new("dir", "/dir", Source::Zoxide);
        s.removed(&directory, Err(anyhow!("denied")), Instant::now());
        assert_eq!(s.status, "Couldn't remove entry: denied");
        s.removed(&target, Ok(()), Instant::now());
        snapshot(&mut s, 80, 7);
        assert_eq!(s.offset, 10);
        assert_eq!(s.cursor, 12);
        assert_eq!(s.selected().unwrap().name, "session-13");
        s.cursor = s.filtered.len() - 1;
        snapshot(&mut s, 80, 7);
        let target = s.selected().unwrap().clone();
        s.removed(&target, Ok(()), Instant::now());
        snapshot(&mut s, 80, 7);
        assert_eq!(s.cursor, 17);
        assert_eq!(s.offset, 14);
    }

    #[test]
    fn state_preserves_literal_and_empty_worktree_placeholders() {
        for placeholder in ["Filter sessions...", ""] {
            let s = State::new(PickerOptions {
                worktree: true,
                placeholder: placeholder.into(),
                ..Default::default()
            });
            assert_eq!(s.options.placeholder, placeholder);
        }
    }

    #[test]
    fn preview_debounce_invalidates_moves_and_toggle() {
        let now = Instant::now();
        let mut s = state(PickerOptions {
            preview: true,
            ..Default::default()
        });
        s.preview_entry = None;
        s.schedule_preview(now);
        let first = s.preview_seq;
        assert_eq!(s.preview_due, Some(now + PREVIEW_DEBOUNCE));
        s.move_cursor(1, now + Duration::from_millis(10));
        assert_ne!(first, s.preview_seq);
        assert_eq!(s.preview_due, Some(now + Duration::from_millis(70)));
        key(&mut s, KeyCode::Char('o'), true);
        assert!(s.preview_due.is_none());
        let seq = s.preview_seq;
        key(&mut s, KeyCode::Char('o'), true);
        assert!(s.preview_due.is_some());
        assert_ne!(seq, s.preview_seq);
    }

    #[test]
    fn confirmation_owns_keys_and_removal_preserves_failed_rows() {
        let mut s = state(PickerOptions::default());
        key(&mut s, KeyCode::Char('x'), true);
        assert!(s.confirm.as_ref().unwrap().yes);
        key(&mut s, KeyCode::Char('z'), false);
        assert!(s.query.is_empty());
        key(&mut s, KeyCode::Right, false);
        assert!(matches!(key(&mut s, KeyCode::Enter, false), Action::None));
        assert!(s.confirm.is_none());
        key(&mut s, KeyCode::Char('x'), true);
        let Action::Remove(entry) = key(&mut s, KeyCode::Enter, false) else {
            panic!("expected removal");
        };
        s.removed(&entry, Err(anyhow!("denied")), Instant::now());
        assert_eq!(s.entries.len(), 3);
        assert!(s.status.contains("denied"));
        s.removed(&entry, Ok(()), Instant::now());
        assert_eq!(s.entries.len(), 2);
        assert!(s.selected().is_some());
        key(&mut s, KeyCode::Char('x'), true); // config entry has no removal action
        assert!(s.confirm.is_none());
    }

    #[test]
    fn input_palette_and_faint_roles_match_pinned_defaults() {
        for worktree in [false, true] {
            let mut s = state(PickerOptions {
                worktree,
                ..Default::default()
            });
            let (_, b) = snapshot(&mut s, 80, 24);
            assert_eq!(b[(2, 0)].fg, Color::Indexed(7));
            assert_eq!(b[(3, 0)].fg, Color::Indexed(7));
            assert_eq!(
                b[(4, 0)].fg,
                if worktree {
                    Color::Indexed(7)
                } else {
                    Color::Indexed(240)
                }
            );
            assert_eq!(b[(4, 0)].modifier.contains(Modifier::REVERSED), worktree);
            assert_eq!(b[(5, 0)].fg, Color::Indexed(240));
            key(&mut s, KeyCode::Char('a'), false);
            let (_, b) = snapshot(&mut s, 80, 24);
            assert_eq!(b[(2, 0)].fg, Color::Indexed(7));
            assert_eq!(b[(4, 0)].fg, Color::Reset);
            s.loading = true;
            let (_, b) = snapshot(&mut s, 80, 24);
            assert_eq!(b[(2, 2)].fg, Color::Reset);
            assert!(b[(2, 2)].modifier.contains(Modifier::DIM));
        }
        let mut s = state(PickerOptions {
            preview: true,
            show_windows: true,
            show_icons: false,
            ..Default::default()
        });
        s.entries[0].window_names = vec!["Editor".into()];
        s.filter();
        let (_, b) = snapshot(&mut s, 120, 30);
        assert_eq!(b[(8, 2)].fg, Color::DarkGray);
        assert!(b[(8, 2)].modifier.contains(Modifier::DIM));
        assert_eq!(b[(48, 0)].fg, Color::DarkGray);
        assert!(!b[(48, 0)].modifier.contains(Modifier::DIM));
        assert_eq!(b[(50, 2)].fg, Color::Reset);
        assert!(b[(50, 2)].modifier.contains(Modifier::DIM));
    }
    #[test]
    fn normal_confirmation_has_reference_padding_wording_and_styles() {
        let mut s = state(PickerOptions::default());
        s.confirm = Some(Confirm {
            entry: Entry::new("X".repeat(70), "PATH_MUST_NOT_APPEAR", Source::Rex),
            yes: true,
        });
        let (text, b) = snapshot(&mut s, 80, 24);
        let (x, y) = (7, 7);
        assert_eq!(b[(x, y)].symbol(), "╭");
        assert_eq!(b[(x + 65, y)].symbol(), "╮");
        assert_eq!(b[(x, y + 8)].symbol(), "╰");
        assert_eq!(b[(x, y)].fg, Color::Blue);
        for row in [y + 1, y + 7] {
            for col in x + 1..x + 65 {
                assert_eq!(b[(col, row)].symbol(), " ");
            }
        }
        assert!(
            text.lines()
                .nth((y + 2) as usize)
                .unwrap()
                .contains("Do you want to kill this Rex session?")
        );
        assert!(!text.contains("PATH_MUST_NOT_APPEAR"));
        assert_eq!(b[(x + 2, y + 4)].symbol(), " ");
        assert_eq!(b[(x + 3, y + 4)].symbol(), "X");
        assert_eq!(b[(x + 62, y + 4)].symbol(), "…");
        assert_eq!(b[(x + 63, y + 4)].symbol(), " ");
        assert!(b[(x + 3, y + 4)].modifier.contains(Modifier::BOLD));
        let bx = x + 3 + (60 - 12) / 2;
        assert!(
            b[(bx + 1, y + 6)]
                .modifier
                .contains(Modifier::REVERSED | Modifier::BOLD)
        );
        assert!(b[(bx + 9, y + 6)].modifier.contains(Modifier::DIM));
        assert_eq!(b[(bx + 9, y + 6)].fg, Color::Reset);
    }
    #[test]
    fn confirmation_path_line_and_narrow_full_width_follow_reference() {
        for (source, name, path, show) in [
            (Source::Rex, "Target", "UNIQUE_OTHER_PATH", false),
            (Source::Zoxide, "UNIQUE_SAME", "UNIQUE_SAME", false),
            (Source::Zoxide, "Target", "UNIQUE_OTHER_PATH", true),
            (Source::Zoxide, "Target", "", false),
        ] {
            let mut s = state(PickerOptions::default());
            s.confirm = Some(Confirm {
                entry: Entry::new(name, path, source),
                yes: false,
            });
            let (text, b) = snapshot(&mut s, 80, 24);
            let h = if show { 10 } else { 9 };
            let y = (24 - h) / 2;
            assert_eq!(b[(7, y)].symbol(), "╭");
            assert_eq!(b[(7, y + h - 1)].symbol(), "╰");
            if path == "UNIQUE_OTHER_PATH" {
                assert_eq!(text.contains(path), show);
            }
            if name == path {
                assert_eq!(text.matches("UNIQUE_SAME").count(), 1);
            }
            if show {
                let row = (0..80).map(|x| b[(x, y + 5)].symbol()).collect::<String>();
                let px = row.find(path).expect("visible path") as u16;
                assert!(b[(px, y + 5)].modifier.contains(Modifier::DIM));
                assert_eq!(b[(px, y + 5)].fg, Color::Reset);
            }
        }
        let mut s = state(PickerOptions::default());
        s.confirm = Some(Confirm {
            entry: Entry::new("Target", "different-path", Source::Zoxide),
            yes: true,
        });
        let (text, b) = snapshot(&mut s, 50, 24);
        let y = (24 - 11) / 2;
        assert_eq!(b[(0, y)].symbol(), "╭");
        assert_eq!(b[(49, y)].symbol(), "╮");
        assert_eq!(b[(0, y + 10)].symbol(), "╰");
        assert!(text.contains("Do you want to remove this directory from"));
        assert!(text.contains("zoxide?"));
        assert!(text.contains("different-path"));
    }

    #[test]
    fn confirmation_buttons_and_target_remain_visible_in_short_terminals() {
        for (width, height) in [(80, 7), (40, 5), (24, 4), (18, 3), (12, 3)] {
            let mut s = state(PickerOptions::default());
            key(&mut s, KeyCode::Char('x'), true);
            let (text, _) = snapshot(&mut s, width, height);
            assert!(text.contains("alpha"), "{width}x{height}: {text}");
            assert!(
                text.contains("Yes") && text.contains("No"),
                "{width}x{height}: {text}"
            );
            assert!(matches!(
                key(&mut s, KeyCode::Enter, false),
                Action::Remove(_)
            ));
        }
    }
    #[test]
    fn confirmation_cannot_delete_when_too_small_and_recovers_on_resize() {
        for (width, height) in [(1, 1), (2, 2), (8, 4), (40, 1), (40, 2), (11, 7)] {
            let mut s = state(PickerOptions::default());
            key(&mut s, KeyCode::Char('x'), true);
            snapshot(&mut s, width, height);
            assert!(matches!(key(&mut s, KeyCode::Enter, false), Action::None));
            assert!(matches!(
                key(&mut s, KeyCode::Char('y'), false),
                Action::None
            ));
            assert!(s.confirm.is_some() && !s.removing);
            let (text, _) = snapshot(&mut s, 40, 4);
            assert!(text.contains("alpha") && text.contains("Yes") && text.contains("No"));
            assert!(matches!(
                key(&mut s, KeyCode::Enter, false),
                Action::Remove(_)
            ));
        }
        let mut s = state(PickerOptions::default());
        key(&mut s, KeyCode::Char('x'), true);
        snapshot(&mut s, 1, 1);
        key(&mut s, KeyCode::Esc, false);
        assert!(s.confirm.is_none() && !s.removing);
    }

    #[test]
    fn render_80x24_and_120x30_preview_and_narrow_sizes() {
        let mut s = state(PickerOptions {
            preview: true,
            group_separator: true,
            ..Default::default()
        });
        s.preview_text = Some("\x1b[31mcolored\x1b[0m\nsecond line".into());
        let (text, _) = snapshot(&mut s, 80, 24);
        assert!(
            text.lines()
                .next()
                .unwrap()
                .starts_with("  > Filter sessions...")
        );
        assert!(text.contains("alpha"));
        assert!(!text.contains("colored"));
        assert_eq!(widths(&s, 80), (60, 0));
        let (text, buffer) = snapshot(&mut s, 120, 30);
        assert_eq!(widths(&s, 120), (48, 72));
        assert_eq!(buffer[(48, 0)].symbol(), "│");
        assert!(text.contains("colored"));
        assert_eq!(buffer[(50, 2)].fg, Color::Red);
        for (width, height) in [(1, 1), (2, 2), (8, 4), (24, 10), (40, 3)] {
            snapshot(&mut s, width, height);
            s.confirm = Some(Confirm {
                entry: s.entries[0].clone(),
                yes: true,
            });
            snapshot(&mut s, width, height);
            s.confirm = None;
        }
    }

    #[test]
    fn groups_can_merge_different_sources_and_split_same_source() {
        let mut s = state(PickerOptions {
            group_separator: true,
            ..Default::default()
        });
        s.entries[0].group = 1;
        s.entries[1].group = 1;
        s.entries[2].group = 2;
        s.entries[2].source = s.entries[1].source;
        s.filter();
        assert_eq!(s.rows, vec![Some(0), Some(1), None, Some(2)]);
        key(&mut s, KeyCode::Char('a'), false);
        assert!(s.rows.iter().all(Option::is_some));
    }

    #[test]
    fn half_page_keys_preserve_filter_and_input_shortcuts_edit() {
        let mut s = state(PickerOptions::default());
        key(&mut s, KeyCode::Char('d'), true);
        assert_eq!(s.cursor, 2);
        key(&mut s, KeyCode::Char('u'), true);
        assert_eq!(s.cursor, 0);
        assert!(s.query.is_empty());
        s.query = "日本語".into();
        s.caret = 3;
        key(&mut s, KeyCode::Char('b'), true);
        assert_eq!(s.caret, 2);
        key(&mut s, KeyCode::Char('f'), true);
        assert_eq!(s.caret, 3);
    }

    #[test]
    fn stale_preview_results_never_overwrite_current_selection() {
        let mut s = state(PickerOptions {
            preview: true,
            ..Default::default()
        });
        let old = s.preview_seq;
        s.move_cursor(1, Instant::now());
        s.preview_loaded(old, Ok("stale".into()));
        assert!(s.preview_text.is_none());
        s.preview_loaded(s.preview_seq, Ok("current".into()));
        assert_eq!(s.preview_text.as_deref(), Some("current"));
        key(&mut s, KeyCode::Char('o'), true);
        s.preview_loaded(s.preview_seq, Ok("hidden".into()));
        assert!(s.preview_text.is_none());
    }

    #[test]
    fn window_labels_format_and_elide_to_available_cells() {
        let mut entry = Entry::new("test", "", Source::Rex);
        entry.window_names = vec!["one".into(), "日本".into(), "three".into()];
        entry.active_window_name = "日本".into();
        assert_eq!(
            window_names_text(&entry, "{name}{active}", 40),
            " one 日本* three"
        );
        assert_eq!(window_names_text(&entry, "{window_name}", 8), " one +2");
        assert_eq!(window_names_text(&entry, "{name}", 2), "");
        let mut s = state(PickerOptions::default());
        s.entries[0] = entry;
        s.filter();
        let (text, _) = snapshot(&mut s, 80, 24);
        assert!(text.contains("one ") && text.contains("three"));
    }

    #[test]
    fn expanded_literal_dollar_project_remains_selectable() {
        const PROJECT: &str = "REXUE_LITERAL_PROJECT_734129";
        const ROOT: &str = "REXUE_LITERAL_ROOT_734129";
        const UNSET: &str = "REXUE_UNSET_LITERAL_734129";
        const FIXTURE: &str = "REXUE_LITERAL_FIXTURE_734129";
        if std::env::var_os(FIXTURE).is_none() {
            let d = tempfile::tempdir().unwrap();
            let root = d.path().join(format!("trees-${UNSET}"));
            let project = root.join("409");
            let cfg = d.path().join("literal.toml");
            std::fs::write(&cfg,format!("[[worktree]]\nrepo='org/repo'\npath={}\nworktree_dir='${{{ROOT}}}'\nicon='W'\n[[session]]\nname='literal-project'\npath='${{{PROJECT}}}'\n",serde_json::to_string(d.path().to_str().unwrap()).unwrap())).unwrap();
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tui::tests::expanded_literal_dollar_project_remains_selectable",
                    "--nocapture",
                ])
                .env(FIXTURE, &cfg)
                .env(PROJECT, &project)
                .env(ROOT, &root)
                .env_remove(UNSET)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        let cfg = crate::config::Config::load(Some(std::path::Path::new(
            &std::env::var_os(FIXTURE).unwrap(),
        )))
        .unwrap();
        let state_dir = tempfile::tempdir().unwrap();
        let manager = crate::session::Manager {
            rex: crate::backend::Rex {
                bin: "/nonexistent-literal-path-test".into(),
                ..Default::default()
            },
            config: cfg,
            state_dir: state_dir.path().into(),
        };
        let rows = manager
            .list(&crate::session::ListOptions {
                config: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].path,
            std::path::PathBuf::from(std::env::var_os(PROJECT).unwrap())
        );
        let mut state = State::new(PickerOptions {
            show_icons: true,
            ..Default::default()
        });
        state.loaded(rows, Instant::now());
        let (text, _) = snapshot(&mut state, 80, 24);
        assert!(text.contains("W literal-project"));
        assert_eq!(state.filtered.len(), 1);
    }

    #[test]
    fn expanded_worktree_icons_render_for_all_equivalent_roots() {
        let d = tempfile::tempdir().unwrap();
        let leaf = format!("rexue-tui-icon-fixture-{}", std::process::id());
        let root = dirs::home_dir().unwrap().join(&leaf);
        for spelling in [
            root.display().to_string(),
            format!("~/{leaf}"),
            format!("${{HOME}}/{leaf}"),
        ] {
            let config = crate::config::Config {
                session: vec![crate::config::SessionConfig {
                    name: "tree-project".into(),
                    path: root.join("123").display().to_string(),
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
            let manager = crate::session::Manager {
                rex: crate::backend::Rex {
                    bin: "/nonexistent-rex-icon-test".into(),
                    ..Default::default()
                },
                config,
                state_dir: d.path().join("state"),
            };
            let rows = manager
                .list(&crate::session::ListOptions {
                    config: true,
                    ..Default::default()
                })
                .unwrap();
            let mut state = State::new(PickerOptions {
                show_icons: true,
                ..Default::default()
            });
            state.loaded(rows, Instant::now());
            let (text, _) = snapshot(&mut state, 80, 24);
            assert!(text.contains("W tree-project"), "{spelling}: {text}");
        }
    }

    #[test]
    fn window_format_preserves_data_and_supports_hash_escape() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("fmt.toml");
        std::fs::write(
            &cfg,
            "[tui]\nwindow_name_format='##{window_name} #W #{window_index}'\n",
        )
        .unwrap();
        let cfg = crate::config::Config::load(Some(&cfg)).unwrap();
        let mut entry = Entry::new("project", "/tmp", Source::Rex);
        entry.window_names = vec!["literal-{active}".into()];
        assert_eq!(
            window_names_text(&entry, &cfg.tui.window_name_format, 100),
            " #{window_name} literal-{active} 1"
        );
        entry.window_names = vec!["same".into(), "same".into()];
        entry.active_window_name = "same".into();
        entry.active_window_index = Some(2);
        assert_eq!(
            window_names_text(&entry, "{window_name}:{window_active}", 80),
            " same:0 same:1"
        );
    }
    #[test]
    fn large_valid_preview_threshold_does_not_force_a_split() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("large.toml");
        std::fs::write(&cfg, "[tui]\npreview_min_width=100000\n").unwrap();
        let cfg = crate::config::Config::load(Some(&cfg)).unwrap();
        let s = State::new(PickerOptions {
            preview: true,
            preview_min_width: cfg.tui.preview_min_width,
            ..Default::default()
        });
        assert_eq!(widths(&s, u16::MAX).1, 0);
    }

    #[test]
    fn ordinary_tmux_window_fields_map_to_real_native_labels() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("windows.toml");
        std::fs::write(&file,"[tui]\nwindow_name_format='#{window_name}:#{window_index}:#{window_active}'\nshow_windows=true\n").unwrap();
        let cfg = crate::config::Config::load(Some(&file)).unwrap();
        let mut entry = Entry::new("project", "/tmp", Source::Rex);
        entry.window_names = vec!["Editor".into(), "Logs".into()];
        entry.active_window_name = "Logs".into();
        let text = window_names_text(&entry, &cfg.tui.window_name_format, 80);
        assert_eq!(text, " Editor:1:0 Logs:2:1");
    }

    #[test]
    fn config_window_names_render_before_creation_and_blocks_are_green() {
        assert_eq!(source_style(Source::RexBlock).fg, Some(Color::Green));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path,"[[window]]\nname='Logs'\n[[session]]\nname='not-created-yet'\npath='/tmp'\nwindows=['Logs']\n").unwrap();
        let cfg = crate::config::Config::load(Some(&path)).unwrap();
        let mut s = State::new(PickerOptions {
            show_windows: true,
            ..Default::default()
        });
        s.loaded(cfg.entries().unwrap(), Instant::now());
        let (text, _) = snapshot(&mut s, 80, 24);
        assert!(text.contains("not-created-yet") && text.contains("Logs"));
    }

    #[test]
    fn configured_source_colors_and_defaults_match_upstream() {
        assert_eq!(parse_color("  Bright-Magenta "), Some(Color::LightMagenta));
        assert_eq!(parse_color("gray"), Some(Color::DarkGray));
        assert_eq!(parse_color("#12aBcF"), Some(Color::Rgb(18, 171, 207)));
        assert_eq!(parse_color("#abc"), Some(Color::Rgb(170, 187, 204)));
        assert_eq!(parse_color("34"), Some(Color::Blue));
        assert_eq!(parse_color("196"), Some(Color::Indexed(196)));
        assert_eq!(parse_color("ansi:31"), Some(Color::Indexed(31)));
        assert_eq!(parse_color("#日"), None);
        let mut s = state(PickerOptions::default());
        s.options
            .glyph_colors
            .insert("rex".into(), "#010203".into());
        let (_, buffer) = snapshot(&mut s, 80, 24);
        assert_eq!(buffer[(2, 2)].fg, Color::Rgb(1, 2, 3));
        assert!(
            buffer
                .content
                .iter()
                .any(|cell| cell.symbol() == "" && cell.fg == Color::Cyan)
        );
    }

    #[test]
    fn worktree_full_width_badges_red_matches_and_refresh_preserve_position() {
        let mut s = State::new(PickerOptions {
            worktree: true,
            preview: true,
            show_icons: false,
            ..Default::default()
        });
        let mut rows = vec![];
        for (number, state) in [(123, "MERGED"), (2, "OPEN"), (42, "CLOSED")] {
            let mut row = Entry::new(
                format!("org/repo #{number} 日本 title {number}"),
                format!("/wk/{number}"),
                Source::Worktree,
            );
            row.active_window_name = state.into();
            rows.push(row);
        }
        s.loaded(rows.clone(), Instant::now());
        assert_eq!(worktree_parts(&s.entries[0]).0, "2");
        assert_eq!(widths(&s, 120), (120, 0));
        let (text, buffer) = snapshot(&mut s, 120, 30);
        assert!(text.contains("OPEN"));
        assert!(text.contains("CLOSED"));
        assert!(text.contains("MERGED"));
        assert!(!text.contains("org/repo"));
        assert!(buffer.content.iter().any(|c| c.symbol() == "M"
            && c.fg == Color::Magenta
            && c.modifier.contains(Modifier::REVERSED)));
        key(&mut s, KeyCode::Char('x'), true);
        assert!(s.confirm.is_none());
        key(&mut s, KeyCode::Char('o'), true);
        assert!(s.preview_due.is_none());
        key(&mut s, KeyCode::Char('t'), false);
        key(&mut s, KeyCode::Down, false);
        let cursor = s.cursor;
        assert!(matches!(
            key(&mut s, KeyCode::Char('r'), true),
            Action::Refresh
        ));
        assert!(matches!(
            key(&mut s, KeyCode::Char('r'), true),
            Action::None
        ));
        s.loaded(rows, Instant::now());
        assert_eq!(s.query, "t");
        assert_eq!(s.cursor, cursor);
        let (_, buffer) = snapshot(&mut s, 80, 24);
        assert!(
            buffer
                .content
                .iter()
                .any(|c| c.symbol() == "t" && c.fg == Color::Red)
        );
        s.query = "#".into();
        s.caret = 1;
        s.changed_query(Instant::now());
        assert!(s.index_query().is_none());
        assert!(!matches!(
            key(&mut s, KeyCode::Char('1'), false),
            Action::Choose(_)
        ));
        snapshot(&mut s, 8, 4);
    }

    #[test]
    fn unicode_word_editing_and_paste_are_modal() {
        let mut s = state(PickerOptions::default());
        s.insert_text("one 日本 three", Instant::now());
        s.key(
            KeyEvent::new(KeyCode::Left, KeyModifiers::ALT),
            10,
            Instant::now(),
        );
        assert_eq!(s.caret, 7);
        s.key(
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
            10,
            Instant::now(),
        );
        assert_eq!(s.query, "one three");
        assert_eq!(s.caret, 4);
        s.key(
            KeyEvent::new(KeyCode::Delete, KeyModifiers::ALT),
            10,
            Instant::now(),
        );
        assert_eq!(s.query, "one ");
        key(&mut s, KeyCode::Char('h'), true);
        assert_eq!(s.query, "one");
        assert!(matches!(
            key(&mut s, KeyCode::Char('v'), true),
            Action::Paste
        ));
        assert!(matches!(
            key(&mut s, KeyCode::Char('v'), true),
            Action::None
        ));
        s.confirm = Some(Confirm {
            entry: s.entries[0].clone(),
            yes: true,
        });
        s.insert_text("ignored", Instant::now());
        assert_eq!(s.query, "one");
        assert!(matches!(
            key(&mut s, KeyCode::Char('y'), true),
            Action::None
        ));
        assert!(s.confirm.is_some());
        assert!(matches!(
            key(&mut s, KeyCode::Char('c'), true),
            Action::Quit
        ));
    }

    #[test]
    fn worktree_startup_refresh_runs_once_and_can_be_disabled() {
        let rows = vec![Entry::new("org/repo #1 title", "/wk/1", Source::Worktree)];
        let mut s = State::new(PickerOptions {
            worktree: true,
            ..Default::default()
        });
        s.loaded(rows.clone(), Instant::now());
        assert!(s.refresh_pending);
        s.refresh_pending = false;
        s.fetching = true;
        s.loaded(rows.clone(), Instant::now());
        assert!(!s.refresh_pending);
        let mut s = State::new(PickerOptions {
            worktree: true,
            auto_refresh: false,
            ..Default::default()
        });
        s.loaded(rows, Instant::now());
        assert!(!s.refresh_pending);
    }

    #[test]
    fn synthesized_alias_targets_keep_configured_icons_in_both_query_modes() {
        let mut s = state(PickerOptions {
            icon_width: 2,
            ..Default::default()
        });
        s.options.aliases.insert(
            "dev".into(),
            Alias {
                alias: "dev".into(),
                target: "unloaded-target".into(),
                auto_connect: false,
            },
        );
        s.options
            .alias_icons
            .insert("unloaded-target".into(), "日".into());
        for query in ["/dev", "dev"] {
            s.query = query.into();
            s.filter();
            assert_eq!(s.filtered.len(), 1);
            assert_eq!(s.filtered[0].entry.icon.as_deref(), Some("日"));
            let (text, _) = snapshot(&mut s, 80, 24);
            assert!(text.contains("日"));
            assert!(text.contains("unloaded-target"));
        }
    }

    #[test]
    fn loaded_worktree_only_icons_do_not_widen_session_inventory_columns() {
        let mut s = state(PickerOptions {
            icon_width: 1,
            show_icons: true,
            show_windows: false,
            glyphs: [("rex".into(), "R".into())].into(),
            ..Default::default()
        });
        let mut wide = Entry::new("wide", "/wide", Source::Rex);
        wide.icon = Some("WIDE".into());
        s.loaded(
            vec![Entry::new("alpha", "/alpha", Source::Rex), wide],
            Instant::now(),
        );
        let (_, b) = snapshot(&mut s, 80, 24);
        assert_eq!(b[(4, 2)].symbol(), "a");
        assert_eq!(b[(7, 3)].symbol(), "w");
    }

    #[test]
    fn unsplit_removal_status_uses_available_physical_width() {
        let mut s = state(PickerOptions::default());
        s.status = format!("Couldn't kill session: {}-TAIL", "x".repeat(35));
        let (text, b) = snapshot(&mut s, 80, 24);
        assert!(text.contains(&s.status));
        assert_eq!(b[(2, 1)].fg, Color::Yellow);
        s.options.preview = true;
        let (_, b) = snapshot(&mut s, 120, 30);
        let row = (0..120).map(|x| b[(x, 1)].symbol()).collect::<String>();
        assert!(!row.contains("-TAIL"));
        assert_eq!(b[(48, 1)].symbol(), "│");
    }

    #[test]
    fn safe_ansi_preview_resets_each_line_and_discards_terminal_commands() {
        let text = ansi_preview(
            "\x1b[31mred\nplain\x1b]52;c;secret\x07!\x1b[2J\x1b[38;2;1;2;3mRGB",
            2,
        );
        assert_eq!(text.lines.len(), 2);
        assert_eq!(text.lines[0].spans[0].style.fg, Some(Color::Red));
        assert_eq!(text.lines[1].spans[0].style.fg, None);
        assert_eq!(
            text.lines[1].spans.last().unwrap().style.fg,
            Some(Color::Rgb(1, 2, 3))
        );
        let plain: String = text
            .lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert_eq!(plain, "redplain!RGB");
        assert!(!plain.contains('\x1b'));
    }
}
