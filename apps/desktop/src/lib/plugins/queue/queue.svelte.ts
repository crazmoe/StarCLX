// iQueue-Warteschlangen, in denen der Benutzer Agent ist. Die App schickt
// bei jeder Änderung den vollen Stand.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Agent = { user_id: string; name: string; logged_in: boolean; paused: boolean };

export type QueueCall = {
  id: string;
  caller_name: string;
  caller_number: string;
  priority: number;
  position: number;
  state: "waiting" | "ringing" | "connected";
  incoming: number;
  connected: number;
  agents: string[];
  bot_output: Record<string, string>;
};

export type Stats = { callers: number; free_agents: number; missed: number; unanswered: number; total: number; avg_wait_secs: number };

export type Queue = { id: string; name: string; agents: Agent[]; calls: QueueCall[]; stats: Stats };

type View = { me: string; queues: Queue[] };

export const queues = $state<View>({ me: "", queues: [] });

let started = false;

/** Einmal beim Start: Stand holen und Änderungen übernehmen. */
export function initQueues() {
  if (started) return;
  started = true;
  listen<View>("queues", (e) => Object.assign(queues, e.payload));
  invoke<View>("queues").then((v) => Object.assign(queues, v)).catch(() => {});
}

/** Eigener Eintrag als Agent in dieser Queue */
export const myAgent = (q: Queue) => q.agents.find((a) => a.user_id === queues.me);

/** Anrufer, die noch auf einen Agenten warten */
export const waitingCalls = () => queues.queues.reduce((n, q) => n + q.calls.filter((c) => c.state !== "connected").length, 0);
