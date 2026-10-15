//! Case-insensitive path helpers. Skyrim treats `Data/` paths case-insensitively,
//! Linux does not, so conflict keys are lowercased and deploy reuses whatever
//! casing already exists in the target.
use std::path::{Component, Path, PathBuf};

/// Normalised key for a relative path inside Data/: lowercase, `/`-separated.
pub fn key(rel: &Path) -> String {
    rel.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Resolve `rel` under `root`, matching each existing component case-insensitively.
/// Components that don't exist yet keep the casing from `rel`.
pub fn resolve_ci(root: &Path, rel: &Path) -> PathBuf {
    let mut cur = root.to_path_buf();
    let mut exists = true;
    for comp in rel.components() {
        let Component::Normal(name) = comp else {
            continue;
        };
        if exists {
            if cur.join(name).symlink_metadata().is_ok() {
                cur.push(name);
                continue;
            }
            let want = name.to_string_lossy().to_lowercase();
            let found = std::fs::read_dir(&cur).ok().and_then(|rd| {
                rd.flatten()
                    .find(|e| e.file_name().to_string_lossy().to_lowercase() == want)
                    .map(|e| e.file_name())
            });
            match found {
                Some(f) => {
                    cur.push(f);
                    continue;
                }
                None => exists = false,
            }
        }
        cur.push(name);
    }
    cur
}

/// Reject path components that would escape a root (`..`, absolute paths).
pub fn is_safe_relative(rel: &Path) -> bool {
    rel.components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_lowercases() {
        assert_eq!(
            key(Path::new("Textures/Foo/BAR.dds")),
            "textures/foo/bar.dds"
        );
    }

    #[test]
    fn resolve_reuses_existing_case() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("Textures/Armor")).unwrap();
        let p = resolve_ci(t.path(), Path::new("textures/armor/new/x.dds"));
        assert_eq!(p, t.path().join("Textures/Armor/new/x.dds"));
    }

    #[test]
    fn unsafe_paths() {
        assert!(!is_safe_relative(Path::new("../etc/passwd")));
        assert!(!is_safe_relative(Path::new("/abs")));
        assert!(is_safe_relative(Path::new("meshes/a.nif")));
    }
}
