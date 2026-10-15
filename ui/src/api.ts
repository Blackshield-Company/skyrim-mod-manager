import { invoke } from "@tauri-apps/api/core";

export interface ModEntry { name: string; enabled: boolean; has_fomod: boolean }
export interface Conflict { path: string; winner: string; losers: string[] }
export interface Diff {
  added: string[]; removed: string[]; changed: [string, string, string][];
  replaces_existing: string[]; unchanged: number;
}
export interface PluginRow { name: string; active: boolean; provider: string | null; masters: string[]; is_master: boolean; is_light: boolean }
export interface Issue { kind: string; plugin: string; master?: string; master_flagged?: string; error?: string }
export interface Overview {
  instance: string;
  data_dir: string | null;
  prefix: string | null;
  plugins_txt: string | null;
  deployed: boolean;
  mods: ModEntry[];
  conflicts: Conflict[];
  diff: Diff | null;
  plugins: PluginRow[];
  issues: Issue[];
}

export const overview = () => invoke<Overview>("overview");
export const installMod = (path: string) => invoke<void>("install_mod", { path });
export const setEnabled = (name: string, enabled: boolean) => invoke<void>("set_enabled", { name, enabled });
export const setPriority = (name: string, index: number) => invoke<void>("set_priority", { name, index });
export const deploy = () => invoke<number>("deploy");
export const undeploy = () => invoke<string>("undeploy");
export const fixOrder = () => invoke<void>("fix_order");
export const writePluginsTxt = () => invoke<string>("write_plugins_txt");
