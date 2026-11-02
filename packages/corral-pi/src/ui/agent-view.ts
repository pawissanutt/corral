import { stripTerminalSequences } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";

/** Host observations only. Parentage is provenance, never a board dependency. */
export interface AgentObservation {
  readonly id: string;
  readonly session: string;
  readonly parent?: string;
  readonly kind: string;
  readonly state: string;
  readonly model: string | null;
  readonly transcript: boolean;
  readonly tokens?: number;
}
export interface AgentRow { readonly text: string; readonly agent?: string }
export function agentRows(identity: HostIdentity, agents: readonly AgentObservation[] | undefined, bound: boolean, selected?: string): AgentRow[] {
  const rows: AgentRow[] = [];
  const add = (...text: string[]) => rows.push(...text.map(text => ({ text })));
  add("Scoped agents · read-only host observations", "j/k or click select · t existing transcript · b jobs · F9 conversation",
    `Task ${identity.task ?? "unavailable"} / run ${identity.run ?? "unavailable"} / assistant ${identity.assistant ?? "unavailable"} · g${identity.generation}`,
    bound ? "Bound host · not canonical task/run settlement" : "STALE / disconnected host observations",
    "Scope: this host and up to 100 retained native children; other agents unavailable.",
    "Parent links are provenance, NOT dependency edges. No stop/restart/resume controls.", "");
  if (!agents) { add("Agent observations unavailable · legacy/missing source"); return rows; }
  for (const agent of agents) rows.push({ agent: agent.id, text: `${selected === agent.id ? ">" : " "} ${agent.kind} · ${agent.id} · ${agent.state}` });
  const agent = agents.find(agent => agent.id === selected);
  if (!agent) { add("Selected agent missing / no longer observed; select an existing row."); return rows; }
  add("", `Session ${agent.session}`, `Parent ${agent.parent ?? "not observed (host root)"}`,
    `Model ${agent.model ?? "unavailable"}`, `Usage ${agent.tokens === undefined ? "unavailable for this source" : agent.tokens + " recorded session tokens"}`,
    "Health unavailable · no liveness probe or success inferred", `Activity source: local ${agent.parent ? "auxiliary runtime outcome" : "SDK session"}`,
    bound && agent.transcript ? "t opens bounded existing JSONL only; does not resume, migrate or rebind" : "Transcript ineligible / unavailable (missing reference or stale binding)");
  return rows.map(row => ({ ...row, text: stripTerminalSequences(row.text).replace(/[\u0000-\u001f\u007f-\u009f]/g, " ") }));
}
