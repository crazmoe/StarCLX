<script lang="ts">
  // Ansichten des freien Arbeitsbereichs als Leiste links. Ein Klick wählt
  // eine Ansicht, „+“ legt eine neue an. Im Bearbeiten-Modus öffnet ein Klick
  // auf die gewählte Ansicht Name, Symbol und Löschen; Ziehen ändert die
  // Reihenfolge (Zeigerereignisse, HTML5-Drag&Drop geht im Linux-Fenster nicht).
  import type { View } from "./prefs.svelte";
  import Icon, { type IconName } from "./Icon.svelte";
  import { t } from "./i18n.svelte";

  let {
    views = $bindable(),
    current,
    editing = false,
    badge,
    onselect,
    onadd,
  }: {
    views: View[];
    current: string;
    editing?: boolean;
    badge: (v: View) => number;
    onselect: (id: string) => void;
    onadd: () => void;
  } = $props();

  const ICONS: IconName[] = ["workspace", "overview", "call", "chat", "contacts", "list", "voicemail", "groups", "person", "headset", "meetings", "bell", "folder", "settings", "videocam", "light"];

  /** Ansicht, deren Name und Symbol gerade offen sind */
  let open = $state<string | null>(null);
  $effect(() => {
    if (!editing) open = null;
  });

  const label = (v: View) => v.name || t("Übersicht");

  let drag: { id: string; sy: number; moved: boolean } | null = null;
  /** Nach dem Ziehen kommt noch ein Klick; der wählt nichts */
  let dragged = false;

  function down(e: PointerEvent, v: View) {
    if (!editing || e.button !== 0) return;
    drag = { id: v.id, sy: e.clientY, moved: false };
  }

  function move(e: PointerEvent) {
    if (!drag) return;
    if (!drag.moved && Math.abs(e.clientY - drag.sy) < 6) return;
    drag.moved = true;
    open = null;
    const over = (document.elementFromPoint(e.clientX, e.clientY) as HTMLElement | null)?.closest<HTMLElement>("[data-view]")?.dataset.view;
    const from = views.findIndex((v) => v.id === drag?.id);
    const to = views.findIndex((v) => v.id === over);
    if (from < 0 || to < 0 || from === to) return;
    const [v] = views.splice(from, 1);
    views.splice(to, 0, v);
  }

  function up() {
    dragged = drag?.moved ?? false;
    drag = null;
  }

  function click(v: View) {
    if (dragged) return void (dragged = false);
    if (editing && v.id === current) open = open === v.id ? null : v.id;
    else {
      open = null;
      onselect(v.id);
    }
  }

  function remove(v: View) {
    const i = views.findIndex((x) => x.id === v.id);
    if (i < 0 || views.length < 2) return;
    views.splice(i, 1);
    open = null;
    onselect(views[Math.max(0, i - 1)].id);
  }
</script>

<svelte:window onpointermove={move} onpointerup={up} onpointercancel={up} />

<nav class="rail" class:editing aria-label={t("Ansichten")}>
  {#each views as v (v.id)}
    <div class="item" data-view={v.id}>
      <button
        class="view"
        class:current={v.id === current}
        title={editing && v.id === current ? t("Name und Symbol ändern") : label(v)}
        onpointerdown={(e) => down(e, v)}
        onclick={() => click(v)}
      >
        <Icon name={(ICONS.includes(v.icon as IconName) ? v.icon : "workspace") as IconName} size={22} />
        <span>{label(v)}</span>
        {#if badge(v)}<b class="unread">{badge(v)}</b>{/if}
        {#if editing && v.id === current}<span class="pen"><Icon name="edit" size={12} /></span>{/if}
      </button>
      {#if open === v.id}
        <div class="pop" role="dialog" aria-label={t("Ansicht bearbeiten")}>
          <label class="field">
            <span>{t("Name")}</span>
            <!-- svelte-ignore a11y_autofocus -->
            <input bind:value={v.name} placeholder={t("Übersicht")} maxlength="30" autofocus onkeydown={(e) => e.key === "Enter" && (open = null)} />
          </label>
          <div class="icons">
            {#each ICONS as ic}
              <button class="ic" class:on={v.icon === ic} onclick={() => (v.icon = ic)} aria-label={ic}><Icon name={ic} size={20} /></button>
            {/each}
          </div>
          <div class="row">
            {#if views.length > 1}
              <button class="del" onclick={() => remove(v)}><Icon name="trash" size={18} /> {t("Ansicht löschen")}</button>
            {/if}
            <span class="spacer"></span>
            <button onclick={() => (open = null)}><Icon name="check" size={18} /></button>
          </div>
        </div>
      {/if}
    </div>
  {/each}
  <button class="view add" title={t("Neue Ansicht")} onclick={onadd}><Icon name="plus" size={22} /></button>
</nav>

<style>
  .rail {
    display: flex; flex-direction: column; gap: 0.2rem; padding: 0.4rem 0.3rem; width: 4.6rem; flex: none;
    background: var(--bar); border-right: 1px solid var(--bar-2); overflow-y: auto; overflow-x: visible; scrollbar-width: none;
  }
  .item { position: relative; }
  .view {
    position: relative; width: 100%; display: flex; flex-direction: column; align-items: center; gap: 0.15rem;
    padding: 0.45rem 0.2rem; background: none; border: none; border-left: 3px solid transparent; border-radius: 6px; color: var(--muted);
  }
  .view span:not(.pen) { font-size: 0.72rem; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .view.current { color: var(--text); background: var(--bar-2); border-left-color: var(--accent); }
  .editing .view:not(.add) { cursor: grab; touch-action: none; }
  .view.add { color: var(--accent-text); }
  .unread {
    position: absolute; top: 0.15rem; right: 0.2rem; background: var(--accent); color: var(--on-accent);
    border-radius: 999px; padding: 0 0.35rem; font-size: 0.7rem;
  }
  .pen { position: absolute; top: 0.2rem; left: 0.3rem; color: var(--accent-text); }
  .pop {
    position: fixed; margin-left: 4.4rem; margin-top: -3.2rem; z-index: 30; width: 15rem; padding: 0.7rem;
    display: flex; flex-direction: column; gap: 0.6rem;
    background: var(--panel); border: 1px solid var(--bar-2); border-radius: 8px; box-shadow: 0 4px 16px rgba(0, 0, 0, 0.35);
  }
  .field { display: flex; flex-direction: column; gap: 0.25rem; font-size: 0.85rem; }
  .icons { display: grid; grid-template-columns: repeat(8, 1fr); gap: 0.2rem; }
  .ic { display: grid; place-items: center; padding: 0.25rem; background: none; border: 1px solid transparent; border-radius: 6px; color: var(--muted); }
  .ic.on { color: var(--accent-text); border-color: var(--accent); }
  .row { display: flex; align-items: center; gap: 0.4rem; }
  .spacer { flex: 1; }
  .del { display: flex; align-items: center; gap: 0.3rem; color: var(--red); }
</style>
