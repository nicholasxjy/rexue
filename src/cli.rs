use crate::{
    backend::Rex,
    config::Config,
    model::{Entry, Source},
    picker,
    session::{ConnectOptions, ListOptions, Manager},
    worktree::{RemoveOptions, Selection, Worktrees},
};
use anyhow::{Context, Result, bail};
use clap::{ArgAction, Args, CommandFactory, Parser, Subcommand, builder::TypedValueParser};
use std::{
    io::{self, Write},
    path::PathBuf,
    time::Duration,
};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "An fzf session picker for Rex",
    long_about = "Manage Rex sessions with the system fzf picker. With no command, opens the picker."
)]
pub struct Cli {
    /// Config file (source filters use --config/-c on list and picker).
    #[arg(short = 'C', long = "config-file", global = true)]
    pub config: Option<PathBuf>,
    /// Rex app client label or full ID; also reads REX_CLIENT.
    #[arg(long, env = "REX_CLIENT", global = true)]
    pub client: Option<String>,
    #[arg(short = 'S', long, env = "REX_SERVER", global = true)]
    pub server: Option<String>,
    #[arg(long, env = "REXUE_REX", default_value = "rex", global = true)]
    pub rex_bin: PathBuf,
    /// Create/reuse without moving any Rex app client.
    #[arg(long, global = true, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
    #[arg(default_value_t = false)]
    pub detach: bool,
    #[command(subcommand)]
    pub command: Option<Commands>,
}
#[derive(Debug, Subcommand)]
pub enum Commands {
    #[command(name = "__picker", hide = true)]
    PickerHelper {
        socket: PathBuf,
        action: String,
        id: Option<String>,
    },
    /// List live sessions, configured projects and frequent directories.
    #[command(alias = "l")]
    List(ListArgs),
    /// Create or reuse a project session and display it in Rex.
    #[command(alias = "cn")]
    Connect(ConnectArgs),
    /// Search and select sessions using system fzf.
    #[command(aliases=["pick","pk"])]
    Picker(PickerArgs),
    /// Preview a running session or project directory.
    #[command(alias = "p")]
    Preview {
        target: String,
        #[arg(short = 'w', long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
        #[arg(default_value_t = false)]
        watch: bool,
        #[arg(long,default_value="500ms",value_parser=parse_duration)]
        interval: Duration,
    },
    /// Switch this Rex client to its previous session.
    #[command(alias = "L")]
    Last,
    /// Manage branch worktrees with Worktrunk and connect their Rex sessions.
    #[command(alias = "wt")]
    Worktree {
        #[command(subcommand)]
        command: WorktreeCommand,
    },
    /// Rename a running session.
    Rename {
        target: Option<String>,
        #[arg(long)]
        to: String,
    },
    /// Generate a shell completion script.
    Completion { shell: clap_complete::Shell },
}
#[derive(Debug, Args, Default, Clone)]
pub struct Sources {
    /// List running agents in every Rex terminal pane.
    #[arg(long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
    #[arg(default_value_t = false)]
    pub agents: bool,
    #[arg(short = 't', long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
    #[arg(default_value_t = false)]
    pub rex: bool,
    #[arg(short = 'c', long = "config", action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
    #[arg(default_value_t = false)]
    pub configs: bool,
    #[arg(short = 'z', long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
    #[arg(default_value_t = false)]
    pub zoxide: bool,
    #[arg(short = 'H', long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
    #[arg(default_value_t = false)]
    pub hide_attached: bool,
    #[arg(short = 'd', long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
    #[arg(default_value_t = false)]
    pub hide_duplicates: bool,
    #[arg(long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
    #[arg(default_value_t = false)]
    pub no_cache: bool,
}
impl Sources {
    fn options(&self) -> ListOptions {
        ListOptions {
            agents: self.agents,
            rex: self.rex,
            config: self.configs,
            zoxide: self.zoxide,
            hide_attached: self.hide_attached,
            hide_duplicates: self.hide_duplicates,
            no_cache: self.no_cache,
            ..Default::default()
        }
    }
}
#[derive(Debug, Args)]
pub struct ListArgs {
    #[command(flatten)]
    pub sources: Sources,
    #[arg(short = 'j', long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool, default_value_t = false)]
    pub json: bool,
    #[arg(short = 'i', long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
    #[arg(default_value_t = false)]
    pub icons: bool,
    #[arg(short = 'n', long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
    #[arg(default_value_t = false)]
    pub no_color: bool,
    #[arg(short = 'b', long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
    #[arg(default_value_t = false)]
    pub blacklisted: bool,
    #[arg(long)]
    pub format: Option<String>,
    #[arg(long, value_delimiter = ',')]
    pub icons_exclude: Vec<String>,
}
impl ListArgs {
    fn validate(&self) -> Result<()> {
        if self.json && self.format.is_some() {
            bail!("--json and --format cannot be used together");
        }
        Ok(())
    }
}
#[derive(Debug, Args)]
pub struct ConnectArgs {
    #[arg(required=true,num_args=1..)]
    pub target: Vec<String>,
    #[arg(short = 'c', long)]
    pub command: Option<String>,
    #[arg(short = 'r', long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
    #[arg(default_value_t = false)]
    pub root: bool,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool, default_value_t = false)]
    pub json: bool,
}
impl ConnectArgs {
    fn options(&self, detach: bool) -> ConnectOptions {
        ConnectOptions {
            detach,
            // Empty commands fall through to configured session startup.
            command: self.command.clone().filter(|s| !s.is_empty()),
            root: self.root,
            name: self.name.clone(),
        }
    }
}
#[derive(Debug, Args, Default, Clone)]
pub struct PickerArgs {
    #[command(flatten)]
    pub sources: Sources,
    #[arg(short = 'i', long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
    pub icons: Option<bool>,
    #[arg(short = 's', long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
    pub separator_aware: Option<bool>,
    #[arg(short = 'p', long)]
    pub prompt: Option<String>,
    #[arg(long)]
    pub placeholder: Option<String>,
    #[arg(short = 'q', long)]
    pub query: Option<String>,
    #[arg(long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
    #[arg(default_value_t = false)]
    pub no_alias_auto: bool,
    #[arg(long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
    pub preview: Option<bool>,
    #[arg(long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
    #[arg(default_value_t = false)]
    pub no_preview: bool,
}
#[derive(Debug, Args, Clone, Default)]
pub struct RepoArgs {
    /// Repository or worktree directory; defaults to the current directory.
    #[arg(long, value_parser = clap::builder::OsStringValueParser::new().map(PathBuf::from))]
    pub path: Option<PathBuf>,
}
impl RepoArgs {
    fn selection(&self) -> Result<Selection> {
        Ok(Selection {
            path: self
                .path
                .as_ref()
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| crate::config::absolute_path(p))
                .transpose()?,
        })
    }
}
#[derive(Debug, Subcommand)]
pub enum WorktreeCommand {
    /// Create a branch and worktree using Worktrunk configuration (no Rex session).
    Create {
        branch: String,
        #[arg(short = 'b', long)]
        base: Option<String>,
        #[command(flatten)]
        repo: RepoArgs,
        #[arg(long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool, default_value_t = false)]
        json: bool,
    },
    /// Connect a branch/path to Rex; omitted target connects the current worktree.
    #[command(alias = "c")]
    Connect {
        target: Option<String>,
        /// Explicit Rex name, useful when a retained session owns the old path.
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        repo: RepoArgs,
        #[arg(long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool, default_value_t = false)]
        json: bool,
    },
    /// List all registered worktrees, including detached checkouts.
    #[command(aliases=["l","ls"])]
    List {
        #[command(flatten)]
        repo: RepoArgs,
        #[arg(short = 'j', long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool, default_value_t = false)]
        json: bool,
    },
    /// Select and connect Rex (Ctrl+W creates; Ctrl+X removes with confirmation).
    #[command(aliases=["pick","pk","p"])]
    Picker {
        #[command(flatten)]
        repo: RepoArgs,
        #[arg(long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool, default_value_t = false)]
        refresh: bool,
        #[arg(short = 'i', long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool)]
        icons: Option<bool>,
        #[arg(short = 'p', long)]
        prompt: Option<String>,
        #[arg(long)]
        placeholder: Option<String>,
        #[arg(short = 'q', long)]
        query: Option<String>,
    },
    /// Remove a selected worktree using Worktrunk protections; keep its Rex session.
    #[command(aliases=["rm","r"])]
    Remove {
        target: String,
        #[command(flatten)]
        repo: RepoArgs,
        #[arg(long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool, default_value_t = false)]
        force: bool,
        #[arg(short = 'D', long, conflicts_with = "no_delete_branch", action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool, default_value_t = false)]
        force_delete: bool,
        #[arg(long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool, default_value_t = false)]
        no_delete_branch: bool,
        #[arg(long, action = ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true", value_parser = parse_bool, default_value_t = false)]
        json: bool,
    },
}
pub fn run(cli: Cli) -> Result<()> {
    if let Some(Commands::PickerHelper { socket, action, id }) = &cli.command {
        return picker::helper(socket, action, id.as_deref());
    }
    if let Some(Commands::Completion { shell }) = &cli.command {
        let mut script = Vec::new();
        clap_complete::generate(*shell, &mut Cli::command(), "rexue", &mut script);
        io::stdout().lock().write_all(&script)?;
        return Ok(());
    }
    let cfg = Config::load(cli.config.as_deref())?;
    let manager = Manager::new(
        Rex {
            bin: cli.rex_bin,
            client: cli.client,
            server: cli.server,
        },
        cfg,
    );
    match cli
        .command
        .unwrap_or(Commands::Picker(PickerArgs::default()))
    {
        Commands::Completion { .. } | Commands::PickerHelper { .. } => unreachable!(),
        Commands::List(args) => {
            args.validate()?;
            let mut opts = args.sources.options();
            opts.blacklisted = args.blacklisted;
            let entries = manager.list(&opts)?;
            if args.json {
                out(&serde_json::to_string_pretty(&entries)?)?;
            } else {
                for entry in entries {
                    out(&configured_format_entry(
                        &entry,
                        args.format.as_deref(),
                        args.icons,
                        !args.no_color,
                        &args.icons_exclude,
                        manager.config.icons.get(entry.source.as_str()),
                    )?)?;
                }
            }
        }
        Commands::Connect(args) => {
            let target = args.target.join(" ");
            if target.is_empty() {
                return Ok(());
            }
            let connection = manager.connect(&target, &args.options(cli.detach))?;
            connection_out(&connection, args.json)?;
        }
        Commands::Picker(args) => {
            pick_session(manager, args, cli.detach)?;
        }
        Commands::Preview {
            target,
            watch,
            interval,
        } => {
            preview_command(&manager, &target, watch, interval)?;
        }
        Commands::Last => {
            connection_out(&manager.last()?, false)?;
        }
        Commands::Worktree { command } => {
            worktree_command(manager, command, cli.detach)?;
        }
        Commands::Rename { target, to } => {
            let id = session_target(&manager, target.as_deref())?;
            let entry = manager.resolve_entry(&id)?;
            manager.rename(&entry, &to)?;
            out(&to)?;
        }
    }
    Ok(())
}
pub fn out(text: &str) -> Result<()> {
    writeln!(io::stdout().lock(), "{text}")?;
    Ok(())
}
fn connection_out(c: &crate::session::Connection, json: bool) -> Result<()> {
    if json {
        out(&serde_json::to_string_pretty(c)?)
    } else {
        out(&c.session_id)
    }
}
fn pick_session(manager: Manager, args: PickerArgs, detach: bool) -> Result<()> {
    let opts = args.sources.options();
    let options = picker_options(&manager.config, &args)?;
    let fetch = manager.clone();
    let preview = manager.clone();
    let remove = manager.clone();
    let rename = manager.clone();
    let chosen = picker::pick(
        move || fetch.list(&opts),
        move |e| preview.preview(e),
        move |e| remove.remove(e),
        move |e, name| rename.rename(e, name),
        options,
    )?;
    if let Some(e) = chosen {
        let c = manager.connect_entry(
            e,
            &ConnectOptions {
                detach,
                ..Default::default()
            },
        )?;
        connection_out(&c, false)?;
    }
    Ok(())
}
fn picker_options(cfg: &Config, args: &PickerArgs) -> Result<picker::PickerOptions> {
    picker_options_for(cfg, args, false)
}

fn picker_options_for(
    cfg: &Config,
    args: &PickerArgs,
    worktree: bool,
) -> Result<picker::PickerOptions> {
    let entries = cfg.entries()?;
    let aliases = entries
        .iter()
        .filter_map(|e| {
            e.alias.as_ref().map(|a| {
                (
                    crate::model::alias_key(a),
                    picker::Alias {
                        alias: a.clone(),
                        target: e.name.clone(),
                        auto_connect: e.alias_auto_connect,
                    },
                )
            })
        })
        .collect();
    let alias_icons = entries
        .iter()
        .filter(|e| e.alias.is_some())
        .filter_map(|e| e.icon.as_ref().map(|icon| (e.name.clone(), icon.clone())))
        .collect();
    let glyphs = cfg
        .icons
        .iter()
        .map(|(s, i)| (s.clone(), i.icon.clone()))
        .collect();
    let icon_width = cfg
        .session
        .iter()
        .filter_map(|session| session.icon.as_deref())
        .chain(
            cfg.wildcard
                .iter()
                .filter_map(|wildcard| wildcard.icon.as_deref()),
        )
        .chain(cfg.icons.values().map(|source| source.icon.as_str()))
        .map(|icon| {
            unicode_width::UnicodeWidthStr::width(
                strip_ansi_escapes::strip_str(icon).trim_end_matches(' '),
            )
        })
        .max()
        .unwrap_or(1)
        .max(1);
    Ok(picker::PickerOptions {
        show_icons: args.icons.unwrap_or(cfg.tui.show_icons),
        separator_aware: args.separator_aware.unwrap_or(cfg.separator_aware),
        prompt: args
            .prompt
            .clone()
            .unwrap_or_else(|| cfg.tui.prompt.clone()),
        placeholder: args.placeholder.clone().unwrap_or_else(|| {
            if cfg.tui.placeholder.is_empty() {
                if worktree {
                    "Filter worktrees..."
                } else {
                    "Filter sessions..."
                }
                .into()
            } else {
                cfg.tui.placeholder.clone()
            }
        }),
        pointer: cfg.tui.pointer.clone(),
        query: args.query.clone().unwrap_or_default(),
        aliases,
        alias_icons,
        alias_filter_prefix: cfg.tui.alias_filter_prefix.clone(),
        glyph_colors: cfg
            .icons
            .iter()
            .map(|(s, i)| (s.clone(), i.color.clone()))
            .collect(),
        alias_auto_connect_delay: crate::config::parse_alias_delay(
            &cfg.tui.alias_auto_connect_delay,
        )?,
        disable_alias_auto_connect: args.no_alias_auto,
        icon_width,
        glyphs,
        preview: !args.no_preview && args.preview.unwrap_or(cfg.tui.preview),
        preview_width: cfg.tui.preview_width,
        preview_min_width: cfg.tui.preview_min_width,
        preview_border: cfg.tui.preview_border.clone(),
        group_separator: cfg.tui.group_separator,
        worktree,
        ..Default::default()
    })
}
pub fn format_entry(
    entry: &Entry,
    format: Option<&str>,
    icons: bool,
    color: bool,
    exclude: &[String],
) -> Result<String> {
    configured_format_entry(entry, format, icons, color, exclude, None)
}
fn configured_format_entry(
    entry: &Entry,
    format: Option<&str>,
    icons: bool,
    color: bool,
    exclude: &[String],
    source: Option<&crate::config::SourceIcon>,
) -> Result<String> {
    let glyph = source
        .map(|s| s.icon.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| entry.source.glyph());
    let name = entry.display_name();
    let decorated = if icons
        && !exclude
            .iter()
            .any(|s| s.trim().eq_ignore_ascii_case(entry.source.as_str()))
    {
        let code = source
            .and_then(|s| ansi_color(&s.color.to_lowercase()))
            .unwrap_or_else(|| source_color(entry.source));
        if color {
            format!("\x1b[{code}m{glyph}\x1b[0m {name}")
        } else {
            format!("{glyph} {name}")
        }
    } else {
        name
    };
    let Some(format) = format else {
        return Ok(decorated);
    };
    let mut output = String::new();
    let mut rest = format;
    while let Some(start) = rest.find('{') {
        output.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            if after.starts_with("fg:") {
                bail!("unclosed color format token");
            }
            output.push_str(&rest[start..]);
            return Ok(output);
        };
        let key = &after[..end];
        let value = match key {
            "name" => decorated.clone(),
            "session" => entry.name.clone(),
            "source" => entry.source.as_str().into(),
            "path" => entry.path.display().to_string(),
            "/fg" => {
                if color {
                    "\x1b[39m".into()
                } else {
                    String::new()
                }
            }
            k if k.starts_with("fg:") => {
                let c = ansi_color(&k[3..])
                    .with_context(|| format!("unknown format color {}", &k[3..]))?;
                if color {
                    format!("\x1b[{c}m")
                } else {
                    String::new()
                }
            }
            _ => format!("{{{key}}}"),
        };
        output.push_str(&value);
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    if color && output.contains('\x1b') {
        output.push_str("\x1b[0m");
    }
    Ok(output)
}
fn source_color(source: Source) -> u8 {
    match source {
        Source::Agent => 35,
        Source::Rex => 34,
        Source::Config => 90,
        Source::Zoxide | Source::Directory => 36,
        Source::Worktree => 36,
    }
}
pub(crate) fn ansi_color(color: &str) -> Option<u8> {
    Some(match color {
        "black" => 30,
        "red" => 31,
        "green" => 32,
        "yellow" => 33,
        "blue" => 34,
        "magenta" => 35,
        "cyan" => 36,
        "white" => 37,
        "gray" | "grey" | "bright-black" => 90,
        "bright-red" => 91,
        "bright-green" => 92,
        "bright-yellow" => 93,
        "bright-blue" => 94,
        "bright-magenta" => 95,
        "bright-cyan" => 96,
        "bright-white" => 97,
        _ => return None,
    })
}
fn session_target(manager: &Manager, target: Option<&str>) -> Result<String> {
    if let Some(target) = target {
        let e = manager.resolve_entry(target)?;
        return e.session_id.context("target session is not running");
    }
    manager.rex.context()?["session_id"]
        .as_str()
        .map(str::to_owned)
        .context("not inside a Rex session; provide a target session")
}
fn worktree_command(manager: Manager, command: WorktreeCommand, detach: bool) -> Result<()> {
    let worktrees = Worktrees::new(&manager.config);
    match command {
        WorktreeCommand::Create {
            branch,
            base,
            repo,
            json,
        } => {
            let prepared = worktrees.prepare(&repo.selection()?, &branch, true, base.as_deref())?;
            if json {
                out(&serde_json::to_string_pretty(&prepared)?)?;
            } else {
                out(&prepared.path.display().to_string())?;
            }
        }
        WorktreeCommand::Connect {
            target,
            repo,
            name,
            json,
        } => {
            let selection = repo.selection()?;
            let prepared = if let Some(target) = target {
                worktrees.prepare(&selection, &target, false, None)?
            } else {
                let current = worktrees.current(&selection)?;
                worktrees.prepare(&selection, &current.path.to_string_lossy(), false, None)?
            };
            let c = manager.connect_path(
                &prepared.path,
                &ConnectOptions {
                    detach,
                    name,
                    ..Default::default()
                },
            )?;
            connection_out(&c, json)?;
        }
        WorktreeCommand::List { repo, json } => {
            let entries = worktrees.list(&repo.selection()?)?;
            if json {
                out(&serde_json::to_string_pretty(&entries)?)?;
            } else {
                for entry in entries {
                    out(&format!("{}  {}", entry.entry().name, entry.path.display()))?;
                }
            }
        }
        WorktreeCommand::Remove {
            target,
            repo,
            force,
            force_delete,
            no_delete_branch,
            json,
        } => {
            let result = worktrees.remove(
                &repo.selection()?,
                &target,
                RemoveOptions {
                    force,
                    force_delete,
                    no_delete_branch,
                },
            )?;
            if json {
                out(&serde_json::to_string_pretty(&result)?)?;
            }
        }
        WorktreeCommand::Picker {
            repo,
            refresh: _,
            icons,
            prompt,
            placeholder,
            query,
        } => {
            let opts = PickerArgs {
                icons,
                prompt,
                placeholder,
                query,
                ..Default::default()
            };
            let mut options = picker_options_for(&manager.config, &opts, true)?;
            options.auto_refresh = false;
            let fetch_cfg = manager.config.clone();
            let remove_cfg = manager.config.clone();
            let create_cfg = manager.config.clone();
            let selection = repo.selection()?;
            let fetch_selection = selection.clone();
            let remove_selection = selection.clone();
            let create_selection = selection.clone();
            let preview = manager.clone();
            let chosen = picker::pick_with_create(
                move || {
                    Ok(Worktrees::new(&fetch_cfg)
                        .list(&fetch_selection)?
                        .iter()
                        .map(|e| e.entry())
                        .collect())
                },
                move |e| preview.preview(e),
                move |e| {
                    Worktrees::new(&remove_cfg).remove(
                        &remove_selection,
                        &e.path.to_string_lossy(),
                        RemoveOptions::default(),
                    )?;
                    Ok(())
                },
                |_, _| bail!("worktrees cannot be renamed from this picker"),
                move |branch| {
                    let prepared = Worktrees::new(&create_cfg).prepare(
                        &create_selection,
                        branch,
                        true,
                        None,
                    )?;
                    Ok(Entry::new(branch, prepared.path, Source::Worktree))
                },
                options,
            )?;
            if let Some(outcome) = chosen {
                let path = match outcome {
                    picker::PickOutcome::Selected(entry) => {
                        worktrees
                            .prepare(&selection, &entry.path.to_string_lossy(), false, None)?
                            .path
                    }
                    picker::PickOutcome::Created(entry) => entry.path,
                };
                let c = manager.connect_path(
                    &path,
                    &ConnectOptions {
                        detach,
                        ..Default::default()
                    },
                )?;
                connection_out(&c, false)?;
            }
        }
    }
    Ok(())
}
// Match strconv.ParseBool, including its deliberately limited case variants.
fn parse_bool(value: &str) -> std::result::Result<bool, String> {
    match value {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Ok(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Ok(false),
        _ => Err(format!("invalid boolean value: {value:?}")),
    }
}
fn parse_duration(value: &str) -> std::result::Result<Duration, String> {
    let duration = humantime::parse_duration(value).map_err(|e| e.to_string())?;
    if duration.is_zero() {
        return Err("interval must be positive".into());
    }
    Ok(duration)
}
fn preview_command(manager: &Manager, target: &str, watch: bool, interval: Duration) -> Result<()> {
    let entry = manager.resolve_entry(target)?;
    let live = entry
        .session_id
        .as_deref()
        .is_some_and(|id| id.starts_with("session:"));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut signals = Vec::new();
    if watch && live {
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            signals.push(signal_hook::flag::register(signal, stop.clone())?);
        }
    }
    let result = (|| -> Result<()> {
        let mut last = String::new();
        let mut painted = false;
        loop {
            let text = manager.preview(&entry)?;
            if !text.is_empty() && (!painted || text != last) {
                let mut stdout = io::stdout().lock();
                if painted {
                    stdout.write_all(b"\x1b[H\x1b[2J")?;
                }
                stdout.write_all(text.as_bytes())?;
                stdout.flush()?;
                painted = true;
                last = text;
            }
            if !watch || !live || stop.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let start = std::time::Instant::now();
            while start.elapsed() < interval && !stop.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(
                    (interval - start.elapsed().min(interval)).min(Duration::from_millis(50)),
                );
            }
        }
        Ok(())
    })();
    for id in signals {
        signal_hook::low_level::unregister(id);
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_selection_preserves_typed_paths_and_rejects_removed_flags() {
        let root = tempfile::tempdir().unwrap();
        let Some(Commands::Worktree {
            command: WorktreeCommand::List { repo, .. },
        }) = Cli::try_parse_from([
            "rexue",
            "worktree",
            "list",
            "--path",
            root.path().to_str().unwrap(),
        ])
        .unwrap()
        .command
        else {
            panic!("expected worktree list")
        };
        assert_eq!(repo.selection().unwrap().path.as_deref(), Some(root.path()));
        for args in [
            vec!["list", "--repo=owner/repo"],
            vec!["connect", "feature", "--pr"],
            vec!["connect", "--browser"],
            vec!["browse"],
        ] {
            assert!(Cli::try_parse_from(["rexue", "worktree"].into_iter().chain(args)).is_err());
        }
    }

    #[test]
    fn worktree_picker_empty_strings_remain_explicit_source_overrides() {
        let mut cfg = Config::default();
        cfg.tui.prompt = "configured prompt".into();
        cfg.tui.placeholder = "configured placeholder".into();
        for value in ["", " ", "custom"] {
            let flags = [
                format!("--prompt={value}"),
                format!("--placeholder={value}"),
                format!("--query={value}"),
            ];
            let Some(Commands::Worktree {
                command:
                    WorktreeCommand::Picker {
                        prompt,
                        placeholder,
                        query,
                        ..
                    },
            }) = Cli::try_parse_from(
                ["rexue", "worktree", "picker"]
                    .into_iter()
                    .chain(flags.iter().map(String::as_str)),
            )
            .unwrap()
            .command
            else {
                panic!("expected worktree picker")
            };
            let options = picker_options(
                &cfg,
                &PickerArgs {
                    prompt,
                    placeholder,
                    query,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(options.prompt, value);
            assert_eq!(options.placeholder, value);
            assert_eq!(options.query, value);
        }
    }

    #[test]
    fn empty_worktree_selectors_use_effective_defaults() {
        for command in ["list", "picker", "connect"] {
            for flags in [vec![], vec!["--path="]] {
                let Some(Commands::Worktree { command }) =
                    Cli::try_parse_from(["rexue", "worktree", command].into_iter().chain(flags))
                        .unwrap()
                        .command
                else {
                    panic!("expected worktree")
                };
                let repo = match command {
                    WorktreeCommand::List { repo, .. }
                    | WorktreeCommand::Picker { repo, .. }
                    | WorktreeCommand::Connect { repo, .. } => repo,
                    _ => unreachable!(),
                };
                assert!(repo.selection().unwrap().path.is_none());
            }
        }
    }

    #[test]
    fn picker_and_format_empty_strings_remain_explicit_source_overrides() {
        let mut cfg = Config::default();
        cfg.tui.prompt = "configured prompt".into();
        cfg.tui.placeholder = "configured placeholder".into();
        for (value, expected_prompt, expected_placeholder) in [
            (None, "configured prompt", "configured placeholder"),
            (Some(""), "", ""),
            (Some(" "), " ", " "),
            (Some("custom"), "custom", "custom"),
        ] {
            let flags = value
                .map(|v| {
                    vec![
                        format!("--prompt={v}"),
                        format!("--placeholder={v}"),
                        format!("--query={v}"),
                    ]
                })
                .unwrap_or_default();
            let args = parsed_picker(&flags.iter().map(String::as_str).collect::<Vec<_>>());
            let options = picker_options(&cfg, &args).unwrap();
            assert_eq!(options.prompt, expected_prompt);
            assert_eq!(options.placeholder, expected_placeholder);
            assert_eq!(options.query, value.unwrap_or_default());
        }
        let entry = Entry::new("project", "/tmp/project", Source::Config);
        for (flags, expected, valid) in [
            (vec![], "project", true),
            (vec!["--format="], "", true),
            (vec!["--format= "], " ", true),
            (vec!["--format={session}"], "project", true),
            (vec!["--format=", "--json"], "", false),
            (vec!["--format=", "--json=false"], "", true),
        ] {
            let Some(Commands::List(args)) =
                Cli::try_parse_from(["rexue", "list"].into_iter().chain(flags.iter().copied()))
                    .unwrap()
                    .command
            else {
                panic!("expected list")
            };
            assert_eq!(args.validate().is_ok(), valid, "{flags:?}");
            assert_eq!(
                configured_format_entry(&entry, args.format.as_deref(), false, false, &[], None)
                    .unwrap(),
                expected
            );
        }
    }

    #[test]
    fn cli_tree_has_no_conflicting_flags() {
        Cli::command().debug_assert();
    }
    #[test]
    fn every_boolean_flag_accepts_values_and_defaults() {
        fn check(command: clap::Command, path: Vec<String>) -> usize {
            let mut count = 0;
            let mut base = path.clone();
            for positional in command
                .get_positionals()
                .filter(|arg| arg.is_required_set())
            {
                let value = positional
                    .get_value_parser()
                    .possible_values()
                    .and_then(|mut values| values.next().map(|value| value.get_name().to_owned()))
                    .unwrap_or_else(|| "1".into());
                base.push(value);
                assert!(positional.get_index().is_some());
            }
            for option in command
                .get_arguments()
                .filter(|arg| arg.is_required_set() && arg.get_index().is_none())
            {
                base.push(format!("--{}", option.get_long().unwrap()));
                base.push("value".into());
            }
            if !command.is_subcommand_required_set() {
                let defaults = Cli::command().try_get_matches_from(&base).unwrap();
                let leaf = path
                    .iter()
                    .skip(1)
                    .fold(&defaults, |m, name| m.subcommand_matches(name).unwrap());
                for arg in command.get_arguments().filter(|arg| {
                    arg.get_value_parser().type_id() == std::any::TypeId::of::<bool>()
                }) {
                    let id = arg.get_id().as_str();
                    let flag = arg.get_long().unwrap();
                    assert_ne!(leaf.get_one::<bool>(id), Some(&true), "{path:?} --{flag}");
                    for (value, expected) in [
                        (None, true),
                        (Some("true"), true),
                        (Some("True"), true),
                        (Some("TRUE"), true),
                        (Some("t"), true),
                        (Some("T"), true),
                        (Some("1"), true),
                        (Some("false"), false),
                        (Some("False"), false),
                        (Some("FALSE"), false),
                        (Some("f"), false),
                        (Some("F"), false),
                        (Some("0"), false),
                    ] {
                        let mut argv = base.clone();
                        argv.push(match value {
                            Some(value) => format!("--{flag}={value}"),
                            None => format!("--{flag}"),
                        });
                        let matches = Cli::command()
                            .try_get_matches_from(&argv)
                            .unwrap_or_else(|e| panic!("{argv:?}: {e}"));
                        let leaf = path
                            .iter()
                            .skip(1)
                            .fold(&matches, |m, name| m.subcommand_matches(name).unwrap());
                        assert_eq!(leaf.get_one::<bool>(id), Some(&expected), "{argv:?}");
                    }
                    let mut invalid = base.clone();
                    invalid.push(format!("--{flag}=yes"));
                    assert!(Cli::try_parse_from(invalid).is_err());
                    count += 1;
                }
            }
            for child in command
                .get_subcommands()
                .filter(|child| child.get_name() != "help")
            {
                let mut child_path = path.clone();
                child_path.push(child.get_name().into());
                count += check(child.clone(), child_path);
            }
            count
        }
        let mut command = Cli::command();
        command.build();
        assert!(check(command, vec!["rexue".into()]) >= 20);
    }

    fn parsed_picker(flags: &[&str]) -> PickerArgs {
        let cli = Cli::try_parse_from(["rexue", "picker"].into_iter().chain(flags.iter().copied()))
            .unwrap();
        let Some(Commands::Picker(args)) = cli.command else {
            panic!("expected picker")
        };
        args
    }

    #[test]
    fn omitted_flags_and_source_free_commands_keep_defaults() {
        let Some(Commands::List(list)) = Cli::try_parse_from(["rexue", "list"]).unwrap().command
        else {
            panic!("expected list");
        };
        assert!(!list.json && !list.icons && !list.no_color && !list.blacklisted);
        for sources in [list.sources, parsed_picker(&[]).sources] {
            assert!(!sources.rex && !sources.configs && !sources.zoxide);
            assert!(!sources.hide_attached && !sources.hide_duplicates && !sources.no_cache);
        }
        let args = parsed_picker(&[]);
        assert_eq!(args.icons, None);
        assert_eq!(args.preview, None);
        assert_eq!(args.separator_aware, None);
        assert!(!args.no_preview && !args.no_alias_auto);
        assert!(Cli::try_parse_from(["rexue"]).unwrap().command.is_none());
    }

    #[test]
    fn boolean_flags_do_not_consume_space_separated_values() {
        let Some(Commands::Connect(args)) =
            Cli::try_parse_from(["rexue", "connect", "--root", "false"])
                .unwrap()
                .command
        else {
            panic!("expected connect")
        };
        assert!(args.root);
        assert_eq!(args.target, ["false"]);
        assert!(Cli::try_parse_from(["rexue", "list", "--icons", "false"]).is_err());
        assert!(Cli::try_parse_from(["rexue", "picker", "--preview", "false"]).is_err());
    }

    #[test]
    fn picker_boolean_overrides_are_effective_and_negative_preview_wins() {
        for configured in [false, true] {
            let mut cfg = Config::default();
            cfg.tui.show_icons = configured;
            cfg.tui.preview = configured;
            cfg.separator_aware = configured;
            let defaults = picker_options(&cfg, &parsed_picker(&[])).unwrap();
            assert_eq!(defaults.show_icons, configured);
            assert_eq!(defaults.preview, configured);
            assert_eq!(defaults.separator_aware, configured);
            for (value, expected) in [("false", false), ("true", true)] {
                let flags = [
                    format!("--icons={value}"),
                    format!("--preview={value}"),
                    format!("--separator-aware={value}"),
                ];
                let options = picker_options(
                    &cfg,
                    &parsed_picker(&flags.iter().map(String::as_str).collect::<Vec<_>>()),
                )
                .unwrap();
                assert_eq!(options.show_icons, expected);
                assert_eq!(options.preview, expected);
                assert_eq!(options.separator_aware, expected);
            }
            let options = picker_options(&cfg, &parsed_picker(&["--no-preview=false"])).unwrap();
            assert_eq!(options.preview, configured);
            for flags in [
                ["--preview=true", "--no-preview=false"],
                ["--no-preview=false", "--preview=true"],
            ] {
                assert!(
                    picker_options(&cfg, &parsed_picker(&flags))
                        .unwrap()
                        .preview
                );
            }
            for flags in [
                ["--preview=true", "--no-preview=true"],
                ["--no-preview=true", "--preview=true"],
            ] {
                assert!(
                    !picker_options(&cfg, &parsed_picker(&flags))
                        .unwrap()
                        .preview
                );
            }
        }
    }

    #[test]
    fn worktree_picker_preserves_explicit_icon_override() {
        for (flag, expected) in [
            (None, None),
            (Some("--icons=false"), Some(false)),
            (Some("--icons"), Some(true)),
        ] {
            let cli = Cli::try_parse_from(["rexue", "worktree", "picker"].into_iter().chain(flag))
                .unwrap();
            let Some(Commands::Worktree {
                command: WorktreeCommand::Picker { icons, .. },
            }) = cli.command
            else {
                panic!("expected worktree picker");
            };
            assert_eq!(icons, expected);
            let mut cfg = Config::default();
            cfg.tui.show_icons = true;
            let options = picker_options(
                &cfg,
                &PickerArgs {
                    icons,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(options.show_icons, expected.unwrap_or(true));
        }
    }

    #[test]
    fn json_format_conflict_depends_on_value() {
        for (flag, valid) in [
            ("--json=false", true),
            ("--json=true", false),
            ("--json", false),
        ] {
            let Some(Commands::List(args)) =
                Cli::try_parse_from(["rexue", "list", flag, "--format", "{name}"])
                    .unwrap()
                    .command
            else {
                panic!("expected list")
            };
            assert_eq!(args.validate().is_ok(), valid);
        }
        let Some(Commands::List(args)) =
            Cli::try_parse_from(["rexue", "list", "--config", "--icons=false", "--json"])
                .unwrap()
                .command
        else {
            panic!("expected list")
        };
        assert!(args.sources.configs && args.json && !args.icons);
        assert!(args.validate().is_ok());
        assert!(
            Cli::try_parse_from([
                "rexue",
                "worktree",
                "connect",
                "feature/widgets",
                "--json=false"
            ])
            .is_ok()
        );
    }

    #[test]
    fn format_separates_raw_name_and_decorated_name() {
        let e = Entry::new("a b", "/tmp/a b", Source::Rex);
        let s = format_entry(
            &e,
            Some("{source}|{session}|{name}|{path}"),
            true,
            false,
            &[],
        )
        .unwrap();
        assert_eq!(s, "rex|a b| a b|/tmp/a b");
    }
    fn loaded_picker_config(text: &str) -> Config {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("picker.toml");
        std::fs::write(&path, text).unwrap();
        Config::load(Some(&path)).unwrap()
    }

    #[test]
    fn picker_icon_inventory_includes_unloaded_wildcards_and_ignores_only_ascii_trailing_spaces() {
        let cfg = loaded_picker_config(
            "[[session]]\nname='configured'\npath='/configured'\nalias='dev'\nicon='日  '\n[[wildcard]]\npattern='/never-loaded/*'\nicon='日本語   '\n[icons.rex]\nicon='R   '\n",
        );
        let options = picker_options(&cfg, &PickerArgs::default()).unwrap();
        assert_eq!(options.icon_width, 6);
        assert_eq!(options.glyphs["rex"], "R   ");
        assert_eq!(options.alias_icons["configured"], "日  ");
        let cfg = loaded_picker_config("[icons.rex]\nicon='x　'\n");
        assert_eq!(
            picker_options(&cfg, &PickerArgs::default())
                .unwrap()
                .icon_width,
            3
        );
        let cfg = loaded_picker_config(
            "[[session]]\nname='configured'\npath='/configured'\nicon='日   '\n",
        );
        assert_eq!(
            picker_options(&cfg, &PickerArgs::default())
                .unwrap()
                .icon_width,
            2
        );
    }

    #[test]
    fn worktree_placeholder_resolution_matches_loader_then_fallback_then_explicit_override() {
        let loaded = loaded_picker_config("");
        assert_eq!(loaded.tui.placeholder, "Filter Sessions...");
        assert_eq!(
            picker_options_for(&loaded, &PickerArgs::default(), true)
                .unwrap()
                .placeholder,
            "Filter Sessions..."
        );
        let configured = loaded_picker_config("[tui]\nplaceholder='Filter sessions...'\n");
        assert_eq!(
            picker_options_for(&configured, &PickerArgs::default(), true)
                .unwrap()
                .placeholder,
            "Filter sessions..."
        );
        let mut raw = Config::default();
        raw.tui.placeholder.clear();
        assert_eq!(
            picker_options_for(&raw, &PickerArgs::default(), true)
                .unwrap()
                .placeholder,
            "Filter worktrees..."
        );
        assert_eq!(
            picker_options(&raw, &PickerArgs::default())
                .unwrap()
                .placeholder,
            "Filter sessions..."
        );
        for placeholder in ["", "Filter sessions..."] {
            let options = picker_options_for(
                &loaded,
                &PickerArgs {
                    placeholder: Some(placeholder.into()),
                    ..Default::default()
                },
                true,
            )
            .unwrap();
            assert_eq!(options.placeholder, placeholder);
            assert!(options.worktree);
        }
        // The loader deliberately replaces an empty config value before picker resolution.
        let loaded_empty = loaded_picker_config("[tui]\nplaceholder=''\n");
        assert_eq!(
            picker_options_for(&loaded_empty, &PickerArgs::default(), true)
                .unwrap()
                .placeholder,
            "Filter Sessions..."
        );
    }

    #[test]
    fn bad_formats_are_errors() {
        let e = Entry::new("p", "/tmp", Source::Config);
        assert_eq!(
            format_entry(&e, Some("{unknown}"), false, false, &[]).unwrap(),
            "{unknown}"
        );
        assert!(format_entry(&e, Some("{fg:unknown}"), false, false, &[]).is_err());
    }
    #[test]
    fn config_file_does_not_shadow_source_filter() {
        let cli = Cli::try_parse_from(["rexue", "-C", "custom.toml", "list", "--config"]).unwrap();
        assert_eq!(cli.config.unwrap(), PathBuf::from("custom.toml"));
    }
}
