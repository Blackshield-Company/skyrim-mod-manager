use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use smm_core::{conflicts, deploy, detect, instance::Instance, plugins};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "smm",
    version,
    about = "Native Linux mod manager for Skyrim SE/AE under Proton"
)]
struct Cli {
    /// Instance (staging) directory. Default: ~/.local/share/skyrim-mod-manager/default
    #[arg(long, global = true, env = "SMM_INSTANCE")]
    instance: Option<PathBuf>,
    /// Target Data/ directory. Default: the detected game's Data/.
    #[arg(long, global = true)]
    data: Option<PathBuf>,
    /// Print JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Detect Skyrim SE installs and their Proton/umu prefix (read-only).
    Detect,
    /// Install an archive (7z/zip/rar) or folder into staging.
    Install {
        source: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },
    /// List installed mods in priority order (last wins).
    List,
    Enable {
        name: String,
    },
    Disable {
        name: String,
    },
    /// Move a mod to a priority index (0 = lowest).
    Priority {
        name: String,
        index: usize,
    },
    /// Remove a mod from staging (only while nothing is deployed).
    Uninstall {
        name: String,
    },
    /// Show which mod wins each overwritten file.
    Conflicts,
    /// Dry-run: show what a deploy would change.
    Plan,
    /// Hardlink enabled mods into Data/ (undeploys an older deploy first).
    Deploy {
        /// Required unless --data points at a test folder: confirms you ran `plan`.
        #[arg(long)]
        yes: bool,
    },
    /// Remove deployed files and restore any game files we displaced.
    Undeploy,
    /// Show plugin load order and problems; optionally fix order and write Plugins.txt.
    Plugins {
        /// Reorder so masters load before dependents.
        #[arg(long)]
        fix: bool,
        /// Write Plugins.txt (default path: detected prefix).
        #[arg(long)]
        write: bool,
        /// Explicit Plugins.txt path.
        #[arg(long)]
        plugins_txt: Option<PathBuf>,
    },
}

fn default_instance() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| detect::home_dir().join(".local/share"));
    base.join("skyrim-mod-manager/default")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let installs = || detect::detect(&detect::home_dir());
    let data_dir = || -> Result<PathBuf> {
        if let Some(d) = &cli.data {
            return Ok(d.clone());
        }
        installs()
            .into_iter()
            .next()
            .map(|i| i.data_dir)
            .context("no Skyrim SE install detected; pass --data")
    };
    let open = || {
        Instance::open(cli.instance.clone().unwrap_or_else(default_instance))
            .context("opening instance")
    };
    let out = |v: serde_json::Value, text: String| {
        if cli.json {
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
        } else if !text.is_empty() {
            println!("{text}");
        }
    };

    match &cli.cmd {
        Cmd::Detect => {
            let f = installs();
            let mut t = String::new();
            if f.is_empty() {
                t.push_str("No Skyrim SE install found.");
            }
            for i in &f {
                t.push_str(&format!(
                    "game:   {}\ndata:   {}\n",
                    i.game_dir.display(),
                    i.data_dir.display()
                ));
                match &i.prefix {
                    Some(p) => t.push_str(&format!(
                        "prefix: {} ({:?})\nplugins.txt: {}\n",
                        p.path.display(),
                        p.kind,
                        p.plugins_txt().display()
                    )),
                    None => t.push_str("prefix: not found\n"),
                }
            }
            out(serde_json::to_value(&f)?, t);
        }
        Cmd::Install { source, name } => {
            let mut inst = open()?;
            let m = inst.install(source, name.as_deref())?.clone();
            let warn = if m.has_fomod {
                "\n  note: archive has a FOMOD installer; files were installed as-is (FOMOD not run)"
            } else {
                ""
            };
            out(
                serde_json::to_value(&m)?,
                format!("installed {}{warn}", m.name),
            );
        }
        Cmd::List => {
            let inst = open()?;
            let t = inst
                .state
                .mods
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    format!(
                        "{i:>3} [{}] {}{}",
                        if m.enabled { "x" } else { " " },
                        m.name,
                        if m.has_fomod { "  (fomod)" } else { "" }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            out(serde_json::to_value(&inst.state.mods)?, t);
        }
        Cmd::Enable { name } => open()?.set_enabled(name, true)?,
        Cmd::Disable { name } => open()?.set_enabled(name, false)?,
        Cmd::Priority { name, index } => open()?.set_priority(name, *index)?,
        Cmd::Uninstall { name } => open()?.uninstall(name)?,
        Cmd::Conflicts => {
            let c = conflicts::conflicts(&open()?)?;
            let t = if c.is_empty() {
                "No file conflicts.".into()
            } else {
                c.iter()
                    .map(|c| {
                        format!(
                            "{}\n    wins: {}   overwrites: {}",
                            c.path,
                            c.winner,
                            c.losers.join(", ")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            out(serde_json::to_value(&c)?, t);
        }
        Cmd::Plan => {
            let d = deploy::diff(&open()?, &data_dir()?)?;
            let mut t = String::new();
            for k in &d.added {
                t.push_str(&format!("+ {k}\n"));
            }
            for k in &d.removed {
                t.push_str(&format!("- {k}\n"));
            }
            for (k, a, b) in &d.changed {
                t.push_str(&format!("~ {k}  ({a} -> {b})\n"));
            }
            for k in &d.replaces_existing {
                t.push_str(&format!(
                    "! {k} replaces an existing game file (will be backed up)\n"
                ));
            }
            t.push_str(&format!(
                "{} added, {} removed, {} changed, {} unchanged",
                d.added.len(),
                d.removed.len(),
                d.changed.len(),
                d.unchanged
            ));
            out(serde_json::to_value(&d)?, t);
        }
        Cmd::Deploy { yes } => {
            if !yes {
                bail!("run `smm plan` first, then `smm deploy --yes`");
            }
            let m = deploy::deploy(&open()?, &data_dir()?)?;
            out(
                serde_json::json!({"deployed": m.entries.len()}),
                format!(
                    "deployed {} files into {}",
                    m.entries.len(),
                    m.data_dir.display()
                ),
            );
        }
        Cmd::Undeploy => {
            let r = deploy::undeploy(&open()?)?;
            let mut t = format!("removed {}, restored {}", r.removed, r.restored);
            for p in &r.skipped_modified {
                t.push_str(&format!(
                    "\n  left in place (modified since deploy): {}",
                    p.display()
                ));
            }
            out(serde_json::to_value(&r)?, t);
        }
        Cmd::Plugins {
            fix,
            write,
            plugins_txt,
        } => {
            let mut inst = open()?;
            let data = data_dir().ok();
            let ps = plugins::collect(&inst, data.as_deref())?;
            let mut order = plugins::load_order(&inst, &ps);
            if *fix {
                order = plugins::fix_order(&order, &ps);
                inst.state.plugin_order = order.clone();
                inst.save()?;
            }
            let active = plugins::active_set(&inst, &order);
            let issues = plugins::check(&order, &active, &ps);
            if *write || plugins_txt.is_some() {
                let path = match plugins_txt {
                    Some(p) => p.clone(),
                    None => installs()
                        .into_iter()
                        .find_map(|i| i.prefix)
                        .map(|p| p.plugins_txt())
                        .context("no prefix detected; pass --plugins-txt")?,
                };
                plugins::write_plugins_txt(&path, &order, &active)?;
                eprintln!("wrote {}", path.display());
            }
            let mut t = String::new();
            for (i, n) in order.iter().enumerate() {
                t.push_str(&format!(
                    "{i:>3} {} {n}\n",
                    if active.contains(&n.to_lowercase()) {
                        "*"
                    } else {
                        " "
                    }
                ));
            }
            if issues.is_empty() {
                t.push_str("No plugin problems.");
            }
            for is in &issues {
                t.push_str(&format!("! {}\n", serde_json::to_string(is)?));
            }
            out(serde_json::json!({"order": order, "issues": issues}), t);
        }
    }
    Ok(())
}
