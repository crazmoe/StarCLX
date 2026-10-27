<script lang="ts">
  // Plugin Nextcloud: verbinden, Adressbücher wählen, Vorgaben für Links.
  // Gilt pro Instanzprofil.
  import { invoke } from "@tauri-apps/api/core";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import Icon from "../../Icon.svelte";
  import Toggle from "../../Toggle.svelte";
  import type { Prefs } from "../../prefs.svelte";
  import { nextcloud } from "./nextcloud.svelte";
  import { t } from "../../i18n.svelte";

  let { draft = $bindable() }: { draft: Prefs } = $props();

  type UntrustedCert = { host: string; port: number; fingerprint: string; reason: string };

  let server = $state("");
  let busy = $state(false);
  let error = $state("");
  let untrusted = $state<UntrustedCert | null>(null);
  let confirmDisconnect = $state(false);

  const s = $derived(nextcloud.status);

  async function connect() {
    busy = true;
    error = "";
    try {
      if (!untrusted) {
        const cert = await invoke<UntrustedCert | null>("nextcloud_check_certificate", { server });
        if (cert) {
          untrusted = cert;
          return;
        }
      }
      await invoke("nextcloud_connect", { server });
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  async function trust() {
    if (!untrusted) return;
    await invoke("trust_certificate", { server, fingerprint: untrusted.fingerprint });
    await connect();
    untrusted = null;
  }

  async function disconnect() {
    if (!confirmDisconnect) return void (confirmDisconnect = true);
    confirmDisconnect = false;
    await invoke("nextcloud_disconnect");
  }

  async function sync() {
    error = "";
    try {
      await invoke("nextcloud_sync");
    } catch (e) {
      error = String(e);
    }
  }
</script>

<section id="nextcloud">
  <h3>Nextcloud</h3>
  <div class="card">
    {#if s.connected}
      <p>{t("Verbunden mit {server} als {user}.", { server: s.server, user: s.display_name || s.login_name })}</p>
      <div class="buttons">
        <button onclick={sync} disabled={s.syncing}><Icon name="history" size={18} /> {s.syncing ? t("Gleicht ab …") : t("Jetzt abgleichen")}</button>
        <button class:warn={confirmDisconnect} onclick={disconnect} onblur={() => (confirmDisconnect = false)}>
          <Icon name="logout" size={18} /> {confirmDisconnect ? t("Wirklich trennen?") : t("Trennen")}
        </button>
      </div>
    {:else if s.login_url}
      <p>{t("Bitte die Anmeldung im Browser bestätigen.")}</p>
      <div class="buttons">
        <button onclick={() => openUrl(s.login_url)}>{t("Browser nochmals öffnen")}</button>
        <button onclick={() => invoke("nextcloud_cancel")}>{t("Abbrechen")}</button>
      </div>
    {:else}
      <p class="small muted">{t("Kontakte aus Nextcloud im Adressbuch und bei Anrufen anzeigen, Aufnahmen und Notizen über Nextcloud teilen. Die Anmeldung läuft im Browser; StarCLX bekommt nur ein App-Passwort.")}</p>
      <form class="row" onsubmit={(e) => { e.preventDefault(); connect(); }}>
        <input type="text" bind:value={server} placeholder="cloud.example.ch" autocomplete="url" />
        <button class="primary" type="submit" disabled={busy || !server.trim()}>{t("Verbinden")}</button>
      </form>
      {#if untrusted}
        <div class="cert">
          <p>{t("Das Zertifikat von {host} ist nicht vertrauenswürdig ({reason}).", { host: untrusted.host, reason: untrusted.reason })}</p>
          <code>{untrusted.fingerprint}</code>
          <div class="buttons">
            <button onclick={trust}>{t("Zertifikat vertrauen")}</button>
            <button onclick={() => (untrusted = null)}>{t("Abbrechen")}</button>
          </div>
        </div>
      {/if}
    {/if}
    {#if error || s.error}<p class="notice">{error || s.error}</p>{/if}
  </div>

  {#if s.connected}
    <h4>{t("Adressbücher")}</h4>
    <div class="card">
      {#each s.books as b (b.href)}
        <Toggle
          checked={b.enabled}
          onchange={(on: boolean) => invoke("nextcloud_set_book", { href: b.href, enabled: on })}
          label={b.enabled ? `${b.name} (${b.count})` : b.name}
        />
      {:else}
        <p class="muted">{s.syncing ? t("Gleicht ab …") : t("Keine Adressbücher gefunden.")}</p>
      {/each}
      <p class="small muted">{t("Eingeschaltete Adressbücher erscheinen im Adressbuch und in der Suche. Kontakte darin lassen sich auch bearbeiten.")}</p>
      <Toggle bind:checked={draft.nextcloud_caller_names} label={t("Namen unbekannter Anrufer aus Nextcloud ergänzen")} />
    </div>

    <h4>{t("Teilen")}</h4>
    <div class="card">
      <label class="field">{t("Ordner in Nextcloud")}
        <input type="text" bind:value={draft.nextcloud_folder} placeholder="StarCLX" />
      </label>
      <p class="small muted">{t("Darin landen die Unterordner Aufnahmen, Notizen und Gesprächsnotizen.")}</p>
      <label class="field">{t("Links laufen ab nach (Tagen, 0 = nie)")}
        <input type="number" min="0" max="365" bind:value={draft.nextcloud_link_days} />
      </label>
      <Toggle bind:checked={draft.nextcloud_link_password} label={t("Links mit Passwort schützen")} />
    </div>
  {/if}
</section>

<style>
  section { padding-top: 0.8rem; }
  h3 { font-size: 1.05rem; margin: 0.6rem 0 0.7rem; }
  h4 { font-size: 0.95rem; margin: 1rem 0 0.5rem; }
  .card { background: var(--panel); border-radius: 4px; padding: 0.7rem 1rem; display: flex; flex-direction: column; gap: 0.4rem; }
  .card p { margin: 0.2rem 0; }
  .small { font-size: 0.85rem; }
  .muted { color: var(--muted); }
  .notice { color: var(--accent-text); }
  .buttons { display: flex; gap: 0.5rem; flex-wrap: wrap; }
  .buttons button { display: flex; align-items: center; gap: 0.35rem; }
  .warn { color: #fff; background: var(--red); border-color: var(--red); }
  .row { display: flex; gap: 0.5rem; max-width: 34rem; }
  .row input, .field input { flex: 1; min-width: 0; padding: 0.35rem 0.5rem; background: var(--panel-2); color: inherit; border: 1px solid var(--line); border-radius: 4px; }
  .field { display: flex; flex-direction: column; gap: 0.3rem; max-width: 34rem; }
  .field input[type="number"] { max-width: 6rem; }
  .primary { background: var(--accent); border-color: var(--accent); color: var(--on-accent); font-weight: 600; }
  .cert { display: flex; flex-direction: column; gap: 0.3rem; margin-top: 0.4rem; }
  .cert code { word-break: break-all; font-size: 0.8rem; }
</style>
