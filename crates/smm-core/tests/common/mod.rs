#![allow(dead_code)]
use std::io::Write;
use std::path::{Path, PathBuf};

/// Minimal Skyrim SE plugin: a TES4 header record with HEDR + MAST/DATA pairs.
pub fn plugin_bytes(masters: &[&str], master_flag: bool, light: bool) -> Vec<u8> {
    let mut sub = Vec::new();
    sub.extend_from_slice(b"HEDR");
    sub.extend_from_slice(&12u16.to_le_bytes());
    sub.extend_from_slice(&1.71f32.to_le_bytes());
    sub.extend_from_slice(&0u32.to_le_bytes());
    sub.extend_from_slice(&0x800u32.to_le_bytes());
    for m in masters {
        let mut z = m.as_bytes().to_vec();
        z.push(0);
        sub.extend_from_slice(b"MAST");
        sub.extend_from_slice(&(z.len() as u16).to_le_bytes());
        sub.extend_from_slice(&z);
        sub.extend_from_slice(b"DATA");
        sub.extend_from_slice(&8u16.to_le_bytes());
        sub.extend_from_slice(&0u64.to_le_bytes());
    }
    let mut flags = 0u32;
    if master_flag {
        flags |= 0x1;
    }
    if light {
        flags |= 0x200;
    }
    let mut rec = Vec::new();
    rec.extend_from_slice(b"TES4");
    rec.extend_from_slice(&(sub.len() as u32).to_le_bytes());
    rec.extend_from_slice(&flags.to_le_bytes());
    rec.extend_from_slice(&0u32.to_le_bytes()); // form id
    rec.extend_from_slice(&0u32.to_le_bytes()); // vc info
    rec.extend_from_slice(&44u16.to_le_bytes()); // version
    rec.extend_from_slice(&0u16.to_le_bytes());
    rec.extend_from_slice(&sub);
    rec
}

pub fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// Build a zip archive from (path, bytes) pairs.
pub fn make_zip(path: &Path, files: &[(&str, Vec<u8>)]) -> PathBuf {
    let f = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default();
    for (name, data) in files {
        z.start_file(*name, opts).unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap();
    path.to_path_buf()
}

/// A fake game: `<tmp>/game/Data` with the base masters, plus an instance dir.
pub struct Fixture {
    pub tmp: tempfile::TempDir,
}

impl Fixture {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("game/Data");
        for m in ["Skyrim.esm", "Update.esm", "Dawnguard.esm"] {
            let masters: Vec<&str> = if m == "Skyrim.esm" {
                vec![]
            } else {
                vec!["Skyrim.esm"]
            };
            write(&data.join(m), &plugin_bytes(&masters, true, false));
        }
        write(&data.join("Textures/Vanilla.dds"), b"vanilla");
        Self { tmp }
    }
    pub fn data(&self) -> PathBuf {
        self.tmp.path().join("game/Data")
    }
    pub fn inst_dir(&self) -> PathBuf {
        self.tmp.path().join("instance")
    }
    pub fn p(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }
}

/// Snapshot of every file under `dir` with its contents, for before/after comparisons.
pub fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut v: Vec<_> = walkdir::WalkDir::new(dir)
        .into_iter()
        .flatten()
        .map(|e| {
            let rel = e.path().strip_prefix(dir).unwrap().to_path_buf();
            let data = if e.file_type().is_file() {
                std::fs::read(e.path()).unwrap()
            } else {
                vec![]
            };
            (rel, data)
        })
        .collect();
    v.sort();
    v
}
