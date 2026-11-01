//! Configurable picker actions. Keys use fzf names; action names remain stable.
use anyhow::{Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::process::Command;

pub(crate) const DEFAULT_BINDINGS: &[(&str, &[&str])] = &[
    ("accept", &["enter", "double-click"]),
    ("cancel", &["esc", "ctrl-c", "ctrl-g", "ctrl-q"]),
    ("up", &["up", "ctrl-k", "ctrl-p"]),
    ("down", &["down", "ctrl-j", "ctrl-n"]),
    ("half_page_up", &["ctrl-u"]),
    ("half_page_down", &["ctrl-d"]),
    ("toggle_preview", &["ctrl-o"]),
    ("rename", &["ctrl-r"]),
    ("remove", &["ctrl-x"]),
    ("refresh", &["ctrl-r"]),
    ("create_worktree", &["ctrl-w"]),
    ("select_1", &["1"]),
    ("select_2", &["2"]),
    ("select_3", &["3"]),
    ("select_4", &["4"]),
    ("select_5", &["5"]),
    ("select_6", &["6"]),
    ("select_7", &["7"]),
    ("select_8", &["8"]),
    ("select_9", &["9"]),
    ("confirm_yes", &["y"]),
    ("confirm_no", &["n", "q"]),
    ("confirm_up", &["up", "ctrl-k", "ctrl-p", "left", "btab"]),
    (
        "confirm_down",
        &["down", "ctrl-j", "ctrl-n", "right", "tab"],
    ),
];

#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "BTreeMap<String, Vec<String>>")]
pub struct Keybindings(BTreeMap<String, Vec<String>>);

impl Default for Keybindings {
    fn default() -> Self {
        Self(
            DEFAULT_BINDINGS
                .iter()
                .map(|(action, keys)| {
                    (
                        (*action).into(),
                        keys.iter().map(|key| (*key).into()).collect(),
                    )
                })
                .collect(),
        )
    }
}

impl TryFrom<BTreeMap<String, Vec<String>>> for Keybindings {
    type Error = anyhow::Error;

    fn try_from(overrides: BTreeMap<String, Vec<String>>) -> Result<Self> {
        let mut bindings = Self::default();
        for (action, keys) in overrides {
            let Some(slot) = bindings.0.get_mut(&action) else {
                bail!("unknown keybinding action {action:?}");
            };
            *slot = keys
                .into_iter()
                .map(|key| {
                    canonical_key(&key).ok_or_else(|| {
                        anyhow::anyhow!("tui.keybindings.{action}: invalid fzf key {key:?}")
                    })
                })
                .collect::<Result<_>>()?;
        }
        for actions in [
            Self::picker_actions(false),
            Self::picker_actions(true),
            Self::confirmation_actions(),
        ] {
            let mut assigned = BTreeMap::new();
            for action in actions {
                for key in bindings.keys(action) {
                    if let Some(previous) = assigned.insert(key, action) {
                        bail!(
                            "tui.keybindings: key {key:?} is assigned to both {previous} and {action}"
                        );
                    }
                }
            }
        }
        Ok(bindings)
    }
}

impl Keybindings {
    pub fn keys(&self, action: &str) -> &[String] {
        &self.0[action]
    }

    pub(crate) fn picker_actions(worktree: bool) -> Vec<&'static str> {
        let mut actions = vec![
            "accept",
            "cancel",
            "up",
            "down",
            "half_page_up",
            "half_page_down",
            "toggle_preview",
            "remove",
        ];
        if worktree {
            actions.extend(["refresh", "create_worktree"]);
        } else {
            actions.push("rename");
            actions.extend(
                DEFAULT_BINDINGS
                    .iter()
                    .map(|(name, _)| *name)
                    .filter(|name| name.starts_with("select_")),
            );
        }
        actions
    }

    pub(crate) fn confirmation_actions() -> Vec<&'static str> {
        vec![
            "accept",
            "cancel",
            "confirm_yes",
            "confirm_no",
            "confirm_up",
            "confirm_down",
        ]
    }

    // Remove the managed default actions before applying the complete resolved map.
    // Printable keys return to text entry when their shortcut is removed.
    pub(crate) fn reset(&self, command: &mut Command, actions: &[&str]) {
        for (_, keys) in DEFAULT_BINDINGS
            .iter()
            .filter(|(action, _)| actions.contains(action))
        {
            for key in *keys {
                let action = if key.chars().count() == 1 {
                    format!("put({key})")
                } else {
                    "ignore".into()
                };
                command.arg(format!("--bind={key}:{action}"));
            }
        }
    }

    pub(crate) fn bind(&self, command: &mut Command, action: &str, fzf_action: &str) {
        for key in self.keys(action) {
            command.arg(format!("--bind={key}:{fzf_action}"));
        }
    }

    pub(crate) fn hint(&self, action: &str, label: &str) -> Option<String> {
        let key = self
            .keys(action)
            .iter()
            .find(|key| !key.ends_with("click"))?;
        let display = if key.chars().count() == 1 {
            key.clone()
        } else {
            key.split('-')
                .map(|part| {
                    let mut chars = part.chars();
                    chars
                        .next()
                        .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join("-")
        };
        Some(format!("{display} {label}"))
    }
}

fn canonical_key(key: &str) -> Option<String> {
    let key = match key {
        "return" | "ctrl-m" => "enter",
        "ctrl-i" => "tab",
        "shift-tab" => "btab",
        "bs" | "bspace" | "ctrl-h" => "backspace",
        "alt-bs" | "alt-bspace" | "ctrl-alt-h" => "alt-backspace",
        "ctrl-bs" | "ctrl-bspace" => "ctrl-backspace",
        "del" => "delete",
        "pgup" => "page-up",
        "pgdn" => "page-down",
        "ctrl-6" => "ctrl-^",
        "ctrl-_" => "ctrl-/",
        _ => key,
    };
    let literal = |value: &str| {
        value.chars().count() == 1
            && value
                .chars()
                .all(|c| !c.is_control() && !c.is_whitespace() && !":,+()[]{}\\".contains(c))
    };
    let named = [
        "enter",
        "space",
        "backspace",
        "tab",
        "btab",
        "esc",
        "delete",
        "up",
        "down",
        "left",
        "right",
        "home",
        "end",
        "insert",
        "page-up",
        "page-down",
        "left-click",
        "right-click",
        "double-click",
        "scroll-up",
        "scroll-down",
        "preview-scroll-up",
        "preview-scroll-down",
    ];
    let navigation = [
        "up",
        "down",
        "left",
        "right",
        "home",
        "end",
        "delete",
        "page-up",
        "page-down",
    ];
    let modified_navigation = [
        "alt-",
        "ctrl-",
        "shift-",
        "alt-shift-",
        "ctrl-alt-",
        "ctrl-shift-",
        "ctrl-alt-shift-",
    ]
    .iter()
    .any(|prefix| {
        key.strip_prefix(prefix)
            .is_some_and(|base| navigation.contains(&base))
    });
    let control = key.strip_prefix("ctrl-alt-").map_or_else(
        || {
            key.strip_prefix("ctrl-").is_some_and(|base| {
                (base.len() == 1 && base.as_bytes()[0].is_ascii_lowercase())
                    || ["space", "backspace", "/", "^", "]", "\\"].contains(&base)
            })
        },
        |base| (base.len() == 1 && base.as_bytes()[0].is_ascii_lowercase()) || base == "backspace",
    );
    let alt = key
        .strip_prefix("alt-")
        .is_some_and(|base| literal(base) || ["enter", "space", "backspace"].contains(&base));
    let function = key
        .strip_prefix('f')
        .and_then(|number| number.parse::<u8>().ok())
        .is_some_and(|number| (1..=12).contains(&number) && key == format!("f{number}"));
    (literal(key) || named.contains(&key) || modified_navigation || control || alt || function)
        .then(|| key.to_owned())
}
