// The tool row kit: one grammar for every tool call in the transcript. A presenter turns a call into a `RowModel`;
// the kit draws every model the same way: `<state glyph> <verb in accent> <target> · <outcome>` with the time
// right-aligned in muted, then at most a few indented body rows. State is always a glyph and words, never colour
// alone. Rows never print a key or the word click: a click on the row or Ctrl+O expands it (the Chat footer names the
// key), and hidden content is only a muted count. Presenters are pure: they read the call (and, through their own
// deps, live host state) and return a model.
//
// Writing a presenter:
//   const presentLs: Presenter = call => {
//     const state = baseState(call);
//     const path = stringArg(call.args, "path") ?? ".";
//     const lines = state === "ok" ? resultText(call.result).split("\n").filter(Boolean).length : 0;
//     return { state, verb: "ls", target: shortPath(path, call.cwd), targetClip: "middle",
//       ...(state === "ok" ? { outcome: `${lines} entries` } : {}),
//       ...(state === "failed" ? { outcome: firstLine(resultText(call.result)) } : {}) };
//   };
// Register it through a `PresenterModule` in `src/ui/tool-rendering.ts`; nested rows use `rowLine`.
//
// Folding: a settled successful row without body offers the transcript a group member (`groupMember()` on the row
// component) when its renderers carry a `RowGrouping` (tool-group.ts). Pi's transcript folds a run of members of one
// kind into the group row the grouping draws; the rows themselves stay as they are.
import { homedir } from "node:os";
import { isAbsolute, relative, resolve } from "node:path";

import type { AgentToolResult, ToolRenderers } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences, truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { Component, TuiMouseEvent, TuiMouseEventResult } from "@earendil-works/pi-tui";

import type { Ticker } from "./activity.ts";
import { spinner } from "./frame.ts";
import { PANEL_BLUE, PANEL_ERROR, PANEL_MUTED, PANEL_SUCCESS, PANEL_TEXT, PANEL_WARNING, RESET, plainLabel } from "./panels-view.ts";

/** What the call is doing now. `writing`: its arguments are still streaming; `waiting`: queued or started but not finished here. */
export type RowState = "writing" | "running" | "ok" | "failed" | "waiting" | "needs-you" | "unknown";
/**
 * One body row under the row line. The kit clips it to the width (collapsed) or wraps it (expanded). With `wrap`, the
 * kit wraps this one logical line to the width even when collapsed and keeps at most `max` rows, from the start
 * (default) or the last `max` with `from: "end"`, marking the cut with `…`: prose such as a question or streaming text.
 */
export interface BodyLine { readonly text: string; readonly tone?: Tone; readonly wrap?: { readonly max: number; readonly from?: "start" | "end" } }
/** Text tones; `added` and `removed` are diff rows. */
export type Tone = "muted" | "text" | "error" | "success" | "warning" | "added" | "removed";
/** A clickable accent label at the end of the row line, e.g. `Agents` to open the job in the Agents view. */
export interface RowView { readonly label: string; open(): void }

export interface RowModel {
  readonly state: RowState;
  /** Accent; what the call does: `bash`, `read`, `Finished the run`. Clipped only as a last resort. */
  readonly verb: string;
  /** Text colour; what it touches. Clips first; `targetClip: "middle"` keeps the end (a file name) visible. */
  readonly target?: string;
  readonly targetClip?: "end" | "middle";
  /** After ` · `; the result in words. Clips last. Defaults: `failed`, `outcome unknown`, `needs you` for those states. */
  readonly outcome?: string;
  /**
   * Muted words after the outcome (` · detail`), e.g. a command's last output line. Shown only when the target and
   * outcome fit whole and at least 8 cells remain; the first thing dropped.
   */
  readonly detail?: string;
  /** Defaults by state: error for failed, warning for unknown and needs-you, muted otherwise. */
  readonly outcomeTone?: Tone;
  /** Muted, right-aligned, before the time. Dropped before the outcome clips. */
  readonly right?: string;
  /**
   * Overrides the time the kit shows: a number is a final duration in ms, `{ since }` an elapsed clock from that
   * epoch ms (a background job that outlives its call). Without it the kit shows `writing`, the elapsed time while
   * running, or the call's final duration.
   */
  readonly duration?: number | { readonly since: number };
  /** Collapsed rows under the line, indented 2 and clipped. A successful call has none; a failure at most 3. */
  readonly body?: readonly BodyLine[];
  /** The full view when the row is expanded (click or Ctrl+O). Without it the expanded row shows `body`. */
  readonly expanded?: (width: number) => readonly BodyLine[] | Component;
  /**
   * A count of what the collapsed row hides, e.g. `41 more lines` or `11 lines`. The kit shows it muted (beside the
   * row, or after the last body row as `… 41 more lines`) only when it holds a number the outcome does not already
   * state; generic nouns (`contents`, `diff`, `source`) are not shown. Pass `expanded` for what opens.
   */
  readonly hiddenHint?: string;
  readonly view?: RowView;
  /** Ticker hold id while the row must keep redrawing after its tool returned (background job, detached cell). */
  readonly live?: string;
}

/** Everything a presenter may read about one call. */
export interface CallInput {
  readonly name: string;
  readonly args: Record<string, unknown>;
  readonly argsComplete: boolean;
  readonly executionStarted: boolean;
  /** The final result, or the latest streaming update when `partial`. */
  readonly result?: AgentToolResult<unknown>;
  readonly partial: boolean;
  readonly isError: boolean;
  /** Final duration as recorded by Pi; absent while running and for some restored rows. */
  readonly durationMs?: number;
  /** Epoch ms of the first render after execution started. */
  readonly startedAt?: number;
  readonly expanded: boolean;
  readonly cwd: string;
  /**
   * Set when `result` carries only a preview of the text (a Code Mode child record): the real totals. Counting
   * presenters use these, and print no count for a clipped text whose total is missing (`outputLines`).
   */
  readonly totals?: { readonly lines?: number; readonly bytes?: number };
  readonly toolCallId: string;
  /** The call never ran: the owner stopped the turn before it started. */
  readonly notRun?: boolean;
}
export type Presenter = (call: CallInput) => RowModel;
/** What the kit itself needs from the host. */
export interface RowDeps { ticker(): Ticker | undefined }

const TONE: Readonly<Record<Tone, () => string>> = {
  muted: () => PANEL_MUTED, text: () => PANEL_TEXT, error: () => PANEL_ERROR, success: () => PANEL_SUCCESS,
  warning: () => PANEL_WARNING, added: () => PANEL_SUCCESS, removed: () => PANEL_ERROR,
};
const BODY_INDENT = 2;
const CONTROL = /[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g;

// ---------------------------------------------------------------------------------------------------------------
// Helpers for presenters

/** The call's state from Pi's lifecycle alone: writing, running, failed or ok. */
export function baseState(call: CallInput): RowState {
  if (!call.result || call.partial) return call.argsComplete || call.executionStarted ? "running" : "writing";
  return call.isError ? "failed" : "ok";
}
/** The text blocks of a result, joined; empty without a result. */
export function resultText(result: AgentToolResult<unknown> | undefined): string {
  return result ? result.content.flatMap(part => part.type === "text" ? [part.text] : []).join("\n") : "";
}
/** A string argument, or undefined when absent or not a string. */
export function stringArg(args: Readonly<Record<string, unknown>>, key: string): string | undefined {
  const value = args[key];
  return typeof value === "string" ? value : undefined;
}
/** A finite number argument (numeric strings count), or undefined. */
export function numberArg(args: Readonly<Record<string, unknown>>, key: string): number | undefined {
  const value = args[key];
  const number = typeof value === "string" && value.trim() !== "" ? Number(value) : value;
  return typeof number === "number" && Number.isFinite(number) ? number : undefined;
}
/** `value` when it is a plain object, else undefined: for reading `result.details`. */
export function recordOf(value: unknown): Readonly<Record<string, unknown>> | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? Object.fromEntries(Object.entries(value)) : undefined;
}
/** The first non-blank line, flattened. */
export function firstLine(text: string): string {
  return plainLabel(stripTerminalSequences(text.split("\n").find(line => line.trim() !== "") ?? ""));
}
/** The last `count` non-blank lines, in order, as muted (or `tone`) body rows. */
export function tailLines(text: string, count: number, tone: Tone = "muted"): BodyLine[] {
  return text.split("\n").filter(line => line.trim() !== "").slice(-count).map(line => ({ text: line, tone }));
}
/** Non-blank line count. */
export function lineCount(text: string): number {
  return text.split("\n").filter(line => line.trim() !== "").length;
}
/** The result's real line count: `call.totals.lines` when the text is a clipped preview (undefined if unknown), else counted. */
export function outputLines(call: Pick<CallInput, "totals">, text: string): number | undefined {
  return call.totals ? call.totals.lines : lineCount(text);
}
/** The result's real size in bytes: `call.totals.bytes` when the text is a clipped preview (undefined if unknown), else measured. */
export function outputBytes(call: Pick<CallInput, "totals">, text: string): number | undefined {
  return call.totals ? call.totals.bytes : Buffer.byteLength(text);
}
/** Body rows for the full text, for `expanded`. */
export function textLines(text: string, tone: Tone = "muted"): BodyLine[] {
  return text.replace(/\s+$/u, "").split("\n").map(line => ({ text: line, tone }));
}
/** A path relative to `cwd` when inside it, otherwise with the home directory as `~`. */
export function shortPath(path: string, cwd: string): string {
  const absolute = isAbsolute(path) ? path : resolve(cwd, path);
  const inside = relative(cwd, absolute);
  if (inside === "") return ".";
  if (!inside.startsWith("..") && !isAbsolute(inside)) return inside;
  const home = homedir();
  return absolute === home || absolute.startsWith(home + "/") ? "~" + absolute.slice(home.length) : absolute;
}
/** `text` clipped to `width` cells by removing its middle, keeping the end (a file name) whole where it fits. */
export function clipMiddle(text: string, width: number): string {
  if (visibleWidth(text) <= width) return text;
  if (width <= 1) return width === 1 ? "…" : "";
  const slash = text.lastIndexOf("/"), base = slash >= 0 ? text.slice(slash) : text;
  // A file name (with its range) that fits stays whole and the directories are clipped. A longer one keeps a short head
  // (up to 8 cells: a task/run prefix) and the rest of its end, so both the prefix and the extension survive.
  const name = visibleWidth(base);
  const tail = name <= width - 1 ? Math.max(Math.ceil((width - 1) * 0.6), name) : width - 1 - Math.min(8, Math.floor((width - 1) * 0.35));
  return clipPlain(text, width - 1 - tail, "") + "…" + lastCells(text, tail);
}
/**
 * Plain `text` clipped to `width` cells, ending with `ellipsis` when cut. Unlike pi-tui's `truncateToWidth` it adds no
 * escape sequence, so a clipped piece keeps the colour it is later wrapped in.
 */
export function clipPlain(text: string, width: number, ellipsis = "…"): string {
  if (visibleWidth(text) <= width) return text;
  const room = Math.max(0, width - visibleWidth(ellipsis));
  let out = "", used = 0;
  for (const char of text) {
    const cells = visibleWidth(char);
    if (used + cells > room) break;
    out += char; used += cells;
  }
  return width <= 0 ? "" : out + ellipsis;
}
function lastCells(text: string, width: number): string {
  const chars = Array.from(text);
  let out = "", used = 0;
  for (let index = chars.length - 1; index >= 0; index--) {
    const char = chars[index]!, cells = visibleWidth(char);
    if (used + cells > width) break;
    out = char + out; used += cells;
  }
  return out;
}
/** A final duration: `0.4s`, `6.5s`, `12s`, `1m 12s`; empty below 100 ms. */
export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 100) return "";
  return ms < 10_000 ? `${(ms / 1000).toFixed(1)}s` : formatElapsed(ms);
}
/** A running clock or a limit: `0s`, `12s`, `1m 12s`, `2m`, `2h 3m`, `1h`. */
export function formatElapsed(ms: number): string {
  const seconds = Math.max(0, Math.floor((Number.isFinite(ms) ? ms : 0) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return seconds % 60 ? `${minutes}m ${seconds % 60}s` : `${minutes}m`;
  return minutes % 60 ? `${Math.floor(minutes / 60)}h ${minutes % 60}m` : `${Math.floor(minutes / 60)}h`;
}

// ---------------------------------------------------------------------------------------------------------------
// Drawing

function flat(text: string): string {
  return plainLabel(stripTerminalSequences(text));
}
function bodyText(text: string): string {
  return stripTerminalSequences(text).replace(/\t/g, "  ").replace(/\r/g, "").replace(CONTROL, " ");
}
function glyph(state: RowState, now: number): string {
  switch (state) {
    case "writing": case "running": return PANEL_BLUE + spinner(now);
    case "ok": return PANEL_SUCCESS + "✓";
    case "failed": return PANEL_ERROR + "✗";
    case "waiting": return PANEL_MUTED + "○";
    case "needs-you": return PANEL_WARNING + "▲";
    case "unknown": return PANEL_WARNING + "!";
  }
}
function defaultOutcome(state: RowState): string | undefined {
  return state === "failed" ? "failed" : state === "unknown" ? "outcome unknown" : state === "needs-you" ? "needs you" : undefined;
}
function defaultTone(state: RowState): Tone {
  return state === "failed" ? "error" : state === "unknown" || state === "needs-you" ? "warning" : "muted";
}
/**
 * The time slot of a row: the model's override, else `writing`, else the running clock from `startedAt`, else
 * `finalMs`. An explicit final duration (nested rows) under 100 ms reads `<n>ms`; a call's own one stays hidden.
 */
export function timeOf(model: RowModel, now: number, startedAt?: number, finalMs?: number): string {
  if (typeof model.duration === "number") return model.duration >= 0 && model.duration < 100 ? `${Math.round(model.duration)}ms` : formatDuration(model.duration);
  if (model.duration) return formatElapsed(now - model.duration.since);
  if (model.state === "writing") return "writing";
  if (model.state === "running") return startedAt === undefined ? "" : formatElapsed(now - startedAt);
  return finalMs === undefined ? "" : formatDuration(finalMs);
}

interface Laid { readonly line: string; readonly view?: { readonly from: number; readonly to: number } }
/**
 * The row line at `width`. Glyph, verb and time never drop. The hint, the right label and the view drop first. Then
 * the outcome keeps its natural width up to 40% of the row and the target takes the rest, never less than 12 cells
 * (or its whole text when shorter); a target that fits gives the outcome its room back. A middle-clipped target keeps
 * its whole last path segment when that fits. The verb clips last. The detail shows only when target and outcome are
 * whole and at least 8 cells remain.
 */
function layout(model: RowModel, width: number, now: number, time: string, hint?: string): Laid {
  const W = Math.max(1, width);
  let verb = flat(model.verb) || "tool", target = model.target ? flat(model.target) : "";
  let outcome = flat(model.outcome ?? defaultOutcome(model.state) ?? "");
  let right = model.right ? flat(model.right) : "", tip = hint ? flat(hint) : "", view = model.view ? flat(model.view.label) : "";
  const rightWidth = () => [right, tip, time, view].filter(Boolean).reduce((sum, part, index) => sum + (index ? 2 : 0) + visibleWidth(part), 0);
  const leftWidth = () => 2 + visibleWidth(verb) + (target ? 1 + visibleWidth(target) : 0) + (outcome ? 3 + visibleWidth(outcome) : 0);
  const total = () => leftWidth() + (rightWidth() ? 2 + rightWidth() : 0);
  if (total() > W) tip = "";
  if (total() > W) right = "";
  if (total() > W) view = "";
  const fullTarget = target, fullOutcome = outcome;
  if (total() > W) {
    // Cells for ` <target>` and ` · <outcome>` after the glyph, the verb and the right side.
    const room = W - 2 - visibleWidth(verb) - (rightWidth() ? 2 + rightWidth() : 0);
    const targetWidth = visibleWidth(target), outcomeWidth = visibleWidth(outcome);
    // A short outcome (a third of the row or less: `exit 0`, `no diagnostics`) stays whole and the target takes what is
    // left (at least 4 cells); a longer one is capped at 40% and the target keeps at least 12 cells.
    const short = outcomeWidth <= Math.floor(W / 3);
    let outcomeRoom = outcome ? (short ? outcomeWidth : Math.min(outcomeWidth, Math.floor(W * 0.4))) : 0;
    let targetRoom = target ? room - 1 - (outcome ? 3 + outcomeRoom : 0) : 0;
    const floor = Math.min(targetWidth, short ? 4 : 12);
    if (target && targetRoom < floor) targetRoom = floor;
    if (target && targetRoom > targetWidth) targetRoom = targetWidth;
    if (outcome) outcomeRoom = room - (target ? 1 + targetRoom : 0) - 3;
    if (target && targetRoom < targetWidth) {
      target = targetRoom < 1 ? "" : model.targetClip === "middle" ? clipMiddle(target, targetRoom) : clipPlain(target, targetRoom);
    }
    if (outcome && outcomeRoom < outcomeWidth) outcome = outcomeRoom >= 2 ? clipPlain(outcome, outcomeRoom) : "";
  }
  if (total() > W) verb = clipPlain(verb, Math.max(1, visibleWidth(verb) - (total() - W)));
  const spare = W - total() - 3;
  const whole = target === fullTarget && outcome === fullOutcome;
  const detail = whole && model.detail && spare >= 8 ? clipPlain(flat(model.detail), spare) : "";
  if (detail) outcome = outcome ? `${outcome} · ${detail}` : detail;
  const tone = TONE[model.outcomeTone ?? defaultTone(model.state)]();
  const left = glyph(model.state, now) + " " + PANEL_BLUE + verb + (target ? " " + PANEL_TEXT + target : "")
    + (outcome ? PANEL_MUTED + " · " + tone + outcome : "");
  const parts = [right, tip, time].filter(Boolean).map(part => PANEL_MUTED + part);
  if (view) parts.push(PANEL_BLUE + view);
  const gap = rightWidth() ? Math.max(2, W - leftWidth() - rightWidth()) : 0;
  const line = truncateToWidth(left + " ".repeat(gap) + parts.join("  "), W, "…") + RESET;
  return view ? { line, view: { from: W - visibleWidth(view), to: W } } : { line };
}

/** One row as a single styled line (state glyph first, time last), clipped to `width`: for nested rows. */
export function rowLine(model: RowModel, width: number, now: number): string {
  return layout(model, width, now, timeOf(model, now), model.body?.length ? undefined : hintOf(model)).line;
}

/**
 * What the kit says about hidden content: a muted count (`11 lines`), only when it has a number and the outcome does
 * not already state it. Generic nouns (`contents`, `diff`, `source`, `full text`) and keys are never shown: a click
 * on the row or Ctrl+O opens it, and the Chat footer names Ctrl+O.
 */
function hintOf(model: RowModel): string | undefined {
  const hidden = model.hiddenHint ? flat(model.hiddenHint) : "";
  return /\d/.test(hidden) && !(model.outcome ?? "").includes(hidden) ? hidden : undefined;
}
/** Body rows, the last one ending with the muted `suffix` (`… 12 more lines`) when given. Text is clipped plain, then styled. */
function bodyRows(lines: readonly BodyLine[], width: number, wrap: boolean, suffix?: string): string[] {
  const room = Math.max(1, width - BODY_INDENT), indent = " ".repeat(BODY_INDENT);
  const rows = lines.flatMap(line => {
    const color = TONE[line.tone ?? "muted"]();
    const plain = !wrap && line.wrap ? boundedWrap(bodyText(line.text).replace(/\s*\n\s*/g, " "), room, line.wrap)
      : bodyText(line.text).split("\n").flatMap(text => wrap ? wrapTextWithAnsi(text, room) : [clipPlain(text, room)]);
    return plain.map(text => ({ text, color }));
  });
  const last = rows[rows.length - 1];
  if (suffix && last) {
    const tail = ` ${suffix}`, keep = Math.max(0, room - visibleWidth(tail));
    return rows.map(row => row === last
      ? indent + row.color + clipPlain(row.text, keep) + PANEL_MUTED + clipPlain(tail, room - Math.min(keep, visibleWidth(row.text)), "") + RESET
      : indent + row.color + row.text + RESET);
  }
  return rows.map(row => indent + row.color + row.text + RESET);
}
/** `text` wrapped to `room` and cut to `max` rows from the start or the end, the cut marked with `…`. */
function boundedWrap(text: string, room: number, limit: NonNullable<BodyLine["wrap"]>): string[] {
  const all = wrapTextWithAnsi(text, room), max = Math.max(1, Math.floor(limit.max));
  if (all.length <= max) return all;
  if (limit.from === "end") {
    const kept = all.slice(-max);
    return ["…" + clipPlain(kept[0]!, room - 1, ""), ...kept.slice(1)];
  }
  const kept = all.slice(0, max), last = kept.length - 1;
  kept[last] = clipPlain(kept[last]!, room - 1, "") + "…";
  return kept;
}

/**
 * The whole row at `width`: the line, then the collapsed body (its last row ending with `… <hidden count>`), or the
 * expanded view. `time` is the time slot; `rowLine` and the renderers compute it.
 */
export function rowBlock(model: RowModel, width: number, now: number, expanded: boolean, time = timeOf(model, now)): { readonly lines: string[]; readonly view?: { readonly from: number; readonly to: number } } {
  const body = model.body ?? [], hint = expanded ? undefined : hintOf(model);
  const laid = layout(model, width, now, time, body.length ? undefined : hint);
  const lines = [laid.line];
  if (expanded && model.expanded) {
    const full = model.expanded(Math.max(1, width - BODY_INDENT));
    lines.push(...("render" in full ? full.render(Math.max(1, width - BODY_INDENT)).map(row => " ".repeat(BODY_INDENT) + row)
      : bodyRows(full, width, true)));
  } else {
    lines.push(...bodyRows(body, width, expanded, hint ? `… ${hint}` : undefined));
  }
  return laid.view ? { lines, view: laid.view } : { lines };
}

// ---------------------------------------------------------------------------------------------------------------
// Pi renderers

type RenderContext = Parameters<NonNullable<ToolRenderers["renderCall"]>>[2];
interface CallFields {
  readonly args: Record<string, unknown>; readonly argsComplete: boolean; readonly executionStarted: boolean;
  readonly expanded: boolean; readonly cwd: string; readonly toolCallId: string; readonly notRun: boolean;
}

/** One settled row as a fold sees it. */
export interface GroupEntry { readonly model: RowModel; readonly call: CallInput }
/** Which rows are of one kind and from how many consecutive rows they fold. */
export interface GroupKind { readonly key: string; readonly min: number }
/** How the rows of one tool fold (tool-group.ts). */
export interface RowGrouping {
  /** The kind of a settled successful collapsed row without body; undefined when it never folds. */
  kind(entry: GroupEntry): GroupKind | undefined;
  /** The group row of `entries` (oldest first, all of one kind) on a row `room` cells wide. */
  summary(entries: readonly GroupEntry[], room: number): RowModel;
}
/** What a group row draws, as Pi's transcript takes it. */
export interface GroupRow {
  readonly lines: readonly string[];
  handleMouse?(event: TuiMouseEvent): TuiMouseEventResult | undefined;
}
/** A row's membership of a fold, as Pi's transcript asks for it (`TranscriptGroupMember`). */
export interface GroupMember {
  readonly key: string;
  readonly min: number;
  render(run: readonly GroupMember[], width: number): GroupRow;
}

/** The group row of `entries` drawn with the kit at `width`, inset like the rows it folds. */
function groupRow(grouping: RowGrouping, entries: readonly GroupEntry[], width: number, inset: number, now: number): GroupRow {
  const pad = Math.max(0, Math.min(inset, Math.floor((width - 1) / 2))), room = Math.max(1, width - pad * 2);
  const model = grouping.summary(entries, room);
  const block = rowBlock(model, room, now, false, timeOf(model, now));
  const lines = block.lines.map(line => " ".repeat(pad) + line);
  const view = block.view, open = model.view;
  if (!view || !open) return { lines };
  return {
    lines,
    handleMouse: event => {
      if (event.type !== "click" || event.button !== "left" || event.y !== 0) return undefined;
      if (event.x < view.from + pad || event.x >= view.to + pad) return undefined;
      open.open();
      return { handled: true };
    },
  };
}

/** One settled row's membership of a fold; the transcript hands every member of a run to the first one's `render`. */
class RowMember implements GroupMember {
  constructor(readonly key: string, readonly min: number, readonly entry: GroupEntry, private readonly grouping: RowGrouping,
    private readonly inset: number) {}
  render(run: readonly GroupMember[], width: number): GroupRow {
    const entries = run.flatMap(member => member instanceof RowMember ? [member.entry] : []);
    return groupRow(this.grouping, entries, width, this.inset, Date.now());
  }
}

/** One tool call's row: recomputed at render time while it is writing, running or live, cached once settled. */
class ToolRow implements Component {
  private fields: CallFields | undefined;
  private result: AgentToolResult<unknown> | undefined;
  private partial = false;
  private isError = false;
  private durationMs: number | undefined;
  private startedAt: number | undefined;
  private inset = 1;
  private cached: RowModel | undefined;
  private member: RowMember | undefined;
  private view: { readonly from: number; readonly to: number; readonly open: () => void } | undefined;
  private hold: { readonly id: string; readonly release: () => void } | undefined;
  constructor(private readonly name: string, private readonly present: Presenter, private readonly deps: RowDeps,
    private readonly grouping?: RowGrouping) {}

  setCall(fields: CallFields, inset: number): void {
    this.fields = fields; this.inset = inset; this.cached = undefined;
  }
  setResult(result: AgentToolResult<unknown>, partial: boolean, isError: boolean, durationMs: number | undefined): void {
    this.result = result; this.partial = partial; this.isError = isError; this.durationMs = durationMs; this.cached = undefined;
  }
  private now(): number {
    return this.deps.ticker()?.now() ?? Date.now();
  }
  call(): CallInput | undefined {
    const fields = this.fields;
    if (!fields) return undefined;
    if (fields.executionStarted && this.startedAt === undefined) this.startedAt = this.now();
    return {
      name: this.name, ...fields, partial: this.partial, isError: this.isError,
      ...(this.result ? { result: this.result } : {}), ...(this.durationMs === undefined ? {} : { durationMs: this.durationMs }),
      ...(this.startedAt === undefined ? {} : { startedAt: this.startedAt }),
    };
  }
  model(call: CallInput): RowModel {
    if (this.cached) return this.cached;
    const model = presentCall(this.present, call);
    if (!model.live && model.state !== "writing" && model.state !== "running" && typeof model.duration !== "object") this.cached = model;
    return model;
  }
  private holdFor(live: string | undefined): void {
    if (this.hold && this.hold.id !== live) { this.hold.release(); this.hold = undefined; }
    if (live && !this.hold) {
      const ticker = this.deps.ticker();
      if (ticker) this.hold = { id: live, release: ticker.hold(live) };
    }
  }
  render(width: number): string[] {
    const call = this.call();
    if (!call) return [];
    const model = this.model(call), now = this.now();
    this.holdFor(model.live);
    const pad = Math.max(0, Math.min(this.inset, Math.floor((width - 1) / 2))), room = Math.max(1, width - pad * 2);
    const time = timeOf(model, now, call.startedAt, call.durationMs);
    const block = rowBlock(model, room, now, call.expanded, time);
    this.view = block.view && model.view ? { from: block.view.from + pad, to: block.view.to + pad, open: () => model.view?.open() } : undefined;
    return block.lines.map(line => " ".repeat(pad) + line);
  }
  /** A click on the view label opens it; any other click falls through to Pi, which toggles this row. */
  handleMouse(event: TuiMouseEvent): TuiMouseEventResult | undefined {
    if (!this.view || event.type !== "click" || event.button !== "left" || event.y !== 0) return undefined;
    if (event.x < this.view.from || event.x >= this.view.to) return undefined;
    this.view.open();
    return { handled: true };
  }
  invalidate(): void { this.cached = undefined; }
  /**
   * The row's group member while it may fold: settled (its model cached), successful, collapsed, without body and not
   * held live, of a kind the grouping names. Pi asks only rows it already found settled and collapsed.
   */
  groupMember(): GroupMember | undefined {
    const grouping = this.grouping, call = grouping ? this.call() : undefined;
    if (!grouping || !call || call.expanded || call.partial || call.isError || call.notRun || !call.result) return undefined;
    const model = this.model(call);
    if (this.member?.entry.model === model) return this.member;
    this.member = undefined;
    if (model !== this.cached || model.state !== "ok" || model.body?.length || model.live) return undefined;
    const kind = grouping.kind({ model, call });
    if (kind) this.member = new RowMember(kind.key, kind.min, { model, call }, grouping, this.inset);
    return this.member;
  }
}

/**
 * `present(call)` as the kit draws it: a call whose arguments are still streaming always reads `writing`; a call that
 * never ran reads `○ <verb> <target> · not run` and nothing more.
 */
export function presentCall(present: Presenter, call: CallInput): RowModel {
  const presented = present(call);
  if (call.notRun) {
    return {
      state: "waiting", verb: presented.verb, outcome: "not run",
      ...(presented.target ? { target: presented.target } : {}), ...(presented.targetClip ? { targetClip: presented.targetClip } : {}),
    };
  }
  return !call.argsComplete && !call.result && presented.state === "running" ? { ...presented, state: "writing" } : presented;
}

const EMPTY: Component = { render: () => [], invalidate() {} };
const rows = new WeakMap<object, ToolRow>();

/**
 * Pi renderers (`renderShell: "self"`) that draw `present(call)` with the kit for tool `name`. The call renderer
 * returns the row; the result renderer records the result on it and draws nothing, so the row is one component
 * that Pi's click toggles and Ctrl+O expands, with Pi's image rows still appended below it. With `grouping`, a
 * settled row offers the transcript a group member, so runs of one kind fold.
 */
export function rowRenderers(present: Presenter, deps: RowDeps, name: string, grouping?: RowGrouping): ToolRenderers {
  const rowOf = (context: RenderContext): ToolRow => {
    const state: unknown = context.state;
    if (state === null || typeof state !== "object") return new ToolRow(name, present, deps, grouping);
    let row = rows.get(state);
    if (!row) { row = new ToolRow(name, present, deps, grouping); rows.set(state, row); }
    return row;
  };
  return {
    renderShell: "self",
    renderCall: (args: unknown, _theme, context) => {
      const row = rowOf(context);
      row.setCall({
        args: recordOf(args) ?? {}, argsComplete: context.argsComplete, executionStarted: context.executionStarted,
        expanded: context.expanded, cwd: context.cwd, toolCallId: context.toolCallId, notRun: context.notRun === true,
      }, context.outputPad);
      return row;
    },
    renderResult: (result, options, _theme, context) => {
      rowOf(context).setResult(result, options.isPartial, context.isError, context.durationMs);
      return EMPTY;
    },
  };
}
