<script lang="ts">
  // Minimieren, Maximieren und Schliessen, wenn das Fenster ohne Rahmen des
  // Desktops läuft (Einstellung "Titelleiste des Systems verwenden" aus).
  // Schliessen versteckt das Fenster wie bisher nur in den Tray.
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { onMount } from "svelte";
  import Icon from "$lib/Icon.svelte";
  import { t } from "$lib/i18n.svelte";

  const win = getCurrentWindow();
  let maximized = $state(false);

  onMount(() => {
    const update = () => win.isMaximized().then((m) => (maximized = m)).catch(() => {});
    update();
    const off = win.onResized(update);
    return () => void off.then((f) => f());
  });
</script>

<div class="winbtns">
  <button title={t("Minimieren")} aria-label={t("Minimieren")} onclick={() => win.minimize()}><Icon name="minimize" size={16} /></button>
  <button title={maximized ? t("Wiederherstellen") : t("Maximieren")} aria-label={maximized ? t("Wiederherstellen") : t("Maximieren")} onclick={() => win.toggleMaximize()}>
    <Icon name={maximized ? "unmaximize" : "maximize"} size={16} />
  </button>
  <button class="close" title={t("Schliessen")} aria-label={t("Schliessen")} onclick={() => win.close()}><Icon name="close" size={16} /></button>
</div>

<style>
  .winbtns { display: flex; align-items: center; gap: 0.25rem; flex: none; }
  .winbtns button {
    display: grid; place-items: center; width: 1.9rem; height: 1.9rem; padding: 0;
    border: none; border-radius: 50%; background: var(--bar-2); color: var(--muted);
  }
  .winbtns button:hover { color: var(--text); background: var(--panel-2); }
  .winbtns .close:hover { color: #fff; background: var(--red); }
</style>
