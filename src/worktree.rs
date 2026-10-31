//! GitHub-backed worktrees. This module never connects Rex or executes session scripts.
use crate::{
    config::{BrowserConfig, Config, WorktreeConfig},
    model::{Entry, Source},
    process,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

pub const CACHE_TTL_SECONDS: u64 = 24 * 60 * 60;
pub const NEGATIVE_CACHE_TTL_SECONDS: u64 = 60 * 60;

#[derive(Clone, Copy)]
enum ListMode {
    Cached,
    Stale,
    Refresh,
}

/// Explicit executable paths make subprocess tests independent of global PATH.
#[derive(Debug, Clone)]
pub struct Binaries {
    pub git: PathBuf,
    pub gh: PathBuf,
    pub open: PathBuf,
    pub osascript: PathBuf,
}
impl Default for Binaries {
    fn default() -> Self {
        Self {
            git: "git".into(),
            gh: "gh".into(),
            open: if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            }
            .into(),
            osascript: "osascript".into(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub repo: Option<String>,
    /// A configured repository, worktree root, or directory inside a linked worktree.
    pub path: Option<PathBuf>,
}
#[derive(Debug, Clone)]
pub struct PreparedWorktree {
    pub path: PathBuf,
    pub number: u64,
    pub created: bool,
    /// Root sends this only when it creates a new Rex session.
    pub startup_command: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeEntry {
    pub repo: String,
    pub number: u64,
    pub path: PathBuf,
    pub title: String,
    pub state: String,
    #[serde(default)]
    pub icon: Option<String>,
}
impl WorktreeEntry {
    pub fn entry(&self) -> Entry {
        let name = if self.title.is_empty() {
            format!("{} #{}", self.repo, self.number)
        } else {
            format!("{} #{} {}", self.repo, self.number, self.title)
        };
        let mut entry = Entry::new(name, &self.path, Source::Worktree);
        entry.active_window_name = self.state.clone();
        entry.icon = self.icon.clone();
        entry
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRef {
    pub repo: String,
    pub number: u64,
    pub is_pr: bool,
}

pub struct Worktrees<'a> {
    config: &'a Config,
    pub binaries: Binaries,
    pub cache_path: PathBuf,
}
impl<'a> Worktrees<'a> {
    pub fn new(config: &'a Config) -> Self {
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(dirs::cache_dir)
            .unwrap_or_else(std::env::temp_dir);
        Self {
            config,
            binaries: Binaries::default(),
            cache_path: cache.join("rexue/github-issues-v1.json"),
        }
    }
    fn git(&self, dir: &Path, args: &[&str]) -> Result<String> {
        process::text(Command::new(&self.binaries.git).current_dir(dir).args(args))
    }
    fn gh(&self, dir: &Path, args: &[&str]) -> Result<String> {
        process::text(Command::new(&self.binaries.gh).current_dir(dir).args(args))
    }
    pub fn select(&self, selection: &Selection) -> Result<&'a WorktreeConfig> {
        if let Some(repo) = &selection.repo {
            return self
                .config
                .worktree
                .iter()
                .find(|c| &c.repo == repo)
                .with_context(|| format!("no [[worktree]] configured for {repo}"));
        }
        let wanted = match &selection.path {
            Some(p) => crate::config::absolute_path(p)?,
            None => std::env::current_dir()?,
        };
        for cfg in &self.config.worktree {
            let repo = expand(&cfg.path)?;
            if same_path(&wanted, &repo) || same_path(&wanted, &worktree_root(cfg)?) {
                return Ok(cfg);
            }
        }
        let listing = self.git(&wanted, &["worktree", "list", "--porcelain", "-z"])?;
        let main = listing
            .split('\0')
            .find_map(|line| line.strip_prefix("worktree "))
            .context("cannot find main Git worktree")?;
        self.config
            .worktree
            .iter()
            .find(|c| expand(&c.path).is_ok_and(|p| same_path(&p, Path::new(main))))
            .with_context(|| format!("no [[worktree]] configured for {}", wanted.display()))
    }
    /// Resolves metadata before any Git mutation. Reuses only registered worktrees.
    pub fn prepare(
        &self,
        selection: &Selection,
        number: u64,
        is_pr: bool,
    ) -> Result<PreparedWorktree> {
        ensure!(number > 0, "issue/PR number must be positive");
        let cfg = self.select(selection)?;
        validate_repo(&cfg.repo)?;
        let repo = expand(&cfg.path)?;
        // issueOrPullRequest distinguishes issues from PRs without swallowing auth errors.
        let metadata = self.metadata(&repo, &cfg.repo, number)?;
        let pr = metadata.get("__typename").and_then(Value::as_str) == Some("PullRequest");
        ensure!(!is_pr || pr, "#{number} is not a pull request");
        let mut key = number;
        let mut detached = false;
        if pr {
            let me = self.gh(&repo, &["api", "user", "--jq", ".login"])?;
            let own = metadata["author"]["login"]
                .as_str()
                .is_some_and(|a| a == me.trim());
            let closing = metadata["closingIssuesReferences"]["nodes"]
                .as_array()
                .and_then(|nodes| nodes.iter().find_map(|n| n["number"].as_u64()));
            let fallback = first_issue_ref(&format!(
                "{}\n{}",
                metadata["title"].as_str().unwrap_or(""),
                metadata["body"].as_str().unwrap_or("")
            ));
            if let Some(issue) = own.then_some(closing.or(fallback)).flatten() {
                key = issue;
            } else {
                detached = true;
            }
        }
        let target = worktree_root(cfg)?.join(key.to_string());
        let listing = self.git(&repo, &["worktree", "list", "--porcelain", "-z"])?;
        let registered = listing
            .split('\0')
            .filter_map(|s| s.strip_prefix("worktree "))
            .any(|p| same_path(Path::new(p), &target));
        let created = !registered;
        if created {
            ensure!(
                !target.exists(),
                "{} exists but is not a registered worktree",
                target.display()
            );
            if cfg.fetch {
                self.git(&repo, &["fetch", "origin"])?;
            }
            fs::create_dir_all(target.parent().context("worktree target has no parent")?)?;
            let base = if cfg.base_branch.is_empty() {
                "origin/main"
            } else {
                &cfg.base_branch
            };
            if detached {
                self.add_worktree(&repo, &target, &["--detach"], base)?;
            } else {
                let template = if cfg.branch_template.is_empty() {
                    "{number}"
                } else {
                    &cfg.branch_template
                };
                let branch = template.replace("{number}", &key.to_string());
                self.git(&repo, &["check-ref-format", "--branch", &branch])?;
                let exists = self
                    .git(
                        &repo,
                        &["show-ref", "--verify", &format!("refs/heads/{branch}")],
                    )
                    .is_ok();
                if exists {
                    self.add_worktree(&repo, &target, &[], &branch)?;
                } else {
                    self.add_worktree(&repo, &target, &["-b", &branch], base)?;
                }
            }
        }
        if detached {
            // gh's explicit detached mode avoids occupying or moving a contributor's branch.
            let checkout = self.gh(
                &target,
                &[
                    "pr",
                    "checkout",
                    &number.to_string(),
                    "--repo",
                    &cfg.repo,
                    "--detach",
                ],
            );
            if let Err(err) = checkout {
                if created {
                    let _ = process::text(
                        Command::new(&self.binaries.git)
                            .current_dir(&repo)
                            .args(["worktree", "remove", "--force"])
                            .arg(&target),
                    );
                }
                return Err(err);
            }
        }
        Ok(PreparedWorktree {
            path: target,
            number: key,
            created,
            startup_command: if created {
                cfg.create_command.clone()
            } else {
                cfg.startup_command.clone()
            },
        })
    }
    fn add_worktree(
        &self,
        repo: &Path,
        target: &Path,
        options: &[&str],
        reference: &str,
    ) -> Result<()> {
        process::text(
            Command::new(&self.binaries.git)
                .current_dir(repo)
                .args(["worktree", "add"])
                .args(options)
                .arg("--")
                .arg(target)
                .arg(reference),
        )?;
        Ok(())
    }
    fn metadata(&self, dir: &Path, repo: &str, number: u64) -> Result<Value> {
        let (owner, name) = repo_parts(repo)?;
        let query = format!(
            "query {{ repository(owner:{},name:{}) {{ issueOrPullRequest(number:{number}) {{ __typename ... on Issue {{ number title body }} ... on PullRequest {{ number title body author {{ login }} closingIssuesReferences(first:10) {{ nodes {{ number }} }} }} }} }} }}",
            json!(owner),
            json!(name)
        );
        let value: Value = serde_json::from_str(
            &self.gh(dir, &["api", "graphql", "-f", &format!("query={query}")])?,
        )?;
        ensure!(
            value.get("errors").is_none(),
            "GitHub metadata query failed: {}",
            value["errors"]
        );
        let node = &value["data"]["repository"]["issueOrPullRequest"];
        ensure!(!node.is_null(), "no issue or PR #{number} in {repo}");
        Ok(node.clone())
    }
    /// Lists local numbered directories with cached metadata, even when stale.
    /// Never invokes gh or writes the cache; suitable for initial picker display.
    pub fn list_cached(&self, selection: &Selection) -> Result<Vec<WorktreeEntry>> {
        self.list_with_mode(selection, ListMode::Cached)
    }
    /// Refreshes every local row from GitHub regardless of cache age.
    /// Failure is returned so callers can preserve their previously displayed rows.
    pub fn refresh(&self, selection: &Selection) -> Result<Vec<WorktreeEntry>> {
        self.list_with_mode(selection, ListMode::Refresh)
    }
    /// Ordinary listings refresh expired metadata and retain stale data when offline.
    /// Explicit refreshes propagate errors just like `refresh`.
    pub fn list(&self, selection: &Selection, refresh: bool) -> Result<Vec<WorktreeEntry>> {
        if refresh {
            self.refresh(selection)
        } else {
            self.list_with_mode(selection, ListMode::Stale)
        }
    }
    fn list_with_mode(&self, selection: &Selection, mode: ListMode) -> Result<Vec<WorktreeEntry>> {
        let cfg = self.select(selection)?;
        validate_repo(&cfg.repo)?;
        let root = worktree_root(cfg)?;
        let dirs = match fs::read_dir(&root) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        let mut entries = Vec::new();
        for item in dirs {
            let item = item?;
            if !item.file_type()?.is_dir() {
                continue;
            }
            let Some(number) = item
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u64>().ok())
                .filter(|n| *n > 0)
            else {
                continue;
            };
            entries.push(WorktreeEntry {
                repo: cfg.repo.clone(),
                number,
                path: item.path(),
                title: String::new(),
                state: String::new(),
                icon: cfg.icon.clone(),
            });
        }
        entries.sort_by_key(|e| e.number);
        let mut cache = load_cache(&self.cache_path);
        let now = timestamp();
        let stale: Vec<_> = entries
            .iter()
            .filter(|e| match mode {
                ListMode::Cached => false,
                ListMode::Refresh => true,
                ListMode::Stale => cache
                    .get(&cache_key(&cfg.repo, e.number))
                    .is_none_or(|c| c.expired(now)),
            })
            .map(|e| e.number)
            .collect();
        let mut updates = BTreeMap::new();
        for batch in stale.chunks(50) {
            let found = match self.issue_batch(&expand(&cfg.path)?, &cfg.repo, batch) {
                Ok(found) => found,
                Err(error) if matches!(mode, ListMode::Refresh) => return Err(error),
                Err(_) => continue,
            };
            for (number, issue) in found {
                updates.insert(
                    cache_key(&cfg.repo, number),
                    CachedIssue {
                        fetched: now,
                        issue,
                    },
                );
            }
        }
        cache.extend(updates.clone());
        if !updates.is_empty() {
            let saved = save_cache(&self.cache_path, updates);
            if matches!(mode, ListMode::Refresh) {
                saved?;
            }
        }
        for entry in &mut entries {
            if let Some(issue) = cache
                .get(&cache_key(&cfg.repo, entry.number))
                .and_then(|c| c.issue.as_ref())
            {
                entry.title = issue.title.clone();
                entry.state = issue.state.clone();
            }
        }
        Ok(entries)
    }
    fn issue_batch(
        &self,
        dir: &Path,
        repo: &str,
        numbers: &[u64],
    ) -> Result<BTreeMap<u64, Option<Issue>>> {
        let (owner, name) = repo_parts(repo)?;
        let fields = numbers.iter().map(|n| format!("n{n}:issueOrPullRequest(number:{n}) {{ ... on Issue {{ title state }} ... on PullRequest {{ title state }} }}")).collect::<Vec<_>>().join(" ");
        let query = format!(
            "query {{ repository(owner:{},name:{}) {{ {fields} }} }}",
            json!(owner),
            json!(name)
        );
        let raw: Value = serde_json::from_str(
            &self.gh(dir, &["api", "graphql", "-f", &format!("query={query}")])?,
        )?;
        ensure!(raw.get("errors").is_none(), "GitHub batch failed");
        let repo = raw["data"]["repository"]
            .as_object()
            .context("missing GitHub repository")?;
        let mut result = BTreeMap::new();
        for number in numbers {
            if let Some(node) = repo.get(&format!("n{number}")) {
                result.insert(
                    *number,
                    if node.is_null() {
                        None
                    } else {
                        Some(serde_json::from_value(node.clone())?)
                    },
                );
            }
        }
        Ok(result)
    }
    pub fn browse_url(&self, path: &Path, pr: bool) -> Result<String> {
        let path = fs::canonicalize(path)?;
        for cfg in &self.config.worktree {
            let root = worktree_root(cfg)?;
            let root = fs::canonicalize(&root).unwrap_or(root);
            let Ok(relative) = path.strip_prefix(&root) else {
                continue;
            };
            let Some(number) = relative
                .components()
                .next()
                .and_then(|c| c.as_os_str().to_str())
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|n| *n > 0)
            else {
                continue;
            };
            validate_repo(&cfg.repo)?;
            if !pr {
                return Ok(format!("https://github.com/{}/issues/{number}", cfg.repo));
            }
            let url = self.gh(
                &root.join(number.to_string()),
                &["pr", "view", "--json", "url", "--jq", ".url"],
            )?;
            let url = url.trim();
            ensure!(
                parse_github_ref(url).is_some_and(|r| r.is_pr),
                "gh returned an invalid pull request URL"
            );
            return Ok(url.into());
        }
        bail!(
            "{} is not inside a configured numbered worktree",
            path.display()
        )
    }
    /// Return the URL for printing. Only opens a browser when print_only is false.
    pub fn browse(&self, path: &Path, pr: bool, print_only: bool) -> Result<String> {
        let url = self.browse_url(path, pr)?;
        if !print_only {
            self.open_url(&url)?;
        }
        Ok(url)
    }
    pub fn open_url(&self, url: &str) -> Result<()> {
        ensure!(
            parse_github_ref(url).is_some(),
            "expected a GitHub issue/PR URL"
        );
        let mut command = Command::new(&self.binaries.open);
        if cfg!(target_os = "macos")
            && let Some(browser) = self
                .config
                .browser
                .as_ref()
                .filter(|b| !b.application.is_empty())
        {
            command.arg("-a").arg(&browser.application);
        }
        process::text(command.arg(url))?;
        Ok(())
    }
    pub fn active_browser_ref(&self) -> Result<GitHubRef> {
        ensure!(
            cfg!(target_os = "macos"),
            "active browser URL requires macOS"
        );
        let browser = self
            .config
            .browser
            .as_ref()
            .context("configure [browser].application first")?;
        let script = browser_script(browser)?;
        let url = process::text(Command::new(&self.binaries.osascript).args(["-e", &script]))?;
        parse_github_ref(url.trim()).context("active browser tab is not a GitHub issue or PR")
    }
}

pub fn worktree_root(cfg: &WorktreeConfig) -> Result<PathBuf> {
    let dir = if cfg.worktree_dir.is_empty() {
        ".wk"
    } else {
        &cfg.worktree_dir
    };
    if dir.starts_with('~') && dir != "~" && !dir.starts_with("~/") {
        bail!("named-user tilde expansion is unsupported: {dir:?}");
    }
    let expanded =
        shellexpand::full(dir).with_context(|| format!("could not expand worktree_dir {dir:?}"))?;
    let dir = PathBuf::from(expanded.as_ref());
    let path = if dir.is_absolute() {
        dir
    } else {
        expand(&cfg.path)?.join(dir)
    };
    crate::config::absolute_path(&path)
}
fn expand(path: &str) -> Result<PathBuf> {
    crate::config::expand_path(path)
}
fn same_path(a: &Path, b: &Path) -> bool {
    fs::canonicalize(a).unwrap_or_else(|_| a.to_owned())
        == fs::canonicalize(b).unwrap_or_else(|_| b.to_owned())
}
fn validate_repo(repo: &str) -> Result<()> {
    repo_parts(repo).map(|_| ())
}
fn repo_parts(repo: &str) -> Result<(&str, &str)> {
    let parts: Vec<_> = repo.split('/').collect();
    ensure!(
        parts.len() == 2
            && parts.iter().all(|s| !s.is_empty()
                && *s != "."
                && *s != ".."
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))),
        "expected GitHub repository owner/name, got {repo:?}"
    );
    Ok((parts[0], parts[1]))
}
pub fn parse_github_ref(raw: &str) -> Option<GitHubRef> {
    let path = raw
        .strip_prefix("https://github.com/")
        .or_else(|| raw.strip_prefix("http://github.com/"))
        .or_else(|| raw.strip_prefix("https://www.github.com/"))
        .or_else(|| raw.strip_prefix("github.com/"))?;
    let mut parts = path.split('/');
    let repo = format!("{}/{}", parts.next()?, parts.next()?);
    validate_repo(&repo).ok()?;
    let is_pr = match parts.next()? {
        "issues" => false,
        "pull" => true,
        _ => return None,
    };
    let number = parts
        .next()?
        .split(['?', '#'])
        .next()?
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)?;
    Some(GitHubRef {
        repo,
        number,
        is_pr,
    })
}
fn first_issue_ref(text: &str) -> Option<u64> {
    text.match_indices('#').find_map(|(i, _)| {
        text[i + 1..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
    })
}
fn browser_script(browser: &BrowserConfig) -> Result<String> {
    ensure!(
        !browser.application.is_empty() && !browser.application.chars().any(char::is_control),
        "invalid browser application"
    );
    let application = browser
        .application
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    // url_command is an explicitly configured AppleScript expression, never shell text.
    let expression = browser
        .url_command
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(if browser.application.eq_ignore_ascii_case("Safari") {
            "URL of current tab of front window"
        } else {
            "URL of active tab of front window"
        });
    Ok(format!(
        "tell application \"{application}\" to return {expression}"
    ))
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Issue {
    title: String,
    state: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedIssue {
    fetched: u64,
    issue: Option<Issue>,
}
impl CachedIssue {
    fn expired(&self, now: u64) -> bool {
        let ttl = if self.issue.is_some() {
            CACHE_TTL_SECONDS
        } else {
            NEGATIVE_CACHE_TTL_SECONDS
        };
        now.saturating_sub(self.fetched) >= ttl
    }
}
type Cache = BTreeMap<String, CachedIssue>;
fn cache_key(repo: &str, number: u64) -> String {
    format!("{repo}#{number}")
}
fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn load_cache(path: &Path) -> Cache {
    fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
fn save_cache(path: &Path, updates: Cache) -> Result<()> {
    use fs2::FileExt;
    use std::io::Write;
    let parent = path.parent().context("cache has no parent")?;
    fs::create_dir_all(parent)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))?;
    lock.lock_exclusive()?;
    let mut cache = load_cache(path);
    cache.extend(updates);
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(&serde_json::to_vec(&cache)?)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    let _ = fs::remove_file(temp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relative_worktree_root_expands_embedded_environment() {
        let d = tempfile::tempdir().unwrap();
        let home = std::env::var("HOME").unwrap();
        let cfg = WorktreeConfig {
            path: d.path().display().to_string(),
            worktree_dir: "prefix/${HOME}".into(),
            ..Default::default()
        };
        let expected = d.path().join("prefix").join(home.trim_start_matches('/'));
        assert_eq!(worktree_root(&cfg).unwrap(), expected);
    }
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    struct Fixture {
        dir: TempDir,
        config: Config,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let repo = dir.path().join("repository with spaces");
            fs::create_dir(&repo).unwrap();
            for args in [
                vec!["init", "-b", "main"],
                vec![
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "initial",
                ],
            ] {
                process::text(Command::new("git").current_dir(&repo).args(args)).unwrap();
            }
            let mut config = Config::default();
            config.worktree.push(serde_json::from_value(json!({
                "repo":"owner/project", "path":repo.to_str().unwrap(), "worktree_dir":".wk", "branch_template":"issue/{number}", "base_branch":"main", "fetch":false,
                "create_command":"echo create", "startup_command":"echo startup"
            })).unwrap());
            Self { dir, config }
        }
        fn service(&self) -> Worktrees<'_> {
            let mut service = Worktrees::new(&self.config);
            service.cache_path = self.dir.path().join("cache/issues.json");
            service.binaries.gh = self.dir.path().join("fake gh");
            service
        }
        fn selection(&self) -> Selection {
            Selection {
                repo: Some("owner/project".into()),
                path: None,
            }
        }
        #[cfg(unix)]
        fn fake(&self, script: &str) {
            let file = self.dir.path().join("fake gh");
            fs::write(&file, format!("#!/bin/sh\n{script}\n")).unwrap();
            fs::set_permissions(file, fs::Permissions::from_mode(0o700)).unwrap();
        }
        #[cfg(unix)]
        fn issue(&self) {
            self.fake("printf '%s' '{\"data\":{\"repository\":{\"issueOrPullRequest\":{\"__typename\":\"Issue\",\"number\":12}}}}'");
        }
    }
    #[test]
    fn strict_github_urls_and_issue_references() {
        assert_eq!(
            parse_github_ref("https://github.com/owner/project/pull/42/files?x=1#fragment"),
            Some(GitHubRef {
                repo: "owner/project".into(),
                number: 42,
                is_pr: true
            })
        );
        for url in [
            "https://evil.test/github.com/a/b/pull/1",
            "https://github.com.evil/a/b/pull/1",
            "https://github.com/a/b/issues/0",
            "https://github.com/a/b/issues/4junk",
            "file:///a/b/pull/1",
        ] {
            assert!(parse_github_ref(url).is_none(), "{url}");
        }
        assert_eq!(first_issue_ref("Fix #12 and #54"), Some(12));
    }
    #[test]
    fn apple_script_quotes_application_and_handles_safari() {
        let browser: BrowserConfig =
            serde_json::from_value(json!({"application":"Safari"})).unwrap();
        assert!(
            browser_script(&browser)
                .unwrap()
                .ends_with("URL of current tab of front window")
        );
        let browser: BrowserConfig =
            serde_json::from_value(json!({"application":"A\" & evil & \""})).unwrap();
        assert!(
            browser_script(&browser)
                .unwrap()
                .contains("A\\\" & evil & \\\"")
        );
    }
    #[cfg(unix)]
    #[test]
    fn create_reuse_select_and_browse_local_worktree() {
        let fixture = Fixture::new();
        fixture.issue();
        let service = fixture.service();
        let first = service.prepare(&fixture.selection(), 12, false).unwrap();
        assert!(first.created);
        assert_eq!(first.startup_command.as_deref(), Some("echo create"));
        assert_eq!(
            service
                .git(&first.path, &["branch", "--show-current"])
                .unwrap()
                .trim(),
            "issue/12"
        );
        let second = service.prepare(&fixture.selection(), 12, false).unwrap();
        assert!(!second.created);
        assert_eq!(second.startup_command.as_deref(), Some("echo startup"));
        fs::create_dir(first.path.join("subdir")).unwrap();
        assert_eq!(
            service
                .select(&Selection {
                    path: Some(first.path.join("subdir")),
                    repo: None
                })
                .unwrap()
                .repo,
            "owner/project"
        );
        assert_eq!(
            service
                .browse(&first.path.join("subdir"), false, true)
                .unwrap(),
            "https://github.com/owner/project/issues/12"
        );
        assert!(!first.path.join("create").exists());
    }
    #[cfg(unix)]
    #[test]
    fn own_pr_uses_closing_issue_and_foreign_pr_is_detached() {
        let fixture = Fixture::new();
        fixture.fake("case \"$*\" in\n  *'api user'*) printf '%s' 'me' ;;\n  *'pr checkout'*) git checkout --detach >/dev/null 2>&1 ;;\n  *) printf '%s' '{\"data\":{\"repository\":{\"issueOrPullRequest\":{\"__typename\":\"PullRequest\",\"author\":{\"login\":\"me\"},\"title\":\"Fix #99\",\"closingIssuesReferences\":{\"nodes\":[{\"number\":12}]}}}}}' ;;\nesac");
        let service = fixture.service();
        let own = service.prepare(&fixture.selection(), 42, true).unwrap();
        assert_eq!(own.number, 12);
        fixture.fake("case \"$*\" in\n  *'api user'*) printf '%s' 'me' ;;\n  *'pr checkout'*'--detach'*) git checkout --detach >/dev/null 2>&1 ;;\n  *'pr checkout'*) exit 9 ;;\n  *) printf '%s' '{\"data\":{\"repository\":{\"issueOrPullRequest\":{\"__typename\":\"PullRequest\",\"author\":{\"login\":\"someone-else\"},\"closingIssuesReferences\":{\"nodes\":[{\"number\":12}]}}}}}' ;;\nesac");
        let foreign = service.prepare(&fixture.selection(), 43, true).unwrap();
        assert_eq!(foreign.number, 43);
        assert!(
            service
                .git(&foreign.path, &["symbolic-ref", "-q", "HEAD"])
                .is_err()
        );
        assert!(
            !service
                .prepare(&fixture.selection(), 43, true)
                .unwrap()
                .created
        );
    }
    #[cfg(unix)]
    #[test]
    fn metadata_failure_and_existing_directory_never_create_worktree() {
        let fixture = Fixture::new();
        fixture.fake("exit 1");
        let service = fixture.service();
        assert!(service.prepare(&fixture.selection(), 12, false).is_err());
        let target = worktree_root(&fixture.config.worktree[0])
            .unwrap()
            .join("12");
        assert!(!target.exists());
        fixture.issue();
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("keep"), "keep").unwrap();
        assert!(service.prepare(&fixture.selection(), 12, false).is_err());
        assert!(target.join("keep").exists());
    }
    #[cfg(unix)]
    #[test]
    fn batched_cache_warm_offline_negative_and_refresh() {
        let fixture = Fixture::new();
        let root = worktree_root(&fixture.config.worktree[0]).unwrap();
        for n in ["2", "12", "unrelated"] {
            fs::create_dir_all(root.join(n)).unwrap();
        }
        fs::write(root.join("3"), "not a directory").unwrap();
        fixture.fake("printf '%s' '{\"data\":{\"repository\":{\"n2\":{\"title\":\"A title\",\"state\":\"OPEN\"},\"n12\":null}}}'");
        let service = fixture.service();
        let entries = service.list(&fixture.selection(), false).unwrap();
        assert_eq!(
            entries.iter().map(|e| e.number).collect::<Vec<_>>(),
            vec![2, 12]
        );
        assert_eq!(entries[0].title, "A title");
        assert_eq!(entries[0].entry().source, Source::Worktree);
        let mut cache = load_cache(&service.cache_path);
        assert!(cache["owner/project#12"].issue.is_none());
        // If a warm listing runs gh, this valid response would change the title.
        fixture.fake("printf '%s' '{\"data\":{\"repository\":{\"n2\":{\"title\":\"Changed\",\"state\":\"CLOSED\"},\"n12\":null}}}'");
        assert_eq!(
            service.list(&fixture.selection(), false).unwrap()[0].title,
            "A title"
        );
        assert_eq!(
            service.list(&fixture.selection(), true).unwrap()[0].title,
            "Changed"
        );
        fixture.fake("exit 1");
        for issue in cache.values_mut() {
            issue.fetched = 0;
        }
        fs::write(&service.cache_path, serde_json::to_vec(&cache).unwrap()).unwrap();
        assert_eq!(
            service.list(&fixture.selection(), false).unwrap()[0].title,
            "A title"
        );
        fs::write(&service.cache_path, "corrupt").unwrap();
        assert_eq!(
            service.list(&fixture.selection(), false).unwrap()[0].title,
            ""
        );
    }
    #[cfg(unix)]
    #[test]
    fn cached_first_listing_never_runs_gh_and_carries_icons() {
        let mut fixture = Fixture::new();
        fixture.config.worktree[0].icon = Some("custom icon".into());
        let root = worktree_root(&fixture.config.worktree[0]).unwrap();
        fs::create_dir_all(root.join("12")).unwrap();
        fixture.fake("printf 'called' > \"$0.calls\"\nexit 1");
        let service = fixture.service();
        let selection = fixture.selection();
        assert_eq!(service.list_cached(&selection).unwrap()[0].title, "");
        save_cache(
            &service.cache_path,
            BTreeMap::from([(
                cache_key("owner/project", 12),
                CachedIssue {
                    fetched: 0,
                    issue: Some(Issue {
                        title: "Stale title".into(),
                        state: "OPEN".into(),
                    }),
                },
            )]),
        )
        .unwrap();
        let entries = service.list_cached(&selection).unwrap();
        assert_eq!(entries[0].title, "Stale title");
        assert_eq!(entries[0].icon.as_deref(), Some("custom icon"));
        assert_eq!(entries[0].entry().icon, entries[0].icon);
        assert!(!fixture.dir.path().join("fake gh.calls").exists());
    }
    #[cfg(unix)]
    #[test]
    fn manual_offline_refresh_errors_and_preserves_cached_rows() {
        let fixture = Fixture::new();
        let root = worktree_root(&fixture.config.worktree[0]).unwrap();
        fs::create_dir_all(root.join("12")).unwrap();
        fixture.fake("printf '%s' '{\"data\":{\"repository\":{\"n12\":{\"title\":\"Cached title\",\"state\":\"OPEN\"}}}}'");
        let service = fixture.service();
        let selection = fixture.selection();
        service.list(&selection, false).unwrap();
        let before = fs::read(&service.cache_path).unwrap();
        fixture.fake("printf 'offline' >&2\nexit 1");
        assert!(service.refresh(&selection).is_err());
        assert!(service.list(&selection, true).is_err());
        assert_eq!(fs::read(&service.cache_path).unwrap(), before);
        assert_eq!(
            service.list_cached(&selection).unwrap()[0].title,
            "Cached title"
        );
        assert_eq!(
            service.list(&selection, false).unwrap()[0].title,
            "Cached title"
        );
    }
    #[cfg(unix)]
    #[test]
    fn negative_cache_expires_after_one_hour_while_positive_lasts_a_day() {
        let fixture = Fixture::new();
        let root = worktree_root(&fixture.config.worktree[0]).unwrap();
        for number in [2, 12] {
            fs::create_dir_all(root.join(number.to_string())).unwrap();
        }
        let service = fixture.service();
        let now = timestamp();
        save_cache(
            &service.cache_path,
            BTreeMap::from([
                (
                    cache_key("owner/project", 2),
                    CachedIssue {
                        fetched: now - 3601,
                        issue: Some(Issue {
                            title: "Positive".into(),
                            state: "OPEN".into(),
                        }),
                    },
                ),
                (
                    cache_key("owner/project", 12),
                    CachedIssue {
                        fetched: now - 3590,
                        issue: None,
                    },
                ),
            ]),
        )
        .unwrap();
        fixture.fake("printf '%s\\n' \"$*\" >> \"$0.calls\"\nprintf '%s' '{\"data\":{\"repository\":{\"n12\":{\"title\":\"Newly available\",\"state\":\"OPEN\"}}}}'");
        service.list(&fixture.selection(), false).unwrap();
        let calls = fixture.dir.path().join("fake gh.calls");
        assert!(!calls.exists());
        let mut cache = load_cache(&service.cache_path);
        cache
            .get_mut(&cache_key("owner/project", 12))
            .unwrap()
            .fetched = now - 3601;
        fs::write(&service.cache_path, serde_json::to_vec(&cache).unwrap()).unwrap();
        let entries = service.list(&fixture.selection(), false).unwrap();
        assert_eq!(entries[0].title, "Positive");
        assert_eq!(entries[1].title, "Newly available");
        let query = fs::read_to_string(calls).unwrap();
        assert!(query.contains("n12:issueOrPullRequest"));
        assert!(!query.contains("n2:issueOrPullRequest"));
    }
    #[cfg(unix)]
    #[test]
    fn existing_branch_is_reused_and_failed_foreign_checkout_is_removed() {
        let fixture = Fixture::new();
        fixture.issue();
        let service = fixture.service();
        let repo = expand(&fixture.config.worktree[0].path).unwrap();
        service.git(&repo, &["branch", "issue/12"]).unwrap();
        let prepared = service.prepare(&fixture.selection(), 12, false).unwrap();
        assert_eq!(
            service
                .git(&prepared.path, &["branch", "--show-current"])
                .unwrap()
                .trim(),
            "issue/12"
        );
        fixture.fake("case \"$*\" in\n  *'api user'*) printf '%s' 'me' ;;\n  *'pr checkout'*) exit 1 ;;\n  *) printf '%s' '{\"data\":{\"repository\":{\"issueOrPullRequest\":{\"__typename\":\"PullRequest\",\"author\":{\"login\":\"other\"}}}}}' ;;\nesac");
        assert!(service.prepare(&fixture.selection(), 44, true).is_err());
        assert!(
            !worktree_root(&fixture.config.worktree[0])
                .unwrap()
                .join("44")
                .exists()
        );
    }
    #[cfg(unix)]
    #[test]
    fn listing_batches_fifty_numbers_per_request() {
        let fixture = Fixture::new();
        let root = worktree_root(&fixture.config.worktree[0]).unwrap();
        for number in 1..=51 {
            fs::create_dir_all(root.join(number.to_string())).unwrap();
        }
        // Each call leaves a marker beside the injected executable, independent of cwd.
        fixture
            .fake("printf 'call\\n' >> \"$0.calls\"\nprintf '%s' '{\"data\":{\"repository\":{}}}'");
        let entries = fixture.service().list(&fixture.selection(), false).unwrap();
        assert_eq!(entries.len(), 51);
        assert_eq!(
            fs::read_to_string(fixture.dir.path().join("fake gh.calls"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }
    #[test]
    fn cache_ttl_boundaries_distinguish_positive_and_negative_entries() {
        let mut cached = CachedIssue {
            fetched: 100,
            issue: None,
        };
        assert!(!cached.expired(100 + 3600 - 1));
        assert!(cached.expired(100 + 3600));
        cached.issue = Some(Issue {
            title: "Title".into(),
            state: "OPEN".into(),
        });
        assert!(!cached.expired(100 + 86400 - 1));
        assert!(cached.expired(100 + 86400));
        assert!(!cached.expired(99));
    }
    #[test]
    fn atomic_cache_merges_and_scopes_repositories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        for repo in ["a/b", "c/d"] {
            save_cache(
                &path,
                BTreeMap::from([(
                    cache_key(repo, 1),
                    CachedIssue {
                        fetched: 1,
                        issue: None,
                    },
                )]),
            )
            .unwrap();
        }
        assert_eq!(load_cache(&path).len(), 2);
        assert!(
            !path
                .with_extension(format!("{}.tmp", std::process::id()))
                .exists()
        );
    }
}
