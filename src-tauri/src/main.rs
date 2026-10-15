#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use smm_core::{conflicts, deploy, detect, instance::Instance, plugins};
use std::path::PathBuf;

/// Where the app works. `SMM_INSTANCE` / `SMM_DATA` override the defaults
/// (handy for testing against a throwaway folder).
struct Paths {
    instance: PathBuf,
    data_dir: Option<PathBuf>,
    prefix: Option<detect::Prefix>,
}

fn paths() -> Paths {
    let found = detect::detect(&detect::home_dir());
    let instance = std::env::var_os("SMM_INSTANCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| detect::home_dir().join(".local/share"))
                .join("skyrim-mod-manager/default")
        });
    let data_dir = std::env::var_os("SMM_DATA")
        .map(PathBuf::from)
        .or_else(|| found.first().map(|i| i.data_dir.clone()));
    Paths {
        instance,
        data_dir,
        prefix: found.into_iter().find_map(|i| i.prefix),
    }
}

fn s<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn open() -> Result<(Instance, Paths), String> {
    let p = paths();
    Ok((Instance::open(&p.instance).map_err(s)?, p))
}

#[derive(Serialize)]
struct PluginRow {
    name: String,
    active: bool,
    provider: Option<String>,
    masters: Vec<String>,
    is_master: bool,
    is_light: bool,
}

#[derive(Serialize)]
struct Overview {
    instance: PathBuf,
    data_dir: Option<PathBuf>,
    prefix: Option<PathBuf>,
    plugins_txt: Option<PathBuf>,
    deployed: bool,
    mods: Vec<smm_core::instance::ModEntry>,
    conflicts: Vec<conflicts::Conflict>,
    diff: Option<deploy::Diff>,
    plugins: Vec<PluginRow>,
    issues: Vec<plugins::Issue>,
}

#[tauri::command]
fn overview() -> Result<Overview, String> {
    let (inst, p) = open()?;
    let ps = plugins::collect(&inst, p.data_dir.as_deref()).map_err(s)?;
    let order = plugins::load_order(&inst, &ps);
    let active = plugins::active_set(&inst, &order);
    let issues = plugins::check(&order, &active, &ps);
    let rows = order
        .iter()
        .filter_map(|n| ps.iter().find(|x| &x.name == n))
        .map(|x| PluginRow {
            name: x.name.clone(),
            active: active.contains(&x.name.to_lowercase()),
            provider: x.provider.clone(),
            masters: x.masters.clone(),
            is_master: x.is_master,
            is_light: x.is_light,
        })
        .collect();
    Ok(Overview {
        diff: match &p.data_dir {
            Some(d) => Some(deploy::diff(&inst, d).map_err(s)?),
            None => None,
        },
        conflicts: conflicts::conflicts(&inst).map_err(s)?,
        deployed: inst.manifest_path().exists(),
        mods: inst.state.mods.clone(),
        plugins: rows,
        issues,
        plugins_txt: p.prefix.as_ref().map(|x| x.plugins_txt()),
        prefix: p.prefix.map(|x| x.path),
        data_dir: p.data_dir,
        instance: p.instance,
    })
}

#[tauri::command]
fn install_mod(path: String) -> Result<(), String> {
    open()?
        .0
        .install(std::path::Path::new(&path), None)
        .map(|_| ())
        .map_err(s)
}

#[tauri::command]
fn set_enabled(name: String, enabled: bool) -> Result<(), String> {
    open()?.0.set_enabled(&name, enabled).map_err(s)
}

#[tauri::command]
fn set_priority(name: String, index: usize) -> Result<(), String> {
    open()?.0.set_priority(&name, index).map_err(s)
}

#[tauri::command]
fn deploy() -> Result<usize, String> {
    let (inst, p) = open()?;
    let d = p.data_dir.ok_or("no Data/ folder detected")?;
    deploy::deploy(&inst, &d)
        .map(|m| m.entries.len())
        .map_err(s)
}

#[tauri::command]
fn undeploy() -> Result<String, String> {
    let r = deploy::undeploy(&open()?.0).map_err(s)?;
    let mut msg = format!("Removed {}, restored {}.", r.removed, r.restored);
    if !r.skipped_modified.is_empty() {
        msg += &format!(
            " {} modified file(s) left in place.",
            r.skipped_modified.len()
        );
    }
    Ok(msg)
}

#[tauri::command]
fn fix_order() -> Result<(), String> {
    let (mut inst, p) = open()?;
    let ps = plugins::collect(&inst, p.data_dir.as_deref()).map_err(s)?;
    let order = plugins::fix_order(&plugins::load_order(&inst, &ps), &ps);
    inst.state.plugin_order = order;
    inst.save().map_err(s)
}

#[tauri::command]
fn write_plugins_txt() -> Result<String, String> {
    let (inst, p) = open()?;
    let target = std::env::var_os("SMM_PLUGINS_TXT")
        .map(PathBuf::from)
        .or_else(|| p.prefix.as_ref().map(|x| x.plugins_txt()))
        .ok_or("no prefix detected")?;
    let ps = plugins::collect(&inst, p.data_dir.as_deref()).map_err(s)?;
    let order = plugins::load_order(&inst, &ps);
    let active = plugins::active_set(&inst, &order);
    plugins::write_plugins_txt(&target, &order, &active).map_err(s)?;
    Ok(target.display().to_string())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            overview,
            install_mod,
            set_enabled,
            set_priority,
            deploy,
            undeploy,
            fix_order,
            write_plugins_txt
        ])
        .run(tauri::generate_context!())
        .expect("error while running skyrim-mod-manager");
}
