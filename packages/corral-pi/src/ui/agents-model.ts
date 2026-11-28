// The row model behind the agents view, the sidebar Context tab and the Agents tab badge. Helper calls and
// commands come from the bound host's snapshot; fleet children (a build session's or any other role's) arrive as ready-made rows through
// `AgentExtras`. Unknown values stay absent and render as "–", never as zero.
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { HelperInspection, HelperMessage } from "../extensions/subagent/inspection.ts";
import { helperText, retentionNote, toolSummary } from "./helper-chat.ts";
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

const BARS = " ▁▂▃▄▅▆▇";
/**
 * The activity sparkline: the last `cells` 5-second buckets of tool calls as bars, right-aligned. Heights scale to
 * the busiest bucket shown, with four calls or more at full height so a quiet agent stays low; an idle bucket is blank.
 */
export function sparkline(pulse: readonly number[], cells: number): string {
  const shown = pulse.slice(-cells), top = Math.max(4, ...shown);
  return shown.map(value => value > 0 ? BARS[Math.max(1, Math.min(7, Math.round(value * 7 / top)))] ?? " " : " ").join("").padStart(cells, " ");
}

/** Glyph and word, so colour is never the only signal. */
export interface Status { readonly glyph: string; readonly tone: Tone; readonly word: string; readonly time?: string }

export type AgentState =
  | "preparing" | "queued" | "running" | "needs" | "stopping" | "done" | "failed" | "cancelled" | "uncertain" | "not_started";
/** The shared vocabulary for an agent state; `time` is kept only for states that ran. */
export function agentStatus(state: AgentState, time?: string): Status {
  const timed = (status: Omit<Status, "time">): Status => time === undefined ? status : { ...status, time };
  switch (state) {
    case "preparing": return { glyph: "○", tone: "muted", word: "preparing" };
    case "queued": return { glyph: "○", tone: "muted", word: "queued" };
    case "running": return timed({ glyph: "●", tone: "accent", word: "running" });
    case "needs": return timed({ glyph: "▲", tone: "warning", word: "needs you" });
    case "stopping": return timed({ glyph: "○", tone: "warning", word: "stopping" });
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
/** The time column and the conversation header: elapsed time while running or once done, `waiting` before it starts, otherwise the state word. */
export function timeLabel(status: Status): string {
  return status.tone === "muted" ? "waiting" : statusLabel(status);
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

/** One stretch of the timeline. `to` absent means still open (drawn up to now). */
export interface AgentSpan { readonly kind: "wait" | "run" | "ask"; readonly from: number; readonly to?: number }
export interface AgentEnd { readonly at: number; readonly outcome: "done" | "failed" | "cancelled" | "uncertain" }

/** Absent fields are unknown and render as "–", never as zero. */
export interface AgentUsage { readonly cost?: number; readonly contextPercent?: number; readonly tokens?: number }

/** What the selected row can do. Absent or false: not offered (no button, no hint, no F1 row). */
export interface AgentCapabilities {
  readonly open: boolean;          // Enter: open its conversation
  readonly transcript: boolean;    // t: paged read-only JSONL
  readonly steer?: boolean;        // s: point the composer at it (build)
  readonly cancel?: boolean;       // x: cancel it (build)
  readonly model?: boolean;        // m: change its model for the next request (build)
  readonly answer?: boolean;       // a: dock its question (build)
}
/** Capability present but not usable in the current state: shown dim with this reason. */
export type Disabled = Partial<Record<keyof AgentCapabilities, string>>;

export interface AgentNote {
  readonly text: string; readonly tone: Tone;
  /** The retention note: the conversation draws it under the prompt, where the dropped messages were. */
  readonly retention?: boolean;
}

/** What HelperChat renders. HelperInspection satisfies it; a build child maps to it. */
export interface ConversationSource {
  readonly request: string; readonly prompt: string; readonly phase: string;
  readonly messages: readonly HelperMessage[]; readonly result?: string;
  /** Some content was clipped or dropped by the retention limit; `dropped` counts the messages dropped from the front. */
  readonly truncated?: boolean; readonly dropped?: number;
}

/**
 * One message between agents, as a conversation shows it: sender and recipient (an agent id, `parent`, `owner`, `all`
 * or another session's key), when it was sent and how it was delivered; `fromHue`/`toHue` when that side is a known agent.
 */
export interface AgentMail {
  readonly at: number; readonly from: string; readonly to: string; readonly text: string;
  readonly delivery: "delivered" | "queued" | "undeliverable" | "answered_from_report";
  readonly fromHue?: number; readonly toHue?: number;
}
/**
 * The messages agent `id` sent or received, oldest first. A message to `all` reached the agents live when it was sent,
 * so it counts for `id` while `live` covers its time (`from` the agent's creation, `to` its end; open while it runs).
 * `hue` gives another agent's hue by id, undefined for `parent`, `owner`, `all` and keys of other sessions.
 */
export function agentMail(id: string, mail: readonly Omit<AgentMail, "fromHue" | "toHue">[], live: { readonly from: number; readonly to?: number },
  hue: (id: string) => number | undefined): AgentMail[] {
  return mail.filter(message => message.from === id || message.to === id
    || (message.to === "all" && message.at >= live.from && (live.to === undefined || message.at <= live.to)))
    .map(message => {
      const fromHue = hue(message.from), toHue = hue(message.to);
      return { at: message.at, from: message.from, to: message.to, text: message.text, delivery: message.delivery,
        ...(fromHue === undefined ? {} : { fromHue }), ...(toHue === undefined ? {} : { toHue }) };
    });
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
  /** When each steer was sent, index for index with `steers`; a steer without a time is drawn after the replies. */
  readonly steeredAt?: readonly number[];
  /** Messages it sent or received from other agents, oldest first; drawn among the replies at the time they were sent. Absent: none. */
  readonly mail?: readonly AgentMail[];
  readonly meta: readonly string[];      // model id, tier, session, parent
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
  readonly messages?: number;            // timeline caption "n messages"; absent or 0: not shown
  readonly origin?: number;              // timeline zero; default earliest createdAt
}
export interface AgentsModel {
  readonly agents: readonly AgentRow[];
  readonly jobs: readonly JobRow[];
  /** false: the session has no such source, so it contributes no rows (unknown, never reported as an empty list). */
  readonly sources: { readonly helpers: boolean; readonly jobs: boolean };
  readonly bound: boolean;
  readonly maxParallel?: number;
  readonly messages?: number;
  readonly origin: number;
}

/** Callbacks for actions. The workbench implements open/transcript/job actions; build adds steer/cancel/model/answer. */
export interface AgentActions {
  open(key: string): void;
  transcript(key: string): void;
  steer?(key: string): void;
  cancel?(key: string): void;
  model?(key: string): void;
  answer?(key: string): void;
}

/** Host observations only. Parentage is provenance, never a board dependency. The host fills it in session.ts. */
export interface AgentObservation {
  readonly id: string; readonly session: string; readonly parent?: string; readonly kind: string;
  readonly state: string; readonly model: string | null; readonly transcript: boolean; readonly tokens?: number;
}

const LIVE: Readonly<Record<AgentState, boolean>> = {
  preparing: true, queued: true, running: true, needs: true, stopping: true, done: false, failed: false, cancelled: false, uncertain: false, not_started: false,
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

/** One row per retained agent helper, in host order (newest first). Web searches are tool calls, not agents. */
export function helperRows(snapshot: PanelSnapshot | null, now: number): AgentRow[] {
  const helpers = snapshot?.helpers?.filter(helper => helper.kind !== "web_search") ?? [];
  const bound = snapshot?.bound ?? false, agents = snapshot?.agents ?? [];
  // Names and hues follow creation order; a stable sort keeps host order (oldest last) for equal timestamps.
  const oldest = [...helpers].reverse().sort((a, b) => a.createdAt - b.createdAt);
  const counts = new Map<string, number>(), names = new Map<string, string>(), hues = new Map<string, number>();
  oldest.forEach((helper, index) => {
    const kind = helper.kind, count = (counts.get(kind) ?? 0) + 1;
    counts.set(kind, count); names.set(helper.request, count > 1 ? `${kind} ${count}` : kind); hues.set(helper.request, index);
  });
  return helpers.map((helper): AgentRow => {
    const state = helperState(helper), status = helperStatus(helper, now), kind = helper.kind;
    const agent = agents.find(item => item.id === helper.request);
    const parent = agent?.parent === undefined ? undefined : agents.find(item => item.session === agent.parent);
    const fast = helper.serviceTier === "priority";
    const meta = [`model ${plainLabel(helperText(helper.model))}`, ...(fast ? ["fast tier (priority)"] : []),
      ...(agent ? [`session ${plainLabel(agent.session)}`] : []),
      ...(agent?.parent === undefined ? [] : [`parent ${parent ? plainLabel(parent.kind) : "session " + plainLabel(agent.parent)}`]),
      ...(agent?.tokens === undefined ? [] : [`${agent.tokens >= 1000 ? `${Number((agent.tokens / 1000).toFixed(1))}k` : agent.tokens} session tokens`])];
    const notes: AgentNote[] = [
      ...(helper.error ? [{ text: "Error: " + plainLabel(helperText(helper.error)), tone: "error" as const }] : []),
      ...(helper.inspectionError ? [{ text: "Inspector warning: " + plainLabel(helperText(helper.inspectionError)), tone: "warning" as const }] : []),
      ...(helper.truncated ? [{ text: retentionNote(helper), tone: "warning" as const, retention: true }] : []),
      ...(bound ? [] : [{ text: "Host closed · this observation may be stale.", tone: "warning" as const }]),
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

/** One row per retained command. Control needs a bound host, its session and the control seam. */
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

/**
 * An explore call that is also a fleet child: the fleet runtime names the child `e-` and the first 30 characters of
 * the research request, and a helper observation of the same call carries that request (or the child id) as its own.
 */
function exploreChild(helper: AgentRow, children: ReadonlySet<string>): boolean {
  return helper.profile === "explore" && (children.has(helper.id) || children.has("e-" + helper.id.slice(0, 30)));
}

/**
 * Fleet children (`extra`) lead, then the helper observations that are not fleet children (an explore call shows
 * once, as its fleet row), their hues continuing after the children's so no two rows share a colour.
 */
export function agentsModel(snapshot: PanelSnapshot | null, now: number,
  options?: { readonly jobControl?: boolean; readonly extra?: AgentExtras }): AgentsModel {
  const extra = options?.extra, children = extra?.agents ?? [], ids = new Set(children.map(child => child.id));
  const helpers = children.length
    ? helperRows(snapshot, now).filter(helper => !exploreChild(helper, ids)).map(helper => ({ ...helper, hue: helper.hue + children.length }))
    : helperRows(snapshot, now);
  const agents = [...children, ...helpers];
  const origin = extra?.origin ?? (agents.length ? Math.min(...agents.map(agent => agent.createdAt)) : now);
  return {
    agents, jobs: jobRows(snapshot, now, options?.jobControl ?? false),
    sources: { helpers: snapshot?.helpers != null, jobs: snapshot?.jobs != null },
    bound: snapshot?.bound ?? false,
    ...(extra?.maxParallel === undefined ? {} : { maxParallel: extra.maxParallel }),
    ...(extra?.messages ? { messages: extra.messages } : {}),
    origin,
  };
}

/** Keep running commands in host order, then the last to exit, not the last to start. */
export function visibleCommands(jobs: readonly JobRow[]): JobRow[] {
  const running: JobRow[] = [];
  let latest: JobRow | undefined;
  for (const job of jobs) {
    if (job.live) running.push(job);
    else if (job.source.state === "exited" && (!latest
      || (job.source.detail?.endedAt ?? -Infinity) > (latest.source.detail?.endedAt ?? -Infinity))) latest = job;
  }
  if (latest) running.push(latest);
  return running;
}

/** Tab badge: live agents (preparing, queued, running, needs) plus running commands. */
export function liveCount(model: AgentsModel): number {
  return model.agents.filter(agent => LIVE[agent.state]).length + model.jobs.filter(job => job.live).length;
}
/** Whether an agent state still counts as live (preparing, queued, running, needs). */
export function isLive(state: AgentState): boolean { return LIVE[state]; }

const AGENT_GROUP: Readonly<Record<AgentState, number>> = {
  needs: 0, running: 1, stopping: 1, queued: 2, preparing: 2, not_started: 2, failed: 3, cancelled: 4, uncertain: 3, done: 4,
};
/** Group by urgency without disturbing host order within a group. */
export function orderedAgents(agents: readonly AgentRow[]): AgentRow[] {
  return [...agents].sort((a, b) => (a.key === "retained-picker" ? 5 : AGENT_GROUP[a.state]) - (b.key === "retained-picker" ? 5 : AGENT_GROUP[b.state]));
}
