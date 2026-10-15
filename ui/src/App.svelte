<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import * as api from "./api";
  import type { Overview } from "./api";

  let ov = $state<Overview | null>(null);
  let tab = $state<"conflicts" | "plan" | "plugins">("conflicts");
  let msg = $state("");
  let err = $state("");
  let busy = $state(false);

  const stats = $derived.by(() => {
    const m = new Map<string, { wins: number; losses: number }>();
    for (const c of ov?.conflicts ?? []) {
      const w = m.get(c.winner) ?? { wins: 0, losses: 0 };
      w.wins++;
      m.set(c.winner, w);
      for (const l of c.losers) {
        const x = m.get(l) ?? { wins: 0, losses: 0 };
        x.losses++;
        m.set(l, x);
      }
    }
    return m;
  });

  async function run(f: () => Promise<unknown>, ok?: (r: any) => string) {
    busy = true;
    err = "";
    try {
      const r = await f();
      if (ok) msg = ok(r);
      ov = await api.overview();
    } catch (e) {
      err = String(e);
    } finally {
      busy = false;
    }
  }

  async function install() {
    const path = await open({ multiple: false, filters: [{ name: "Mod archives", extensions: ["7z", "zip", "rar"] }] });
    if (typeof path === "string") run(() => api.installMod(path), () => "Installed.");
  }

  run(async () => {});
</script>

<header>
  <h1>Skyrim Mod Manager</h1>
  <div class="paths">
    <span title="Data dir">Data: <code>{ov?.data_dir ?? "not detected"}</code></span>
    <span title="Wine prefix">Prefix: <code>{ov?.prefix ?? "not detected"}</code></span>
  </div>
  <div class="actions">
    <button onclick={install} disabled={busy}>Install archive…</button>
    <button class="primary" onclick={() => (tab = "plan")} disabled={busy}>Review deploy</button>
    <button onclick={() => run(api.undeploy, (r) => r)} disabled={busy || !ov?.deployed}>Undeploy</button>
  </div>
</header>

{#if err}<div class="err">{err}</div>{/if}
{#if msg}<div class="msg">{msg}</div>{/if}

<main>
  <section class="mods">
    <h2>Mods <small>(bottom wins)</small></h2>
    {#if ov && ov.mods.length === 0}<p class="dim">No mods installed yet.</p>{/if}
    <ol>
      {#each ov?.mods ?? [] as m, i (m.name)}
        {@const s = stats.get(m.name)}
        <li class:off={!m.enabled}>
          <input type="checkbox" checked={m.enabled} onchange={() => run(() => api.setEnabled(m.name, !m.enabled))} />
          <span class="name">{m.name}</span>
          {#if m.has_fomod}<span class="tag warn" title="FOMOD installer not run">fomod</span>{/if}
          {#if s?.wins}<span class="tag win" title="files this mod wins">+{s.wins}</span>{/if}
          {#if s?.losses}<span class="tag lose" title="files overwritten by later mods">−{s.losses}</span>{/if}
          <span class="move">
            <button onclick={() => run(() => api.setPriority(m.name, i - 1))} disabled={i === 0}>▲</button>
            <button onclick={() => run(() => api.setPriority(m.name, i + 1))} disabled={i === (ov?.mods.length ?? 0) - 1}>▼</button>
          </span>
        </li>
      {/each}
    </ol>
  </section>

  <section class="detail">
    <nav>
      <button class:active={tab === "conflicts"} onclick={() => (tab = "conflicts")}>Conflicts ({ov?.conflicts.length ?? 0})</button>
      <button class:active={tab === "plan"} onclick={() => (tab = "plan")}>Deploy plan</button>
      <button class:active={tab === "plugins"} onclick={() => (tab = "plugins")}>
        Plugins {#if ov?.issues.length}<span class="tag lose">{ov.issues.length}</span>{/if}
      </button>
    </nav>

    {#if tab === "conflicts"}
      {#if ov?.conflicts.length}
        <table>
          <thead><tr><th>Path</th><th>Winner</th><th>Overwritten</th></tr></thead>
          <tbody>
            {#each ov.conflicts as c (c.path)}
              <tr><td><code>{c.path}</code></td><td class="win">{c.winner}</td><td class="lose">{c.losers.join(", ")}</td></tr>
            {/each}
          </tbody>
        </table>
      {:else}<p class="dim">No file conflicts between enabled mods.</p>{/if}
    {:else if tab === "plan"}
      {#if ov?.diff}
        {@const d = ov.diff}
        <p>{d.added.length} added · {d.removed.length} removed · {d.changed.length} changed · {d.unchanged} unchanged</p>
        <ul class="diff">
          {#each d.added as k}<li class="add">+ {k}</li>{/each}
          {#each d.removed as k}<li class="del">− {k}</li>{/each}
          {#each d.changed as [k, a, b]}<li class="chg">~ {k} <small>({a} → {b})</small></li>{/each}
          {#each d.replaces_existing as k}<li class="warn">! {k} replaces an existing game file (backed up, restored on undeploy)</li>{/each}
        </ul>
        <button class="primary" onclick={() => run(api.deploy, (n) => `Deployed ${n} files.`)} disabled={busy}>Deploy (hardlinks)</button>
      {:else}<p class="dim">No Data/ folder detected.</p>{/if}
    {:else}
      {#if ov?.issues.length}
        <ul class="issues">
          {#each ov.issues as is}
            <li>
              <b>{is.plugin}</b>:
              {#if is.kind === "missing_master"}missing master <code>{is.master}</code>
              {:else if is.kind === "inactive_master"}master <code>{is.master}</code> is inactive
              {:else if is.kind === "master_after_dependent"}master <code>{is.master}</code> loads after it
              {:else if is.kind === "non_master_before_master"}loads before master-flagged <code>{is.master_flagged}</code>
              {:else}{is.error}{/if}
            </li>
          {/each}
        </ul>
      {/if}
      <table>
        <thead><tr><th>#</th><th>Plugin</th><th>Flags</th><th>From</th><th>Masters</th></tr></thead>
        <tbody>
          {#each ov?.plugins ?? [] as p, i (p.name)}
            <tr class:off={!p.active}>
              <td>{i.toString(16).toUpperCase().padStart(2, "0")}</td>
              <td>{p.active ? "*" : ""}{p.name}</td>
              <td>{p.is_light ? "ESL" : p.is_master ? "ESM" : ""}</td>
              <td class="dim">{p.provider ?? "game"}</td>
              <td class="dim">{p.masters.join(", ")}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      <div class="actions">
        <button onclick={() => run(api.fixOrder, () => "Load order fixed.")} disabled={busy}>Fix order</button>
        <button onclick={() => run(api.writePluginsTxt, (p) => `Wrote ${p}`)} disabled={busy || !ov?.plugins_txt}>Write Plugins.txt</button>
      </div>
    {/if}
  </section>
</main>
