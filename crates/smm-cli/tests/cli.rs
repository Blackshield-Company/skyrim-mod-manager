//! End-to-end CLI test against a throwaway fake game folder. HOME is pointed at a
//! temp dir so detection can never see a real install.
use std::path::Path;
use std::process::Command;

fn smm(home: &Path, args: &[&str]) -> (bool, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_smm"))
        .args(args)
        .env("HOME", home)
        .env_remove("SMM_GAME_DIR")
        .env_remove("SMM_PREFIX")
        .env_remove("XDG_DATA_HOME")
        .output()
        .unwrap();
    (
        o.status.success(),
        String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr),
    )
}

#[test]
fn full_cycle() {
    let t = tempfile::tempdir().unwrap();
    let home = t.path();
    let data = home.join("game/Data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("keep.txt"), b"x").unwrap();
    let src = home.join("modsrc");
    std::fs::create_dir_all(src.join("textures")).unwrap();
    std::fs::write(src.join("textures/a.dds"), b"a").unwrap();
    let inst = home.join("inst");
    let i = inst.to_str().unwrap();
    let d = data.to_str().unwrap();

    let (ok, out) = smm(home, &["detect"]);
    assert!(ok && out.contains("No Skyrim SE install found"), "{out}");
    let (ok, out) = smm(
        home,
        &[
            "--instance",
            i,
            "install",
            src.to_str().unwrap(),
            "--name",
            "Tex",
        ],
    );
    assert!(ok, "{out}");
    let (ok, out) = smm(home, &["--instance", i, "--data", d, "plan"]);
    assert!(ok && out.contains("+ textures/a.dds"), "{out}");
    let (ok, _) = smm(home, &["--instance", i, "--data", d, "deploy"]);
    assert!(!ok, "deploy without --yes must refuse");
    let (ok, out) = smm(home, &["--instance", i, "--data", d, "deploy", "--yes"]);
    assert!(ok, "{out}");
    assert!(data.join("textures/a.dds").exists());
    let (ok, out) = smm(home, &["--instance", i, "undeploy"]);
    assert!(ok && out.contains("removed 1"), "{out}");
    assert!(!data.join("textures").exists());
    assert!(data.join("keep.txt").exists());
    let (ok, out) = smm(home, &["--instance", i, "--json", "list"]);
    assert!(ok && out.contains("\"Tex\""), "{out}");
}
