<script lang="ts">
  import { connection, initConnection } from "#lib/connection.svelte.ts";
  // Schnellwahl aus dem Tray: Rufnummernfeld mit Adressbuchsuche und nur die
  // Besetztlampenfelder und Direktwahltasten, darunter die Favoriten. Klick ruft an bzw. holt einen klingelnden Anruf heran.
  // Laufende Gespräche stehen oben und lassen sich auf ein Feld ziehen, um zu
  // vermitteln (Ziehen zwischen Fenstern geht nicht, daher hier selbst).
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import DialSearch from "#lib/DialSearch.svelte";
  import CallCard from "#lib/plugins/call/CallCard.svelte";
  import FkeyTile from "#lib/plugins/fkeys/FkeyTile.svelte";
  import Favorites from "#lib/plugins/favorites/Favorites.svelte";
  import { favorites, loadFavorites, resetFavorites } from "#lib/plugins/favorites/favorites-state.svelte.ts";
  import { callDrag, fkeys, isTransferKey, loadFkeys, press, resetFkeys } from "#lib/plugins/fkeys/fkeys.svelte.ts";
  import { initPhone, phone } from "#lib/plugins/call/phone.svelte.ts";
  import { loadPrefs, prefs } from "#lib/prefs.svelte.ts";
  import { t } from "#lib/i18n.svelte.ts";

  // Inhalt nach Einstellung: Funktionstasten, Favoriten oder beides
  const content = $derived(prefs.value?.quick_content ?? "");
  const showFkeys = $derived(content !== "favorites");
  const showFavorites = $derived(content !== "fkeys");
  const blfs = $derived(showFkeys ? fkeys.keys.filter(isTransferKey) : []);

  onMount(() => {
    initPhone();
    initConnection();
    loadPrefs().catch(() => {});
    loadFkeys();
    // Ohne diesen Aufruf kämen die Favoriten erst beim nächsten Einblenden:
    // Die Favoriten-Ansicht lädt sie selbst, erscheint aber erst mit Inhalt.
    loadFavorites();
    // Beim Einblenden neu laden; so gilt auch eine inzwischen geänderte Sprache.
    const reload = () => {
      if (document.visibilityState !== "visible") return;
      loadFkeys();
      loadFavorites();
      loadPrefs().catch(() => {});
    };
    document.addEventListener("visibilitychange", reload);
    // Kontowechsel: Tasten des alten Kontos weg, die des neuen laden
    const reset = () => { resetFkeys(); resetFavorites(); };
    const offs = [listen("switching", reset), listen("logged-out", reset), listen("session", () => { loadFkeys(); loadFavorites(); }), listen("prefs-saved", () => loadPrefs().catch(() => {}))];
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

  // Angedockt bekommt die Leiste beim Ausfahren keinen Fokus; ein Klick
  // hinein holt ihn, damit man ins Suchfeld tippen kann.
  function focusDock() {
    if (!document.hasFocus()) invoke("quick_dock_focus");
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") invoke("quick_hide");
  }
</script>

<svelte:window {onkeydown} />

<main class="quick" onpointerenter={enter} onpointerleave={leave} onpointerdown={focusDock}>
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
      {#if showFkeys && !(showFavorites && favorites.list.length)}
        <p class="muted">{fkeys.error || (fkeys.loaded ? t("Keine Besetztlampenfelder eingerichtet.") : t("Lade …"))}</p>
      {:else if !showFkeys && favorites.loaded && !favorites.list.length}
        <p class="muted">{t("Keine Favoriten vorhanden.")}</p>
      {/if}
    {/each}
    {#if showFavorites && favorites.list.length}<Favorites compact />{/if}
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
