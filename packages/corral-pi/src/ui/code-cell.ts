// A Code Mode cell in the transcript (CODE-1..6, 8..13, STR-6): one header row in the tool row grammar (state glyph,
// `code`, the call count with its states, the tools it used, the time, `cell n`) and under it the calls the cell
// actually made, each drawn by the same presenter as a direct call of that tool, then what the script printed or
// returned. The rows come from the runtime's records of the cell (`details.cells`, partial updates while it runs in
// the foreground, `CodeRuntime.snapshot` and `watch` once it detached), never from parsing the script. While the
// script streams in, the row shows its newest lines. The model-facing report text is not drawn.
import type { AgentToolResult, ToolRenderers } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences, truncateToWidth, visibleWidth, type Component, type SelectionRow } from "@earendil-works/pi-tui";
import type { CellReport, ChildOutcome } from "../extensions/codemode/report.ts";
import type { Ticker } from "./activity.ts";
import { spinner } from "./frame.ts";
import { PANEL_ERROR, PANEL_MUTED, PANEL_SUCCESS, PANEL_TEXT, PANEL_WARNING, RESET } from "./panels-view.ts";
import { type ToolRenderDeps, presenterFor } from "./tool-rendering.ts";
import { blockRows, wrappedRows, type DrawnRow } from "./selection.ts";
import { type CallInput, type Presenter, type RowModel, clipPlain, rowLine } from "./tool-row.ts";

type RenderContext = Parameters<NonNullable<ToolRenderers["renderCall"]>>[2];

/** The runtime's view of the cells this session started (`CodeRuntime`). */
export interface CellSource {
  snapshot(toolCallId: string): CellReport | undefined;
  watch(toolCallId: string, changed: () => void): () => void;
}
/** What a cell row reads from the host; `codeCellRenderers` builds it from the shared `ToolRenderDeps`. */
export interface CodeCellDeps {
  /** The shared redraw clock; a detached cell holds it until it settles. */
  ticker(): Ticker | undefined;
  /** The Code Mode runtime; absent when Code Mode is off. */
  code(): CellSource | undefined;
  /** The presenter of a direct call of `name`, used for the calls a cell made. */
  presenter(name: string): Presenter;
}

/** Above this many children a collapsed cell folds: the ok ones between the first three and the last, running ones past three, failed ones past eight. */
const FOLD_ABOVE = 5, FOLD_HEAD = 3, RUNNING_SHOWN = 3, FAILED_SHOWN = 8;
/** Visual rows of printed output and return value: collapsed, and at most when expanded. */
const OUTPUT_ROWS = 5, EXPANDED_OUTPUT_ROWS = 400;
/** Script lines shown while it streams in. */
const WRITING_TAIL = 5;
const FINAL_STATUSES: ReadonlySet<string> = new Set(["ok", "error", "cancelled", "timeout", "reset", "closed", "not_found"]);
/** Width tiers, in the row's own columns (the terminal less the output padding): `cell n` from 100 terminal columns, the per-tool summary from 61. */
const CELL_LABEL_MIN = 96, SUMMARY_MIN = 56;
const INDENT = "  ";
const LABEL_WIDTH = 9;

type Tone = "muted" | "text" | "error" | "success" | "warning";
interface Counts { readonly total: number; readonly ok: number; readonly failed: number; readonly running: number }

/** One cell as this view reads it, from a current report or one stored before child records existed. */
export interface CellView {
  readonly cell: string;
  readonly status: string;
  readonly stateLost: boolean;
  readonly error?: string;
  /** Printed text, one part per print: JSON compacted to one line, plain text as written. */
  readonly printed: readonly OutputPart[];
  readonly images: number;
  readonly value?: OutputPart;
  /** Every recorded child in call order, or only the failures when `recorded` is false. */
  readonly children: readonly ChildOutcome[];
  /** The report carries every child (it was written with child records). */
  readonly recorded: boolean;
  /** Calls beyond the recorded children, by tool. */
  readonly omitted: Readonly<Record<string, number>>;
  /** The report's own counts, when it has them. */
  readonly calls?: Counts;
  readonly startedAt?: number;
  readonly durationMs?: number;
}
/**
 * A piece of output. Collapsed, JSON is one clipped row of its compact form (it repeats what the call rows say) and
 * plain text wraps; expanded, every part wraps as written (`full`).
 */
export interface OutputPart { readonly text: string; readonly full: string; readonly json: boolean }

/** `text` as an output part: compacted to one line when the whole of it is a JSON object or array. */
function outputPart(text: string): OutputPart {
  const full = clean(text).trimEnd(), trimmed = text.trim();
  if (trimmed.startsWith("{") || trimmed.startsWith("[")) {
    try { return { text: clean(JSON.stringify(JSON.parse(trimmed))), full, json: true }; } catch { /* plain text */ }
  }
  return { text: full, full, json: false };
}

/** Shared by the call and result renders of one tool row; Pi starts it as an empty object. */
interface RowState {
  result?: AgentToolResult<unknown>;
  /** The final (not partial) result arrived. */
  final?: boolean;
  /** Releases the runtime watch and the ticker hold of a detached cell. */
  release?: () => void;
  /** The detached cell settled; the row follows nothing any more. */
  ended?: boolean;
  /** When this row first saw a running cell that reports no start time. */
  seen?: number;
  source?: { readonly code: string; readonly lines: readonly string[] };
}

/** Display text without terminal controls: tabs become two spaces, other control characters a space. */
function clean(text: string): string {
  return stripTerminalSequences(text).replace(/\r/g, "").replace(/\t/g, "  ").replace(/[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/g, " ");
}
function color(tone: Tone): string {
  return tone === "muted" ? PANEL_MUTED : tone === "error" ? PANEL_ERROR : tone === "warning" ? PANEL_WARNING
    : tone === "success" ? PANEL_SUCCESS : PANEL_TEXT;
}
function counted(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}
/** `cell-12` as `cell 12`. */
function cellLabel(cell: string): string {
  return cell.replace(/^cell-/, "cell ");
}
/** `corral_content_read` as `content read`. */
function toolLabel(name: string): string {
  return name.replace(/^corral_/, "").replaceAll("_", " ");
}
function resultText(result: AgentToolResult<unknown> | undefined): string {
  return (result?.content ?? []).flatMap(part => part.type === "text" ? [part.text] : []).join("\n");
}
function firstLine(text: string): string {
  return clean(text).split("\n").find(line => line.trim()) ?? "";
}

function readCounts(value: unknown): Counts | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  const number = (key: string) => {
    const field: unknown = key in value ? Reflect.get(value, key) : undefined;
    return typeof field === "number" ? field : 0;
  };
  return { total: number("total"), ok: number("ok"), failed: number("failed"), running: number("running") };
}
function readChildren(value: unknown): ChildOutcome[] {
  return Array.isArray(value) ? value.filter((child): child is ChildOutcome =>
    typeof child === "object" && child !== null && "tool" in child && typeof child.tool === "string") : [];
}
/**
 * A stored or live cell report as a `CellView`; undefined for anything else. Sessions recorded before child records
 * existed have no `children`: their failures stand in, and the counts come from `calls`.
 */
export function readCell(value: unknown): CellView | undefined {
  if (typeof value !== "object" || value === null || !("cell" in value) || !("status" in value)) return undefined;
  const { cell, status } = value;
  if (typeof cell !== "string" || typeof status !== "string") return undefined;
  const field = (key: string): unknown => key in value ? Reflect.get(value, key) : undefined;
  const rawOutputs = field("outputs"), outputs: readonly unknown[] = Array.isArray(rawOutputs) ? rawOutputs : [];
  const texts: string[] = [];
  let images = 0;
  for (const item of outputs) {
    if (typeof item !== "object" || item === null || !("type" in item)) continue;
    if (item.type === "text" && "text" in item && typeof item.text === "string") texts.push(item.text);
    else if (item.type === "image") images++;
  }
  const error = field("error");
  const errorText = typeof error === "object" && error !== null && "message" in error && typeof error.message === "string"
    ? `${"name" in error && typeof error.name === "string" ? error.name + ": " : ""}${error.message}` : undefined;
  const recorded = Array.isArray(field("children"));
  const omittedRaw = field("childrenOmitted");
  const omitted: Record<string, number> = {};
  if (typeof omittedRaw === "object" && omittedRaw !== null) {
    for (const [tool, n] of Object.entries(omittedRaw)) if (typeof n === "number" && n > 0) omitted[tool] = n;
  }
  const returned = field("value"), started = field("startedAt"), duration = field("durationMs"), calls = readCounts(field("calls"));
  return {
    cell, status, stateLost: field("stateLost") === true,
    ...(errorText === undefined ? {} : { error: errorText }),
    printed: texts.filter(text => text.trim()).map(outputPart), images,
    ...(typeof returned === "string" ? { value: outputPart(returned) } : {}),
    children: readChildren(recorded ? field("children") : field("failures")), recorded, omitted,
    ...(calls ? { calls } : {}),
    ...(typeof started === "number" ? { startedAt: started } : {}),
    ...(typeof duration === "number" ? { durationMs: duration } : {}),
  };
}
/** The cell reports in a codemode result's details. */
function reportsOf(result: AgentToolResult<unknown> | undefined): readonly CellView[] {
  const details: unknown = result?.details;
  if (typeof details !== "object" || details === null || !("cells" in details) || !Array.isArray(details.cells)) return [];
  return details.cells.flatMap(cell => readCell(cell) ?? []);
}

function settled(cell: CellView): boolean {
  return FINAL_STATUSES.has(cell.status);
}
function omittedTotal(cell: CellView): number {
  return Object.values(cell.omitted).reduce((sum, n) => sum + n, 0);
}
/** The cell's call counts; running children of a cell nobody follows any more are `unknown`, not running. */
function countsOf(cell: CellView): Counts {
  if (cell.recorded && omittedTotal(cell) === 0) {
    const of = (ok: boolean | undefined) => cell.children.filter(child => child.ok === ok).length;
    return { total: cell.children.length, ok: of(true), failed: of(false), running: of(undefined) };
  }
  return cell.calls ?? { total: cell.children.length, ok: 0, failed: cell.children.length, running: 0 };
}
/** The tools a cell called, in first-call order (`read ×3, bash`), at most three named. */
function toolSummary(cell: CellView): string {
  if (!cell.recorded) return "";
  const tally = new Map<string, number>();
  for (const child of cell.children) tally.set(child.tool, (tally.get(child.tool) ?? 0) + 1);
  for (const [tool, n] of Object.entries(cell.omitted)) tally.set(tool, (tally.get(tool) ?? 0) + n);
  const named = [...tally].map(([tool, n]) => toolLabel(tool) + (n > 1 ? ` ×${n}` : ""));
  return named.length > 3 ? `${named.slice(0, 3).join(", ")} +${named.length - 3}` : named.join(", ");
}
/** A settled cell's status in words when it is not `ok`. */
function statusWord(status: string): string {
  switch (status) {
    case "error": return "failed";
    case "cancelled": return "cancelled";
    case "timeout": return "timed out";
    case "reset": return "stopped by a reset";
    case "closed": return "stopped when the session closed";
    default: return "";
  }
}
function durationText(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  const whole = Math.floor(seconds);
  return `${Math.floor(whole / 60)}m ${String(whole % 60).padStart(2, "0")}s`;
}

function parseArgs(text: string | undefined): Record<string, unknown> {
  if (text === undefined) return {};
  try {
    const value: unknown = JSON.parse(text);
    return typeof value === "object" && value !== null && !Array.isArray(value) ? Object.fromEntries(Object.entries(value)) : {};
  } catch { return {}; }
}

/**
 * A child record as the call its tool's presenter draws: arguments, result preview, structured result and timing. The
 * preview is clipped, so the result's real size travels as `totals`; a record without sizes (older sessions) passes
 * empty totals, so presenters print no count rather than the preview's.
 */
export function childCall(child: ChildOutcome, parent: string, cwd: string, expanded: boolean): CallInput {
  const reason = [child.code, child.message].filter(Boolean).join(": ");
  const text = child.ok === false && reason && !child.preview ? reason : child.preview ?? reason;
  return {
    name: child.tool, args: parseArgs(child.args), argsComplete: true, executionStarted: true,
    ...(child.ok === undefined ? {} : { result: { content: [{ type: "text" as const, text }], details: child.structured ?? {} } }),
    partial: false, isError: child.ok === false,
    ...(child.durationMs === undefined ? {} : { durationMs: child.durationMs }),
    ...(typeof child.startedAt === "number" ? { startedAt: child.startedAt } : {}),
    totals: { ...(child.resultLines === undefined ? {} : { lines: child.resultLines }),
      ...(child.resultBytes === undefined ? {} : { bytes: child.resultBytes }) },
    expanded, cwd, toolCallId: child.id ?? `${parent}/${child.call}`,
  };
}

/** One child drawn by its tool's presenter, marked when it ran after the cell or its outcome went unrecorded. */
function childModel(child: ChildOutcome, deps: CodeCellDeps, context: RenderContext, orphan: boolean): RowModel {
  const call = childCall(child, context.toolCallId, context.cwd, context.expanded);
  let model: RowModel;
  try { model = deps.presenter(child.tool)(call); }
  catch { model = { state: call.isError ? "failed" : call.result ? "ok" : "running", verb: toolLabel(child.tool) }; }
  if (orphan && child.ok === undefined) return { ...model, state: "unknown", outcome: "outcome not recorded", outcomeTone: "warning" };
  // A nested row has no Pi row clock: its time comes from the record.
  const duration = model.duration ?? (child.durationMs !== undefined ? child.durationMs
    : child.ok === undefined && typeof child.startedAt === "number" ? { since: child.startedAt } : undefined);
  const timed = duration === undefined ? model : { ...model, duration };
  if (child.late) return { ...timed, right: timed.right ? `${timed.right} · after the cell` : "after the cell" };
  return timed;
}

/** A collapsed cell's rows: a child, or a fold line standing for `count` children of one kind. */
export type FoldEntry = { readonly child: ChildOutcome } | { readonly count: number; readonly kind: "ok" | "running" | "failed" };

/**
 * Which children a collapsed cell shows. Up to five: all. Above: the first three and the last ok one, at most three
 * running and eight failed; the rest of each kind fold into one line where the first of them stood.
 */
export function foldChildren(children: readonly ChildOutcome[]): readonly FoldEntry[] {
  if (children.length <= FOLD_ABOVE) return children.map(child => ({ child }));
  let lastOk = -1;
  children.forEach((child, index) => { if (child.ok === true) lastOk = index; });
  const kindOf = (child: ChildOutcome) => child.ok === true ? "ok" as const : child.ok === false ? "failed" as const : "running" as const;
  const cap = { running: RUNNING_SHOWN, failed: FAILED_SHOWN };
  const shownOf = { ok: 0, running: 0, failed: 0 };
  const folded = { ok: 0, running: 0, failed: 0 };
  const keep = children.map((child, index) => {
    const kind = kindOf(child);
    const kept = kind === "ok" ? index < FOLD_HEAD || index === lastOk : shownOf[kind] < cap[kind];
    if (kept) shownOf[kind]++; else folded[kind]++;
    return kept;
  });
  const entries: FoldEntry[] = [], placed = new Set<string>();
  children.forEach((child, index) => {
    if (keep[index]) { entries.push({ child }); return; }
    const kind = kindOf(child);
    if (!placed.has(kind)) { placed.add(kind); entries.push({ count: folded[kind], kind }); }
  });
  return entries;
}

/**
 * `label  text` for each part, at most `rows` rows, and how many rows were cut. Collapsed, a JSON part is one row
 * clipped to the width and plain text wraps (continuation rows under the text); expanded, every part wraps in full.
 */
function labelled(label: string, parts: readonly OutputPart[], width: number, rows: number, tone: Tone, expanded: boolean): { readonly lines: string[]; readonly hidden: number; readonly drawn: readonly DrawnRow[] } {
  const room = Math.max(1, width - LABEL_WIDTH);
  let next = 0;
  const wrap = (text: string) => text.split("\n").flatMap(line => line.length === 0 ? [{ text: "", copy: "", line: next++ }] : wrappedRows(line, room, next++));
  const wrapped = parts.flatMap(part => !expanded && part.json ? [{ text: part.text, copy: part.text, line: next++ }] : wrap(expanded ? part.full : part.text));
  while (wrapped.length > 0 && wrapped[wrapped.length - 1]!.text === "") wrapped.pop();
  const kept = wrapped.slice(0, Math.max(0, rows));
  const lines = kept.map((row, index) => (index === 0 ? PANEL_MUTED + label.padEnd(LABEL_WIDTH) : " ".repeat(LABEL_WIDTH))
    + color(tone) + truncateToWidth(row.text, room, "…") + RESET);
  return { lines, hidden: wrapped.length - kept.length, drawn: kept.map(row => clipped(row, room)) };
}
/** `row` as drawn clipped to `room`: a cut row copies what shows, without its `…`. */
function clipped(row: DrawnRow, room: number): DrawnRow {
  return visibleWidth(row.text) <= room ? row : { ...row, copy: clipPlain(row.text, room - 1, "") };
}
function muted(text: string, width: number, indent = INDENT): string {
  return indent + PANEL_MUTED + truncateToWidth(text, Math.max(1, width - indent.length), "…") + RESET;
}

/** The cell's source reformatted by Bun's transpiler (one statement a line); the original text when it does not parse. */
function sourceLines(state: RowState, code: string): readonly string[] {
  if (state.source?.code === code) return state.source.lines;
  let text = code;
  try {
    const bun: unknown = Reflect.get(globalThis, "Bun");
    const Transpiler: unknown = typeof bun === "object" && bun !== null ? Reflect.get(bun, "Transpiler") : undefined;
    if (typeof Transpiler === "function") {
      const transpiler: unknown = Reflect.construct(Transpiler, [{ loader: "js" }]);
      const transform: unknown = typeof transpiler === "object" && transpiler !== null ? Reflect.get(transpiler, "transformSync") : undefined;
      const printed: unknown = typeof transform === "function" ? Reflect.apply(transform, transpiler, [code]) : undefined;
      if (typeof printed === "string" && printed.trim()) text = printed;
    }
  } catch { text = code; }
  const lines = clean(text).trimEnd().split("\n");
  state.source = { code, lines };
  return lines;
}

/** Everything one render reads: the row's context, its stored result and the runtime's current report. */
interface View {
  readonly args: { readonly code?: string; readonly cancel?: string; readonly reset: boolean };
  readonly context: RenderContext;
  readonly state: RowState;
  readonly cell: CellView | undefined;
  /** Cells a reset in this call stopped before its own cell ran. */
  readonly stopped: readonly CellView[];
  /** The runtime still knows this cell, so its report is current. */
  readonly live: boolean;
  /** The tool call returned while the cell kept running. */
  readonly background: boolean;
  readonly now: number;
}

function viewOf(deps: CodeCellDeps, context: RenderContext, now: number): View {
  const state = context.state as RowState, raw: unknown = context.args;
  const read = (key: string): unknown => typeof raw === "object" && raw !== null && key in raw ? Reflect.get(raw, key) : undefined;
  const code = read("code"), cancel = read("cancel");
  const args = { ...(typeof code === "string" ? { code } : {}), ...(typeof cancel === "string" ? { cancel } : {}), reset: read("reset") === true };
  const reports = reportsOf(state.result);
  const stored = args.cancel !== undefined ? reports[0] : args.code !== undefined ? reports.at(-1) : undefined;
  const snapshot = args.cancel === undefined && args.code !== undefined ? readCell(deps.code()?.snapshot(context.toolCallId)) : undefined;
  return {
    args, context, state, cell: snapshot ?? stored, live: snapshot !== undefined, now,
    stopped: args.cancel !== undefined ? [] : args.code !== undefined ? reports.slice(0, -1) : reports,
    background: state.final === true && stored?.status === "detached",
  };
}

/**
 * Whether the cell still runs as far as this row can know: the runtime answers for it, or its tool call is still
 * executing. A report that says `running` or `detached` but that nobody follows any more (a resumed session, or one
 * recorded before the runtime kept records) is history: nothing will update it.
 */
function cellRunning(view: View, cell: CellView): boolean {
  return !settled(cell) && (view.live || view.state.final !== true);
}
const MOVED = "moved to the background · its result arrived as a later message";

/** The time slot of the header: the cell's total once settled, else a clock from its start. */
function timing(view: View, cell: CellView, running: boolean): Pick<RowModel, "duration"> {
  if (!running) {
    if (cell.durationMs !== undefined) return { duration: cell.durationMs };
    return !view.background && view.context.durationMs !== undefined ? { duration: view.context.durationMs } : {};
  }
  view.state.seen ??= view.now;
  return { duration: { since: cell.startedAt ?? view.state.seen } };
}

/** The header row of a code cell; a cell with exactly one call is that call's row behind `code ›`. */
function headerModel(view: View, deps: CodeCellDeps, width: number): RowModel {
  const { cell, context, args } = view;
  if (!context.argsComplete) {
    const lines = args.code ? args.code.split("\n").length : 0;
    return { state: "writing", verb: "code", ...(lines ? { target: counted(lines, "line", "lines") } : {}) };
  }
  if (!cell) {
    const failed = view.state.final === true && context.isError;
    const text = failed ? firstLine(resultText(view.state.result)) : "";
    return { state: failed ? "failed" : "running", verb: "code", ...(text ? { outcome: text, outcomeTone: "error" as const } : {}) };
  }
  const running = cellRunning(view, cell), history = !settled(cell) && !running;
  const orphan = !running && !view.live, counts = countsOf(cell);
  const unknown = orphan ? counts.running : 0, active = orphan ? 0 : counts.running;
  const cellFailed = !running && !history && cell.status !== "ok";
  const state = running ? "running" : history ? "waiting" : cellFailed ? "failed" : unknown > 0 ? "unknown" : "ok";
  const right = [view.background && !history ? (running ? "in background" : "settled in background") : "",
    width >= CELL_LABEL_MIN ? cellLabel(cell.cell) : ""].filter(Boolean).join(" · ");
  // The error itself is a body row; the header says only how the cell ended.
  const failure = cellFailed ? statusWord(cell.status) || "failed" : "";
  const lost = cell.stateLost ? "variables lost: kernel restarted" : "";
  const time = timing(view, cell, running);

  if (history) {
    const tally = counts.total === 0 ? "" : [counted(counts.total, "call", "calls"), counts.ok ? `✓${counts.ok}` : "",
      counts.failed ? `✗${counts.failed}` : "", unknown ? `!${unknown}` : ""].filter(Boolean).join(" ");
    return { state: "waiting", verb: "code", ...(tally ? { target: tally } : {}), outcome: MOVED, outcomeTone: "muted",
      ...(right ? { right } : {}), ...time };
  }

  if (cell.recorded && cell.children.length === 1 && omittedTotal(cell) === 0) {
    const child = childModel(cell.children[0]!, deps, context, orphan);
    const outcome = [failure || child.outcome || "", lost].filter(Boolean).join(" · ");
    return { state: cellFailed ? "failed" : running ? "running" : child.state, verb: `code › ${child.verb}`,
      ...(child.target ? { target: child.target } : {}),
      ...(outcome ? { outcome, outcomeTone: failure ? "error" as const : lost && !child.outcome ? "warning" as const : child.outcomeTone ?? "text" } : {}),
      ...(right ? { right } : {}), ...time };
  }
  const tally = counts.total === 0 ? "no tool calls" : [counted(counts.total, "call", "calls"), counts.ok ? `✓${counts.ok}` : "",
    active ? `${spinner(view.now)}${active}` : "", unknown ? `!${unknown}` : ""].filter(Boolean).join(" ");
  const summary = width >= SUMMARY_MIN ? toolSummary(cell) : "";
  const outcome = [counts.failed ? `✗${counts.failed}` : "", failure, lost].filter(Boolean).join(" · ");
  const tone = counts.failed || failure ? "error" as const : "warning" as const;
  return { state, verb: "code", target: [tally, summary].filter(Boolean).join(" · "),
    ...(outcome ? { outcome, outcomeTone: tone } : {}), ...(right ? { right } : {}), ...time };
}

/** `cancel cell 2 · already finished`, `reset kernel · variables discarded`. */
function controlModel(view: View): RowModel {
  const { args, cell, context } = view;
  const pending = { state: context.executionStarted ? "running" as const : "writing" as const };
  if (args.cancel !== undefined) {
    const target = cellLabel(args.cancel);
    if (!view.state.final) return { ...pending, verb: "cancel", target };
    if (context.isError || !cell) return { state: "failed", verb: "cancel", target, outcome: firstLine(resultText(view.state.result)), outcomeTone: "error" };
    if (cell.status === "not_found") return { state: "ok", verb: "cancel", target, outcome: "already finished", outcomeTone: "muted" };
    const total = countsOf(cell).total, calls = total ? `, ${counted(total, "call", "calls")}` : "";
    const after = cell.durationMs === undefined ? "" : ` after ${durationText(cell.durationMs)}`;
    const outcome = cell.status === "cancelled" ? `stopped${after}${calls}` : `it had ${statusWord(cell.status) || "finished"}${calls}`;
    return { state: "ok", verb: "cancel", target, outcome: [outcome, cell.stateLost ? "variables lost" : ""].filter(Boolean).join(" · ") };
  }
  if (!view.state.final) return { ...pending, verb: "reset", target: "kernel" };
  const stopped = view.stopped.map(stop => `stopped ${cellLabel(stop.cell)}`).join(", ");
  if (context.isError) return { state: "failed", verb: "reset", target: "kernel", outcome: firstLine(resultText(view.state.result)), outcomeTone: "error" };
  return { state: "ok", verb: "reset", target: "kernel", outcome: ["variables discarded", stopped].filter(Boolean).join(" · ") };
}

/**
 * Rows under the header: the calls, the error, printed output and return value, then `… n more rows` when output was
 * cut; expanded adds every call, all output and the source. Rows name no keys: the footer carries `Ctrl+O expand`.
 * Each of the error, printed output, return value and source goes to `blocks` for semantic selection, without its
 * label, in the rows' coordinates.
 */
function bodyLines(view: View, deps: CodeCellDeps, width: number, blocks: SelectionRow[][] = []): string[] {
  const { cell, context, args, now } = view;
  const expanded = context.expanded, inner = Math.max(1, width - INDENT.length);
  const lines: string[] = [];
  const code = args.code ?? "";
  if (!context.argsComplete) {
    if (code) lines.push(...clean(code).trimEnd().split("\n").slice(-WRITING_TAIL).map(line => muted(line, width)));
    return lines;
  }
  if (args.reset && view.stopped.length > 0) lines.push(muted(`reset first: stopped ${view.stopped.map(stop => cellLabel(stop.cell)).join(", ")}`, width));
  if (!cell) return lines;
  const running = cellRunning(view, cell), orphan = !running && !view.live;
  const single = cell.recorded && cell.children.length === 1 && omittedTotal(cell) === 0;
  if (!single) {
    const entries: readonly FoldEntry[] = expanded ? cell.children.map(child => ({ child })) : foldChildren(cell.children);
    for (const entry of entries) {
      if (!("child" in entry)) {
        lines.push(muted(`… ${entry.count} more ${entry.kind === "ok" ? "✓" : entry.kind === "failed" ? "✗" : "running"}`, width));
        continue;
      }
      const model = childModel(entry.child, deps, context, orphan);
      lines.push(INDENT + rowLine(model, inner, now));
      for (const body of expanded ? model.body ?? [] : entry.child.ok === false ? (model.body ?? []).slice(0, 1) : []) {
        lines.push(INDENT + INDENT + color(bodyTone(body.tone)) + truncateToWidth(clean(body.text), Math.max(1, inner - INDENT.length), "…") + RESET);
      }
    }
    const unrecorded = cell.recorded ? 0 : countsOf(cell).total - cell.children.length;
    if (unrecorded > 0) lines.push(muted(`… ${counted(unrecorded, "more call", "more calls")} not recorded in this session`, width));
    const omitted = omittedTotal(cell);
    if (omitted > 0) lines.push(muted(`… ${counted(omitted, "more call", "more calls")} after the first ${cell.children.length}`, width));
  }
  const output = (block: ReturnType<typeof labelled>) => {
    blocks.push(blockRows(block.drawn, lines.length, INDENT.length + LABEL_WIDTH));
    lines.push(...block.lines.map(line => INDENT + line));
  };
  if (cell.error) {
    const error = [{ text: clean(cell.error), full: clean(cell.error), json: false }];
    output(labelled("error", error, inner, expanded ? 12 : 2, "error", expanded));
  }
  let rows = expanded ? EXPANDED_OUTPUT_ROWS : OUTPUT_ROWS, cut = 0;
  if (cell.printed.length > 0) {
    const block = labelled("printed", cell.printed, inner, rows, "text", expanded);
    output(block);
    rows -= block.lines.length; cut += block.hidden;
  }
  if (cell.images > 0) lines.push(muted(`${"printed".padEnd(LABEL_WIDTH)}${counted(cell.images, "image", "images")}, shown below`, width));
  if (cell.value !== undefined) {
    const block = labelled("returned", [cell.value], inner, Math.max(1, rows), "text", expanded);
    output(block);
    cut += block.hidden;
  }
  if (view.background && running) lines.push(muted("the agent continues; this row updates when the cell settles", width));
  if (code && expanded) {
    lines.push(muted("source", width));
    const room = Math.max(1, inner - INDENT.length);
    const source = sourceLines(view.state, code).map((line, index) => clipped({ text: line, copy: line, line: index }, room));
    blocks.push(blockRows(source, lines.length, INDENT.length * 2));
    lines.push(...source.map(row => INDENT + INDENT + PANEL_TEXT + truncateToWidth(row.text, room, "…") + RESET));
  }
  if (cut > 0) lines.push(muted(`… ${counted(cut, "more row", "more rows")}`, width));
  return lines;
}

function bodyTone(tone: string | undefined): Tone {
  return tone === "error" || tone === "removed" ? "error" : tone === "warning" ? "warning" : tone === "muted" ? "muted"
    : tone === "success" || tone === "added" ? "success" : "text";
}

/** While the tool call returned but its cell still runs: watch the runtime and hold the clock; release once it settled. */
function track(view: View, deps: CodeCellDeps): void {
  const { state, context, cell } = view;
  if (state.ended) return;
  const running = cell !== undefined && (!settled(cell) || (view.live && cell.children.some(child => child.ok === undefined)));
  if (view.background && view.live && running) {
    if (state.release) return;
    const unwatch = deps.code()?.watch(context.toolCallId, context.invalidate);
    const unhold = deps.ticker()?.hold(`code:${context.toolCallId}`);
    state.release = () => { unwatch?.(); unhold?.(); };
    return;
  }
  if (state.release) {
    state.release();
    state.release = undefined;
    state.ended = true;
  }
}

/**
 * The whole row for `width`: the header and the rows under it. `blocks` receives the output blocks (error, printed,
 * returned, source) for semantic selection, in the row's coordinates.
 */
export function cellLines(deps: CodeCellDeps, context: RenderContext, width: number, now: number, blocks: SelectionRow[][] = []): string[] {
  const pad = Math.max(0, Math.min(context.outputPad ?? 0, Math.floor((width - 1) / 2))), room = Math.max(1, width - pad * 2);
  const view = viewOf(deps, context, now);
  const control = view.args.cancel !== undefined || (view.args.reset && view.args.code === undefined);
  const header = control ? controlModel(view) : headerModel(view, deps, room);
  const body: SelectionRow[][] = [];
  const lines = [rowLine(header, room, now), ...control ? [] : bodyLines(view, deps, room, body)];
  blocks.push(...body.map(rows => rows.map(row => ({ ...row, y: row.y + 1, left: row.left + pad }))));
  const margin = " ".repeat(pad);
  return lines.map(line => margin + (visibleWidth(line) > room ? truncateToWidth(line, room, "…") : line));
}

/**
 * Pi renderers for `codemode`, self-framed like every tool row, from the shared tool rendering deps: nested calls use
 * the same presenters as direct calls.
 */
export function codeCellRenderers(deps: ToolRenderDeps): ToolRenderers {
  return cellRenderers({ ticker: () => deps.ticker(), code: () => deps.code(), presenter: name => presenterFor(name, deps) });
}

/**
 * The renderers over narrow deps (the test seam). The call render draws the whole cell from what the result render
 * stored (partial updates while the cell runs in the foreground, then the final result) and from the runtime.
 */
export function cellRenderers(deps: CodeCellDeps): ToolRenderers {
  const now = () => deps.ticker()?.now() ?? Date.now();
  return {
    renderShell: "self",
    renderCall: (_args, _theme, context): Component => {
      track(viewOf(deps, context, now()), deps);
      let drawn: { readonly width: number; readonly blocks: readonly (readonly SelectionRow[])[] } | undefined;
      return {
        render: width => {
          const blocks: SelectionRow[][] = [], lines = cellLines(deps, context, width, now(), blocks);
          drawn = { width, blocks };
          return lines;
        },
        selectionRows: (width, y) => drawn?.width === width ? drawn.blocks.find(rows => rows.some(row => row.y === y)) : undefined,
        invalidate() {},
      };
    },
    renderResult: (result, options, _theme, context): Component => {
      const state = context.state as RowState;
      state.result = result;
      if (!options.isPartial) state.final = true;
      track(viewOf(deps, context, now()), deps);
      return { render: () => [], invalidate() {} };
    },
  };
}
