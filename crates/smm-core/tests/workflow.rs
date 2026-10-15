mod common;
use common::*;
use smm_core::{conflicts, deploy, instance::Instance, plugins};
use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

#[test]
fn install_zip_with_wrapper_and_data_folder() {
    let fx = Fixture::new();
    let z = make_zip(
        &fx.p("Cool Armor-123.zip"),
        &[
            (
                "Cool Armor/Data/CoolArmor.esp",
                plugin_bytes(&["Skyrim.esm"], false, false),
            ),
            ("Cool Armor/Data/meshes/armor/cool.nif", b"nif".to_vec()),
            ("Cool Armor/fomod/ModuleConfig.xml", b"<config/>".to_vec()),
        ],
    );
    let mut inst = Instance::open(fx.inst_dir()).unwrap();
    let m = inst.install(&z, None).unwrap().clone();
    assert_eq!(m.name, "Cool Armor-123");
    assert!(m.has_fomod);
    let files = inst.mod_files("Cool Armor-123").unwrap();
    assert_eq!(
        files,
        vec![
            Path::new("CoolArmor.esp").to_path_buf(),
            Path::new("meshes/armor/cool.nif").to_path_buf()
        ]
    );
    // persisted
    let again = Instance::open(fx.inst_dir()).unwrap();
    assert_eq!(again.state.mods.len(), 1);
    // duplicate refused
    assert!(inst.install(&z, None).is_err());
}

#[test]
fn rejects_bad_names() {
    let fx = Fixture::new();
    let mut inst = Instance::open(fx.inst_dir()).unwrap();
    let src = fx.p("src");
    write(&src.join("a.esp"), b"x");
    assert!(inst.install(&src, Some("../evil")).is_err());
    assert!(inst.install(&src, Some(".hidden")).is_err());
}

fn two_conflicting_mods(fx: &Fixture) -> Instance {
    let mut inst = Instance::open(fx.inst_dir()).unwrap();
    let a = fx.p("src/a");
    write(&a.join("textures/shared.dds"), b"from A");
    write(&a.join("textures/onlya.dds"), b"A");
    write(
        &a.join("A.esp"),
        &plugin_bytes(&["Skyrim.esm"], false, false),
    );
    let b = fx.p("src/b");
    write(&b.join("Textures/SHARED.dds"), b"from B");
    write(&b.join("textures/vanilla.dds"), b"B replaces vanilla");
    inst.install(&a, Some("ModA")).unwrap();
    inst.install(&b, Some("ModB")).unwrap();
    inst
}

#[test]
fn conflict_table_case_insensitive_last_wins() {
    let fx = Fixture::new();
    let mut inst = two_conflicting_mods(&fx);
    let c = conflicts::conflicts(&inst).unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].path, "textures/shared.dds");
    assert_eq!(c[0].winner, "ModB");
    assert_eq!(c[0].losers, vec!["ModA"]);
    inst.set_priority("ModA", 1).unwrap();
    assert_eq!(conflicts::conflicts(&inst).unwrap()[0].winner, "ModA");
    inst.set_enabled("ModB", false).unwrap();
    assert!(conflicts::conflicts(&inst).unwrap().is_empty());
}

#[test]
fn dry_run_then_deploy_then_clean_undeploy() {
    let fx = Fixture::new();
    let inst = two_conflicting_mods(&fx);
    let before = snapshot(&fx.data());

    // Dry run changes nothing.
    let d = deploy::diff(&inst, &fx.data()).unwrap();
    assert_eq!(d.added.len(), 4);
    assert_eq!(d.replaces_existing, vec!["textures/vanilla.dds"]);
    assert_eq!(snapshot(&fx.data()), before);
    assert!(!inst.manifest_path().exists());

    let m = deploy::deploy(&inst, &fx.data()).unwrap();
    assert!(m.complete);
    // Existing "Textures" casing is reused, winner is ModB, files are hardlinks.
    let shared = fx.data().join("Textures/SHARED.dds");
    assert!(!fx.data().join("textures").exists());
    assert_eq!(std::fs::read(&shared).unwrap(), b"from B");
    assert_eq!(
        shared.metadata().unwrap().ino(),
        inst.mod_dir("ModB")
            .join("Textures/SHARED.dds")
            .metadata()
            .unwrap()
            .ino()
    );
    assert_eq!(
        std::fs::read(fx.data().join("Textures/Vanilla.dds")).unwrap(),
        b"B replaces vanilla"
    );
    assert!(fx.data().join("A.esp").exists());

    // Nothing changed -> redeploy diff is all unchanged.
    let d2 = deploy::diff(&inst, &fx.data()).unwrap();
    assert!(d2.added.is_empty() && d2.removed.is_empty() && d2.changed.is_empty());
    assert_eq!(d2.unchanged, 4);

    let r = deploy::undeploy(&inst).unwrap();
    assert_eq!(r.removed, 4);
    assert_eq!(r.restored, 1);
    assert!(r.skipped_modified.is_empty());
    assert_eq!(
        snapshot(&fx.data()),
        before,
        "undeploy must restore Data/ exactly"
    );
    assert!(!inst.manifest_path().exists());
}

#[test]
fn redeploy_reports_changes_and_creates_dirs_cleanly() {
    let fx = Fixture::new();
    let mut inst = two_conflicting_mods(&fx);
    let before = snapshot(&fx.data());
    let c = fx.p("src/c");
    write(&c.join("meshes/deep/nested/x.nif"), b"nif");
    inst.install(&c, Some("ModC")).unwrap();
    deploy::deploy(&inst, &fx.data()).unwrap();
    assert!(fx.data().join("meshes/deep/nested/x.nif").exists());

    inst.set_enabled("ModC", false).unwrap();
    inst.set_priority("ModA", 5).unwrap();
    let d = deploy::diff(&inst, &fx.data()).unwrap();
    assert_eq!(d.removed, vec!["meshes/deep/nested/x.nif"]);
    assert_eq!(
        d.changed,
        vec![("textures/shared.dds".into(), "ModB".into(), "ModA".into())]
    );

    deploy::deploy(&inst, &fx.data()).unwrap();
    assert!(
        !fx.data().join("meshes").exists(),
        "dirs we created are removed when no longer needed"
    );
    assert_eq!(
        std::fs::read(fx.data().join("Textures/shared.dds")).unwrap(),
        b"from A"
    );
    deploy::undeploy(&inst).unwrap();
    assert_eq!(snapshot(&fx.data()), before);
}

#[test]
fn undeploy_leaves_user_modified_files() {
    let fx = Fixture::new();
    let inst = two_conflicting_mods(&fx);
    deploy::deploy(&inst, &fx.data()).unwrap();
    // User replaces a deployed file with their own (new inode).
    let f = fx.data().join("Textures/onlya.dds");
    std::fs::remove_file(&f).unwrap();
    std::fs::write(&f, b"user edit").unwrap();
    let r = deploy::undeploy(&inst).unwrap();
    assert_eq!(r.skipped_modified, vec![f.clone()]);
    assert_eq!(std::fs::read(&f).unwrap(), b"user edit");
    // Manifest is kept with only the leftover so nothing is forgotten.
    let m = deploy::load_manifest(&inst).unwrap().unwrap();
    assert_eq!(m.entries.len(), 1);
    // Uninstall is blocked while a manifest exists.
    let mut inst = inst;
    assert!(inst.uninstall("ModA").is_err());
}

#[test]
fn undeploy_recovers_from_interrupted_deploy() {
    let fx = Fixture::new();
    let inst = two_conflicting_mods(&fx);
    let before = snapshot(&fx.data());
    deploy::deploy(&inst, &fx.data()).unwrap();
    // Simulate a crash right after manifest write: method unknown.
    let mut m = deploy::load_manifest(&inst).unwrap().unwrap();
    for e in m.entries.iter_mut() {
        e.method = None;
    }
    m.complete = false;
    std::fs::write(inst.manifest_path(), serde_json::to_vec(&m).unwrap()).unwrap();
    let r = deploy::undeploy(&inst).unwrap();
    assert!(r.skipped_modified.is_empty());
    assert_eq!(snapshot(&fx.data()), before);
}

#[test]
fn plugins_masters_order_and_plugins_txt() {
    let fx = Fixture::new();
    let mut inst = Instance::open(fx.inst_dir()).unwrap();
    let src = fx.p("src/p");
    write(
        &src.join("Patch.esp"),
        &plugin_bytes(&["Skyrim.esm", "Lib.esm"], false, false),
    );
    write(
        &src.join("Lib.esm"),
        &plugin_bytes(&["Skyrim.esm"], true, false),
    );
    write(
        &src.join("Orphan.esp"),
        &plugin_bytes(&["Missing.esm"], false, false),
    );
    write(
        &src.join("Small.esl"),
        &plugin_bytes(&["Skyrim.esm"], true, true),
    );
    inst.install(&src, Some("Plugins")).unwrap();

    let ps = plugins::collect(&inst, Some(&fx.data())).unwrap();
    let small = ps.iter().find(|p| p.name == "Small.esl").unwrap();
    assert!(small.is_light && small.is_master);
    let patch = ps.iter().find(|p| p.name == "Patch.esp").unwrap();
    assert_eq!(patch.masters, vec!["Skyrim.esm", "Lib.esm"]);

    let order = plugins::load_order(&inst, &ps);
    assert_eq!(&order[..3], &["Skyrim.esm", "Update.esm", "Dawnguard.esm"]);
    let active = plugins::active_set(&inst, &order);
    let issues = plugins::check(&order, &active, &ps);
    assert_eq!(
        issues,
        vec![plugins::Issue::MissingMaster {
            plugin: "Orphan.esp".into(),
            master: "Missing.esm".into()
        }]
    );

    // A bad saved order: Patch before Lib.
    inst.state.plugin_order = vec!["Patch.esp".into(), "Lib.esm".into()];
    let bad = plugins::load_order(&inst, &ps);
    let issues = plugins::check(&bad, &active, &ps);
    assert!(issues.contains(&plugins::Issue::MasterAfterDependent {
        plugin: "Patch.esp".into(),
        master: "Lib.esm".into()
    }));
    let fixed = plugins::fix_order(&bad, &ps);
    let pos = |n: &str| fixed.iter().position(|x| x == n).unwrap();
    assert!(pos("Lib.esm") < pos("Patch.esp"));
    assert!(plugins::check(&fixed, &active, &ps)
        .iter()
        .all(|i| matches!(i, plugins::Issue::MissingMaster { .. })));

    // Deactivate Lib -> inactive master flagged.
    inst.state.inactive_plugins = vec!["Lib.esm".into()];
    let active2 = plugins::active_set(&inst, &fixed);
    assert!(
        plugins::check(&fixed, &active2, &ps).contains(&plugins::Issue::InactiveMaster {
            plugin: "Patch.esp".into(),
            master: "Lib.esm".into()
        })
    );

    // Plugins.txt: implicit masters omitted, '*' for active, CRLF, backup of existing file.
    let pt = fx.p("prefix/AppData/Local/Skyrim Special Edition/Plugins.txt");
    write(&pt, b"*Old.esp\r\n");
    plugins::write_plugins_txt(&pt, &fixed, &active2).unwrap();
    let txt = std::fs::read_to_string(&pt).unwrap();
    assert!(!txt.contains("Skyrim.esm"));
    assert!(
        txt.contains("\nLib.esm\r\n") || txt.contains("Lib.esm\r\n") && !txt.contains("*Lib.esm")
    );
    assert!(txt.contains("*Patch.esp\r\n"));
    assert_eq!(
        std::fs::read_to_string(pt.with_extension("txt.smm-bak")).unwrap(),
        "*Old.esp\r\n"
    );
    let (o, a) = plugins::parse_plugins_txt(&txt);
    assert_eq!(o.len(), fixed.len() - 3);
    assert!(a.contains("patch.esp") && !a.contains("lib.esm"));
    let _ = HashSet::<String>::new();
}
