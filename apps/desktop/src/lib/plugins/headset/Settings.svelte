<script lang="ts">
  // Plugin Headset: Einstellungen und Test. Tasten und LEDs steuert Rust
  // (src-tauri/src/plugins/headset).
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import Icon from "../../Icon.svelte";
  import Toggle from "../../Toggle.svelte";
  import type { Prefs } from "../../prefs.svelte";
  import { t } from "../../i18n.svelte";

  let { draft = $bindable() }: { draft: Prefs } = $props();

  let headset = $state<{ devices: string[]; bluetooth: string[]; error: string | null }>({ devices: [], bluetooth: [], error: null });
  let testing = $state(false);

  onMount(() => {
    invoke<typeof headset>("headset_info").then((h) => (headset = h), () => {});
  });

  async function test() {
    testing = true;
    try {
      await invoke("headset_test");
    } finally {
      testing = false;
    }
  }
</script>

<section id="headset">
  <h3>{t("Headset-Tasten")}</h3>
  <div class="card">
    <Toggle bind:checked={draft.headset} label={t("Tasten am Headset verwenden (Jabra, Poly, EPOS, Bluetooth)")} />
    <p class="small muted">{t("Gesprächstaste: annehmen und auflegen · Stummtaste: Mikrofon stumm · Das Headset klingelt bei Anrufen und zeigt Gespräch und Stummschaltung an.")}</p>
    <p class="small muted">{t("Telefonietasten, Klingeln und LEDs gehen nur über USB: per Kabel oder mit dem Dongle des Herstellers (z. B. Jabra Link). Direkt per Bluetooth verbundene Headsets: Nur die Play/Pause-Taste nimmt an und legt auf, und auch das nur, wenn das Headset sie sendet.")}</p>
    <label class="field">
      <span>{t("Play/Pause als Anruftaste")}</span>
      <select bind:value={draft.play_pause_call} disabled={!draft.headset}>
        <option value="auto">{t("Nur ohne Headset mit Telefonietasten")}</option>
        <option value="always">{t("Immer")}</option>
        <option value="never">{t("Nie")}</option>
      </select>
    </label>
    <p class="small muted">{t("Manche Bluetooth-Headsets (z. B. Jabra Evolve 65 mit bt-headset-hid) senden für die Gesprächstaste beides, Telefonietaste und Play/Pause. Dann nimmt der eine Druck an und der andere legt gleich wieder auf; deshalb zählt Play/Pause standardmässig nur, wenn kein Headset mit Telefonietasten angeschlossen ist.")}</p>
    <p class="muted">
      {#if headset.devices.length}
        {t("Angeschlossen:")} {headset.devices.join(", ")}
      {/if}
      {#if headset.bluetooth.length}
        {#if headset.devices.length}<br />{/if}
        {t("Bluetooth (Play/Pause):")} {headset.bluetooth.join(", ")}
      {/if}
      {#if !headset.devices.length && !headset.bluetooth.length}
        {t("Kein Headset mit Telefonietasten angeschlossen.")}
      {/if}
    </p>
    {#if headset.error}<p class="notice">{headset.error}</p>{/if}
    <!-- Selbst klingeln können nur USB-Headsets; Bluetooth klingelt über den Klingelton -->
    {#if headset.devices.length || !headset.bluetooth.length}
      <button class="play" onclick={test} disabled={testing || !draft.headset || !headset.devices.length}>
        <Icon name="headset" size={18} /> {testing ? t("Teste …") : t("Klingeln testen")}
      </button>
    {:else}
      <p class="small muted">{t("Bluetooth-Headsets klingeln nicht selbst, sondern über den Klingelton. Unter „Klingeltöne“ lassen sie sich als Klingelgerät wählen und testen.")}</p>
    {/if}
  </div>
</section>

<style>
  section { padding-top: 0.8rem; }
  h3 { font-size: 1.05rem; margin: 0.6rem 0 0.7rem; }
  .card { background: var(--panel); border-radius: 4px; padding: 0.7rem 1rem; display: flex; flex-direction: column; gap: 0.2rem; }
  .small { font-size: 0.85rem; margin: -0.3rem 0 0.6rem; }
  .muted { color: var(--muted); margin: 0.3rem 0; }
  .notice { color: var(--accent-text); margin: 0; flex: 1; }
  .field { display: flex; flex-direction: column; gap: 0.3rem; margin-top: 0.4rem; }
  .field select { padding: 0.35rem 0.5rem; background: var(--panel-2); color: inherit; border: 1px solid var(--line); border-radius: 4px; max-width: 22rem; }
  .play { align-self: flex-start; display: flex; align-items: center; gap: 0.4rem; margin-top: 0.4rem; }
</style>
