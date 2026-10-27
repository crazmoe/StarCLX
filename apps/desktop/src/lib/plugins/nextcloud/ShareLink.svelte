<script lang="ts">
  // Lädt eine Datei in Nextcloud hoch und zeigt den Link zum Weitergeben.
  import { onMount } from "svelte";
  import { portal } from "../../portal";
  import Icon from "../../Icon.svelte";
  import type { Shared } from "./nextcloud.svelte";
  import { locale, t } from "../../i18n.svelte";

  let { title, run, onclose }: { title: string; run: () => Promise<Shared>; onclose: () => void } = $props();

  let result = $state<Shared | null>(null);
  let error = $state("");
  let copied = $state("");

  onMount(() => {
    run().then((r) => (result = r), (e) => (error = String(e)));
  });

  async function copy(text: string, what: string, input: HTMLInputElement | undefined) {
    try {
      await navigator.clipboard.writeText(text);
      copied = what;
    } catch {
      // Ohne Zugriff auf die Zwischenablage: markieren, Strg+C reicht
      input?.select();
    }
  }

  let linkInput = $state<HTMLInputElement>();
  let pwInput = $state<HTMLInputElement>();
  const expires = $derived(result?.expires ? new Date(result.expires).toLocaleDateString(locale(), { dateStyle: "medium" }) : "");
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onclose()} />

<div class="layer" use:portal>
<div class="scrim" role="presentation" onclick={onclose}></div>
<div class="dialog" role="dialog" aria-label={title}>
  <h4><Icon name="cloud" size={20} /> {title}</h4>
  {#if error}
    <p class="notice">{error}</p>
  {:else if !result}
    <p class="muted">{t("Wird in Nextcloud abgelegt …")}</p>
  {:else}
    <p class="muted small">{t("Abgelegt unter {path}", { path: result.path })}</p>
    {#if result.url}
      <label class="field">{t("Link")}
        <span class="row">
          <input readonly value={result.url} bind:this={linkInput} onfocus={(e) => e.currentTarget.select()} />
          <button class="icon" title={t("Kopieren")} onclick={() => copy(result!.url, "link", linkInput)}><Icon name={copied === "link" ? "check" : "copy"} size={18} /></button>
        </span>
      </label>
      {#if result.password}
        <label class="field">{t("Passwort")}
          <span class="row">
            <input readonly value={result.password} bind:this={pwInput} onfocus={(e) => e.currentTarget.select()} />
            <button class="icon" title={t("Kopieren")} onclick={() => copy(result!.password, "pw", pwInput)}><Icon name={copied === "pw" ? "check" : "copy"} size={18} /></button>
          </span>
        </label>
      {/if}
      <p class="muted small">{expires ? t("Der Link gilt bis {date}.", { date: expires }) : t("Der Link läuft nicht ab.")}</p>
    {/if}
  {/if}
  <div class="actions">
    <span class="spacer"></span>
    <button class="primary" onclick={onclose}>{result || error ? t("Schliessen") : t("Abbrechen")}</button>
  </div>
</div>
</div>

<style>
  .scrim { position: fixed; inset: 0; background: #0007; z-index: 50; }
  .dialog {
    position: fixed; z-index: 51; left: 50%; top: 50%; transform: translate(-50%, -50%); width: min(30rem, 92vw);
    background: var(--panel); border: 1px solid var(--line); border-radius: 8px; padding: 1rem; display: flex; flex-direction: column; gap: 0.6rem;
  }
  h4 { margin: 0; display: flex; align-items: center; gap: 0.4rem; }
  .field { display: flex; flex-direction: column; gap: 0.25rem; color: var(--muted); font-size: 0.9rem; }
  .row { display: flex; gap: 0.4rem; align-items: center; }
  .row input { flex: 1; min-width: 0; color: var(--text); }
  .icon { width: 2rem; height: 2rem; padding: 0; display: grid; place-items: center; border-radius: 50%; }
  .small { font-size: 0.85rem; }
  .actions { display: flex; gap: 0.6rem; }
  .spacer { flex: 1; }
  .primary { background: var(--accent); border-color: var(--accent); color: var(--on-accent); font-weight: 600; }
  .muted { color: var(--muted); margin: 0; word-break: break-all; }
  .notice { color: var(--accent-text); margin: 0; }
</style>
