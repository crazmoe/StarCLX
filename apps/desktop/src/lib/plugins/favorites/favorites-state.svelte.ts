// Favoriten der Anlage: Kategorien mit Kollegen, Gruppen und Rufnummern,
// dieselben wie in den STARFACE-Apps. Dazu je Favorit ein Ablauf, was ein
// Klick auslöst (nur lokal, die Anlage kennt so etwas nicht).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { action, phone, run } from "../call/phone.svelte";
import { openConversation } from "../chat/chat-state.svelte";
import { consultAndTransfer, fkeys, type Membership, type UserState } from "../fkeys/fkeys.svelte";
import { t } from "../../i18n.svelte";
import { prefs, savePrefs, type FlowStep } from "../../prefs.svelte";

export type Favorite = {
  id: string;
  name: string;
  position: number;
  kind: "user" | "group" | "number";
  target_id: string;
  target_name: string;
  email: string;
  number: string;
  numbers: string[];
};
export type Category = { id: string; name: string; position: number; favorites: Favorite[] };
export type Candidate = { kind: "user" | "group"; id: string; name: string; number: string };
export type NewTarget = { kind: "user"; id: string } | { kind: "group"; id: string } | { kind: "number"; number: string };

export const favorites = $state({
  list: [] as Category[],
  loaded: false,
  /** Kein Recht für Favoriten */
  forbidden: false,
  error: "",
  notice: "",
});

let started = false;

export async function loadFavorites() {
  if (!started) {
    started = true;
    listen<Category[]>("favorites", (e) => {
      favorites.list = e.payload;
      favorites.loaded = true;
    });
    listen("resumed", () => loadFavorites());
  }
  try {
    const list = await invoke<Category[] | null>("favorites");
    favorites.forbidden = list === null;
    favorites.list = list ?? [];
    favorites.error = "";
    favorites.loaded = true;
  } catch (e) {
    favorites.error = String(e);
  }
}

export function resetFavorites() {
  favorites.list = [];
  favorites.loaded = false;
  favorites.forbidden = false;
  favorites.error = "";
  favorites.notice = "";
}

/** Angezeigter Name: eigener Name, sonst der des Ziels */
export const favName = (f: Favorite) => f.name || f.target_name || f.number;

/** Präsenz eines Kollegen oder einer Gruppe (aus dem Funktionstasten-Plugin) */
export const favState = (f: Favorite): UserState | undefined => (f.kind === "number" ? undefined : fkeys.presence[f.target_id]);

/** Zustand für die Farbe wie beim Besetztlampenfeld */
export function favColor(f: Favorite): string {
  const s = favState(f);
  if (!s) return "";
  if (s.telephony === "ringing") return "ringing";
  if (s.telephony === "active") return "busy";
  if (s.telephony === "queue_pause") return "wrapup";
  if (s.dnd) return "dnd";
  return s.telephony === "unavailable" ? "off" : "free";
}

// ── Bearbeiten (alles auf der Anlage; die Liste kommt per Ereignis zurück) ──

async function call(cmd: string, args: Record<string, unknown>) {
  favorites.notice = "";
  try {
    await invoke(cmd, args);
    return true;
  } catch (e) {
    favorites.notice = String(e);
    return false;
  }
}

/** Änderung an die Anlage; falls kein Ereignis zurückkommt, selbst nachladen */
async function edit(cmd: string, args: Record<string, unknown>) {
  const ok = await call(cmd, args);
  if (ok) setTimeout(() => loadFavorites(), 800);
  return ok;
}

/** Position hinter dem letzten Eintrag. Die Anlage zählt nicht zwingend ab 0
 *  (bei STARFACE ab 1), daher immer von den gemeldeten Positionen ausgehen. */
export const nextPosition = (list: { position: number }[]) => (list.length ? Math.max(...list.map((x) => x.position)) + 1 : 0);

export const createCategory = (name: string) => edit("favorite_category_create", { name, position: nextPosition(favorites.list) });
export const renameCategory = (id: string, name: string) => edit("favorite_category_rename", { id, name });
export const deleteCategory = (id: string) => edit("favorite_category_delete", { id });
export const moveCategory = (id: string, position: number) => edit("favorite_category_move", { id, position });

export function createFavorite(category: string, name: string, target: NewTarget) {
  const position = nextPosition(favorites.list.find((c) => c.id === category)?.favorites ?? []);
  return edit("favorite_create", { category, name, position, target });
}
export const renameFavorite = (id: string, name: string) => edit("favorite_rename", { id, name });
export const deleteFavorite = (id: string) => edit("favorite_delete", { id });
/** In derselben Kategorie verschieben (`category` = null) oder in eine andere */
export const moveFavorite = (id: string, category: string | null, position: number) => edit("favorite_move", { id, category, position });

export const searchTargets = (term: string) => invoke<Candidate[]>("favorite_search", { term });

// ── Aktionen und Abläufe ─────────────────────────────────────────────────────

/** Schritte, aus denen sich ein Ablauf zusammensetzt; `text` = braucht eine Eingabe */
export const stepTypes = (): { kind: string; label: string; text?: string; groups?: boolean }[] => [
  { kind: "auto", label: t("Heranholen, wenn es klingelt, sonst anrufen") },
  { kind: "call", label: t("Anrufen") },
  { kind: "pickup", label: t("Heranholen, wenn es klingelt") },
  { kind: "consult", label: t("Rückfrage (Auflegen vermittelt)") },
  { kind: "transfer", label: t("Gespräch direkt weiterleiten") },
  { kind: "chat", label: t("Chat öffnen") },
  { kind: "message", label: t("Chat-Nachricht senden"), text: t("Nachricht") },
  { kind: "mail", label: t("E-Mail schreiben") },
  { kind: "open", label: t("URL oder Programm öffnen"), text: t("URL oder Befehl, $(callerid) = Nummer") },
  { kind: "copy", label: t("Nummer kopieren") },
  { kind: "group_on", label: t("In Gruppen anmelden"), groups: true },
  { kind: "group_off", label: t("Aus Gruppen abmelden"), groups: true },
  { kind: "group_toggle", label: t("Gruppen an-/abmelden"), groups: true },
];

/** Gruppen, in denen man Mitglied ist (für die Gruppen-Schritte) */
export const memberships = $state({ list: [] as Membership[] });
export async function loadMemberships() {
  try {
    memberships.list = await invoke<Membership[]>("favorite_groups");
  } catch (e) {
    favorites.notice = String(e);
  }
}

/** Gruppen eines Schritts; ohne Auswahl bei einem Gruppen-Favoriten diese Gruppe */
function stepGroups(f: Favorite, step: FlowStep): string[] {
  if (step.groups?.length) return step.groups;
  return f.kind === "group" ? [f.target_id] : [];
}

/** Ablauf eines Favoriten; ohne eigenen der Standard */
export const flowOf = (f: Favorite): FlowStep[] => prefs.value?.favorite_flows?.[f.id] ?? [{ kind: "auto" }];
export const hasOwnFlow = (f: Favorite) => !!prefs.value?.favorite_flows?.[f.id];

export async function saveFlow(f: Favorite, steps: FlowStep[] | null) {
  if (!prefs.value) return;
  const flows = { ...(prefs.value.favorite_flows ?? {}) };
  if (steps && steps.length) flows[f.id] = steps;
  else delete flows[f.id];
  await savePrefs({ ...$state.snapshot(prefs.value), favorite_flows: flows });
}

/** Gesprochenes Gespräch, auf das sich Rückfrage und Weiterleiten beziehen */
const activeCall = () => phone.status.calls.find((c) => c.phase === "connected" && !c.consultation_of);

/** Welche Schritte für diesen Favoriten gerade gehen (fürs Menü) */
export function possible(f: Favorite, kind: string): boolean {
  switch (kind) {
    case "auto":
    case "call":
    case "copy":
      return !!f.number;
    case "pickup":
      return f.kind !== "number" && favState(f)?.telephony === "ringing";
    case "consult":
    case "transfer":
      return !!f.number && !!activeCall();
    case "chat":
    case "message":
      return f.kind === "user";
    case "mail":
      return !!f.email;
    case "open":
      return true;
    case "group_on":
    case "group_off":
    case "group_toggle":
      return f.kind === "group";
    default:
      return false;
  }
}

/** Chat-Gespräch mit dem Kollegen öffnen; der Chat-Reiter zeigt es */
async function chatWith(f: Favorite): Promise<string | undefined> {
  try {
    const jid = await invoke<string>("favorite_chat_jid", { userId: f.target_id });
    window.dispatchEvent(new CustomEvent("show-tab", { detail: "chat" }));
    await openConversation(jid);
    return jid;
  } catch (e) {
    favorites.notice = String(e);
  }
}

/** Einen Schritt ausführen; false bricht den Ablauf ab */
export async function runStep(f: Favorite, step: FlowStep): Promise<boolean> {
  const c = activeCall();
  switch (step.kind) {
    case "auto":
      if (favState(f)?.telephony === "ringing" && f.kind !== "number") return runStep(f, { kind: "pickup" });
      return runStep(f, { kind: "call" });
    case "call":
      if (!f.number) {
        favorites.notice = t("{name} hat keine Rufnummer.", { name: favName(f) });
        return false;
      }
      return run("phone_dial", { number: f.number });
    case "pickup":
      // Klingelt es nicht, ist nichts zu tun; der Ablauf geht weiter
      if (f.kind === "number" || favState(f)?.telephony !== "ringing") return true;
      return call("fkey_grab", { userId: f.target_id });
    case "consult":
      if (!c || !f.number) return true;
      await consultAndTransfer(c, f.number);
      return true;
    case "transfer":
      if (!c || !f.number) return true;
      return action("forward", c.id, f.number);
    case "chat":
      if (f.kind !== "user") return true;
      return !!(await chatWith(f));
    case "message": {
      if (f.kind !== "user" || !step.text?.trim()) return true;
      const jid = await chatWith(f);
      if (!jid) return false;
      try {
        await invoke("chat_send", { peer: jid, body: step.text });
        await openConversation(jid);
        return true;
      } catch (e) {
        favorites.notice = String(e);
        return false;
      }
    }
    case "mail":
      if (!f.email) return true;
      await openUrl(`mailto:${encodeURIComponent(f.email)}`).catch((e) => (favorites.notice = String(e)));
      return true;
    case "open":
      if (!step.text?.trim()) return true;
      return call("call_action_run", { target: step.text, number: f.number });
    case "copy":
      await navigator.clipboard.writeText(f.number).catch(() => {});
      favorites.notice = t("Nummer kopiert");
      return true;
    case "group_on":
    case "group_off":
    case "group_toggle": {
      const ids = stepGroups(f, step);
      if (!ids.length) return true;
      const on = step.kind === "group_toggle" ? null : step.kind === "group_on";
      try {
        const now = await invoke<boolean>("favorite_group_login", { ids, on });
        favorites.notice = now ? t("In Gruppen angemeldet") : t("Aus Gruppen abgemeldet");
        return true;
      } catch (e) {
        favorites.notice = String(e);
        return false;
      }
    }
    default:
      return true;
  }
}

/** Klick auf einen Favoriten: seinen Ablauf Schritt für Schritt ausführen */
export async function press(f: Favorite) {
  favorites.notice = "";
  for (const step of flowOf(f)) {
    if (!(await runStep(f, step))) return;
  }
}
