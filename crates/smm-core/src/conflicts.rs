//! File-conflict table: for every path provided by more than one enabled mod,
//! which mod wins (highest priority = latest in the mod list).
use crate::error::Result;
use crate::instance::Instance;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Provider {
    pub mod_name: String,
    /// Path inside the mod folder, original casing.
    pub rel_path: PathBuf,
}

/// Every Data-relative path (lowercased key) → providers in priority order (last wins).
pub type FileMap = BTreeMap<String, Vec<Provider>>;

pub fn file_map(inst: &Instance) -> Result<FileMap> {
    let mut map: FileMap = BTreeMap::new();
    for m in inst.enabled_mods() {
        for rel in inst.mod_files(&m.name)? {
            map.entry(crate::paths::key(&rel))
                .or_default()
                .push(Provider {
                    mod_name: m.name.clone(),
                    rel_path: rel,
                });
        }
    }
    Ok(map)
}

#[derive(Debug, Clone, Serialize)]
pub struct Conflict {
    pub path: String,
    pub winner: String,
    /// Mods whose copy is overwritten, lowest priority first.
    pub losers: Vec<String>,
}

pub fn conflicts(inst: &Instance) -> Result<Vec<Conflict>> {
    Ok(file_map(inst)?
        .into_iter()
        .filter(|(_, p)| p.len() > 1)
        .map(|(path, p)| {
            let winner = p.last().unwrap().mod_name.clone();
            let losers = p[..p.len() - 1]
                .iter()
                .map(|x| x.mod_name.clone())
                .collect();
            Conflict {
                path,
                winner,
                losers,
            }
        })
        .collect())
}

/// Per-mod summary: how many files it wins and loses.
#[derive(Debug, Clone, Serialize)]
pub struct ModConflictSummary {
    pub mod_name: String,
    pub wins: usize,
    pub losses: usize,
}

pub fn summary(inst: &Instance) -> Result<Vec<ModConflictSummary>> {
    let c = conflicts(inst)?;
    Ok(inst
        .enabled_mods()
        .map(|m| ModConflictSummary {
            mod_name: m.name.clone(),
            wins: c.iter().filter(|x| x.winner == m.name).count(),
            losses: c.iter().filter(|x| x.losers.contains(&m.name)).count(),
        })
        .collect())
}
