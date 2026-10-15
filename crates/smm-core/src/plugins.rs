//! Plugin (ESP/ESM/ESL) handling: read masters, build a load order, flag
//! missing masters and wrong ordering, write Plugins.txt.
use crate::conflicts::file_map;
use crate::error::{Error, IoCtx, Result};
use crate::instance::Instance;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Always loaded first by Skyrim SE/AE, never written to Plugins.txt.
pub const IMPLICIT: &[&str] = &[
    "Skyrim.esm",
    "Update.esm",
    "Dawnguard.esm",
    "HearthFires.esm",
    "Dragonborn.esm",
];

pub fn is_plugin_name(name: &str) -> bool {
    let l = name.to_lowercase();
    l.ends_with(".esp") || l.ends_with(".esm") || l.ends_with(".esl")
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    pub name: String,
    pub path: PathBuf,
    /// Mod that provides it, or `None` for files already in Data/ (base game, CC, manual).
    pub provider: Option<String>,
    pub masters: Vec<String>,
    pub is_master: bool,
    pub is_light: bool,
    pub parse_error: Option<String>,
}

pub fn read_plugin(path: &Path) -> Result<(Vec<String>, bool, bool)> {
    let mut p = esplugin::Plugin::new(esplugin::GameId::SkyrimSE, path);
    p.parse_file(esplugin::ParseOptions::header_only())
        .map_err(|e| Error::Plugin {
            path: path.into(),
            msg: e.to_string(),
        })?;
    let masters = p.masters().map_err(|e| Error::Plugin {
        path: path.into(),
        msg: e.to_string(),
    })?;
    Ok((masters, p.is_master_file(), p.is_light_plugin()))
}

fn info(name: String, path: PathBuf, provider: Option<String>) -> PluginInfo {
    match read_plugin(&path) {
        Ok((masters, is_master, is_light)) => PluginInfo {
            name,
            path,
            provider,
            masters,
            is_master,
            is_light,
            parse_error: None,
        },
        Err(e) => {
            let l = name.to_lowercase();
            PluginInfo {
                is_master: l.ends_with(".esm") || l.ends_with(".esl"),
                is_light: l.ends_with(".esl"),
                name,
                path,
                provider,
                masters: vec![],
                parse_error: Some(e.to_string()),
            }
        }
    }
}

/// Plugins present in `data_dir` (excluding ones we deployed) plus plugins from enabled mods.
pub fn collect(inst: &Instance, data_dir: Option<&Path>) -> Result<Vec<PluginInfo>> {
    let ours: HashSet<PathBuf> = crate::deploy::load_manifest(inst)?
        .map(|m| {
            m.entries
                .into_iter()
                .filter(|e| e.backup.is_none())
                .map(|e| e.target)
                .collect()
        })
        .unwrap_or_default();
    let mut out: Vec<PluginInfo> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    if let Some(d) = data_dir {
        if let Ok(rd) = std::fs::read_dir(d) {
            let mut v: Vec<_> = rd
                .flatten()
                .filter(|e| e.path().is_file() && !ours.contains(&e.path()))
                .collect();
            v.sort_by_key(|e| e.file_name());
            for e in v {
                let n = e.file_name().to_string_lossy().to_string();
                if is_plugin_name(&n) {
                    seen.insert(n.to_lowercase(), out.len());
                    out.push(info(n, e.path(), None));
                }
            }
        }
    }
    let fm = file_map(inst)?;
    // Iterate mods in priority order so the append order follows the mod list.
    for m in inst.enabled_mods() {
        for (key, providers) in &fm {
            let w = providers.last().unwrap();
            if w.mod_name != m.name || key.contains('/') || !is_plugin_name(key) {
                continue;
            }
            let name = w
                .rel_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string();
            let pi = info(
                name,
                inst.mod_dir(&w.mod_name).join(&w.rel_path),
                Some(w.mod_name.clone()),
            );
            match seen.get(key) {
                Some(&i) => out[i] = pi,
                None => {
                    seen.insert(key.clone(), out.len());
                    out.push(pi);
                }
            }
        }
    }
    Ok(out)
}

/// Build a load order: implicit masters, then the saved order, then new plugins
/// (masters before non-masters, otherwise in mod priority order).
pub fn load_order(inst: &Instance, plugins: &[PluginInfo]) -> Vec<String> {
    let by_lower: HashMap<String, &PluginInfo> =
        plugins.iter().map(|p| (p.name.to_lowercase(), p)).collect();
    let mut order: Vec<String> = Vec::new();
    let mut used = HashSet::new();
    let mut push = |n: &str, order: &mut Vec<String>| {
        if let Some(p) = by_lower.get(&n.to_lowercase()) {
            if used.insert(n.to_lowercase()) {
                order.push(p.name.clone());
            }
        }
    };
    for n in IMPLICIT {
        push(n, &mut order);
    }
    for n in &inst.state.plugin_order {
        push(n, &mut order);
    }
    for p in plugins.iter().filter(|p| p.is_master) {
        push(&p.name, &mut order);
    }
    for p in plugins.iter().filter(|p| !p.is_master) {
        push(&p.name, &mut order);
    }
    order
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Issue {
    MissingMaster {
        plugin: String,
        master: String,
    },
    InactiveMaster {
        plugin: String,
        master: String,
    },
    MasterAfterDependent {
        plugin: String,
        master: String,
    },
    NonMasterBeforeMaster {
        plugin: String,
        master_flagged: String,
    },
    ParseError {
        plugin: String,
        error: String,
    },
}

pub fn check(order: &[String], active: &HashSet<String>, plugins: &[PluginInfo]) -> Vec<Issue> {
    let by_lower: HashMap<String, &PluginInfo> =
        plugins.iter().map(|p| (p.name.to_lowercase(), p)).collect();
    let pos: HashMap<String, usize> = order
        .iter()
        .enumerate()
        .map(|(i, n)| (n.to_lowercase(), i))
        .collect();
    let is_active = |n: &str| active.contains(&n.to_lowercase());
    let mut issues = Vec::new();
    for (i, name) in order.iter().enumerate() {
        let Some(p) = by_lower.get(&name.to_lowercase()) else {
            continue;
        };
        if !is_active(name) {
            continue;
        }
        if let Some(e) = &p.parse_error {
            issues.push(Issue::ParseError {
                plugin: name.clone(),
                error: e.clone(),
            });
        }
        for m in &p.masters {
            match pos.get(&m.to_lowercase()) {
                None => issues.push(Issue::MissingMaster {
                    plugin: name.clone(),
                    master: m.clone(),
                }),
                Some(_) if !is_active(m) => issues.push(Issue::InactiveMaster {
                    plugin: name.clone(),
                    master: m.clone(),
                }),
                Some(&mi) if mi > i => issues.push(Issue::MasterAfterDependent {
                    plugin: name.clone(),
                    master: m.clone(),
                }),
                _ => {}
            }
        }
        if !p.is_master {
            if let Some(later) = order[i + 1..].iter().find(|n| {
                by_lower.get(&n.to_lowercase()).is_some_and(|q| q.is_master) && is_active(n)
            }) {
                issues.push(Issue::NonMasterBeforeMaster {
                    plugin: name.clone(),
                    master_flagged: later.clone(),
                });
            }
        }
    }
    issues
}

/// Reorder so that master-flagged plugins come first and every plugin loads after its
/// masters, keeping the existing order wherever possible (stable).
pub fn fix_order(order: &[String], plugins: &[PluginInfo]) -> Vec<String> {
    let by_lower: HashMap<String, &PluginInfo> =
        plugins.iter().map(|p| (p.name.to_lowercase(), p)).collect();
    let present: HashSet<String> = order.iter().map(|n| n.to_lowercase()).collect();
    let (masters, rest): (Vec<_>, Vec<_>) = order
        .iter()
        .cloned()
        .partition(|n| by_lower.get(&n.to_lowercase()).is_some_and(|p| p.is_master));
    let mut out: Vec<String> = Vec::new();
    let mut placed: HashSet<String> = HashSet::new();
    fn place(
        n: &str,
        by: &HashMap<String, &PluginInfo>,
        present: &HashSet<String>,
        placed: &mut HashSet<String>,
        out: &mut Vec<String>,
        stack: &mut HashSet<String>,
    ) {
        let l = n.to_lowercase();
        if placed.contains(&l) || !stack.insert(l.clone()) {
            return;
        }
        if let Some(p) = by.get(&l) {
            for m in &p.masters {
                if present.contains(&m.to_lowercase()) {
                    let mname = by
                        .get(&m.to_lowercase())
                        .map(|q| q.name.clone())
                        .unwrap_or(m.clone());
                    place(&mname, by, present, placed, out, stack);
                }
            }
        }
        placed.insert(l);
        out.push(n.to_string());
    }
    for group in [masters, rest] {
        for n in group {
            place(
                &n,
                &by_lower,
                &present,
                &mut placed,
                &mut out,
                &mut HashSet::new(),
            );
        }
    }
    out
}

/// Render Plugins.txt content (`*` marks active plugins; implicit masters omitted).
pub fn render_plugins_txt(order: &[String], active: &HashSet<String>) -> String {
    let mut s = String::from("# This file is used by Skyrim to keep track of your downloaded content.\n# Written by skyrim-mod-manager.\n");
    for n in order {
        if IMPLICIT.iter().any(|i| i.eq_ignore_ascii_case(n)) {
            continue;
        }
        if active.contains(&n.to_lowercase()) {
            s.push('*');
        }
        s.push_str(n);
        s.push_str("\r\n");
    }
    s
}

/// Write Plugins.txt atomically. The first time, an existing file is kept as `Plugins.txt.smm-bak`.
pub fn write_plugins_txt(path: &Path, order: &[String], active: &HashSet<String>) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).ctx(d)?;
    }
    let bak = path.with_extension("txt.smm-bak");
    if path.exists() && !bak.exists() {
        std::fs::copy(path, &bak).ctx(&bak)?;
    }
    let tmp = path.with_extension("txt.smm-tmp");
    std::fs::write(&tmp, render_plugins_txt(order, active)).ctx(&tmp)?;
    std::fs::rename(&tmp, path).ctx(path)
}

/// Parse an existing Plugins.txt into (order, active set).
pub fn parse_plugins_txt(text: &str) -> (Vec<String>, HashSet<String>) {
    let mut order = Vec::new();
    let mut active = HashSet::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let (on, name) = match l.strip_prefix('*') {
            Some(n) => (true, n),
            None => (false, l),
        };
        if on {
            active.insert(name.to_lowercase());
        }
        order.push(name.to_string());
    }
    (order, active)
}

/// Active set: every present plugin except ones the user turned off. Implicit masters are always active.
pub fn active_set(inst: &Instance, order: &[String]) -> HashSet<String> {
    let off: HashSet<String> = inst
        .state
        .inactive_plugins
        .iter()
        .map(|n| n.to_lowercase())
        .collect();
    order
        .iter()
        .map(|n| n.to_lowercase())
        .filter(|n| !off.contains(n) || IMPLICIT.iter().any(|i| i.eq_ignore_ascii_case(n)))
        .collect()
}
