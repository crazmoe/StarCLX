<script lang="ts">
  import { connection, initConnection } from "#lib/connection.svelte.ts";
  // Schnellwahl aus dem Tray: Rufnummernfeld mit Adressbuchsuche und nur die
  // Besetztlampenfelder und Direktwahltasten. Klick ruft an bzw. holt einen klingelnden Anruf heran.
  // Laufende Gespräche stehen oben und lassen sich auf ein Feld ziehen, um zu
  // vermitteln (Ziehen zwischen Fenstern geht nicht, daher hier selbst).
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import DialSearch from "#lib/DialSearch.svelte";
  import CallCard from "#lib/plugins/call/CallCard.svelte";
  import FkeyTile from "#lib/plugins/fkeys/FkeyTile.svelte";
  import { callDrag, fkeys, isTransferKey, loadFkeys, press, resetFkeys } from "#lib/plugins/fkeys/fkeys.svelte.ts";
  import { initPhone, phone } from "#lib/plugins/call/phone.svelte.ts";
  import { loadPrefs } from "#lib/prefs.svelte.ts";
  import { t } from "#lib/i18n.svelte.ts";

  const blfs = $derived(fkeys.keys.filter(isTransferKey));

  onMount(() => {
    initPhone();
    initConnection();
    loadPrefs().catch(() => {});
    loadFkeys();
    // Beim Einblenden neu laden; so gilt auch eine inzwischen geänderte Sprache.
    const reload = () => {
      if (document.visibilityState !== "visible") return;
      loadFkeys();
      loadPrefs().catch(() => {});
    };
    document.addEventListener("visibilitychange", reload);
    // Kontowechsel: Tasten des alten Kontos weg, die des neuen laden
    const offs = [listen("switching", resetFkeys), listen("logged-out", resetFkeys), listen("session", () => loadFkeys())];
    return () => {
      document.removeEventListener("visibilitychange", reload);
      offs.forEach((p) => p.then((off) => off()));
    };
  });

  // Eingefahrene Leiste am Rand: Maus drauf fährt aus, Maus weg wieder ein
  // (nicht während ein Gespräch gezogen wird). Ob die Leiste angedockt und
  // eingefahren ist, weiss nur das Programm; sonst bleibt der Aufruf wirkungslos.
  let collapse: ReturnType<typeof setTimeout> | undefined;
  function enter() {
    clearTimeout(collapse);
    invoke("quick_dock_expand", { open: true });
  }
  function leave() {
    clearTimeout(collapse);
    collapse = setTimeout(() => {
      if (!callDrag.call) invoke("quick_dock_expand", { open: false });
    }, 700);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") invoke("quick_hide");
  }
</script>

<svelte:window {onkeydown} />

<main class="quick" onpointerenter={enter} onpointerleave={leave}>
  <DialSearch />
  {#if !connection.online}<p class="notice">{t("Keine Verbindung zur Anlage")}</p>
  {:else if phone.notice}<p class="notice">{phone.notice}</p>{/if}
  {#if phone.status.calls.length}
    <div class="calls">
      {#each phone.status.calls as call (call.id)}<CallCard {call} />{/each}
    </div>
  {/if}
  {#if fkeys.notice && connection.online}<p class="notice">{fkeys.notice}</p>{/if}
  <div class="list">
    {#each blfs as k (k.id)}
      <FkeyTile key={k} onclick={() => press(k)} />
    {:else}
      <p class="muted">{fkeys.error || (fkeys.loaded ? t("Keine Besetztlampenfelder eingerichtet.") : t("Lade …"))}</p>
    {/each}
  </div>
</main>

<style>
  :global(body) { margin: 0; background: var(--bg); }
  .quick { height: 100vh; box-sizing: border-box; padding: 0.7rem; display: flex; flex-direction: column; gap: 0.6rem; }
  .calls { display: flex; flex-direction: column; gap: 0.4rem; max-height: 50vh; overflow: auto; }
  .list { flex: 1; overflow: auto; display: flex; flex-direction: column; gap: 0.4rem; }
  .muted { color: var(--muted); }
  .notice { color: var(--accent-text); margin: 0; font-size: 0.85rem; }
</style>
