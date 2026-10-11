// Rechte des Benutzers auf der Anlage (Admin: Benutzer → Rechte). Ohne Recht
// wird die Funktion ausgegraut statt mit einer Fehlermeldung zu scheitern.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Permission =
  | "instant_messaging"
  | "redirection"
  | "group_redirection"
  | "fkey_module_key"
  | "ifmc"
  | "ifmc_edit"
  | "calllist"
  | "voicemail"
  | "call_recording"
  | "conference"
  | "addressbook";

/** `null`: unbekannt (noch nicht geladen oder Abfrage fehlgeschlagen), dann ist alles erlaubt */
export const permissions = $state({ list: null as string[] | null });

export const can = (p: Permission) => permissions.list === null || permissions.list.includes(p);

export async function loadPermissions() {
  try {
    permissions.list = await invoke<string[] | null>("permissions");
  } catch {
    permissions.list = null;
  }
}

let started = false;

/** Einmal beim Start: entzogene oder erteilte Rechte sofort übernehmen */
export function initPermissions() {
  if (started) return;
  started = true;
  listen("me-permission", () => loadPermissions());
}
