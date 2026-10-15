//! Deploy staged mods into a game `Data/` folder with hardlinks, and undo it.
//!
//! Safety rules:
//! * A manifest listing every file we will touch is written **before** the first change.
//! * Game files that would be overwritten are moved into `<instance>/backup/` first and
//!   restored on undeploy.
//! * Undeploy only removes a file if it is still the exact file we put there
//!   (same inode for hardlinks, same size+mtime for copies). Anything the user or
//!   the game changed is left alone and reported.
//! * Only directories we created are removed, and only when empty.
use crate::conflicts::file_map;
use crate::error::{Error, IoCtx, Result};
use crate::instance::Instance;
use crate::paths::resolve_ci;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanKind {
    /// New file in Data/.
    Create,
    /// Data/ already has this file (vanilla or another tool); it will be backed up.
    ReplaceExisting,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanEntry {
    pub key: String,
    pub mod_name: String,
    pub source: PathBuf,
    pub target: PathBuf,
    pub kind: PlanKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub data_dir: PathBuf,
    pub entries: Vec<PlanEntry>,
    pub dirs_to_create: Vec<PathBuf>,
}

/// Difference between what's deployed now (manifest) and what a new deploy would produce.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Diff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Same path, different winning mod.
    pub changed: Vec<(String, String, String)>,
    pub replaces_existing: Vec<String>,
    pub unchanged: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    Hardlink,
    Copy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub key: String,
    pub mod_name: String,
    pub source: PathBuf,
    pub target: PathBuf,
    pub method: Option<Method>,
    pub ino: u64,
    pub size: u64,
    pub mtime: i64,
    pub backup: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub data_dir: PathBuf,
    pub complete: bool,
    pub created_dirs: Vec<PathBuf>,
    pub entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UndeployReport {
    pub removed: usize,
    pub restored: usize,
    /// Files left in place because they no longer match what we deployed.
    pub skipped_modified: Vec<PathBuf>,
}

pub fn plan(inst: &Instance, data_dir: &Path) -> Result<Plan> {
    let fm = file_map(inst)?;
    let deployed = load_manifest(inst)?;
    let ours: std::collections::HashSet<PathBuf> = deployed
        .iter()
        .flat_map(|m| {
            m.entries
                .iter()
                .filter(|e| e.backup.is_none())
                .map(|e| e.target.clone())
        })
        .collect();
    let mut entries = Vec::new();
    let mut dirs = std::collections::BTreeSet::new();
    for (key, providers) in fm {
        let w = providers.last().unwrap();
        let target = resolve_ci(data_dir, &w.rel_path);
        let exists = target.symlink_metadata().is_ok() && !ours.contains(&target);
        let mut d = target.parent();
        while let Some(p) = d {
            if p == data_dir || !p.starts_with(data_dir) || p.exists() {
                break;
            }
            dirs.insert(p.to_path_buf());
            d = p.parent();
        }
        entries.push(PlanEntry {
            key,
            mod_name: w.mod_name.clone(),
            source: inst.mod_dir(&w.mod_name).join(&w.rel_path),
            target,
            kind: if exists {
                PlanKind::ReplaceExisting
            } else {
                PlanKind::Create
            },
        });
    }
    Ok(Plan {
        data_dir: data_dir.to_path_buf(),
        entries,
        dirs_to_create: dirs.into_iter().collect(),
    })
}

/// Dry-run: compare a fresh plan with the current manifest (empty if nothing deployed).
pub fn diff(inst: &Instance, data_dir: &Path) -> Result<Diff> {
    let p = plan(inst, data_dir)?;
    let cur: BTreeMap<String, String> = load_manifest(inst)?
        .map(|m| m.entries.into_iter().map(|e| (e.key, e.mod_name)).collect())
        .unwrap_or_default();
    let mut d = Diff::default();
    let mut seen = std::collections::HashSet::new();
    for e in &p.entries {
        seen.insert(e.key.clone());
        if e.kind == PlanKind::ReplaceExisting {
            d.replaces_existing.push(e.key.clone());
        }
        match cur.get(&e.key) {
            None => d.added.push(e.key.clone()),
            Some(old) if *old != e.mod_name => {
                d.changed
                    .push((e.key.clone(), old.clone(), e.mod_name.clone()))
            }
            Some(_) => d.unchanged += 1,
        }
    }
    d.removed = cur.keys().filter(|k| !seen.contains(*k)).cloned().collect();
    Ok(d)
}

pub fn load_manifest(inst: &Instance) -> Result<Option<Manifest>> {
    let p = inst.manifest_path();
    if !p.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(
        &std::fs::read_to_string(&p).ctx(&p)?,
    )?))
}

fn write_manifest(inst: &Instance, m: &Manifest) -> Result<()> {
    let p = inst.manifest_path();
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(m)?).ctx(&tmp)?;
    std::fs::rename(&tmp, &p).ctx(&p)
}

/// Deploy enabled mods into `data_dir`. If something is already deployed it is
/// cleanly undeployed first.
pub fn deploy(inst: &Instance, data_dir: &Path) -> Result<Manifest> {
    if !data_dir.is_dir() {
        return Err(Error::Other(format!(
            "data dir does not exist: {}",
            data_dir.display()
        )));
    }
    if load_manifest(inst)?.is_some() {
        let r = undeploy(inst)?;
        if !r.skipped_modified.is_empty() {
            return Err(Error::Other(format!(
                "undeploy left {} modified file(s) in place; resolve them before redeploying",
                r.skipped_modified.len()
            )));
        }
    }
    let p = plan(inst, data_dir)?;
    let backup_root = inst.backup_dir();
    let mut m = Manifest {
        version: 1,
        data_dir: data_dir.to_path_buf(),
        complete: false,
        created_dirs: p.dirs_to_create.clone(),
        entries: p
            .entries
            .iter()
            .map(|e| ManifestEntry {
                key: e.key.clone(),
                mod_name: e.mod_name.clone(),
                source: e.source.clone(),
                target: e.target.clone(),
                method: None,
                ino: 0,
                size: 0,
                mtime: 0,
                backup: (e.kind == PlanKind::ReplaceExisting).then(|| {
                    backup_root.join(e.target.strip_prefix(data_dir).unwrap_or(Path::new(&e.key)))
                }),
            })
            .collect(),
    };
    // Write the manifest before touching Data/, so a crash can always be undone.
    write_manifest(inst, &m)?;
    for d in &p.dirs_to_create {
        std::fs::create_dir_all(d).ctx(d)?;
    }
    for e in m.entries.iter_mut() {
        if let Some(b) = &e.backup {
            if let Some(bp) = b.parent() {
                std::fs::create_dir_all(bp).ctx(bp)?;
            }
            move_file(&e.target, b)?;
        }
        let method = match std::fs::hard_link(&e.source, &e.target) {
            Ok(()) => Method::Hardlink,
            Err(err) if err.raw_os_error() == Some(18) => {
                // EXDEV: staging and Data/ are on different filesystems.
                std::fs::copy(&e.source, &e.target).ctx(&e.target)?;
                Method::Copy
            }
            Err(err) => {
                return Err(Error::Io {
                    path: e.target.clone(),
                    source: err,
                })
            }
        };
        let md = e.target.metadata().ctx(&e.target)?;
        e.method = Some(method);
        e.ino = md.ino();
        e.size = md.len();
        e.mtime = md.mtime();
    }
    m.complete = true;
    write_manifest(inst, &m)?;
    Ok(m)
}

fn move_file(from: &Path, to: &Path) -> Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(18) => {
            std::fs::copy(from, to).ctx(to)?;
            std::fs::remove_file(from).ctx(from)
        }
        Err(e) => Err(Error::Io {
            path: from.into(),
            source: e,
        }),
    }
}

fn is_ours(e: &ManifestEntry) -> bool {
    let Ok(md) = e.target.symlink_metadata() else {
        return false;
    };
    if !md.is_file() {
        return false;
    }
    match e.method {
        Some(Method::Hardlink) => md.ino() == e.ino,
        Some(Method::Copy) => md.len() == e.size && md.mtime() == e.mtime,
        // Crash between manifest write and link: it's ours only if it's the source inode.
        None => e
            .source
            .metadata()
            .map(|s| s.ino() == md.ino())
            .unwrap_or(false),
    }
}

/// Remove everything the manifest says we deployed and restore backups.
pub fn undeploy(inst: &Instance) -> Result<UndeployReport> {
    let Some(m) = load_manifest(inst)? else {
        return Ok(UndeployReport::default());
    };
    let mut r = UndeployReport::default();
    for e in m.entries.iter().rev() {
        let present = e.target.symlink_metadata().is_ok();
        if present {
            if is_ours(e) {
                std::fs::remove_file(&e.target).ctx(&e.target)?;
                r.removed += 1;
            } else {
                r.skipped_modified.push(e.target.clone());
                continue;
            }
        }
        if let Some(b) = &e.backup {
            if b.exists() {
                move_file(b, &e.target)?;
                r.restored += 1;
            }
        }
    }
    let mut dirs = m.created_dirs.clone();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for d in dirs {
        let _ = std::fs::remove_dir(&d); // only succeeds when empty
    }
    if r.skipped_modified.is_empty() {
        let p = inst.manifest_path();
        std::fs::remove_file(&p).ctx(&p)?;
        let _ = remove_empty_dirs(&inst.backup_dir());
    } else {
        // Keep a manifest of what's left so a later undeploy can retry.
        let left: Vec<_> = m
            .entries
            .into_iter()
            .filter(|e| r.skipped_modified.contains(&e.target))
            .collect();
        write_manifest(
            inst,
            &Manifest {
                entries: left,
                complete: true,
                ..m
            },
        )?;
    }
    Ok(r)
}

fn remove_empty_dirs(d: &Path) -> std::io::Result<()> {
    if !d.is_dir() {
        return Ok(());
    }
    for e in std::fs::read_dir(d)?.flatten() {
        if e.path().is_dir() {
            remove_empty_dirs(&e.path())?;
        }
    }
    std::fs::remove_dir(d).or(Ok(()))
}
