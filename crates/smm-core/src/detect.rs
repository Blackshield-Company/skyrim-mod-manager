//! Read-only detection of Skyrim SE/AE and its Wine prefix.
//!
//! Nothing in this module writes to disk.
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const SKYRIM_SE_APPID: &str = "489830";
pub const GAME_DIR_NAME: &str = "Skyrim Special Edition";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PrefixKind {
    Proton,
    Umu,
    Custom,
}

#[derive(Debug, Clone, Serialize)]
pub struct Prefix {
    pub kind: PrefixKind,
    /// The `pfx` / WINEPREFIX root (contains `drive_c`).
    pub path: PathBuf,
}

impl Prefix {
    /// `.../AppData/Local/Skyrim Special Edition`, where Plugins.txt lives.
    /// Prefers an existing user dir (`steamuser` for Proton), else falls back to `steamuser`.
    pub fn appdata_dir(&self) -> PathBuf {
        let users = self.path.join("drive_c/users");
        let mut candidates: Vec<PathBuf> = vec![users.join("steamuser")];
        if let Ok(rd) = std::fs::read_dir(&users) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n != "Public" && n != "steamuser" {
                    candidates.push(e.path());
                }
            }
        }
        for c in &candidates {
            let d = c.join("AppData/Local").join(GAME_DIR_NAME);
            if d.is_dir() {
                return d;
            }
        }
        candidates[0].join("AppData/Local").join(GAME_DIR_NAME)
    }

    pub fn plugins_txt(&self) -> PathBuf {
        self.appdata_dir().join("Plugins.txt")
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Installation {
    pub game_dir: PathBuf,
    pub data_dir: PathBuf,
    pub library: PathBuf,
    pub prefix: Option<Prefix>,
}

/// Candidate Steam roots on a typical Linux box (native, legacy symlink, Flatpak).
pub fn steam_roots(home: &Path) -> Vec<PathBuf> {
    let mut v = vec![
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".steam/root"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ];
    let mut seen = std::collections::HashSet::new();
    v.retain(|p| p.is_dir() && seen.insert(std::fs::canonicalize(p).unwrap_or(p.clone())));
    v
}

/// Library paths listed in a `libraryfolders.vdf` text.
pub fn parse_library_folders(text: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(vdf) = keyvalues_parser::parse(text).map(keyvalues_parser::Vdf::from) else {
        return out;
    };
    if let Some(obj) = vdf.value.get_obj() {
        for vals in obj.values() {
            for v in vals {
                if let Some(o) = v.get_obj() {
                    if let Some(p) = o
                        .get("path")
                        .and_then(|p| p.first())
                        .and_then(|p| p.get_str())
                    {
                        out.push(PathBuf::from(p));
                    }
                }
            }
        }
    }
    out
}

/// All Steam libraries for a Steam root (the root itself plus libraryfolders.vdf entries).
pub fn libraries(steam_root: &Path) -> Vec<PathBuf> {
    let mut libs = vec![steam_root.to_path_buf()];
    for f in ["steamapps/libraryfolders.vdf", "config/libraryfolders.vdf"] {
        if let Ok(t) = std::fs::read_to_string(steam_root.join(f)) {
            for p in parse_library_folders(&t) {
                if !libs.contains(&p) {
                    libs.push(p);
                }
            }
        }
    }
    libs
}

/// Look for Skyrim SE in one Steam library.
pub fn find_in_library(lib: &Path) -> Option<Installation> {
    let game_dir = lib.join("steamapps/common").join(GAME_DIR_NAME);
    if !game_dir.join("SkyrimSE.exe").is_file() && !game_dir.join("Data/Skyrim.esm").is_file() {
        return None;
    }
    let pfx = lib
        .join("steamapps/compatdata")
        .join(SKYRIM_SE_APPID)
        .join("pfx");
    let prefix = pfx.join("drive_c").is_dir().then_some(Prefix {
        kind: PrefixKind::Proton,
        path: pfx,
    });
    Some(Installation {
        data_dir: crate::paths::resolve_ci(&game_dir, Path::new("Data")),
        game_dir,
        library: lib.to_path_buf(),
        prefix,
    })
}

/// umu-launcher prefixes (e.g. `~/Games/umu/<name>`) that contain a Skyrim SE appdata dir.
pub fn umu_prefixes(home: &Path) -> Vec<Prefix> {
    let mut out = Vec::new();
    for base in [home.join("Games/umu"), home.join(".local/share/umu")] {
        let Ok(rd) = std::fs::read_dir(&base) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.join("drive_c").is_dir() {
                let pre = Prefix {
                    kind: PrefixKind::Umu,
                    path: p,
                };
                if pre.appdata_dir().is_dir() {
                    out.push(pre);
                }
            }
        }
    }
    out
}

/// Detect installs under `home`. `SMM_GAME_DIR` / `SMM_PREFIX` override detection.
pub fn detect(home: &Path) -> Vec<Installation> {
    let mut found = Vec::new();
    if let Ok(g) = std::env::var("SMM_GAME_DIR") {
        let game_dir = PathBuf::from(g);
        let prefix = std::env::var("SMM_PREFIX").ok().map(|p| Prefix {
            kind: PrefixKind::Custom,
            path: p.into(),
        });
        found.push(Installation {
            data_dir: crate::paths::resolve_ci(&game_dir, Path::new("Data")),
            library: game_dir.clone(),
            game_dir,
            prefix,
        });
        return found;
    }
    for root in steam_roots(home) {
        for lib in libraries(&root) {
            if let Some(i) = find_in_library(&lib) {
                if !found
                    .iter()
                    .any(|f: &Installation| f.game_dir == i.game_dir)
                {
                    found.push(i);
                }
            }
        }
    }
    // No Proton prefix (e.g. launched via umu): attach the first umu prefix.
    let umu = umu_prefixes(home);
    for i in found.iter_mut() {
        if i.prefix.is_none() {
            i.prefix = umu.first().cloned();
        }
    }
    found
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VDF: &str = r#""libraryfolders"
{
    "0"
    {
        "path"      "/home/u/.local/share/Steam"
        "apps" { "228980" "1" }
    }
    "1"
    {
        "path"      "/mnt/games/SteamLibrary"
        "apps" { "489830" "1" }
    }
}"#;

    #[test]
    fn parses_library_folders() {
        let libs = parse_library_folders(VDF);
        assert_eq!(
            libs,
            vec![
                PathBuf::from("/home/u/.local/share/Steam"),
                PathBuf::from("/mnt/games/SteamLibrary")
            ]
        );
    }

    #[test]
    fn detects_fake_steam_layout() {
        let home = tempfile::tempdir().unwrap();
        let steam = home.path().join(".local/share/Steam");
        let lib2 = home.path().join("lib2");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        std::fs::write(
            steam.join("steamapps/libraryfolders.vdf"),
            format!("\"libraryfolders\"\n{{\n \"0\" {{ \"path\" \"{}\" }}\n \"1\" {{ \"path\" \"{}\" }}\n}}", steam.display(), lib2.display()),
        )
        .unwrap();
        let game = lib2.join("steamapps/common/Skyrim Special Edition");
        std::fs::create_dir_all(game.join("Data")).unwrap();
        std::fs::write(game.join("SkyrimSE.exe"), b"").unwrap();
        let pfx = lib2.join("steamapps/compatdata/489830/pfx");
        std::fs::create_dir_all(
            pfx.join("drive_c/users/steamuser/AppData/Local/Skyrim Special Edition"),
        )
        .unwrap();

        let found = detect(home.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].game_dir, game);
        let p = found[0].prefix.as_ref().unwrap();
        assert_eq!(p.kind, PrefixKind::Proton);
        assert!(p
            .plugins_txt()
            .ends_with("steamuser/AppData/Local/Skyrim Special Edition/Plugins.txt"));
    }

    #[test]
    fn falls_back_to_umu_prefix() {
        let home = tempfile::tempdir().unwrap();
        let steam = home.path().join(".local/share/Steam");
        let game = steam.join("steamapps/common/Skyrim Special Edition");
        std::fs::create_dir_all(game.join("Data")).unwrap();
        std::fs::write(game.join("Data/Skyrim.esm"), b"").unwrap();
        let umu = home.path().join("Games/umu/skyrim");
        std::fs::create_dir_all(umu.join("drive_c/users/u/AppData/Local/Skyrim Special Edition"))
            .unwrap();
        let found = detect(home.path());
        assert_eq!(found.len(), 1);
        let p = found[0].prefix.as_ref().unwrap();
        assert_eq!(p.kind, PrefixKind::Umu);
        assert!(p.appdata_dir().starts_with(&umu));
    }
}
