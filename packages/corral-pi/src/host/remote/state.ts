import type { BoardControl, ExitState } from "../../build/types.ts";
import type { JobInspection } from "../../extensions/jobs/runtime.ts";
import { utf8Prefix } from "../../roles/assistant/owner-action.ts";
import type { TodoLedger } from "../../ui/todo.ts";
import type { QueuedInput } from "../queue.ts";
import type { RemoteRegistry } from "./registry.ts";

export const LIVE_STATE_BYTES = 12 * 1024;

export interface Counts { live: number; waiting: number; total: number }
export interface Activity { phase: string; since: string }
export interface CardBrief { id: string; kind: string; title: string; child?: string; expires_at?: string }
export interface HostNotice { id: string; tone: string; text: string; until?: string }
export interface UsageBrief {
  tokens: number; cost_usd?: number; cost_partial: boolean;
  context_tokens: number; context_window: number; context_percent?: number;
}
interface ExitBrief {
  phase: ExitState["phase"]; detail?: string; choice?: "keep" | "land" | "iterate"; landed?: string; base_branch?: string;
  repair?: { round: number; of: number };
}
interface BoardBrief { open: number; blocked: number; failed: number; proposed: boolean }
interface PlanBrief { done: number; total: number; current?: string; stale: boolean }
interface Capability { id: string; unavailable?: string }
export interface SessionState {
  mode: "interactive" | "headless"; turn: number; open: boolean; paused: boolean;
  activity?: Activity; last_error?: string; model?: string; pending_model?: string;
  effort?: string; effort_levels: readonly string[]; usage: UsageBrief;
  queue: QueuedInput[]; queue_total: number; cards: CardBrief[]; exit?: ExitBrief;
  children: Counts; jobs: Counts; changes?: number; board?: BoardBrief; plan?: PlanBrief;
  notices: HostNotice[]; can: Capability[];
}

export type SessionPart = "cards" | "children" | "jobs" | "changes" | "usage" | "plan" | "context" | "board" | "record" | "models" | "commands";
export interface StateProjection { state: SessionState; parts: Partial<Record<SessionPart, unknown>> }

export interface BoundedSessionState { state: SessionState; json: string; bytes: number }

export function boundState(state: SessionState): BoundedSessionState {
  let json = JSON.stringify(state), bytes = Buffer.byteLength(json);
  if (bytes <= LIVE_STATE_BYTES) return { state, json, bytes };
  const clipped = { ...state, notices: [...state.notices], queue: [...state.queue], cards: state.cards.map(card => ({ ...card })) };
  for (const list of [clipped.notices, clipped.queue]) {
    while (bytes > LIVE_STATE_BYTES && list.length) {
      const tail = list.pop();
      bytes -= Buffer.byteLength(JSON.stringify(tail)) + Number(list.length > 0);
    }
  }
  for (let i = clipped.cards.length - 1; bytes > LIVE_STATE_BYTES && i >= 0; i--) {
    const card = clipped.cards[i]!;
    bytes -= Buffer.byteLength(JSON.stringify(card.title)) - 2;
    card.title = "";
  }
  // Extremely escaped IDs and optional diagnostics must not defeat the byte bound.
  while (bytes > LIVE_STATE_BYTES && clipped.cards.length) {
    bytes -= Buffer.byteLength(JSON.stringify(clipped.cards.pop())) + Number(clipped.cards.length > 0);
  }
  for (const field of ["last_error", "pending_model", "activity", "plan", "exit"] as const) {
    if (bytes <= LIVE_STATE_BYTES) break;
    if (clipped[field] !== undefined) bytes -= Buffer.byteLength(JSON.stringify({ [field]: clipped[field] })) - 1;
    delete clipped[field];
  }
  json = JSON.stringify(clipped);
  return { state: clipped, json, bytes: Buffer.byteLength(json) };
}

const REMOTE_CAPABILITIES: Readonly<Record<string, string | undefined>> = {
  cards: "cards", child_file: "children", child_act: "child_control", jobs: "jobs", job_act: "job_control",
  changes: "changes", usage: "usage", plan: "plan", context: "context", commands: "commands", board: "board",
  record: "record", exit_open: "exit", compact: "compact", export: "export", owner_bash: "shell",
};

export function capabilities(registry: RemoteRegistry, options: { open: boolean; closing: boolean; rewind: boolean }): Capability[] {
  // These operations have fixed bridge handlers, not registered detail methods.
  const can: Capability[] = ["send", "steer", "follow_up", "attachments", "queue", "model"].map(id => ({ id }));
  if (options.rewind) can.push({ id: "rewind" });
  for (const name of registry.names()) {
    const id = REMOTE_CAPABILITIES[name];
    if (typeof id === "string") can.push({ id });
  }
  for (const capability of can) {
    if (options.closing) capability.unavailable = "Session is stopping";
    else if (capability.id === "model" && options.open) capability.unavailable = "Wait for the current turn to finish";
  }
  return can;
}

export function exitBrief(exit: ExitState): ExitBrief {
  return { phase: exit.phase, detail: exit.detail === undefined ? undefined : utf8Prefix(exit.detail, 512),
    choice: exit.record?.choice, landed: exit.record?.landed,
    base_branch: exit.record?.baseBranch === undefined ? undefined : utf8Prefix(exit.record.baseBranch, 256) };
}

export function boardBrief(board: BoardControl): BoardBrief {
  let open = 0, blocked = 0, failed = 0;
  for (const task of board.tasks()) {
    if (task.phase !== "landed") open++;
    if (task.phase === "blocked") blocked++;
    else if (task.phase === "failed") failed++;
  }
  return { open, blocked, failed, proposed: board.proposed() !== undefined };
}

export function planBrief(ledger: TodoLedger): PlanBrief {
  let done = 0, total = 0, current: string | undefined;
  for (const phase of ledger.current?.phases ?? []) for (const step of phase.steps) {
    total++;
    if (step.status.state === "done") done++;
    else if (step.status.state === "in_progress") current ??= utf8Prefix(step.label, 160);
  }
  return { done, total, current, stale: ledger.unavailable };
}

export function jobCounts(jobs: readonly JobInspection[], recovered: number): Counts {
  let live = 0, waiting = 0;
  for (const job of jobs) {
    if (job.state === "running") live++;
    if (job.watches.some(watch => watch.armed)) waiting++;
  }
  return { live, waiting, total: jobs.length + recovered };
}
