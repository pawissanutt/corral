// Code Mode on the Corral-owned persistent kernel (T-683). Tool execution and model-facing
// reports use Pi's ordinary tool results; bounded runtime records also feed live rows.
import type { AgentToolCallOutcome } from "@earendil-works/pi-agent-core";
import type { JsonValue } from "@earendil-works/pi-ai";
import type { ExtensionToolContext } from "@earendil-works/pi-coding-agent";

import { type CodeChild, childKey, runChild } from "./child.ts";
import { type CellResult, CodeKernel, type CodeKernelOptions, KernelError, type KernelEvent, type ToolRequest } from "./kernel.ts";
import { clipUtf8 } from "./kernel-protocol.ts";
import { type CellReport, type ChildOutcome, classify, contentText, noticeText } from "./report.ts";

export const CODE_CALL_ENTRY = "corral_code_call";
export const CODE_CELL_ENTRY = "corral_code_cell";
export const DEFAULT_WAIT_MS = 60_000;
export const MAX_WAIT_MS = 600_000;
const CHILDREN_LIMIT = 200;
const ARGS_LIMIT = 2 * 1024;
const CELL_ARGS_LIMIT = 32 * 1024;
const HISTORY_LIMIT = 64;
const CHANGE_MS = 100;

export interface CodeClock {
  now(): number;
  /** Returns a cancellation function. Kept injectable so notification tests need no sleeps. */
  schedule(changed: () => void, delayMs: number): () => void;
}

const clock: CodeClock = {
  now: () => Date.now(),
  schedule: (changed, delay) => {
    const timer = setTimeout(changed, delay);
    timer.unref();
    return () => clearTimeout(timer);
  },
};

export interface CodeHost {
  emit(kind: string, data: Record<string, unknown>): void;
  /** Defaults to corral_code_call; cell settlements use a separate, recovery-neutral entry type. */
  journal(data: Record<string, unknown>, customType?: string): void;
  /** Data of corral_code_cell and corral_code_call entries, in journal order. */
  history?(): readonly unknown[];
  notify(key: string, text: string): Promise<string>;
  readonly kernel?: Pick<CodeKernelOptions, "limits" | "worker">;
  readonly clock?: CodeClock;
}

export type Executor = Pick<ExtensionToolContext, "executeTool">;
type CallCounts = { -readonly [K in keyof CellReport["calls"]]: CellReport["calls"][K] };
const emptyCounts = (): CallCounts => ({ total: 0, ok: 0, failed: 0, running: 0 });

interface Cell {
  readonly id: string;
  readonly parent: string;
  readonly ctx: Executor;
  readonly children: ChildOutcome[];
  readonly omitted: Record<string, number>;
  readonly failures: ChildOutcome[];
  readonly calls: CallCounts;
  readonly result: Promise<CellResult>;
  readonly startedAt: number;
  argsBytes: number;
  detached: boolean;
  reported: boolean;
  terminate: boolean;
  durationMs?: number;
  settled?: CellResult;
}

interface Watchers {
  readonly listeners: Set<() => void>;
  last?: number;
  cancel?: () => void;
  final: boolean;
}

class CodeCallError extends Error {
  constructor(readonly code: string, message: string) {
    super(message);
    this.name = "CodeCallError";
  }
}

const reason = (error: unknown) => (error instanceof Error ? error.message : String(error)).slice(0, 512);
const record = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const scalar = (value: unknown): boolean => value === null || ["string", "number", "boolean"].includes(typeof value);
const short = (value: unknown): unknown => typeof value === "string" ? value.slice(0, 200) : value;

/** Preserve useful top-level targets without ever storing an invalid JSON prefix. */
function argumentsFor(input: unknown, room: number): Pick<ChildOutcome, "args" | "argsBytes"> {
  const json = JSON.stringify(input ?? {});
  const argsBytes = Buffer.byteLength(json);
  if (argsBytes <= room) return { args: json, argsBytes };
  const summary: Record<string, unknown> = { _clipped: true };
  if (record(input)) {
    for (const [key, value] of Object.entries(input)) {
      const field = scalar(value) ? short(value) : "[omitted]";
      summary[key] = field;
      if (Buffer.byteLength(JSON.stringify(summary)) > room) delete summary[key];
    }
  }
  const args = JSON.stringify(summary);
  return { argsBytes, ...(Buffer.byteLength(args) <= room ? { args } : {}) };
}

/** Keep the whole result if it fits; otherwise retain bounded scalar fields for the row. */
function structuredPreview(value: JsonValue | undefined): unknown {
  if (value === undefined) return;
  if (Buffer.byteLength(JSON.stringify(value)) <= ARGS_LIMIT) return value;
  if (typeof value === "string") return short(value);
  if (!record(value)) return;
  const preview: Record<string, unknown> = Object.create(null);
  for (const [key, field] of Object.entries(value)) {
    if (!scalar(field)) continue;
    preview[key] = short(field);
    if (Buffer.byteLength(JSON.stringify(preview)) > ARGS_LIMIT) delete preview[key];
  }
  return preview;
}

/** Journal data is optional and older sessions have no cell records. */
function historicalReport(value: unknown): { parent: string; report: CellReport } | undefined {
  if (!record(value) || value.phase !== "settled" || typeof value.tool_call_id !== "string" || !record(value.report)) return;
  const report = value.report;
  const calls = report.calls;
  if (typeof report.cell !== "string" || !["ok", "error", "cancelled", "timeout", "reset", "closed"].includes(String(report.status))
    || typeof report.startedAt !== "number" || typeof report.durationMs !== "number"
    || typeof report.stateLost !== "boolean" || typeof report.truncated !== "boolean"
    || !Array.isArray(report.children) || !Array.isArray(report.failures) || !record(calls)
    || !["total", "ok", "failed", "running"].every(key => typeof calls[key] === "number")) return;
  return { parent: value.tool_call_id, report: { ...report, outputs: [] } as unknown as CellReport };
}

export class CodeRuntime {
  private kernel: CodeKernel | undefined;
  private sequence = 0;
  private closed = false;
  private deferred: Cell[] | undefined;
  private readonly cells = new Map<string, Cell>();
  private readonly parents = new Map<string, Cell>();
  private readonly finished = new Map<string, CellReport>();
  private readonly watchers = new Map<string, Watchers>();
  private readonly notices = new Set<Promise<void>>();
  private readonly clock: CodeClock;

  constructor(private readonly host: CodeHost) {
    this.clock = host.clock ?? clock;
    this.restore();
  }

  private restore(): void {
    const reconciled = new Set<string>();
    for (const entry of this.host.history?.() ?? []) {
      const saved = historicalReport(entry);
      if (saved) {
        this.remember(saved.parent, saved.report);
        continue;
      }
      // A sibling may finish after the kernel's settlement record. Recovery still reads
      // its ordinary child entry; use the same entry to finish the resumed row too.
      if (!record(entry) || entry.phase !== "settled" || !record(entry.parent)
        || typeof entry.parent.tool_call_id !== "string" || typeof entry.parent.call !== "string"
        || typeof entry.tool !== "string" || typeof entry.ok !== "boolean" || typeof entry.duration_ms !== "number") continue;
      const report = this.finished.get(entry.parent.tool_call_id);
      const key = `${entry.parent.tool_call_id}/${entry.parent.call}`;
      if (!report || reconciled.has(key) || report.calls.running === 0) continue;
      const index = report.children.findIndex(child => child.key === key);
      const existing = report.children[index];
      const ordinal = Number(entry.parent.call.split(".").at(-1));
      if (existing?.ok !== undefined || (!existing && (!Number.isInteger(ordinal)
        || ordinal <= CHILDREN_LIMIT || ordinal > report.calls.total))) continue;
      reconciled.add(key);
      const child: ChildOutcome = { ...existing, tool: entry.tool, call: entry.parent.call, key,
        startedAt: existing?.startedAt ?? report.startedAt, durationMs: entry.duration_ms, late: true, ok: entry.ok,
        ...(entry.effect === "not_started" || entry.effect === "unknown" ? { effect: entry.effect } : {}),
        ...(typeof entry.code === "string" ? { code: entry.code } : {}),
        ...(typeof entry.message === "string" ? { message: entry.message } : {}),
        ...(entry.structured === undefined ? {} : { structured: entry.structured }),
        ...(typeof entry.result_lines === "number" ? { resultLines: entry.result_lines } : {}),
        ...(typeof entry.result_bytes === "number" ? { resultBytes: entry.result_bytes } : {}),
      };
      const children = report.children.map((item, n) => n === index ? child : item);
      const failures = entry.ok ? report.failures : [...report.failures, child]
        .sort((a, b) => Number(a.call.split(".").at(-1)) - Number(b.call.split(".").at(-1)));
      this.finished.set(entry.parent.tool_call_id, { ...report, children, failures, calls: {
        ...report.calls, running: report.calls.running - 1, ok: report.calls.ok + Number(entry.ok),
        failed: report.calls.failed + Number(!entry.ok),
      } });
    }
  }

  snapshot(toolCallId: string): CellReport | undefined {
    const cell = this.parents.get(toolCallId);
    return cell ? this.report(cell, cell.settled) : this.finished.get(toolCallId);
  }

  watch(toolCallId: string, changed: () => void): () => void {
    if (this.closed || this.finished.has(toolCallId)) return () => undefined;
    let group = this.watchers.get(toolCallId);
    if (!group) {
      group = { listeners: new Set(), final: false };
      this.watchers.set(toolCallId, group);
    }
    group.listeners.add(changed);
    return () => {
      group.listeners.delete(changed);
      if (group.listeners.size === 0 && this.watchers.get(toolCallId) === group) {
        group.cancel?.();
        this.watchers.delete(toolCallId);
      }
    };
  }

  private changed(parent: string, final = false): void {
    const group = this.watchers.get(parent);
    if (!group) return;
    group.final ||= final;
    if (group.cancel) return;
    const notify = () => {
      group.cancel = undefined;
      group.last = this.clock.now();
      try {
        for (const listener of [...group.listeners]) listener();
      } finally {
        if (group.final && this.watchers.get(parent) === group) {
          group.listeners.clear();
          this.watchers.delete(parent);
        }
      }
    };
    const delay = group.last === undefined ? 0 : Math.max(0, CHANGE_MS - (this.clock.now() - group.last));
    if (delay === 0) notify();
    else group.cancel = this.clock.schedule(notify, delay);
  }

  private releaseWatchers(): void {
    for (const group of this.watchers.values()) {
      group.cancel?.();
      group.listeners.clear();
    }
    this.watchers.clear();
  }

  private remember(parent: string, report: CellReport): void {
    this.finished.delete(parent);
    this.finished.set(parent, report);
    if (this.finished.size > HISTORY_LIMIT) {
      const oldest = this.finished.keys().next().value;
      if (oldest !== undefined) this.finished.delete(oldest);
    }
  }

  private current(): CodeKernel {
    this.kernel ??= new CodeKernel({
      session: "codemode", tools: (request) => this.call(request),
      onEvent: (event) => this.observe(event), ...this.host.kernel,
    });
    return this.kernel;
  }

  async run(parent: string, code: string, ctx: Executor, signal: AbortSignal | undefined,
    waitMs = DEFAULT_WAIT_MS): Promise<CellReport> {
    const id = `cell-${++this.sequence}`;
    const startedAt = this.clock.now();
    let result: Promise<CellResult>;
    try {
      if (this.closed) throw new KernelError("kernel_closed", "Code Mode is closed for this host");
      result = this.current().submit(code, { cell: id }).result;
    } catch (error) {
      if (!(error instanceof KernelError)) throw error;
      const report: CellReport = { cell: id, status: "error", stateLost: false, truncated: false, outputs: [], calls: emptyCounts(), failures: [],
        children: [], startedAt, durationMs: Math.max(0, this.clock.now() - startedAt),
        error: { name: error.name, message: error.message, code: error.code } };
      this.remember(parent, report);
      this.changed(parent, true);
      return report;
    }
    const cell: Cell = { id, parent, ctx, children: [], omitted: {}, failures: [], calls: emptyCounts(), result,
      startedAt, argsBytes: 0, detached: false, reported: false, terminate: false };
    this.cells.set(id, cell);
    this.parents.set(parent, cell);
    this.host.emit("code_cell", { cell: id, phase: "submitted", parent });
    void result.then((settled) => this.settle(cell, settled));
    return this.foreground(cell, signal, waitMs);
  }

  private async foreground(cell: Cell, signal: AbortSignal | undefined, waitMs: number): Promise<CellReport> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let abort: (() => void) | undefined;
    const outcome = await Promise.race([
      cell.result.then(() => "settled" as const),
      new Promise<"detach">((resolve) => { timer = setTimeout(() => resolve("detach"), waitMs); }),
      new Promise<"abort">((resolve) => {
        abort = () => resolve("abort");
        if (signal?.aborted) abort();
        else signal?.addEventListener("abort", abort, { once: true });
      }),
    ]);
    clearTimeout(timer);
    if (abort) signal?.removeEventListener("abort", abort);
    if (outcome === "detach" && cell.settled === undefined) {
      cell.detached = true;
      this.host.emit("code_cell", { cell: cell.id, phase: "detached", parent: cell.parent, wait_ms: waitMs });
      this.changed(cell.parent);
      return this.report(cell, undefined, waitMs);
    }
    cell.reported = true;
    if (outcome === "abort") this.kernel?.cancel(cell.id);
    return this.report(cell, await cell.result);
  }

  async cancel(id: string): Promise<CellReport> {
    const cell = this.cells.get(id);
    if (!cell) return { cell: id, status: "not_found", stateLost: false, truncated: false, outputs: [], calls: emptyCounts(), failures: [],
      children: [], startedAt: this.clock.now() };
    cell.reported = true;
    this.kernel?.cancel(id);
    return this.report(cell, await cell.result);
  }

  async reset(): Promise<CellReport[]> {
    const running = this.kernel?.status().running;
    const cell = running === undefined ? undefined : this.cells.get(running);
    if (cell) cell.reported = true;
    await this.kernel?.reset();
    return cell ? [this.report(cell, await cell.result)] : [];
  }

  async rebind(): Promise<void> {
    this.releaseWatchers();
    this.deferred ??= [];
    const kernel = this.kernel;
    this.kernel = undefined;
    await kernel?.close();
  }

  bound(): void {
    const deferred = this.deferred ?? [];
    this.deferred = undefined;
    for (const cell of deferred) this.track(this.deliver(cell));
  }

  async close(): Promise<void> {
    this.closed = true;
    this.releaseWatchers();
    const kernel = this.kernel;
    this.kernel = undefined;
    await kernel?.close();
    for (const cell of this.deferred ?? []) this.host.emit("code_cell_unreported", { cell: cell.id, reason: "closed" });
    this.deferred = undefined;
  }

  async notified(): Promise<void> {
    while (this.notices.size > 0) await Promise.allSettled([...this.notices]);
  }

  state(): Record<string, unknown> {
    return {
      closed: this.closed, kernel: this.kernel?.status() ?? null,
      cells: [...this.cells.values()].map((cell) => ({ cell: cell.id, parent: cell.parent, detached: cell.detached })),
    };
  }

  private settle(cell: Cell, result: CellResult): void {
    cell.settled = result;
    cell.durationMs = Math.max(0, this.clock.now() - cell.startedAt);
    this.cells.delete(cell.id);
    this.parents.delete(cell.parent);
    const report = this.report(cell, result);
    this.remember(cell.parent, report);
    if (cell.detached) this.host.journal({ phase: "settled", tool_call_id: cell.parent, cell: cell.id,
      status: result.status, duration_ms: cell.durationMs, counts: { ...cell.calls },
      report: { cell: report.cell, status: report.status, error: report.error,
        calls: report.calls, failures: report.failures, children: report.children, childrenOmitted: report.childrenOmitted,
        startedAt: report.startedAt, durationMs: report.durationMs, stateLost: report.stateLost, truncated: report.truncated },
    }, CODE_CELL_ENTRY);
    this.changed(cell.parent, true);
    this.host.emit("code_cell", { cell: cell.id, phase: "settled", status: result.status,
      state_lost: result.stateLost, detached: cell.detached });
    if (!cell.detached || cell.reported) return;
    cell.reported = true;
    if (this.deferred) this.deferred.push(cell);
    else this.track(this.deliver(cell));
  }

  private track(delivery: Promise<void>): void {
    this.notices.add(delivery);
    void delivery.finally(() => this.notices.delete(delivery));
  }

  private async deliver(cell: Cell): Promise<void> {
    const key = `code:${cell.parent}:${cell.id}`;
    if (this.closed) {
      this.host.emit("code_cell_unreported", { cell: cell.id, key, reason: "closed" });
      return;
    }
    const disposition = await this.host.notify(key, noticeText(this.report(cell, cell.settled)))
      .catch((error: unknown) => `rejected: ${reason(error)}`);
    this.host.emit("code_cell_notified", { cell: cell.id, key, disposition });
  }

  private observe(event: KernelEvent): void {
    if (event.kind === "kernel_started") {
      this.host.emit("code_kernel_started", { module: event.module });
    } else if (event.kind === "state_lost") {
      this.host.emit("code_state_lost", { reason: event.reason, cells: event.cells });
    } else if (event.kind === "call_discarded") {
      this.host.emit("code_call_discarded", { cell: event.cell, call: event.call, tool: event.tool });
    }
  }

  private async call(request: ToolRequest): Promise<unknown> {
    const cell = this.cells.get(request.cell);
    if (!cell || this.closed || request.signal.aborted) {
      this.host.emit("code_call", { cell: request.cell, call: request.call, tool: request.tool, phase: "refused", reason: "stale_binding" });
      throw new CodeCallError("stale_binding", "the cell's binding was replaced or settled; nothing ran");
    }
    const parent = { tool_call_id: cell.parent, cell: cell.id, call: request.call };
    const child: CodeChild = { key: childKey(parent), parent, tool: request.tool };
    const retained = cell.children.length < CHILDREN_LIMIT;
    const args = retained ? argumentsFor(request.input, Math.min(ARGS_LIMIT, CELL_ARGS_LIMIT - cell.argsBytes)) : {};
    const outcome: ChildOutcome = { tool: request.tool, call: request.call, key: child.key,
      startedAt: this.clock.now(), ...args, ...(cell.detached ? { late: true } : {}) };
    const ordinal = ++cell.calls.total;
    cell.calls.running++;
    if (retained) {
      cell.children.push(outcome);
      cell.argsBytes += Buffer.byteLength(outcome.args ?? "");
    } else cell.omitted[request.tool] = (cell.omitted[request.tool] ?? 0) + 1;
    const recordCall = (phase: "started" | "settled") => {
      const settled = phase === "settled" ? { ok: outcome.ok === true, effect: outcome.effect ?? null,
        duration_ms: outcome.durationMs, ...(outcome.ok ? {} : { code: outcome.code, message: clipUtf8(outcome.message ?? "", 300) }),
        ...(outcome.resultLines === undefined ? {} : { result_lines: outcome.resultLines, result_bytes: outcome.resultBytes }),
        ...(outcome.structured === undefined ? {} : { structured: outcome.structured }) } : {};
      this.host.journal({ phase, tool: request.tool, parent, ...settled });
      this.host.emit("code_call", { cell: cell.id, call: request.call, key: child.key, tool: request.tool, phase, ...settled });
    };
    const complete = () => {
      outcome.durationMs = Math.max(0, this.clock.now() - outcome.startedAt);
      if (cell.detached || cell.settled) outcome.late = true;
      cell.calls.running--;
      if (outcome.ok) cell.calls.ok++;
      else {
        cell.calls.failed++;
        // Failure summaries remain in call order even when parallel calls finish out of order.
        const index = cell.failures.findIndex(failure => Number(failure.call.split(".").at(-1)) > ordinal);
        if (index < 0) cell.failures.push(outcome);
        else cell.failures.splice(index, 0, outcome);
      }
      recordCall("settled");
      this.changed(cell.parent);
    };
    recordCall("started");
    this.changed(cell.parent);
    let result: AgentToolCallOutcome;
    try {
      result = await runChild(child, () => cell.ctx.executeTool(request.tool, request.input ?? {}, {
        signal: request.signal,
        onUpdate: update => {
          if (retained) {
            outcome.preview = clipUtf8(contentText(update.content).split("\n", 12).join("\n"), 1024);
            this.changed(cell.parent);
          }
        },
      }));
    } catch (error) {
      Object.assign(outcome, { ok: false, code: "tool_error", message: reason(error), effect: "unknown" });
      complete();
      throw new CodeCallError("tool_error", outcome.message ?? "");
    }
    outcome.id = result.toolCall.id;
    const resultText = contentText(result.result.content);
    const structured = result.result.structuredContent;
    if (result.result.content.some(part => part.type === "text")) {
      outcome.resultBytes = Buffer.byteLength(resultText);
      outcome.resultLines = resultText.length === 0 ? 0 : 1;
      for (let offset = resultText.indexOf("\n"); offset !== -1; offset = resultText.indexOf("\n", offset + 1)) {
        outcome.resultLines++;
      }
    }
    if (retained) {
      outcome.structured = structuredPreview(structured);
      const previewText = resultText || (outcome.structured === undefined ? "" : JSON.stringify(outcome.structured));
      outcome.preview = clipUtf8(previewText.split("\n", 12).join("\n"), 1024);
    }
    Object.assign(outcome, classify(child, result, resultText || outcome.preview || ""));
    if (outcome.ok && result.result.terminate === true) cell.terminate = true;
    complete();
    // The script receives the full structured result, not the bounded journal preview.
    if (structured !== undefined && child.denied === undefined) return structured;
    if (!outcome.ok) throw new CodeCallError(outcome.code ?? "tool_error", outcome.message ?? "");
    return resultText;
  }

  private report(cell: Cell, result: CellResult | undefined, waitMs?: number): CellReport {
    return {
      cell: cell.id, status: result?.status ?? (cell.detached ? "detached" : "running"),
      startedAt: cell.startedAt, ...(cell.durationMs === undefined ? {} : { durationMs: cell.durationMs }),
      stateLost: result?.stateLost ?? false, truncated: result?.truncated ?? false,
      ...(result?.value === undefined ? {} : { value: result.value }), ...(result?.error ? { error: result.error } : {}),
      outputs: result?.outputs ?? [], calls: cell.calls, children: cell.children,
      ...(Object.keys(cell.omitted).length ? { childrenOmitted: cell.omitted } : {}), failures: cell.failures,
      ...(waitMs === undefined ? {} : { waitMs }), ...(cell.terminate ? { terminate: true } : {}),
    };
  }
}
