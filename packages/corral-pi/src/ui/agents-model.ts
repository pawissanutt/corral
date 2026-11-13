// The row model behind the agents view, the sidebar Context tab and the Agents tab badge. Helper calls and
// background jobs come from the bound host's snapshot; build children arrive later as ready-made rows through
// `AgentExtras`. Unknown values stay absent and render as "–", never as zero.
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { HelperInspection, HelperMessage } from "../extensions/subagent/inspection.ts";
import { helperText } from "./helper-chat.ts";
import {
  plainLabel, PANEL_BG, PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_SELECTED, PANEL_SUCCESS, PANEL_TEXT, PANEL_WARNING,
  type PanelJob, type PanelSnapshot,
} from "./panels-view.ts";

export type Tone = "text" | "muted" | "accent" | "success" | "warning" | "error";
export function toneColor(tone: Tone): string {
  switch (tone) {
    case "text": return PANEL_TEXT;
    case "muted": return PANEL_MUTED;
    case "accent": return PANEL_BLUE;
    case "success": return PANEL_SUCCESS;
    case "warning": return PANEL_WARNING;
    case "error": return PANEL_ERROR;
  }
}
export interface Chip { readonly text: string; readonly tone: Tone }
/** A raised label: the selected-row surface with a toned glyph and word, so colour is never the only signal. */
export function chip({ text, tone }: Chip): string {
  return PANEL_SELECTED + toneColor(tone) + " " + text + " " + PANEL_BG + PANEL_TEXT;
}
/** Chips flow left to right after `lead` and wrap to new rows; a chip wider than a row is clipped. */
export function chipRows(chips: readonly Chip[], width: number, lead = ""): string[] {
  const rows: string[] = [];
  let row = lead, used = visibleWidth(lead);
  for (const item of chips) {
    const text = truncateToWidth(item.text, Math.max(1, width - 2), "…"), size = visibleWidth(text) + 2;
    const gap = used === 0 ? 0 : row === lead ? 2 : 1;
    if (used > 0 && used + gap + size > width) { rows.push(row); row = chip({ text, tone: item.tone }); used = size; continue; }
    row += " ".repeat(gap) + chip({ text, tone: item.tone }); used += gap + size;
  }
  if (row) rows.push(row);
  return rows;
}
export function bar(color: string, text: string): string { return color + "▎ " + PANEL_TEXT + text; }

/** 38s, 3m20s, 1h05m. */
export function duration(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(s / 3600)}h${String(Math.floor(s % 3600 / 60)).padStart(2, "0")}m`;
}
export function ago(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return s < 60 ? `${s}s ago` : s < 3600 ? `${Math.floor(s / 60)}m ago` : s < 86400 ? `${Math.floor(s / 3600)}h ago` : `${Math.floor(s / 86400)}d ago`;
}
/** "$0.12", or "–" when the cost is unknown. */
export function money(cost: number | undefined): string {
  return cost === undefined || !Number.isFinite(cost) ? "–" : `$${cost.toFixed(2)}`;
}

/** Glyph and word, so colour is never the only signal. */
export interface Status { readonly glyph: string; readonly tone: Tone; readonly word: string; readonly time?: string }

export type AgentState =
  | "preparing" | "queued" | "running" | "needs" | "done" | "failed" | "cancelled" | "uncertain" | "not_started";
/** The shared vocabulary for an agent state; `time` is kept only for states that ran. */
export function agentStatus(state: AgentState, time?: string): Status {
  const timed = (status: Omit<Status, "time">): Status => time === undefined ? status : { ...status, time };
  switch (state) {
    case "preparing": return { glyph: "○", tone: "muted", word: "preparing" };
    case "queued": return { glyph: "○", tone: "muted", word: "queued" };
    case "running": return timed({ glyph: "●", tone: "accent", word: "running" });
    case "needs": return timed({ glyph: "▲", tone: "warning", word: "needs you" });
    case "done": return timed({ glyph: "✓", tone: "success", word: "done" });
    case "failed": return timed({ glyph: "✗", tone: "error", word: "failed" });
    case "cancelled": return timed({ glyph: "✗", tone: "error", word: "cancelled" });
    case "uncertain": return timed({ glyph: "!", tone: "warning", word: "uncertain" });
    case "not_started": return { glyph: "!", tone: "warning", word: "not started" };
  }
}
/** The short state text beside a glyph: elapsed time while running or once done, otherwise the word. */
export function statusLabel(status: Status): string {
  return status.tone === "accent" || status.tone === "success" ? status.time ?? status.word : status.word;
}
export function jobStatus(job: PanelJob, now: number): Status {
  const d = job.detail;
  const time = d?.startedAt === undefined ? undefined : duration((d.endedAt ?? now) - d.startedAt);
  const timed = (status: Omit<Status, "time">): Status => time === undefined ? status : { ...status, time };
  if (job.state === "running") return timed({ glyph: "●", tone: "accent", word: "running" });
  if (job.state !== "exited") return timed({ glyph: "!", tone: "warning", word: plainLabel(job.state) });
  if (d?.signal) return timed({ glyph: "✗", tone: "error", word: plainLabel(d.signal) });
  if (d?.exitCode === 0) return timed({ glyph: "✓", tone: "success", word: "exit 0" });
  if (d?.exitCode != null) return timed({ glyph: "✗", tone: "error", word: `exit ${d.exitCode}` });
  return timed({ glyph: "!", tone: "warning", word: "exit unknown" });
}
function helperState(helper: HelperInspection): AgentState {
  switch (helper.phase) {
    case "preparing": case "queued": case "running": return helper.phase;
    case "completed": return helper.error ? "failed" : "done";
    case "uncertain": return "uncertain";
    case "not_started": return "not_started";
  }
}
export function helperStatus(helper: HelperInspection, now: number): Status {
  const time = helper.startedAt === undefined ? undefined : duration((helper.endedAt ?? now) - helper.startedAt);
  return agentStatus(helperState(helper), time);
}
export function helperKindName(kind: HelperInspection["kind"]): string {
  switch (kind) { case "explore": return "explore"; case "oracle": return "oracle"; case "web_search": return "search"; case "shadow": return "shadow"; case "expert": return "expert"; }
}

function toolArgs(text: string | undefined): Readonly<Record<string, unknown>> | undefined {
  if (text === undefined) return undefined;
  try {
    const value: unknown = JSON.parse(text);
    return value !== null && typeof value === "object" && !Array.isArray(value) ? Object.fromEntries(Object.entries(value)) : undefined;
  } catch { return undefined; }
}
/** One plain line for a tool call: its name, then the argument that says what it touches (`read src/a.ts:10-20`). */
export function toolSummary(message: HelperMessage): string {
  const name = plainLabel(helperText(message.tool ?? "tool")), args = toolArgs(message.arguments);
  const field = (key: string) => { const value = args?.[key]; return typeof value === "string" || typeof value === "number" ? plainLabel(helperText(String(value))) : undefined; };
  const offset = Number(args?.["offset"]), limit = Number(args?.["limit"]);
  const range = Number.isInteger(offset) && offset > 0 ? `:${offset}${Number.isInteger(limit) && limit > 0 ? `-${offset + limit - 1}` : ""}` : "";
  const summary = !args ? plainLabel(helperText(message.arguments ?? "")).slice(0, 80)
    : name === "read" ? (field("path") ?? "") + range
    : name === "grep" ? [field("pattern") === undefined ? "" : `"${field("pattern")}"`, field("path"), field("glob")].filter(Boolean).join(" ")
    : field("command") ?? field("path") ?? field("pattern") ?? field("query") ?? field("url")
      ?? Object.values(args).filter((value): value is string => typeof value === "string").map(value => plainLabel(helperText(value))).join(" ");
  return (name + " " + summary.slice(0, 160)).trim();
}

/** One stretch of the timeline. `to` absent means still open (drawn up to now). */
export interface AgentSpan { readonly kind: "wait" | "run" | "ask"; readonly from: number; readonly to?: number }
export interface AgentEnd { readonly at: number; readonly outcome: "done" | "failed" | "cancelled" | "uncertain" }

/** Absent fields are unknown and render as "–", never as zero. */
export interface AgentUsage { readonly cost?: number; readonly contextPercent?: number; readonly tokens?: number }

/** What the selected row can do. Absent or false: not offered (no button, no hint, no F1 row). */
export interface AgentCapabilities {
  readonly open: boolean;          // Enter: open its conversation
  readonly transcript: boolean;    // t: bounded read-only JSONL
  readonly steer?: boolean;        // s: point the composer at it (build)
  readonly cancel?: boolean;       // x: cancel it (build)
  readonly model?: boolean;        // m: change its model for the next request (build)
  readonly answer?: boolean;       // Enter on a `needs` row opens its question (build)
}
/** Capability present but not usable in the current state: shown dim with this reason. */
export type Disabled = Partial<Record<keyof AgentCapabilities, string>>;

export interface AgentNote { readonly text: string; readonly tone: Tone }

/** What HelperChat renders. HelperInspection satisfies it; a build child maps to it. */
export interface ConversationSource {
  readonly request: string; readonly prompt: string; readonly phase: string;
  readonly messages: readonly HelperMessage[]; readonly result?: string;
}

export interface AgentRow {
  readonly key: string;                  // "helper:<request>" | "child:<id>"
  readonly source: "helper" | "child";
  readonly id: string;                   // request id or child id
  readonly name: string;                 // column text, unique enough to read
  readonly title: string;                // one line: prompt's first line or ChildSpec.title
  readonly hue: number;                  // index into agentColor()
  readonly profile: string;              // helper kind, or the child's profile
  readonly model: string;                // short id
  readonly pendingModel?: string;
  readonly state: AgentState;
  readonly status: Status;
  readonly createdAt: number; readonly startedAt?: number; readonly endedAt?: number;
  readonly activity: string;
  readonly usage?: AgentUsage;
  readonly files?: number; readonly outside?: number;
  readonly pulse?: readonly number[];    // 0..7 per bucket, newest last
  readonly spans: readonly AgentSpan[];
  readonly end?: AgentEnd;
  readonly owns?: readonly string[]; readonly after?: readonly string[];
  readonly steers?: readonly string[]; readonly question?: string;
  readonly meta: readonly string[];      // model id, tier, generation, session, parent
  readonly notes: readonly AgentNote[];
  readonly capabilities: AgentCapabilities;
  readonly disabled?: Disabled;
  readonly conversation?: ConversationSource;
}

export interface JobCapabilities {
  readonly output: boolean; readonly search: boolean; readonly follow: boolean;
  readonly peek: boolean; readonly background: boolean; readonly stop: boolean;
}
export interface JobRow {
  readonly key: string;                  // "job:<id>"
  readonly job: string; readonly command: string; readonly status: Status;
  readonly live: boolean; readonly capabilities: JobCapabilities; readonly source: PanelJob;
}

export interface AgentExtras {
  readonly agents: readonly AgentRow[];  // rendered before helper rows
  readonly maxParallel?: number;         // timeline caption "up to n at once"
  readonly origin?: number;              // timeline zero; default earliest createdAt
}
export interface AgentsModel {
  readonly agents: readonly AgentRow[];
  readonly jobs: readonly JobRow[];
  /** false: the session has no such source; the section is omitted and one muted line says so. */
  readonly sources: { readonly helpers: boolean; readonly jobs: boolean };
  readonly bound: boolean;
  readonly maxParallel?: number;
  readonly origin: number;
}

/** Callbacks for actions. The workbench implements open/transcript/job actions; build adds steer/cancel/model. */
export interface AgentActions {
  open(key: string): void;
  transcript(key: string): void;
  steer?(key: string): void;
  cancel?(key: string): void;
  model?(key: string): void;
}

/** Host observations only. Parentage is provenance, never a board dependency. The host fills it in session.ts. */
export interface AgentObservation {
  readonly id: string; readonly session: string; readonly parent?: string; readonly kind: string;
  readonly state: string; readonly model: string | null; readonly transcript: boolean; readonly tokens?: number;
}

const LIVE: Readonly<Record<AgentState, boolean>> = {
  preparing: true, queued: true, running: true, needs: true, done: false, failed: false, cancelled: false, uncertain: false, not_started: false,
};

function firstLine(text: string): string {
  return plainLabel(helperText(text.split("\n").find(line => line.trim()) ?? ""));
}
function helperActivity(helper: HelperInspection, state: AgentState): string {
  switch (state) {
    case "running": {
      const tool = helper.messages.filter(message => message.kind === "tool" && (message.phase === "running" || message.phase === "requested")).at(-1);
      return tool ? toolSummary(tool) : "writing answer";
    }
    case "preparing": case "queued": return "waiting for a helper slot";
    case "done": {
      const answer = helper.result === undefined ? "" : firstLine(helper.result);
      return answer ? `answered · ${answer}` : "answered";
    }
    case "failed": return firstLine(helper.error ?? "") || "failed";
    case "uncertain": return "interrupted · outcome uncertain";
    case "not_started": return "not started";
    default: return state;
  }
}
function helperSpans(helper: HelperInspection): AgentSpan[] {
  const created = helper.createdAt, started = helper.startedAt, ended = helper.endedAt;
  if (started === undefined) return [ended === undefined ? { kind: "wait", from: created } : { kind: "wait", from: created, to: ended }];
  return [...(started > created ? [{ kind: "wait" as const, from: created, to: started }] : []),
    ended === undefined ? { kind: "run", from: started } : { kind: "run", from: started, to: ended }];
}
function helperEnd(helper: HelperInspection, state: AgentState): AgentEnd | undefined {
  if (helper.endedAt === undefined) return undefined;
  const outcome = state === "done" ? "done" : state === "failed" ? "failed" : "uncertain";
  return { at: helper.endedAt, outcome };
}

/** One row per retained helper call, in host order (newest first). Helpers report no cost, context or files. */
export function helperRows(snapshot: PanelSnapshot | null, now: number): AgentRow[] {
  const helpers = snapshot?.helpers ?? [], bound = snapshot?.bound ?? false, agents = snapshot?.agents ?? [];
  // Names and hues follow creation order; a stable sort keeps host order (oldest last) for equal timestamps.
  const oldest = [...helpers].reverse().sort((a, b) => a.createdAt - b.createdAt);
  const counts = new Map<string, number>(), names = new Map<string, string>(), hues = new Map<string, number>();
  oldest.forEach((helper, index) => {
    const kind = helperKindName(helper.kind), count = (counts.get(kind) ?? 0) + 1;
    counts.set(kind, count); names.set(helper.request, count > 1 ? `${kind} ${count}` : kind); hues.set(helper.request, index);
  });
  return helpers.map((helper): AgentRow => {
    const state = helperState(helper), status = helperStatus(helper, now), kind = helperKindName(helper.kind);
    const agent = agents.find(item => item.id === helper.request);
    const parent = agent?.parent === undefined ? undefined : agents.find(item => item.session === agent.parent);
    const fast = helper.serviceTier === "priority";
    const meta = [`model ${plainLabel(helperText(helper.model))}`, ...(fast ? ["fast tier (priority)"] : []), `generation g${helper.generation}`,
      ...(agent ? [`session ${plainLabel(agent.session)}`] : []),
      ...(agent?.parent === undefined ? [] : [`parent ${parent ? plainLabel(parent.kind) : "session " + plainLabel(agent.parent)}`]),
      ...(agent?.tokens === undefined ? [] : [`${agent.tokens >= 1000 ? `${Number((agent.tokens / 1000).toFixed(1))}k` : agent.tokens} session tokens`])];
    const notes: AgentNote[] = [
      ...(helper.error ? [{ text: "Error: " + plainLabel(helperText(helper.error)), tone: "error" as const }] : []),
      ...(helper.inspectionError ? [{ text: "Inspector warning: " + plainLabel(helperText(helper.inspectionError)), tone: "warning" as const }] : []),
      ...(helper.truncated ? [{ text: "Some content was omitted by retention limits.", tone: "warning" as const }] : []),
      ...(bound ? [] : [{ text: "Host unbound · this observation may be stale.", tone: "warning" as const }]),
    ];
    const end = helperEnd(helper, state);
    return {
      key: `helper:${helper.request}`, source: "helper", id: helper.request,
      name: names.get(helper.request) ?? kind, title: firstLine(helper.prompt), hue: hues.get(helper.request) ?? 0,
      profile: kind, model: plainLabel(helperText(helper.model.slice(helper.model.lastIndexOf("/") + 1))) + (fast ? " fast" : ""),
      state, status, createdAt: helper.createdAt,
      ...(helper.startedAt === undefined ? {} : { startedAt: helper.startedAt }),
      ...(helper.endedAt === undefined ? {} : { endedAt: helper.endedAt }),
      activity: helperActivity(helper, state),
      ...(agent?.tokens === undefined ? {} : { usage: { tokens: agent.tokens } }),
      spans: helperSpans(helper), ...(end ? { end } : {}),
      meta, notes,
      capabilities: { open: true, transcript: bound && agent?.transcript === true },
      conversation: helper,
    };
  });
}

/** One row per background job in host order. Control needs a bound host, its session and the control seam. */
export function jobRows(snapshot: PanelSnapshot | null, now: number, control: boolean): JobRow[] {
  const bound = snapshot?.bound ?? false, controls = control && bound && snapshot?.inspector?.session !== undefined;
  return (snapshot?.jobs ?? []).map((job): JobRow => {
    const live = job.state === "running", output = bound && job.detail !== undefined && job.state !== "lost";
    return {
      key: `job:${job.job}`, job: job.job, command: plainLabel(job.detail?.command ?? job.job), status: jobStatus(job, now), live,
      capabilities: { output, search: output, follow: output && live, peek: controls && live, background: controls && live, stop: controls && live },
      source: job,
    };
  });
}

export function agentsModel(snapshot: PanelSnapshot | null, now: number,
  options?: { readonly jobControl?: boolean; readonly extra?: AgentExtras }): AgentsModel {
  const extra = options?.extra;
  const agents = [...extra?.agents ?? [], ...helperRows(snapshot, now)];
  const origin = extra?.origin ?? (agents.length ? Math.min(...agents.map(agent => agent.createdAt)) : now);
  return {
    agents, jobs: jobRows(snapshot, now, options?.jobControl ?? false),
    sources: { helpers: snapshot?.helpers != null, jobs: snapshot?.jobs != null },
    bound: snapshot?.bound ?? false,
    ...(extra?.maxParallel === undefined ? {} : { maxParallel: extra.maxParallel }),
    origin,
  };
}

/** Tab badge: live agents (preparing, queued, running, needs) plus running jobs. */
export function liveCount(model: AgentsModel): number {
  return model.agents.filter(agent => LIVE[agent.state]).length + model.jobs.filter(job => job.live).length;
}
/** Whether an agent state still counts as live (preparing, queued, running, needs). */
export function isLive(state: AgentState): boolean { return LIVE[state]; }
