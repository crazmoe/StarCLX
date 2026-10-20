<script lang="ts" module>
  import { t } from "./i18n.svelte";

  export type Device = { name: string; description: string };
  export const DEFAULT = "@default";

  /** Bluetooth-Geräte heissen bei PipeWire `bluez_output.<Adresse>.<Nr>`;
   *  die Nummer wechselt mit dem Profil (Musik bzw. Headset). */
  function bluetoothKey(name: string) {
    const m = /^(bluez_(?:output|input)\.[^.]+)/.exec(name);
    return m ? m[1] : null;
  }

  /** USB-Geräte heissen `alsa_output.usb-<Hersteller>_<Gerät>_<Serie>-00.<Profil>` */
  function usbKey(name: string) {
    const m = /^(alsa_(?:output|input)\.usb-[^.]+)/.exec(name);
    return m ? m[1] : null;
  }

  const deviceKey = (name: string) => bluetoothKey(name) ?? usbKey(name);

  /** Derselbe Knoten, oder dasselbe Gerät in einem anderen Profil */
  export function sameDevice(a: string, b: string) {
    const k = deviceKey(a);
    return a === b || (k !== null && k === deviceKey(b));
  }

  /** `alsa_output.usb-0b0e_Jabra_Link_380_08C8C2294988-00` → „Jabra Link 380“ */
  function usbName(key: string) {
    const parts = key.replace(/^alsa_(?:output|input)\.usb-/, "").split("_");
    return parts.length > 2 ? parts.slice(1, -1).join(" ") : null;
  }

  /** Das vorhandene Gerät zu einem gespeicherten Namen */
  export const findDevice = (name: string, devices: Device[]) =>
    devices.find((d) => d.name === name) ?? devices.find((d) => sameDevice(d.name, name));

  /** Gespeicherte Reihenfolge plus neu gefundene Geräte am Ende */
  export function mergeOrder(order: string[], devices: Device[]) {
    const out = order.length ? [...order] : [DEFAULT];
    if (!out.includes(DEFAULT)) out.unshift(DEFAULT);
    for (const d of devices) if (!out.some((n) => sameDevice(n, d.name))) out.push(d.name);
    return out;
  }

  // Namen von Bluetooth- und USB-Geräten, die schon einmal verbunden waren
  const KNOWN = "starclx.bluetooth-names";
  function known(): Record<string, string> {
    try {
      return JSON.parse(localStorage.getItem(KNOWN) ?? "{}");
    } catch {
      return {};
    }
  }
  export function rememberNames(devices: Device[]) {
    const names = known();
    let changed = false;
    for (const d of devices) {
      const k = deviceKey(d.name);
      if (k && d.description && names[k] !== d.description) {
        names[k] = d.description;
        changed = true;
      }
    }
    if (!changed) return;
    try {
      localStorage.setItem(KNOWN, JSON.stringify(names));
    } catch {
      // ohne Speicher bleibt es bei der Adresse
    }
  }

  /** Name für ein Gerät, das gerade fehlt: der zuletzt gesehene, bei
   *  Bluetooth sonst der aus dem System (gekoppelte Geräte), bei USB der
   *  Gerätename aus dem Knotennamen */
  function absentLabel(name: string, system: Record<string, string>) {
    const bt = bluetoothKey(name);
    if (bt) return system[name] ?? known()[bt] ?? t("Bluetooth-Gerät");
    const usb = usbKey(name);
    if (usb) return known()[usb] ?? usbName(usb) ?? t("USB-Gerät");
    return name;
  }
</script>

<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import Icon from "./Icon.svelte";

  let { order = $bindable([]), devices }: { order: string[]; devices: Device[] } = $props();
  let selected = $state(0);
  $effect(() => rememberNames(devices));

  // Namen fehlender Bluetooth-Geräte vom System
  let systemNames = $state<Record<string, string>>({});
  $effect(() => {
    const missing = order.filter((n) => bluetoothKey(n) && !findDevice(n, devices));
    if (!missing.length) return;
    invoke<Record<string, string>>("bluetooth_names", { devices: missing }).then(
      (names) => (systemNames = { ...systemNames, ...names }),
      () => {},
    );
  });

  const label = (name: string) =>
    name === DEFAULT ? t("Systemstandard") : findDevice(name, devices)?.description || absentLabel(name, systemNames);
  const present = (name: string) => name === DEFAULT || !!findDevice(name, devices);
  // Das Gerät, das tatsächlich benutzt wird
  const active = $derived(order.find(present));

  function move(delta: number) {
    const to = selected + delta;
    if (to < 0 || to >= order.length) return;
    const next = [...order];
    [next[selected], next[to]] = [next[to], next[selected]];
    order = next;
    selected = to;
  }
  function remove() {
    if (order[selected] === DEFAULT) return;
    order = order.filter((_, i) => i !== selected);
    selected = Math.min(selected, order.length - 1);
  }
</script>

<div class="list" role="listbox">
  {#each order as name, i (name)}
    <button class="row" class:sel={i === selected} class:absent={!present(name)} onclick={() => (selected = i)} role="option" aria-selected={i === selected}>
      <span>{label(name)}</span>
      {#if !present(name)}<small>{t("nicht verbunden")}</small>{/if}
      {#if name === active}<small class="act">{t("wird verwendet")}</small>{/if}
    </button>
  {/each}
</div>
<div class="acts">
  <button onclick={remove} disabled={order[selected] === DEFAULT}>{t("Löschen")}</button>
  <span class="spacer"></span>
  <button onclick={() => move(1)} disabled={selected >= order.length - 1}><Icon name="down" size={18} /> {t("Nach unten")}</button>
  <button onclick={() => move(-1)} disabled={selected <= 0}><Icon name="up" size={18} /> {t("Nach oben")}</button>
</div>

<style>
  .list { display: flex; flex-direction: column; gap: 2px; margin: 0.4rem 0; }
  .row { display: flex; align-items: center; gap: 0.6rem; text-align: left; border-radius: 0; border: 1px solid transparent; background: var(--panel-2); padding: 0.55rem 0.8rem; }
  .row.absent { background: var(--bar-2); color: var(--muted); }
  .row.sel { border-color: var(--accent); background: #1c1e21; }
  small { font-size: 0.78rem; color: var(--muted); }
  .act { margin-left: auto; color: var(--green); }
  .acts { display: flex; gap: 0.5rem; align-items: center; }
  .acts button { display: flex; align-items: center; gap: 0.35rem; }
  .spacer { flex: 1; }
</style>
