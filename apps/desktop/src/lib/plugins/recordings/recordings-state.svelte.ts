// Gesprächsaufzeichnungen der Anlage, gemeinsam für Reiter und Kachel.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Recording = {
  id: string;
  cdr_id: string;
  caller: string;
  callee: string;
  number: string;
  start: number;
  end: number;
  duration_secs: number;
};

/** `disabled`: kein Recht oder die Anlage bietet keine Aufzeichnungen an */
export const recordings = $state({ list: [] as Recording[], total: 0, error: "", disabled: false });

export async function loadRecordings() {
  try {
    const res = await invoke<{ list: Recording[]; total: number } | null>("recordings");
    recordings.list = res?.list ?? [];
    recordings.total = res?.total ?? 0;
    recordings.disabled = res === null;
    recordings.error = "";
  } catch (e) {
    recordings.error = String(e);
  }
}

let started = false;

/** Einmal beim Start: bei jeder Änderung auf der Anlage neu laden. */
export function initRecordings() {
  if (started) return;
  started = true;
  listen("recordings-changed", () => loadRecordings());
  // Recht erteilt oder entzogen
  listen("me-permission", () => loadRecordings());
}

/** MIME-Typ anhand der ersten Bytes; die Anlage nennt das Format nicht */
export function mimeOf(data: ArrayBuffer) {
  const b = new Uint8Array(data.slice(0, 4));
  if (b[0] === 0x49 && b[1] === 0x44 && b[2] === 0x33) return "audio/mpeg";
  if (b[0] === 0xff && (b[1] & 0xe0) === 0xe0) return "audio/mpeg";
  if (b[0] === 0x4f && b[1] === 0x67 && b[2] === 0x67 && b[3] === 0x53) return "audio/ogg";
  return "audio/wav";
}
