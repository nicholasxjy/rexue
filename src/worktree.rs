//! Worktrunk owns worktree discovery, paths, configuration, hooks and lifecycle.
use crate::{
    config::{Config, expand_path},
    model::{Entry, Source},
    process,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Command,
};

pub const MIN_VERSION: &str = "0.81.0";

#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// A repository, linked worktree, or directory inside one. Defaults to cwd.
    pub path: Option<PathBuf>,
}
impl Selection {
    fn directory(&self) -> Result<PathBuf> {
        self.path
            .clone()
            .map_or_else(std::env::current_dir, Ok)
            .context("cannot determine worktree repository directory")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeEntry {
    pub repo: PathBuf,
    pub branch: Option<String>,
    pub path: PathBuf,
    pub head: String,
    #[serde(default)]
    pub icon: Option<String>,
}
impl WorktreeEntry {
    pub fn entry(&self) -> Entry {
        let name = self.branch.clone().unwrap_or_else(|| {
            format!(
                "(detached {})",
                self.head.chars().take(8).collect::<String>()
            )
        });
        let mut entry = Entry::new(name, &self.path, Source::Worktree);
        entry.icon = self.icon.clone();
        entry
    }
}
#[derive(Debug, Serialize)]
pub struct PreparedWorktree {
    pub path: PathBuf,
    pub created: bool,
}
#[derive(Debug, Clone, Copy, Default)]
pub struct RemoveOptions {
    pub force: bool,
    pub force_delete: bool,
    pub no_delete_branch: bool,
}

pub struct Worktrees<'a> {
    config: &'a Config,
    pub binary: PathBuf,
}
impl<'a> Worktrees<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self {
            config,
            binary: std::env::var_os("REXUE_WT")
                .unwrap_or_else(|| "wt".into())
                .into(),
        }
    }
    fn check_version(&self) -> Result<()> {
        let output = process::text(Command::new(&self.binary).arg("--version"))
            .with_context(|| format!("Worktrunk {MIN_VERSION} or newer is required; install worktrunk and put wt on PATH (or set REXUE_WT)"))?;
        let version = output
            .trim()
            .strip_prefix("wt v")
            .context("cannot read Worktrunk version; expected `wt vMAJOR.MINOR.PATCH`")?;
        let parts: Vec<_> = version
            .split(['.', '-', '+', ' '])
            .take(3)
            .map(str::parse::<u64>)
            .collect::<std::result::Result<_, _>>()
            .context("cannot parse Worktrunk version; use a supported wt release")?;
        ensure!(
            parts.len() == 3 && (parts[0], parts[1], parts[2]) >= (0, 81, 0),
            "Worktrunk {MIN_VERSION} or newer is required (found {}); upgrade worktrunk",
            output.trim()
        );
        Ok(())
    }
    fn command(&self, directory: &Path) -> Command {
        let mut command = Command::new(&self.binary);
        command.current_dir(directory);
        command
    }
    pub fn list(&self, selection: &Selection) -> Result<Vec<WorktreeEntry>> {
        self.check_version()?;
        let dir = selection.directory()?;
        let output = process::text(self.command(&dir).args(["list", "--format=json"]))
            .with_context(|| format!("could not list worktrees in {}", dir.display()))?;
        let mut entries = parse_list(&output, &dir)?;
        // Match the configured repository by a registered path, including a linked
        // checkout. No assumption about a template or a common parent directory.
        let icon = self.config.worktree.iter().find_map(|cfg| {
            let configured = expand_path(&cfg.path).ok()?;
            entries
                .iter()
                .any(|e| same_path(&configured, &e.path))
                .then(|| cfg.icon.clone())
                .flatten()
        });
        for entry in &mut entries {
            entry.icon = icon.clone();
        }
        Ok(entries)
    }
    pub fn list_all(&self) -> Result<Vec<WorktreeEntry>> {
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for cfg in &self.config.worktree {
            let selection = Selection {
                path: Some(expand_path(&cfg.path)?),
            };
            for entry in self.list(&selection)? {
                if seen.insert(canonical(&entry.path)) {
                    result.push(entry);
                }
            }
        }
        Ok(result)
    }
    /// Resolve the current registered checkout without firing switch hooks.
    pub fn current(&self, selection: &Selection) -> Result<PreparedWorktree> {
        let dir = canonical(&selection.directory()?);
        let entry = self
            .list(selection)?
            .into_iter()
            .filter(|entry| dir.starts_with(canonical(&entry.path)))
            .max_by_key(|entry| entry.path.components().count())
            .context("current directory is not inside a registered worktree")?;
        Ok(PreparedWorktree {
            path: entry.path,
            created: false,
        })
    }
    /// Worktrunk resolves paths and applies its own config and hook approvals.
    pub fn prepare(
        &self,
        selection: &Selection,
        target: &str,
        create: bool,
        base: Option<&str>,
    ) -> Result<PreparedWorktree> {
        ensure!(!target.is_empty(), "provide a branch or worktree path");
        ensure!(
            !target.starts_with('-'),
            "worktree target cannot start with '-'"
        );
        ensure!(create || base.is_none(), "--base requires worktree create");
        self.check_version()?;
        let mut command = self.command(&selection.directory()?);
        command.args(["switch", "--no-cd", "--format=json"]);
        if create {
            command.arg("--create");
        }
        if let Some(base) = base {
            command.arg("--base").arg(base);
        }
        command.arg(target);
        let output = process::interactive_text(&mut command)
            .context("Worktrunk switch failed; see wt diagnostics above (project hooks may require `wt config approvals add`)")?;
        let value: Value =
            serde_json::from_str(&output).context("invalid Worktrunk switch JSON")?;
        let path = value["path"]
            .as_str()
            .context("Worktrunk switch result has no path")?;
        ensure!(
            Path::new(path).is_absolute(),
            "Worktrunk returned a non-absolute worktree path"
        );
        Ok(PreparedWorktree {
            path: path.into(),
            created: value["action"] == "created",
        })
    }
    /// Resolve a registered checkout before its directory is removed.
    pub fn resolve(&self, selection: &Selection, target: &str) -> Result<WorktreeEntry> {
        ensure!(
            !target.is_empty(),
            "provide the worktree branch or path to remove"
        );
        let dir = selection.directory()?;
        let target_path = Path::new(target);
        let target_path = if target_path.is_absolute() {
            target_path.to_owned()
        } else {
            dir.join(target_path)
        };
        self.list(selection)?
            .into_iter()
            .find(|entry| {
                entry.branch.as_deref() == Some(target) || same_path(&entry.path, &target_path)
            })
            .context("target is not a registered worktree; use `rexue worktree list` to select one")
    }
    /// Only remove a selected registered worktree, never a branch-only row.
    pub fn remove(
        &self,
        selection: &Selection,
        target: &str,
        options: RemoveOptions,
    ) -> Result<Value> {
        let entry = self.resolve(selection, target)?;
        let mut command = self.command(&selection.directory()?);
        command.args(["remove", "--foreground", "--format=json"]);
        if options.force {
            command.arg("--force");
        }
        if options.force_delete {
            command.arg("--force-delete");
        }
        if options.no_delete_branch {
            command.arg("--no-delete-branch");
        }
        command.arg("--").arg(&entry.path);
        let output = process::interactive_text(&mut command)
            .context("Worktrunk remove failed; see wt diagnostics above; no force flags are enabled by default")?;
        serde_json::from_str(&output).context("invalid Worktrunk remove JSON")
    }
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}
fn same_path(a: &Path, b: &Path) -> bool {
    canonical(a) == canonical(b)
}

fn parse_list(output: &str, directory: &Path) -> Result<Vec<WorktreeEntry>> {
    let value: Value = serde_json::from_str(output).context("invalid Worktrunk list JSON")?;
    let (rows, schema2) = if let Some(rows) = value.as_array() {
        (rows, false)
    } else {
        ensure!(
            value["schema"] == 2,
            "unsupported Worktrunk list JSON schema; supported schemas are 1 and 2"
        );
        (
            value["items"]
                .as_array()
                .context("Worktrunk list result has no items")?,
            true,
        )
    };
    fn path_of(row: &Value, schema2: bool) -> Option<&str> {
        if schema2 {
            row["worktree"]["path"].as_str()
        } else {
            row["path"].as_str()
        }
    }
    let repo = rows
        .iter()
        .find(|row| {
            if schema2 {
                row["worktree"]["main"] == true
            } else {
                row["is_main"] == true
            }
        })
        .and_then(|row| path_of(row, schema2))
        .map(PathBuf::from)
        .unwrap_or_else(|| canonical(directory));
    let mut result = Vec::new();
    for row in rows {
        let Some(path) = path_of(row, schema2) else {
            // A user may enable branches/remotes in Worktrunk list config.
            // Those rows have no checkout and cannot be connected or removed.
            if schema2 && row.get("worktree").is_none_or(Value::is_null)
                || !schema2 && row["kind"] == "branch"
            {
                continue;
            }
            bail!("Worktrunk worktree row has no path");
        };
        ensure!(
            Path::new(path).is_absolute(),
            "Worktrunk returned a non-absolute worktree path"
        );
        result.push(WorktreeEntry {
            repo: repo.clone(),
            path: path.into(),
            branch: row["branch"].as_str().map(str::to_owned),
            head: if schema2 {
                row["head"]["sha"].as_str()
            } else {
                row["commit"]["sha"].as_str()
            }
            .unwrap_or("")
            .into(),
            icon: None,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_both_worktrunk_schemas_and_detached_checkouts() {
        let rows = parse_list(r#"{"schema":2,"items":[{"branch":"feature/a","head":{"sha":"12345678"},"worktree":{"path":"/trees/feature-a","main":true}},{"branch":null,"head":{"sha":"87654321"},"worktree":{"path":"/elsewhere/detached"}},{"branch":"remote-only"}]}"#, Path::new("/trees")).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].branch.as_deref(), Some("feature/a"));
        assert_eq!(rows[1].entry().name, "(detached 87654321)");
        assert_eq!(rows[1].repo, Path::new("/trees/feature-a"));
        let old = parse_list(r#"[{"branch":"main","path":"/repo","is_main":true,"commit":{"sha":"abc"}},{"kind":"branch","branch":"other"}]"#, Path::new("/repo")).unwrap();
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].head, "abc");
    }
    #[test]
    fn rejects_unknown_schema_and_malformed_paths() {
        for json in [
            r#"{"schema":3,"items":[]}"#,
            r#"{"schema":2}"#,
            r#"[{"path":"relative"}]"#,
            r#"[{"branch":"lost"}]"#,
        ] {
            assert!(parse_list(json, Path::new("/repo")).is_err(), "{json}");
        }
    }
}
