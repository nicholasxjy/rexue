use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use unicode_casefold::{Locale, UnicodeCaseFold, Variant};

/// Unicode simple, non-Turkic folding for case-insensitive aliases.
/// Keep single-character folds (ß never expands to ss; dotted İ stays distinct).
pub fn alias_key(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            let mut lower = ch.to_lowercase();
            let first = lower.next().unwrap_or(ch);
            if lower.next().is_none() { first } else { ch }
        })
        .case_fold_with(Variant::Simple, Locale::NonTurkic)
        .collect()
}

#[cfg(test)]
mod folding_tests {
    use super::alias_key;
    #[test]
    fn unicode_simple_alias_folding() {
        for (a, b) in [("ä", "Ä"), ("Σ", "ς"), ("ſ", "S"), ("K", "k"), ("ẞ", "ß")] {
            assert_eq!(alias_key(a), alias_key(b));
        }
        for (a, b) in [("ß", "ss"), ("İ", "i\u{307}"), ("I", "ı")] {
            assert_ne!(alias_key(a), alias_key(b));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Agent,
    Rex,
    Config,
    Zoxide,
    Directory,
    Worktree,
}
impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Rex => "rex",
            Self::Config => "config",
            Self::Zoxide => "zoxide",
            Self::Directory => "directory",
            Self::Worktree => "worktree",
        }
    }
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Agent => "󰚩",
            Self::Rex => "",
            Self::Config => "",
            Self::Zoxide | Self::Directory => "",
            Self::Worktree => "",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub app: String,
    pub state: String,
    pub session_name: String,
    pub window_id: String,
    pub block_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    #[serde(default)]
    pub path: PathBuf,
    pub source: Source,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tab_names: Vec<String>,
    /// Display indices into tab_names; session and pane identity use full IDs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agent_tab_indices: Vec<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<Agent>,
    #[serde(default)]
    pub group: usize,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub alias_auto_connect: bool,
    #[serde(default)]
    pub score: f64,
}
impl Entry {
    pub fn new(name: impl Into<String>, path: impl Into<PathBuf>, source: Source) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            source,
            session_id: None,
            tab_names: Vec::new(),
            agent_tab_indices: Vec::new(),
            agent: None,
            group: 0,
            icon: None,
            alias: None,
            alias_auto_connect: false,
            score: 0.0,
        }
    }
    pub fn target(&self) -> &str {
        self.agent
            .as_ref()
            .map(|agent| agent.block_id.as_str())
            .or(self.session_id.as_deref())
            .unwrap_or(&self.name)
    }
    pub fn display_name(&self) -> String {
        match &self.agent {
            Some(agent) => format!("{} [{}]", self.name, agent.state),
            None => self.name.clone(),
        }
    }
}
