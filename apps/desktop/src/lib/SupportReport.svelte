<script lang="ts" module>
  export type SupportSent = { id: string; reason: string; time: number; delivered: boolean };
  export type SupportInfo = { host: string; auto: string[]; sent: SupportSent[]; pending: number; first_notice: boolean };
</script>

<script lang="ts">
  // Fehlerberichte an StarDash: nur sichtbar, wenn die Firma den Sammler in
  // der Vorgabedatei eingeschaltet hat.
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import Icon from "./Icon.svelte";
  import { t } from "./i18n.svelte";

  let info = $state<SupportInfo | null>(null);
  let comment = $state("");
  let busy = $state(false);
  let result = $state("");
  let error = $state("");

  const reasons: Record<string, string> = {
    manual: "Von dir gesendet",
    crash: "Absturz",
    sip: "Softphone-Fehler",
    login: "Anmeldefehler",
    wake: "Keine Verbindung nach Standby",
  };

  async function load() {
    info = await invoke<SupportInfo | null>("support_info").catch(() => null);
  }

  async function send() {
    busy = true;
    error = result = "";
    try {
      const sent = await invoke<SupportSent>("support_send", { comment });
      result = sent.delivered
        ? t("Gesendet. Bericht-Nr. {id}: Nenne sie dem Support.", { id: sent.id })
        : t("Der Support ist gerade nicht erreichbar. Bericht {id} wird später gesendet.", { id: sent.id });
      comment = "";
      await load();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  const when = (s: number) => new Date(s * 1000).toLocaleString();

  onMount(load);
</script>

{#if info}
  <hr />
  <p><strong>{t("Fehlerberichte an den Support")}</strong></p>
  <p class="muted">
    {t("Deine Firma sammelt Fehlerberichte dieser App auf {host}. Gesendet werden technische Protokolle ohne Gesprächsinhalte; Telefonnummern, Namen und Chat-Texte werden vorher unkenntlich gemacht.", { host: info.host })}
    {#if info.auto.length}{t("Nach Abstürzen und bestimmten Fehlern geschieht das automatisch.")}{/if}
  </p>
  <label class="field">{t("Was ist passiert?")}
    <textarea rows="3" bind:value={comment} placeholder={t("z. B. Anruf an Kunde brach nach 10 Sekunden ab")}></textarea>
  </label>
  <div class="buttons">
    <button onclick={send} disabled={busy}><Icon name="send" size={18} /> {busy ? t("Sende …") : t("Protokoll an Support senden")}</button>
  </div>
  {#if result}<p class="ok">{result}</p>{/if}
  {#if error}<p class="error">{error}</p>{/if}
  {#if info.sent.length}
    <p class="small muted">{t("Zuletzt gesendet")}</p>
    <ul class="sent">
      {#each info.sent as s (s.id)}
        <li><code>{s.id}</code> {when(s.time)} · {t(reasons[s.reason] ?? s.reason)}{#if !s.delivered} · {t("wartet")}{/if}</li>
      {/each}
    </ul>
  {/if}
{/if}

<style>
  hr { border: none; border-top: 1px solid var(--line); margin: 0.8rem 0 0.4rem; width: 100%; }
  p { margin: 0.3rem 0; }
  .muted { color: var(--muted); }
  .small { font-size: 0.85rem; }
  .field { display: flex; flex-direction: column; gap: 0.3rem; margin-top: 0.6rem; max-width: 34rem; }
  textarea { width: 100%; box-sizing: border-box; resize: vertical; font: inherit; padding: 0.35rem 0.5rem; background: var(--panel-2); color: inherit; border: 1px solid var(--line); border-radius: 4px; }
  .buttons { display: flex; flex-wrap: wrap; gap: 0.6rem; margin: 0.4rem 0 0.6rem; }
  .buttons button { display: flex; align-items: center; gap: 0.4rem; }
  .sent { margin: 0; padding-left: 1.1rem; font-size: 0.85rem; color: var(--muted); }
  .ok { color: var(--green); }
  .error { color: var(--red); }
</style>
