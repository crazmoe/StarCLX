// Nextcloud-Verbindung: Stand für Einstellungen und Teilen-Knöpfe.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Book = { href: string; name: string; writable: boolean; enabled: boolean; count: number };
export type NextcloudStatus = {
  connected: boolean;
  server: string;
  login_name: string;
  display_name: string;
  /** Anmeldung im Browser läuft: deren Seite */
  login_url: string;
  syncing: boolean;
  error: string;
  books: Book[];
};
/** Abgelegte Datei; `url` leer = ohne Link */
export type Shared = { path: string; url: string; password: string; expires: string };
/** Unterordner: Aufnahmen, Notizen, Gesprächsnotizen (folgt mit der Transkription) */
export type Kind = "recording" | "note" | "summary";

export const nextcloud = $state({
  status: { connected: false, server: "", login_name: "", display_name: "", login_url: "", syncing: false, error: "", books: [] } as NextcloudStatus,
});

let started = false;

/** Einmal beim Start */
export function initNextcloud() {
  if (started) return;
  started = true;
  invoke<NextcloudStatus>("nextcloud_status").then((s) => (nextcloud.status = s), () => {});
  listen<NextcloudStatus>("nextcloud", (e) => (nextcloud.status = e.payload));
}

export function shareRecording(id: string, name: string) {
  return invoke<Shared>("nextcloud_share_recording", { id, name, link: true });
}

export function shareText(kind: Kind, name: string, text: string) {
  return invoke<Shared>("nextcloud_share_text", { kind, name, text, link: true });
}

/** Zeitstempel für Dateinamen: 2026-10-27_1430 */
export function stamp(ms: number) {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}_${p(d.getHours())}${p(d.getMinutes())}`;
}
