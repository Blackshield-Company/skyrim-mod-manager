//! A managed instance: a staging folder holding one directory per installed mod,
//! plus `state.json` (mod order / enabled flags / plugin order) and the deploy manifest.
//!
//! Layout:
//! ```text
//! <root>/state.json
//! <root>/mods/<name>/...      files laid out as they go into Data/
//! <root>/deploy-manifest.json  written by deploy, consumed by undeploy
//! <root>/backup/...            originals displaced by a deploy
//! ```
use crate::error::{Error, IoCtx, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModEntry {
    pub name: String,
    pub enabled: bool,
    /// True when the archive carried a FOMOD installer that the MVP did not run.
    #[serde(default)]
    pub has_fomod: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    /// Install order = priority; later entries win conflicts.
    pub mods: Vec<ModEntry>,
    /// User plugin load order (file names). Unlisted plugins are appended.
    #[serde(default)]
    pub plugin_order: Vec<String>,
    /// Plugins the user explicitly deactivated.
    #[serde(default)]
    pub inactive_plugins: Vec<String>,
}

pub struct Instance {
    pub root: PathBuf,
    pub state: State,
}

const DATA_DIRS: &[&str] = &[
    "meshes",
    "textures",
    "scripts",
    "interface",
    "sound",
    "music",
    "skse",
    "seq",
    "strings",
    "shadersfx",
    "lodsettings",
    "grass",
    "video",
    "materials",
    "calientetools",
    "netscriptframework",
];
const DATA_EXTS: &[&str] = &["esp", "esm", "esl", "bsa", "ba2"];

impl Instance {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(root.join("mods")).ctx(&root)?;
        let sp = root.join("state.json");
        let state = if sp.exists() {
            serde_json::from_str(&std::fs::read_to_string(&sp).ctx(&sp)?)?
        } else {
            State::default()
        };
        Ok(Self { root, state })
    }

    pub fn save(&self) -> Result<()> {
        let sp = self.root.join("state.json");
        let tmp = self.root.join("state.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.state)?).ctx(&tmp)?;
        std::fs::rename(&tmp, &sp).ctx(&sp)
    }

    pub fn mods_dir(&self) -> PathBuf {
        self.root.join("mods")
    }

    pub fn mod_dir(&self, name: &str) -> PathBuf {
        self.mods_dir().join(name)
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.root.join("deploy-manifest.json")
    }

    pub fn backup_dir(&self) -> PathBuf {
        self.root.join("backup")
    }

    fn validate_name(name: &str) -> Result<()> {
        if name.is_empty()
            || name.contains('/')
            || name.contains('\0')
            || name == "."
            || name == ".."
            || name.starts_with('.')
        {
            return Err(Error::InvalidName(name.into()));
        }
        Ok(())
    }

    fn index(&self, name: &str) -> Result<usize> {
        self.state
            .mods
            .iter()
            .position(|m| m.name == name)
            .ok_or_else(|| Error::ModNotFound(name.into()))
    }

    /// Install an archive (7z/zip/rar/… via libarchive) or a directory into staging.
    pub fn install(&mut self, source: &Path, name: Option<&str>) -> Result<&ModEntry> {
        let name = match name {
            Some(n) => n.to_string(),
            None => default_name(source),
        };
        Self::validate_name(&name)?;
        if self.state.mods.iter().any(|m| m.name == name) || self.mod_dir(&name).exists() {
            return Err(Error::ModExists(name));
        }
        let tmp = self.root.join(".tmp").join(&name);
        if tmp.exists() {
            std::fs::remove_dir_all(&tmp).ctx(&tmp)?;
        }
        std::fs::create_dir_all(&tmp).ctx(&tmp)?;
        let res = (|| -> Result<bool> {
            if source.is_dir() {
                copy_tree(source, &tmp)?;
            } else {
                extract_archive(source, &tmp)?;
            }
            let has_fomod = find_child_ci(&tmp, "fomod").is_some() || walk_has_fomod(&tmp);
            let data_root = find_data_root(&tmp);
            if let Some(f) = find_child_ci(&data_root, "fomod") {
                std::fs::remove_dir_all(&f).ctx(&f)?;
            }
            let dest = self.mod_dir(&name);
            std::fs::rename(&data_root, &dest).ctx(&dest)?;
            Ok(has_fomod)
        })();
        let _ = std::fs::remove_dir_all(&tmp);
        let has_fomod = res?;
        self.state.mods.push(ModEntry {
            name,
            enabled: true,
            has_fomod,
        });
        self.save()?;
        Ok(self.state.mods.last().unwrap())
    }

    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> Result<()> {
        let i = self.index(name)?;
        self.state.mods[i].enabled = enabled;
        self.save()
    }

    /// Move a mod to `new_index` in priority order (0 = lowest).
    pub fn set_priority(&mut self, name: &str, new_index: usize) -> Result<()> {
        let i = self.index(name)?;
        let m = self.state.mods.remove(i);
        let ni = new_index.min(self.state.mods.len());
        self.state.mods.insert(ni, m);
        self.save()
    }

    /// Remove a mod from staging. Refuses while a deploy is active.
    pub fn uninstall(&mut self, name: &str) -> Result<()> {
        if self.manifest_path().exists() {
            return Err(Error::AlreadyDeployed);
        }
        let i = self.index(name)?;
        let d = self.mod_dir(name);
        if d.exists() {
            std::fs::remove_dir_all(&d).ctx(&d)?;
        }
        self.state.mods.remove(i);
        self.save()
    }

    pub fn enabled_mods(&self) -> impl Iterator<Item = &ModEntry> {
        self.state.mods.iter().filter(|m| m.enabled)
    }

    /// Relative files (path inside Data/) of a staged mod.
    pub fn mod_files(&self, name: &str) -> Result<Vec<PathBuf>> {
        let root = self.mod_dir(name);
        let mut v = Vec::new();
        for e in walkdir::WalkDir::new(&root).follow_links(false) {
            let e = e.map_err(|e| Error::Other(e.to_string()))?;
            if e.file_type().is_file() {
                v.push(e.path().strip_prefix(&root).unwrap().to_path_buf());
            }
        }
        v.sort();
        Ok(v)
    }
}

fn default_name(source: &Path) -> String {
    let mut n = source
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "mod".into());
    for ext in [".tar.gz", ".tar.xz", ".7z", ".zip", ".rar", ".tar"] {
        if n.to_lowercase().ends_with(ext) {
            n.truncate(n.len() - ext.len());
            break;
        }
    }
    n
}

fn extract_archive(src: &Path, dest: &Path) -> Result<()> {
    let f = std::fs::File::open(src).ctx(src)?;
    compress_tools::uncompress_archive(f, dest, compress_tools::Ownership::Ignore)
        .map_err(|e| Error::Archive(format!("{}: {e}", src.display())))?;
    // libarchive already refuses absolute paths and `..`, but double-check nothing escaped.
    for e in walkdir::WalkDir::new(dest) {
        let e = e.map_err(|e| Error::Other(e.to_string()))?;
        if e.path_is_symlink() {
            std::fs::remove_file(e.path()).ctx(e.path())?;
        }
    }
    Ok(())
}

fn copy_tree(src: &Path, dest: &Path) -> Result<()> {
    for e in walkdir::WalkDir::new(src).follow_links(false) {
        let e = e.map_err(|e| Error::Other(e.to_string()))?;
        let rel = e.path().strip_prefix(src).unwrap();
        let t = dest.join(rel);
        if e.file_type().is_dir() {
            std::fs::create_dir_all(&t).ctx(&t)?;
        } else if e.file_type().is_file() {
            std::fs::copy(e.path(), &t).ctx(&t)?;
        }
    }
    Ok(())
}

fn find_child_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
}

fn walk_has_fomod(dir: &Path) -> bool {
    walkdir::WalkDir::new(dir)
        .max_depth(3)
        .into_iter()
        .flatten()
        .any(|e| {
            e.file_type().is_dir()
                && e.file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case("fomod")
        })
}

fn looks_like_data(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    rd.flatten().any(|e| {
        let n = e.file_name().to_string_lossy().to_lowercase();
        if e.path().is_dir() {
            DATA_DIRS.contains(&n.as_str())
        } else {
            n.rsplit('.')
                .next()
                .map(|x| DATA_EXTS.contains(&x))
                .unwrap_or(false)
        }
    })
}

/// Pick the directory whose contents map onto `Data/`.
/// Handles `Data/` wrappers and a single top-level folder wrapper.
pub(crate) fn find_data_root(extracted: &Path) -> PathBuf {
    let mut cur = extracted.to_path_buf();
    for _ in 0..3 {
        if looks_like_data(&cur) {
            return cur;
        }
        if let Some(d) = find_child_ci(&cur, "data").filter(|d| d.is_dir()) {
            cur = d;
            continue;
        }
        let entries: Vec<_> = std::fs::read_dir(&cur)
            .map(|r| r.flatten().collect())
            .unwrap_or_default();
        let dirs: Vec<_> = entries
            .iter()
            .filter(|e| {
                e.path().is_dir()
                    && !e
                        .file_name()
                        .to_string_lossy()
                        .eq_ignore_ascii_case("fomod")
            })
            .collect();
        if dirs.len() == 1 && entries.iter().all(|e| e.path().is_dir()) {
            cur = dirs[0].path();
            continue;
        }
        break;
    }
    if looks_like_data(&cur) {
        cur
    } else {
        extracted.to_path_buf()
    }
}
