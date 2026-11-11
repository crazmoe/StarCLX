<script lang="ts">
  // Ein Favorit wie ein Besetztlampenfeld: Bild auf einer Fläche in der
  // Telefonfarbe, Name und Status. Klick führt den Ablauf aus, das Menü (⋯ oder
  // Rechtsklick) bietet alle Aktionen. Ein Gespräch lässt sich daraufziehen.
  import { avatarOf, callDrag, chatText } from "../fkeys/fkeys.svelte";
  import ChatBubble from "../../ChatBubble.svelte";
  import Icon from "../../Icon.svelte";
  import { t } from "../../i18n.svelte";
  import { favColor, favName, favState, hasOwnFlow, type Favorite } from "./favorites.svelte";

  let { fav, onclick, onmenu }: { fav: Favorite; onclick: () => void; onmenu: (e: MouseEvent) => void } = $props();

  const color = $derived(favColor(fav));
  const presence = $derived(favState(fav));
  const avatar = $derived(fav.kind === "user" ? avatarOf(fav.target_id) : null);
  const initials = $derived(
    favName(fav)
      .split(/\s+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((w) => w[0]?.toUpperCase())
      .join(""),
  );
  const stateText: Record<string, string> = $derived({ busy: t("im Gespräch"), ringing: t("klingelt"), wrapup: t("Nachbearbeitung"), free: t("frei"), off: t("nicht erreichbar"), dnd: t("Bitte nicht stören") });
  const sub = $derived(chatText(presence) || fav.number);
  const title = $derived(
    [favName(fav), fav.number, stateText[color], presence?.redirect ? t("Umleitung aktiv") : "", hasOwnFlow(fav) ? t("Eigener Ablauf beim Klick") : ""]
      .filter(Boolean)
      .join(" · "),
  );
</script>

<div class="fav {color}" class:target={callDrag.call && callDrag.over === `fav:${fav.id}`} data-fav={fav.id} data-fav-number={fav.number}>
  <button class="main" {title} {onclick} oncontextmenu={(e) => { e.preventDefault(); onmenu(e); }}>
    {#if fav.kind === "number"}
      <span class="contact"><Icon name="call" size={18} /></span>
    {:else}
      <span class="blf">
        {#if color === "off"}<span class="dash">–</span>{/if}
        <span class="pic">
          {#if fav.kind === "group"}<Icon name="groups" size={20} />{:else if avatar}<img src={avatar} alt="" />{:else}{initials}{/if}
        </span>
        {#if presence?.dnd}
          <span class="badge dnd" title={t("Bitte nicht stören")}>
            <svg viewBox="0 0 12 12"><circle cx="6" cy="6" r="5.5" /><rect x="3" y="5.1" width="6" height="1.8" rx="0.5" /></svg>
          </span>
        {/if}
        {#if presence?.chat}<span class="badge chat"><ChatBubble state={presence.chat} /></span>{/if}
        {#if presence?.redirect}
          <span class="badge redirect" title={t("Umleitung aktiv")}>
            <svg viewBox="0 0 12 12"><circle cx="6" cy="6" r="5.5" /><path class="mark" d="M3.4 3V7.4h5M6.8 5.6l1.9 1.8-1.9 1.8" /></svg>
          </span>
        {/if}
      </span>
    {/if}
    <span class="txt"><strong>{favName(fav)}</strong>{#if sub}<small>{sub}</small>{/if}</span>
    {#if hasOwnFlow(fav)}<span class="flow" aria-hidden="true"><Icon name="play" size={14} /></span>{/if}
  </button>
  <button class="more" title={t("Aktionen")} aria-label={t("Aktionen")} onclick={onmenu}><Icon name="more" size={18} /></button>
</div>

<style>
  .fav { display: flex; align-items: stretch; background: var(--panel-2); border: 1px solid var(--line); border-radius: 6px; min-width: 0; }
  .fav:hover { border-color: var(--accent); }
  .fav.target { border-color: var(--accent); outline: 2px solid var(--accent); }
  .main { flex: 1; min-width: 0; display: flex; align-items: center; gap: 0.6rem; text-align: left; padding: 0.4rem 0.2rem 0.4rem 0.6rem; background: none; border: 0; min-height: calc(2.5rem + 0.8rem); }
  .more { flex: none; background: none; border: 0; padding: 0 0.3rem; color: var(--muted); }
  .more:hover { color: var(--text); }
  .flow { color: var(--accent-text); flex: none; display: grid; margin-left: auto; }
  .blf { position: relative; flex: none; width: 3.3rem; height: 2.5rem; border-radius: 1.25rem 1.25rem 1.25rem 0.25rem; background: #69737d; }
  .off .blf, .dnd .blf { border-radius: 1.25rem 1.25rem 0.25rem 1.25rem; }
  .free .blf { background: var(--green); }
  .busy .blf { background: var(--red); }
  .ringing .blf { background: var(--yellow); }
  .wrapup .blf { background: var(--accent); }
  .pic {
    position: absolute; right: 0.15rem; top: 0.15rem; width: 2.2rem; height: 2.2rem; border-radius: 50%;
    overflow: hidden; display: grid; place-items: center; background: #c9ccd0; color: #555b62;
    font-size: 0.8rem; font-weight: 600; letter-spacing: 0.02em;
  }
  .pic img { width: 100%; height: 100%; object-fit: cover; }
  .dash { position: absolute; left: 0.3rem; top: 50%; transform: translateY(-55%); color: #fff; font-weight: 700; }
  .badge { position: absolute; display: grid; }
  .badge svg { width: 100%; height: 100%; overflow: visible; }
  .badge .mark { fill: none; stroke: #fff; stroke-width: 1.2; stroke-linecap: round; stroke-linejoin: round; }
  .badge.dnd { left: 0.05rem; top: -0.15rem; width: 0.95rem; height: 0.95rem; }
  .badge.dnd circle { fill: var(--red); stroke: #fff; stroke-width: 1; }
  .badge.dnd rect { fill: #fff; }
  .badge.chat { right: -0.35rem; top: -0.3rem; width: 1rem; height: 0.95rem; }
  .badge.redirect { left: 0.05rem; bottom: -0.15rem; width: 0.95rem; height: 0.95rem; }
  .badge.redirect circle { fill: #111; stroke: #fff; stroke-width: 1; }
  .contact { flex: none; width: 2.2rem; height: 2.2rem; border-radius: 50%; display: grid; place-items: center; background: #c9ccd0; color: #555b62; }
  .txt { min-width: 0; display: flex; flex-direction: column; }
  .txt strong { font-weight: 600; overflow: hidden; display: -webkit-box; -webkit-line-clamp: 2; line-clamp: 2; -webkit-box-orient: vertical; overflow-wrap: anywhere; line-height: 1.2; }
  .txt small { color: var(--muted); font-size: 0.78rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
