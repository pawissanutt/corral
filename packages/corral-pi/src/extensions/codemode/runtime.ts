// Code Mode on the Corral-owned persistent kernel (T-683). One runtime per host session owns one
// lazily started CodeKernel (kernel.ts) and turns each tool request of a cell into a nested call
// through the calling `codemode` tool's ctx.executeTool, so Pi's validation and the host's tool_call
// checks apply exactly as to a direct call. Each child runs under its CodeChild identity (child.ts),
// is journaled in the session before it starts and after it settles (corral recovery classifies a
// child that never settled from its receipt), and is recorded for the cell's report.
//
// A cell runs in the foreground for up to wait_ms, then detaches: the call returns a handle and the
// settlement is reported exactly once, by the cancel call that stopped it or else by one message
// through the host's bridge delivery. A reload or close replaces the binding: the kernel is closed,
// its cells settle `closed`, and no request of the old binding reaches a tool again.
import type { ExtensionToolContext } from "@earendil-works/pi-coding-agent";

import { type CodeChild, childKey, runChild } from "./child.ts";
import { type CellResult, CodeKernel, type CodeKernelOptions, KernelError, type KernelEvent, type ToolRequest } from "./kernel.ts";
import { type CellReport, type ChildOutcome, classify, contentText, counts, noticeText } from "./report.ts";

export const CODE_CALL_ENTRY = "corral_code_call";
export const DEFAULT_WAIT_MS = 60_000;
export const MAX_WAIT_MS = 600_000;

export interface CodeHost {
  readonly generation: number;
  emit(kind: string, data: Record<string, unknown>): void;
  /** Appends one `corral_code_call` entry to the bound session. */
  journal(data: Record<string, unknown>): void;
  /** Delivers a detached cell's one notice; resolves to the delivery disposition. */
  notify(key: string, text: string): Promise<string>;
  readonly kernel?: Pick<CodeKernelOptions, "limits" | "worker">;
}

export type Executor = Pick<ExtensionToolContext, "executeTool">;

interface Cell {
  readonly id: string;
  readonly parent: string;
  readonly epoch: number;
  readonly ctx: Executor;
  readonly children: ChildOutcome[];
  readonly result: Promise<CellResult>;
  detached: boolean;
  reported: boolean;
  terminate: boolean;
  settled?: CellResult;
}

class CodeCallError extends Error {
  constructor(readonly code: string, message: string) {
    super(message);
    this.name = "CodeCallError";
  }
}

const reason = (error: unknown) => (error instanceof Error ? error.message : String(error)).slice(0, 512);

export class CodeRuntime {
  private kernel: CodeKernel | undefined;
  private epoch = 1;
  private sequence = 0;
  private closed = false;
  /** Detached settlements a reload produced, delivered once the replacement binding starts. */
  private deferred: Cell[] | undefined;
  private readonly cells = new Map<string, Cell>();
  private readonly notices = new Set<Promise<void>>();

  constructor(private readonly host: CodeHost) {}

  private current(): CodeKernel {
    this.kernel ??= new CodeKernel({
      session: `g${this.host.generation}-e${this.epoch}`, tools: (request) => this.call(request),
      onEvent: (event) => this.observe(event), ...this.host.kernel,
    });
    return this.kernel;
  }

  async run(parent: string, code: string, ctx: Executor, signal: AbortSignal | undefined,
    waitMs = DEFAULT_WAIT_MS): Promise<CellReport> {
    const id = `cell-${++this.sequence}`;
    let result: Promise<CellResult>;
    try {
      if (this.closed) throw new KernelError("kernel_closed", "Code Mode is closed for this host");
      result = this.current().submit(code, { cell: id }).result;
    } catch (error) {
      if (!(error instanceof KernelError)) throw error;
      return { cell: id, status: "error", stateLost: false, truncated: false, outputs: [], calls: counts([]), failures: [],
        error: { name: error.name, message: error.message, code: error.code } };
    }
    const cell: Cell = { id, parent, epoch: this.epoch, ctx, children: [], result, detached: false, reported: false,
      terminate: false };
    this.cells.set(id, cell);
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
      return this.report(cell, undefined, waitMs);
    }
    cell.reported = true;
    if (outcome === "abort") this.kernel?.cancel(cell.id);
    return this.report(cell, await cell.result);
  }

  /** Stops a live cell; this call reports its settlement, so no notice follows. */
  async cancel(id: string): Promise<CellReport> {
    const cell = this.cells.get(id);
    if (!cell) return { cell: id, status: "not_found", stateLost: false, truncated: false, outputs: [], calls: counts([]), failures: [] };
    cell.reported = true;
    this.kernel?.cancel(id);
    return this.report(cell, await cell.result);
  }

  /** Discards every variable; the running cell, if any, settles `reset` and is reported here. */
  async reset(): Promise<CellReport[]> {
    const running = this.kernel?.status().running;
    const cell = running === undefined ? undefined : this.cells.get(running);
    if (cell) cell.reported = true;
    await this.kernel?.reset();
    return cell ? [this.report(cell, await cell.result)] : [];
  }

  /** A reload replaces the binding: no request of the old one runs, and its cells settle `closed`. */
  async rebind(): Promise<void> {
    this.epoch++;
    this.deferred ??= [];
    const kernel = this.kernel;
    this.kernel = undefined;
    await kernel?.close();
  }

  /** The replacement binding is live: deliver what the old one left. */
  bound(): void {
    const deferred = this.deferred ?? [];
    this.deferred = undefined;
    for (const cell of deferred) this.track(this.deliver(cell));
  }

  async close(): Promise<void> {
    this.closed = true;
    this.epoch++;
    const kernel = this.kernel;
    this.kernel = undefined;
    await kernel?.close();
    for (const cell of this.deferred ?? []) {
      this.host.emit("code_cell_unreported", { cell: cell.id, reason: "closed" });
    }
    this.deferred = undefined;
  }

  /** Every notice delivery started so far has finished. */
  async notified(): Promise<void> {
    while (this.notices.size > 0) await Promise.allSettled([...this.notices]);
  }

  state(): Record<string, unknown> {
    return {
      epoch: this.epoch, closed: this.closed, kernel: this.kernel?.status() ?? null,
      cells: [...this.cells.values()].map((cell) => ({ cell: cell.id, parent: cell.parent, detached: cell.detached })),
    };
  }

  private settle(cell: Cell, result: CellResult): void {
    cell.settled = result;
    this.cells.delete(cell.id);
    this.host.emit("code_cell", { cell: cell.id, phase: "settled", status: result.status, generation: result.generation,
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
    const key = `code:g${this.host.generation}:${cell.id}`;
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
      this.host.emit("code_kernel_started", { module: event.module, generation: event.generation });
    } else if (event.kind === "state_lost") {
      this.host.emit("code_state_lost", { reason: event.reason, generation: event.generation, cells: event.cells });
    } else if (event.kind === "call_discarded") {
      this.host.emit("code_call_discarded", { cell: event.cell, call: event.call, tool: event.tool });
    }
  }

  private async call(request: ToolRequest): Promise<unknown> {
    const cell = this.cells.get(request.cell);
    if (!cell || cell.epoch !== this.epoch || this.closed || request.signal.aborted) {
      this.host.emit("code_call", { cell: request.cell, call: request.call, tool: request.tool, phase: "refused",
        reason: "stale_binding" });
      throw new CodeCallError("stale_binding", "the cell's binding was replaced or settled; nothing ran");
    }
    const parent = { tool_call_id: cell.parent, cell: cell.id, call: request.call };
    const child: CodeChild = { key: childKey(parent), parent, tool: request.tool };
    const outcome: ChildOutcome = { tool: request.tool, call: request.call, key: child.key };
    cell.children.push(outcome);
    const record = (phase: "started" | "settled") => {
      const settled = phase === "settled" ? { ok: outcome.ok === true, effect: outcome.effect ?? null,
        ...(outcome.request_id ? { request_id: outcome.request_id } : {}) } : {};
      this.host.journal({ phase, tool: request.tool, generation: this.host.generation, kernel: request.generation,
        parent, ...settled });
      this.host.emit("code_call", { cell: cell.id, call: request.call, key: child.key, tool: request.tool, phase, ...settled });
    };
    record("started");
    let result: Awaited<ReturnType<Executor["executeTool"]>>;
    try {
      result = await runChild(child, () => cell.ctx.executeTool(request.tool, request.input ?? {}, { signal: request.signal }));
    } catch (error) {
      Object.assign(outcome, { ok: false, code: "tool_error", message: reason(error), effect: "unknown" });
      record("settled");
      throw new CodeCallError("tool_error", outcome.message ?? "");
    }
    Object.assign(outcome, classify(child, result));
    if (outcome.ok && result.result.terminate === true) cell.terminate = true;
    record("settled");
    // The predecessor contract (T-681): a structured tool resolves to its structuredContent,
    // refusals included, so receipts are read without parsing text.
    const structured = result.result.structuredContent;
    if (structured !== undefined && child.denied === undefined) return structured;
    if (!outcome.ok) throw new CodeCallError(outcome.code ?? "tool_error", outcome.message ?? "");
    return contentText(result.result.content);
  }

  private report(cell: Cell, result: CellResult | undefined, waitMs?: number): CellReport {
    return {
      cell: cell.id, status: result?.status ?? "detached", ...(result ? { generation: result.generation } : {}),
      stateLost: result?.stateLost ?? false, truncated: result?.truncated ?? false,
      ...(result?.value === undefined ? {} : { value: result.value }), ...(result?.error ? { error: result.error } : {}),
      outputs: result?.outputs ?? [], calls: counts(cell.children),
      failures: cell.children.filter((child) => child.ok === false).map((child) => ({ ...child })),
      ...(waitMs === undefined ? {} : { waitMs }), ...(cell.terminate ? { terminate: true } : {}),
    };
  }
}
