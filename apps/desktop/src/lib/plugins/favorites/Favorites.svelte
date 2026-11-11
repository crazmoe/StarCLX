<script lang="ts">
  // Favoriten wie in den STARFACE-Apps: Kategorien mit Kollegen, Gruppen und
  // Rufnummern, auf der Anlage gespeichert. Mit dem Stift lassen sich
  // Kategorien und Favoriten anlegen, umbenennen, verschieben und löschen.
  // Klick führt den Ablauf des Favoriten aus (Standard: anrufen bzw.
  // heranholen), ⋯ oder Rechtsklick zeigt alle Aktionen.
  import { onMount } from "svelte";
  import FavTile from "./FavTile.svelte";
  import Icon from "../../Icon.svelte";
  import { portal } from "../../portal";
  import { t } from "../../i18n.svelte";
  import { connection } from "../../connection.svelte";
  import type { FlowStep } from "../../prefs.svelte";
  import {
    createCategory, createFavorite, deleteCategory, deleteFavorite, favName, favorites, flowOf, hasOwnFlow, loadFavorites,
    moveCategory, moveFavorite, possible, press, renameCategory, renameFavorite, runStep, saveFlow, searchTargets, stepTypes,
    type Candidate, type Category, type Favorite,
  } from "./favorites-state.svelte";

  /** Schmal (Schnellwahlleiste): eine Spalte, ohne Bearbeiten */
  let { compact = false }: { compact?: boolean } = $props();

  onMount(() => { loadFavorites(); });

  let editing = $state(false);
  /** Eingeklappte Kategorien (nur in diesem Fenster) */
  let folded = $state<Record<string, boolean>>({});

  // ── Menü ──
  let menu = $state<{ fav: Favorite; x: number; y: number } | null>(null);
  function openMenu(fav: Favorite, e: MouseEvent) {
    const w = 260;
    menu = { fav, x: Math.min(e.clientX, window.innerWidth - w - 8), y: Math.min(e.clientY, window.innerHeight - 340) };
  }
  const menuActions = $derived(menu ? stepTypes().filter((s) => s.kind !== "auto" && !s.text && possible(menu!.fav, s.kind)) : []);
  function act(kind: string) {
    const f = menu?.fav;
    menu = null;
    if (f) runStep(f, { kind });
  }

  // ── Ablauf ──
  let flow = $state<{ fav: Favorite; steps: FlowStep[] } | null>(null);
  function editFlow(fav: Favorite) {
    menu = null;
    flow = { fav, steps: structuredClone($state.snapshot(flowOf(fav))) };
  }
  function moveStep(i: number, d: number) {
    if (!flow) return;
    const s = flow.steps;
    const j = i + d;
    if (j < 0 || j >= s.length) return;
    [s[i], s[j]] = [s[j], s[i]];
  }
  async function saveFlowDialog(reset = false) {
    if (!flow) return;
    const steps = reset ? null : flow.steps.filter((s) => s.kind);
    const isDefault = steps?.length === 1 && steps[0].kind === "auto";
    await saveFlow(flow.fav, isDefault ? null : steps);
    flow = null;
  }
  const needsText = (kind: string) => stepTypes().find((s) => s.kind === kind)?.text;

  // ── Kategorie / Favorit umbenennen ──
  let rename = $state<{ kind: "cat" | "fav"; id: string; name: string } | null>(null);
  async function saveRename() {
    if (!rename) return;
    const r = rename;
    rename = null;
    if (r.kind === "cat") await renameCategory(r.id, r.name);
    else await renameFavorite(r.id, r.name);
  }

  // ── Neue Kategorie ──
  let newCat = $state<string | null>(null);
  async function saveNewCat() {
    const name = newCat?.trim();
    newCat = null;
    if (name) await createCategory(name);
  }

  // ── Neuer Favorit ──
  let add = $state<{ category: string; mode: "search" | "number"; term: string; number: string; name: string; hits: Candidate[]; pick: Candidate | null } | null>(null);
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  function search() {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(async () => {
      if (!add) return;
      const term = add.term;
      try {
        const hits = await searchTargets(term);
        if (add && add.term === term) add.hits = hits;
      } catch (e) {
        favorites.notice = String(e);
      }
    }, 250);
  }
  async function saveAdd() {
    if (!add) return;
    const a = add;
    if (a.mode === "number") {
      if (!a.number.trim()) return;
      add = null;
      await createFavorite(a.category, a.name, { kind: "number", number: a.number });
    } else {
      if (!a.pick) return;
      add = null;
      await createFavorite(a.category, a.name, { kind: a.pick.kind, id: a.pick.id });
    }
  }

  async function removeCategory(c: Category) {
    if (!confirm(t("Kategorie „{name}“ mit allen Favoriten löschen?", { name: c.name }))) return;
    await deleteCategory(c.id);
  }
  async function removeFavorite(f: Favorite) {
    if (!confirm(t("Favorit „{name}“ löschen?", { name: favName(f) }))) return;
    await deleteFavorite(f.id);
  }
  /** Favorit in eine andere Kategorie, ans Ende */
  function toCategory(f: Favorite, id: string) {
    const target = favorites.list.find((c) => c.id === id);
    if (target) moveFavorite(f.id, id, target.favorites.length);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") menu = null;
  }
</script>

<svelte:window {onkeydown} />

<div class="favs" class:compact>
  {#if !compact}
    <div class="bar">
      {#if editing}
        <button onclick={() => (newCat = "")}><Icon name="plus" size={16} /> {t("Kategorie")}</button>
      {/if}
      <span class="spacer"></span>
      {#if !favorites.forbidden}
        <button class="icon" class:on={editing} title={editing ? t("Fertig") : t("Favoriten bearbeiten")} aria-label={editing ? t("Fertig") : t("Favoriten bearbeiten")} onclick={() => (editing = !editing)}>
          <Icon name={editing ? "check" : "edit"} size={18} />
        </button>
      {/if}
    </div>
  {/if}
  {#if favorites.error && connection.online}<p class="error">{t(favorites.error)}</p>{/if}
  {#if favorites.notice}<p class="error">{favorites.notice}</p>{/if}
  {#if favorites.forbidden}
    <p class="muted">{t("Dir fehlt auf der Anlage das Recht für Favoriten.")}</p>
  {:else if favorites.loaded && !favorites.list.length}
    <p class="muted">{compact ? t("Keine Favoriten.") : t("Noch keine Favoriten. Mit dem Stift legst du Kategorien und Favoriten an; sie erscheinen auch in den STARFACE-Apps.")}</p>
  {/if}

  {#each favorites.list as c, ci (c.id)}
    <section>
      <header>
        <button class="fold" aria-expanded={!folded[c.id]} onclick={() => (folded[c.id] = !folded[c.id])}>
          <span class="chev" class:closed={folded[c.id]}><Icon name="chevron" size={16} /></span>
          <span class="name">{c.name || t("Favoriten")}</span>
          <span class="count">{c.favorites.length}</span>
        </button>
        {#if editing}
          <button class="icon" title={t("Favorit hinzufügen")} aria-label={t("Favorit hinzufügen")} onclick={() => (add = { category: c.id, mode: "search", term: "", number: "", name: "", hits: [], pick: null })}><Icon name="plus" size={16} /></button>
          <button class="icon" title={t("Umbenennen")} aria-label={t("Umbenennen")} onclick={() => (rename = { kind: "cat", id: c.id, name: c.name })}><Icon name="edit" size={16} /></button>
          <button class="icon" title={t("Nach oben")} aria-label={t("Nach oben")} disabled={ci === 0} onclick={() => moveCategory(c.id, ci - 1)}><Icon name="up" size={16} /></button>
          <button class="icon" title={t("Nach unten")} aria-label={t("Nach unten")} disabled={ci === favorites.list.length - 1} onclick={() => moveCategory(c.id, ci + 1)}><Icon name="down" size={16} /></button>
          <button class="icon" title={t("Löschen")} aria-label={t("Löschen")} onclick={() => removeCategory(c)}><Icon name="trash" size={16} /></button>
        {/if}
      </header>
      {#if !folded[c.id]}
        {#if editing}
          <ul class="edit">
            {#each c.favorites as f, fi (f.id)}
              <li>
                <span class="ename">{favName(f)}{#if f.name && f.target_name && f.name !== f.target_name}<small> · {f.target_name}</small>{/if}<small> · {f.number}</small></span>
                <button class="icon" title={t("Umbenennen")} aria-label={t("Umbenennen")} onclick={() => (rename = { kind: "fav", id: f.id, name: f.name || f.target_name })}><Icon name="edit" size={16} /></button>
                <button class="icon" class:on={hasOwnFlow(f)} title={t("Ablauf beim Klick")} aria-label={t("Ablauf beim Klick")} onclick={() => editFlow(f)}><Icon name="play" size={16} /></button>
                <button class="icon" title={t("Nach oben")} aria-label={t("Nach oben")} disabled={fi === 0} onclick={() => moveFavorite(f.id, null, fi - 1)}><Icon name="up" size={16} /></button>
                <button class="icon" title={t("Nach unten")} aria-label={t("Nach unten")} disabled={fi === c.favorites.length - 1} onclick={() => moveFavorite(f.id, null, fi + 1)}><Icon name="down" size={16} /></button>
                {#if favorites.list.length > 1}
                  <select title={t("In Kategorie verschieben")} aria-label={t("In Kategorie verschieben")} value={c.id} onchange={(e) => toCategory(f, (e.target as HTMLSelectElement).value)}>
                    {#each favorites.list as o (o.id)}<option value={o.id}>{o.name || t("Favoriten")}</option>{/each}
                  </select>
                {/if}
                <button class="icon" title={t("Löschen")} aria-label={t("Löschen")} onclick={() => removeFavorite(f)}><Icon name="trash" size={16} /></button>
              </li>
            {:else}
              <li class="muted">{t("Leer")}</li>
            {/each}
          </ul>
        {:else}
          <div class="grid">
            {#each c.favorites as f (f.id)}
              <FavTile fav={f} onclick={() => press(f)} onmenu={(e) => openMenu(f, e)} />
            {/each}
          </div>
        {/if}
      {/if}
    </section>
  {/each}
</div>

{#if menu}
  {@const m = menu}
  <div class="layer" use:portal>
    <div class="scrim clear" role="presentation" onclick={() => (menu = null)} oncontextmenu={(e) => { e.preventDefault(); menu = null; }}></div>
    <div class="menu" role="menu" style:left="{m.x}px" style:top="{m.y}px">
      <div class="mhead">{favName(m.fav)}{#if m.fav.number}<small>{m.fav.number}</small>{/if}</div>
      {#each menuActions as a (a.kind)}
        <button role="menuitem" onclick={() => act(a.kind)}>{a.label}</button>
      {/each}
      {#if !compact}<button role="menuitem" class="sep" onclick={() => editFlow(m.fav)}>{t("Ablauf beim Klick …")}</button>{/if}
    </div>
  </div>
{/if}

{#if flow}
  {@const fl = flow}
  <div class="layer" use:portal>
    <div class="scrim" role="presentation" onclick={() => (flow = null)}></div>
    <div class="dialog" role="dialog" aria-label={t("Ablauf beim Klick")}>
      <h4>{t("Ablauf beim Klick")}: {favName(fl.fav)}</h4>
      <p class="small muted">{t("Diese Schritte laufen nacheinander ab, wenn du auf den Favoriten klickst. Schritte, die gerade nicht passen (z. B. Rückfrage ohne Gespräch), werden übersprungen. Der Ablauf gilt nur auf diesem Rechner.")}</p>
      <ol class="steps">
        {#each fl.steps as s, i (i)}
          <li>
            <div class="srow">
              <select bind:value={s.kind} aria-label={t("Schritt")}>
                {#each stepTypes() as o (o.kind)}<option value={o.kind}>{o.label}</option>{/each}
              </select>
              <button class="icon" title={t("Nach oben")} aria-label={t("Nach oben")} disabled={i === 0} onclick={() => moveStep(i, -1)}><Icon name="up" size={16} /></button>
              <button class="icon" title={t("Nach unten")} aria-label={t("Nach unten")} disabled={i === fl.steps.length - 1} onclick={() => moveStep(i, 1)}><Icon name="down" size={16} /></button>
              <button class="icon" title={t("Löschen")} aria-label={t("Löschen")} onclick={() => fl.steps.splice(i, 1)}><Icon name="trash" size={16} /></button>
            </div>
            {#if needsText(s.kind)}
              <input type="text" placeholder={needsText(s.kind)} bind:value={s.text} />
            {/if}
          </li>
        {/each}
      </ol>
      <button class="addstep" onclick={() => fl.steps.push({ kind: "call" })}><Icon name="plus" size={16} /> {t("Schritt hinzufügen")}</button>
      <div class="actions">
        <button onclick={() => saveFlowDialog(true)}>{t("Standard")}</button>
        <span class="spacer"></span>
        <button onclick={() => (flow = null)}>{t("Abbrechen")}</button>
        <button class="primary" onclick={() => saveFlowDialog()}>{t("Speichern")}</button>
      </div>
    </div>
  </div>
{/if}

{#if rename || newCat !== null}
  <div class="layer" use:portal>
    <div class="scrim" role="presentation" onclick={() => { rename = null; newCat = null; }}></div>
    <div class="dialog" role="dialog" aria-label={rename ? t("Umbenennen") : t("Neue Kategorie")}>
      <form onsubmit={(e) => { e.preventDefault(); if (rename) saveRename(); else saveNewCat(); }}>
        <h4>{rename ? t("Umbenennen") : t("Neue Kategorie")}</h4>
        {#if rename}
          <!-- svelte-ignore a11y_autofocus -->
          <input type="text" bind:value={rename.name} autofocus />
        {:else}
          <!-- svelte-ignore a11y_autofocus -->
          <input type="text" bind:value={newCat} placeholder={t("Name")} autofocus />
        {/if}
        <div class="actions">
          <span class="spacer"></span>
          <button type="button" onclick={() => { rename = null; newCat = null; }}>{t("Abbrechen")}</button>
          <button type="submit" class="primary">{t("Speichern")}</button>
        </div>
      </form>
    </div>
  </div>
{/if}

{#if add}
  {@const a = add}
  <div class="layer" use:portal>
    <div class="scrim" role="presentation" onclick={() => (add = null)}></div>
    <div class="dialog" role="dialog" aria-label={t("Favorit hinzufügen")}>
      <form onsubmit={(e) => { e.preventDefault(); saveAdd(); }}>
        <h4>{t("Favorit hinzufügen")}</h4>
        <div class="modes">
          <button type="button" class:on={a.mode === "search"} onclick={() => (a.mode = "search")}>{t("Kollege oder Gruppe")}</button>
          <button type="button" class:on={a.mode === "number"} onclick={() => (a.mode = "number")}>{t("Rufnummer")}</button>
        </div>
        {#if a.mode === "search"}
          <!-- svelte-ignore a11y_autofocus -->
          <input type="search" placeholder={t("Name oder Nummer suchen")} bind:value={a.term} oninput={search} autofocus />
          <ul class="hits">
            {#each a.hits as h (h.kind + h.id)}
              <li>
                <button type="button" class:on={a.pick === h} onclick={() => (a.pick = h)}>
                  <Icon name={h.kind === "group" ? "groups" : "person"} size={16} />
                  <span>{h.name}</span><small>{h.number}</small>
                </button>
              </li>
            {:else}
              {#if a.term.trim()}<li class="muted small">{t("Keine Treffer")}</li>{/if}
            {/each}
          </ul>
        {:else}
          <!-- svelte-ignore a11y_autofocus -->
          <input type="text" placeholder={t("Rufnummer")} bind:value={a.number} autofocus />
        {/if}
        <label class="row"><span>{t("Anzeigename")}</span><input type="text" bind:value={a.name} placeholder={a.pick?.name ?? t("optional")} /></label>
        <div class="actions">
          <span class="spacer"></span>
          <button type="button" onclick={() => (add = null)}>{t("Abbrechen")}</button>
          <button type="submit" class="primary" disabled={a.mode === "search" ? !a.pick : !a.number.trim()}>{t("Hinzufügen")}</button>
        </div>
      </form>
    </div>
  </div>
{/if}

<style>
  /* Platz für den Rollbalken, damit er Stift und Haken nicht überdeckt */
  .favs { height: 100%; overflow: auto; display: flex; flex-direction: column; gap: 0.6rem; scrollbar-gutter: stable; padding-right: 0.4rem; box-sizing: border-box; }
  .bar { display: flex; align-items: center; gap: 0.4rem; }
  .bar button { display: inline-flex; align-items: center; gap: 0.3rem; }
  .spacer { flex: 1; }
  .icon { background: none; border: 0; padding: 0.2rem; color: var(--muted); display: inline-grid; place-items: center; }
  .icon:hover:not(:disabled) { color: var(--text); }
  .icon.on { color: var(--accent-text); }
  .icon:disabled { opacity: 0.35; }
  section { display: flex; flex-direction: column; gap: 0.4rem; }
  header { display: flex; align-items: center; gap: 0.2rem; }
  .fold { flex: 1; min-width: 0; display: flex; align-items: center; gap: 0.35rem; background: none; border: 0; padding: 0.1rem 0; text-align: left; color: var(--text); }
  .fold .name { font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .count { color: var(--muted); font-size: 0.8rem; }
  .chev { display: grid; transition: transform 0.15s; color: var(--muted); }
  .chev.closed { transform: rotate(-90deg); }
  .grid { display: grid; gap: 0.5rem; grid-template-columns: repeat(auto-fill, minmax(11rem, 1fr)); }
  .compact { height: auto; overflow: visible; padding-right: 0; }
  .compact .grid { grid-template-columns: 1fr; gap: 0.4rem; }
  .edit { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.2rem; }
  .edit li { display: flex; align-items: center; gap: 0.2rem; padding: 0.25rem 0.4rem; background: var(--panel-2); border: 1px solid var(--line); border-radius: 6px; }
  .ename { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .ename small { color: var(--muted); }
  .edit select { max-width: 9rem; }
  .muted { color: var(--muted); margin: 0; }
  .error { color: var(--accent-text); margin: 0; }
  .small { font-size: 0.85rem; margin: 0; }
  .scrim { position: fixed; inset: 0; background: #0007; z-index: 30; }
  .scrim.clear { background: transparent; }
  .menu {
    position: fixed; z-index: 31; width: 260px; background: var(--panel); border: 1px solid var(--line); border-radius: 8px;
    padding: 0.3rem; display: flex; flex-direction: column; box-shadow: 0 4px 16px #0005;
  }
  .menu button { background: none; border: 0; text-align: left; padding: 0.4rem 0.6rem; border-radius: 4px; }
  .menu button:hover { background: var(--panel-2); }
  .menu .sep { border-top: 1px solid var(--line); border-radius: 0; margin-top: 0.2rem; }
  .mhead { padding: 0.3rem 0.6rem 0.4rem; font-weight: 600; display: flex; flex-direction: column; }
  .mhead small { color: var(--muted); font-weight: 400; }
  .dialog {
    position: fixed; z-index: 31; left: 50%; top: 50%; transform: translate(-50%, -50%); width: min(30rem, 92vw); max-height: 88vh; overflow: auto;
    background: var(--panel); border: 1px solid var(--line); border-radius: 8px; padding: 1rem; display: flex; flex-direction: column; gap: 0.6rem; box-sizing: border-box;
  }
  form { display: contents; }
  .dialog h4 { margin: 0 0 0.2rem; }
  input, select { padding: 0.3rem 0.5rem; background: var(--panel-2); color: inherit; border: 1px solid var(--line); border-radius: 4px; font: inherit; min-width: 0; }
  .row { display: grid; grid-template-columns: 7.5rem minmax(0, 1fr); align-items: center; gap: 0.6rem; }
  .actions { display: flex; gap: 0.6rem; margin-top: 0.4rem; }
  .primary { background: var(--accent); border-color: var(--accent); color: var(--on-accent); font-weight: 600; }
  .modes { display: flex; gap: 0.4rem; }
  .modes button.on, .hits button.on { border-color: var(--accent); outline: 1px solid var(--accent); }
  .hits { list-style: none; margin: 0; padding: 0; max-height: 14rem; overflow: auto; display: flex; flex-direction: column; gap: 0.2rem; }
  .hits button { width: 100%; display: flex; align-items: center; gap: 0.4rem; text-align: left; }
  .hits button span { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .hits small { color: var(--muted); }
  .steps { margin: 0; padding-left: 1.4rem; display: flex; flex-direction: column; gap: 0.5rem; }
  .steps li { display: flex; flex-direction: column; gap: 0.3rem; }
  .srow { display: flex; align-items: center; gap: 0.2rem; }
  .srow select { flex: 1; }
  .addstep { align-self: flex-start; display: inline-flex; align-items: center; gap: 0.3rem; }
</style>
