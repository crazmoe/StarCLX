<script lang="ts">
  import { connection } from "../../connection.svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import Icon from "../../Icon.svelte";
  import { run, canDial } from "../call/phone.svelte";
  import { loadRecordings, mimeOf, recordings, type Recording } from "./recordings-state.svelte";
  import { locale, t } from "../../i18n.svelte";
  import ShareLink from "../nextcloud/ShareLink.svelte";
  import { nextcloud, shareRecording, stamp } from "../nextcloud/nextcloud.svelte";

  let term = $state("");
  let notice = $state("");
  let playing = $state<string | null>(null);
  let loading = $state<string | null>(null);
  let confirmDelete = $state<string | null>(null);
  let audio: HTMLAudioElement | null = null;
  let url = "";

  const ready = $derived(canDial());
  const shown = $derived.by(() => {
    const q = term.trim().toLowerCase();
    if (!q) return recordings.list;
    return recordings.list.filter((r) => [r.caller, r.callee, r.number].some((s) => s.toLowerCase().includes(q)));
  });

  onMount(() => {
    loadRecordings();
    return stop;
  });

  function stop() {
    audio?.pause();
    audio = null;
    if (url) URL.revokeObjectURL(url);
    url = "";
    playing = null;
  }

  async function play(r: Recording) {
    if (playing === r.id) return stop();
    stop();
    loading = r.id;
    notice = "";
    try {
      const data = await invoke<ArrayBuffer>("recording_audio", { id: r.id });
      url = URL.createObjectURL(new Blob([data], { type: mimeOf(data) }));
      audio = new Audio(url);
      audio.onended = stop;
      await audio.play();
      playing = r.id;
    } catch (e) {
      notice = t("Abspielen fehlgeschlagen: {e}", { e: String(e) });
      stop();
    } finally {
      loading = null;
    }
  }

  async function save(r: Recording) {
    const name = fileName(r);
    notice = "";
    try {
      if (await invoke<boolean>("recording_save", { id: r.id, name })) notice = t("Gespeichert.");
    } catch (e) {
      notice = String(e);
    }
  }

  /** In Nextcloud teilen: Datei samt Link */
  let sharing = $state<Recording | null>(null);
  /** Dateiname beim Speichern und Teilen */
  const fileName = (r: Recording) =>
    `${t("Aufzeichnung")}_${(r.caller || r.number || t("Unbekannt")).replace(/[^\p{L}\d+-]+/gu, "_")}_${stamp(r.start)}`;

  async function remove(r: Recording) {
    notice = "";
    if (playing === r.id) stop();
    try {
      await invoke("recording_delete", { ids: [r.id] });
      await loadRecordings();
    } catch (e) {
      notice = String(e);
    }
  }

  function when(ms: number) {
    const d = new Date(ms);
    const today = new Date();
    const time = d.toLocaleTimeString(locale(), { hour: "2-digit", minute: "2-digit" });
    return d.toDateString() === today.toDateString() ? time : `${d.toLocaleDateString(locale())} ${time}`;
  }

  const dur = (s: number) => `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
</script>

<div class="rec">
  {#if recordings.disabled}
  <p class="muted">{t("Für diesen Benutzer sind keine Aufzeichnungen verfügbar.")}</p>
  {:else}
  <div class="bar">
    <span class="count">
      {recordings.total > recordings.list.length
        ? t("Neueste {n} von {total}", { n: String(recordings.list.length), total: String(recordings.total) })
        : t("{n} Aufzeichnungen", { n: String(recordings.list.length) })}
    </span>
    <label class="filter">
      <Icon name="search" size={18} />
      <input bind:value={term} placeholder={t("Name oder Nummer")} />
    </label>
  </div>
  {#if recordings.error && connection.online}<p class="error">{recordings.error}</p>{/if}
  {#if notice && connection.online}<p class="error">{notice}</p>{/if}
  <div class="list">
    {#each shown as r (r.id)}
      <div class="row">
        <button class="playbtn" title={playing === r.id ? t("Stopp") : t("Abspielen")} disabled={loading === r.id} onclick={() => play(r)}>
          <Icon name={playing === r.id ? "pause" : "play"} size={20} />
        </button>
        <div class="who">
          <strong>{r.caller || r.number || t("Unbekannt")}</strong>
          {#if r.callee}<small>{t("an {who}", { who: r.callee })}</small>{/if}
        </div>
        <span class="when">{when(r.start)}<small>{dur(r.duration_secs)}</small></span>
        <div class="acts">
          <button class="icon" title={t("Als Datei speichern")} onclick={() => save(r)}><Icon name="down" size={18} /></button>
          {#if nextcloud.status.connected}
            <button class="icon" title={t("In Nextcloud teilen")} onclick={() => (sharing = r)}><Icon name="cloud" size={18} /></button>
          {/if}
          <button
            class="icon"
            class:warn={confirmDelete === r.id}
            title={confirmDelete === r.id ? t("Nochmals klicken zum Löschen") : t("Löschen")}
            onclick={() => (confirmDelete === r.id ? remove(r) : (confirmDelete = r.id))}
            onblur={() => confirmDelete === r.id && (confirmDelete = null)}
          ><Icon name="trash" size={18} /></button>
          {#if r.number}
            <button class="call" title={t("Zurückrufen")} disabled={!ready} onclick={() => run("phone_dial", { number: r.number })}><Icon name="call" size={18} /></button>
          {/if}
        </div>
      </div>
    {:else}
      <p class="muted">{term.trim() ? t("Keine Treffer.") : t("Keine Aufzeichnungen vorhanden.")}</p>
    {/each}
  </div>
  {/if}
</div>

{#if sharing}
  {@const r = sharing}
  <ShareLink title={t("Aufzeichnung teilen")} run={() => shareRecording(r.id, fileName(r))} onclose={() => (sharing = null)} />
{/if}

<style>
  .rec { display: flex; flex-direction: column; height: 100%; min-height: 0; gap: 0.5rem; }
  .bar { display: flex; align-items: center; gap: 0.3rem; flex-wrap: wrap; min-height: 2.4rem; }
  .count { color: var(--muted); font-size: 0.9rem; padding-left: 0.3rem; }
  .filter { margin-left: auto; min-width: 0; display: flex; align-items: center; gap: 0.4rem; padding: 0 0.6rem; background: var(--bar-2); border: 1px solid var(--line); border-radius: 999px; }
  .filter input { border: none; background: none; outline: none; padding: 0.4rem 0; width: 12rem; min-width: 0; text-overflow: ellipsis; }
  .list { flex: 1; overflow: auto; border-radius: 4px; padding: 0.2rem 0.8rem 0.8rem; }
  .row { display: flex; align-items: center; gap: 0.8rem; padding: 0.45rem 0.3rem; border-top: 1px solid var(--line); }
  .playbtn { width: 2.3rem; height: 2.3rem; padding: 0; border-radius: 50%; display: grid; place-items: center; }
  .who { flex: 1; min-width: 0; display: flex; flex-direction: column; }
  .who strong, .who small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .who small { color: var(--muted); }
  .when { display: flex; flex-direction: column; align-items: flex-end; font-variant-numeric: tabular-nums; }
  .when small { color: var(--muted); font-size: 0.75rem; }
  .acts { display: flex; align-items: center; gap: 0.25rem; }
  .icon { width: 2rem; height: 2rem; padding: 0; display: grid; place-items: center; background: none; border: 1px solid transparent; border-radius: 50%; color: var(--muted); }
  .icon:hover:not(:disabled) { border-color: var(--line); color: var(--text); }
  .icon.warn { color: #fff; background: var(--red); }
  .call { width: 2.1rem; height: 2.1rem; padding: 0; border-radius: 50%; display: grid; place-items: center; background: var(--green); border: none; color: #fff; }
  .call:disabled { opacity: 0.4; }
  .muted { color: var(--muted); }
  .error { color: var(--accent-text); margin: 0; }
</style>
