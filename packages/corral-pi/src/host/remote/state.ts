import { createHash } from "node:crypto";
import { BridgeRefusal, SESSION_PART_MAX } from "../../bridge/protocol.ts";
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
interface LandBrief {
  steps: { id: string; state: string; note?: string; started_at?: string; ended_at?: string }[];
  gate?: { attempt?: string; log?: string; command?: string; index?: number; of?: number; tail: string[] };
}
interface CandidateBrief {
  state: NonNullable<ExitState["candidate"]>["state"]; base?: string; base_tip?: string; head?: string; source?: string;
  conflicts?: string[]; gate_attempt?: string; moved_to?: string; detail?: string;
}
interface ExitBrief {
  phase: ExitState["phase"]; detail?: string; choice?: "keep" | "land" | "iterate"; landed?: string; base_branch?: string;
  repair?: { round: number; of: number }; land?: LandBrief; candidate?: CandidateBrief;
}
/** The wire's bounds on a gate log tail: its last lines, each clipped. */
const LAND_TAIL_LINES = 8, LAND_TAIL_BYTES = 200;
/** The wire's bounds on a candidate's conflicted paths: the first ones, each clipped. */
const CONFLICT_PATHS = 20, CONFLICT_BYTES = 240;
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

export const jsonBytes = (value: unknown): number => Buffer.byteLength(JSON.stringify(value));

/** Keep the original prefix, without splitting a Unicode code point or exceeding its JSON budget. */
export function clipText(text: string, bytes: number): string {
  const prefix = utf8Prefix(text, bytes);
  if (jsonBytes(prefix) - 2 <= bytes) return prefix;
  let lo = 0, hi = prefix.length;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (jsonBytes(prefix.slice(0, mid)) - 2 <= bytes) lo = mid; else hi = mid - 1;
  }
  return prefix.slice(0, lo).replace(/[\uD800-\uDBFF]$/, "");
}

export function optionalText(text: string | undefined, bytes: number): string | undefined {
  return text === undefined ? undefined : clipText(text, bytes);
}

export const isoTime = (at: number): string => new Date(at).toISOString();
export const optionalTime = (at: number | undefined): string | undefined => at === undefined ? undefined : isoTime(at);
export const fingerprintPart = (value: unknown): string =>
  createHash("sha256").update(JSON.stringify(value)).digest("base64url").slice(0, 22);

export function parseCursor(after: unknown): string | undefined {
  if (after === undefined) return undefined;
  if (typeof after !== "string" || Buffer.byteLength(after) > 128) throw new BridgeRefusal("bad_input", "invalid part cursor");
  return after;
}

export const partCursor = (key: string, offset: number): string => `${key}.${offset}`;
export function cursorOffset(after: string, key: string): number {
  const token = /^([A-Za-z0-9_-]{22})\.(0|[1-9]\d{0,15})$/.exec(after);
  if (!token || !Number.isSafeInteger(Number(token[2]))) throw new BridgeRefusal("bad_input", "invalid part cursor");
  if (token[1] !== key) throw new BridgeRefusal("stale_page", "this part changed; read it again from the first page");
  return Number(token[2]);
}

/** Admit whole elements, counting JSON delimiters and escaping, not just UTF-8 field lengths. */
export function fitRows<T>(rows: readonly T[], start: number, count: number, budget: number): T[] {
  const kept: T[] = [];
  for (let i = start, end = Math.min(rows.length, start + count); i < end; i++) {
    const row = rows[i]!;
    budget -= jsonBytes(row) + Number(kept.length > 0);
    if (budget < 0) break;
    kept.push(row);
  }
  return kept;
}

export function pageRows<T>(rows: readonly T[], after: string | undefined, key: string, count: number, overhead: number) {
  const start = after === undefined ? 0 : cursorOffset(after, key);
  if (start > rows.length) throw new BridgeRefusal("bad_input", "part cursor is outside the list");
  const items = fitRows(rows, start, count, SESSION_PART_MAX - overhead - 64);
  const end = start + items.length;
  if (!items.length && end < rows.length) throw new BridgeRefusal("response_too_large", "one list element exceeds the part bound");
  return { items, next: end < rows.length ? partCursor(key, end) : undefined };
}

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
  // A gate log tail is the largest optional detail of an exit: it goes before the exit itself does.
  if (bytes > LIVE_STATE_BYTES && clipped.exit?.land?.gate?.tail.length) {
    const gate = { ...clipped.exit.land.gate, tail: [] };
    bytes -= jsonBytes(clipped.exit.land.gate) - jsonBytes(gate);
    clipped.exit = { ...clipped.exit, land: { ...clipped.exit.land, gate } };
  }
  // So are a candidate's conflicted paths: its state and detail still say it conflicted.
  if (bytes > LIVE_STATE_BYTES && clipped.exit?.candidate?.conflicts) {
    const { conflicts: _, ...candidate } = clipped.exit.candidate;
    bytes -= jsonBytes(clipped.exit.candidate) - jsonBytes(candidate);
    clipped.exit = { ...clipped.exit, candidate };
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
  board_plan_task: "board_plan_task",
  record: "record", exit_open: "exit", compact: "compact", export: "export", owner_bash: "shell",
  model_favorite: "model_favorite",
};

export function capabilities(registry: RemoteRegistry, options: { open: boolean; closing: boolean; rewind: boolean }): Capability[] {
  // These operations have fixed bridge handlers, not registered detail methods.
  const can: Capability[] = ["send", "steer", "follow_up", "attachments", "queue", "model"].map(id => ({ id }));
  if (options.rewind) can.push({ id: "rewind" });
  for (const name of registry.names()) {
    const id = REMOTE_CAPABILITIES[name];
    if (typeof id !== "string" || ((id === "board" || id === "board_plan_task") && !registry.get("board_refresh"))) continue;
    can.push({ id });
  }
  for (const capability of can) {
    if (options.closing) capability.unavailable = "Session is stopping";
    else if (capability.id === "jobs" && !registry.get("job_output")) capability.unavailable = "Managed jobs are not enabled";
    else if (options.open && (capability.id === "model" || capability.id === "compact" || capability.id === "export")) {
      capability.unavailable = "Wait for the current turn to finish";
    }
  }
  return can;
}

function landBrief(land: NonNullable<ExitState["land"]>): LandBrief {
  const gate = land.gate;
  return {
    steps: land.steps.map(step => ({ id: step.id, state: step.state, note: optionalText(step.note, 160),
      started_at: optionalTime(step.startedAt), ended_at: optionalTime(step.endedAt) })),
    gate: gate && { attempt: gate.attempt, log: optionalText(gate.log, 512), command: optionalText(gate.command, 240),
      index: gate.index, of: gate.of, tail: gate.tail.slice(-LAND_TAIL_LINES).map(line => clipText(line, LAND_TAIL_BYTES)) },
  };
}

function candidateBrief(candidate: NonNullable<ExitState["candidate"]>): CandidateBrief {
  const sha = (text: string | undefined) => optionalText(text, 64);
  return { state: candidate.state, base: optionalText(candidate.base, 256), base_tip: sha(candidate.baseTip), head: sha(candidate.head),
    source: sha(candidate.source), gate_attempt: optionalText(candidate.gateAttempt, 64), moved_to: sha(candidate.movedTo),
    ...(candidate.conflicts?.length ? { conflicts: candidate.conflicts.slice(0, CONFLICT_PATHS).map(path => clipText(path, CONFLICT_BYTES)) } : {}),
    detail: optionalText(candidate.detail, 512) };
}

export function exitBrief(exit: ExitState): ExitBrief {
  return { phase: exit.phase, detail: exit.detail === undefined ? undefined : utf8Prefix(exit.detail, 512),
    choice: exit.record?.choice, landed: exit.record?.landed,
    // The record names the base once Land succeeded; a running or failed Land already knows it, a prepared candidate too.
    base_branch: optionalText(exit.record?.baseBranch ?? exit.land?.base ?? exit.candidate?.base, 256), repair: exit.repair,
    land: exit.land && landBrief(exit.land),
    // A host that prepared nothing says nothing: the app never shows a candidate it did not serve.
    ...(exit.candidate ? { candidate: candidateBrief(exit.candidate) } : {}) };
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
