// A Corral-owned persistent JavaScript kernel for Code Mode (T-682), on Bun. One CodeKernel per
// bound host session (CodeKernels keys them) runs cells strictly in submission order on its own
// worker thread (kernel-worker.ts); tool calls inside one cell may overlap. Tools enter only
// through the injected ToolBridge, which applies the host's normal checks to every call.
//
// Lifecycle: top-level variables, functions and classes survive from cell to cell within one
// worker generation. They do not survive reset, recovery, close or a host/process restart, and
// nothing is snapshotted or replayed: a new generation starts empty, `state_lost` reports every
// discarded generation, and each CellResult names the generation that ran it. Canonical state
// (todos, receipts) never lives in the heap. Cancelling a queued cell removes it. An active cell
// cannot be stopped in place, because code it left suspended in the shared heap could resume
// later, so cancelling it or reaching its deadline terminates and replaces the worker: the cell
// settles with `stateLost` and `state_lost` names the reason. A cell is reported finished only
// once the Promise jobs it queued have drained, so its deadline also bounds detached work it left
// running; after that none of its code runs on its own (see kernel-worker.ts). Results of a
// settled cell or a replaced worker are discarded and never reach another cell.
import { Worker } from "node:worker_threads";

import {
  type HostMessage, type KernelOutput, OutputBudget, type OutputLimits, type WireError, type WorkerMessage,
} from "./kernel-protocol.ts";

export type { KernelOutput, OutputLimits, WireError } from "./kernel-protocol.ts";

const COMPILED = /\$bunfs|~BUN|%7EBUN/.test(import.meta.url);
/** In the compiled host the worker is an extra entrypoint (scripts/build-pi-host.sh) addressed from its root. */
export const KERNEL_WORKER: string | URL = COMPILED
  ? "./src/extensions/codemode/kernel-worker.ts" : new URL("./kernel-worker.ts", import.meta.url);

const CELL_ID = /^[\w.:-]{1,128}$/;
const MAX_DELAY = 2_147_483_647;

export interface KernelLimits {
  readonly cellTimeoutMs: number;
  /** How long close and reset wait for a terminated worker to exit before releasing it. */
  readonly exitWaitMs: number;
  readonly maxQueuedCells: number;
  readonly maxCodeBytes: number;
  readonly output: OutputLimits;
}

export const DEFAULT_KERNEL_LIMITS: KernelLimits = {
  cellTimeoutMs: 15 * 60_000, exitWaitMs: 2_000, maxQueuedCells: 16, maxCodeBytes: 256 * 1024,
  output: { maxTextBytes: 64 * 1024, maxValueBytes: 16 * 1024, maxImages: 4, maxImageBytes: 4 * 1024 * 1024, maxErrorBytes: 4096 },
};

export interface ToolRequest {
  readonly session: string;
  readonly generation: number;
  readonly cell: string;
  readonly call: string;
  readonly tool: string;
  readonly input: unknown;
  /** Aborted when the cell settles, is cancelled or its worker is replaced; a later result is discarded. */
  readonly signal: AbortSignal;
}

/** Runs one nested tool call; the resolved value must be JSON-serializable. */
export type ToolBridge = (request: ToolRequest) => Promise<unknown>;

export type CellStatus = "ok" | "error" | "cancelled" | "timeout" | "reset" | "closed";
export type StateLossReason = "reset" | "cancelled" | "timeout" | "worker_failed";

export interface CellResult {
  readonly cell: string;
  readonly status: CellStatus;
  readonly generation: number;
  /** The rendered value of an explicit `return`; undefined when the cell returned nothing. */
  readonly value?: string;
  readonly outputs: readonly KernelOutput[];
  readonly error?: WireError;
  readonly truncated: boolean;
  /** The generation that ran this cell was discarded while it ran: its variables are gone. */
  readonly stateLost: boolean;
}

export interface CellHandle {
  readonly cell: string;
  readonly result: Promise<CellResult>;
}

export type KernelEvent = { readonly session: string; readonly generation: number } & (
  | { readonly kind: "kernel_started"; readonly module: string }
  | { readonly kind: "cell_queued" | "cell_started"; readonly cell: string }
  | { readonly kind: "call_started" | "call_discarded"; readonly cell: string; readonly call: string; readonly tool: string }
  | { readonly kind: "call_finished"; readonly cell: string; readonly call: string; readonly tool: string; readonly ok: boolean }
  | { readonly kind: "cell_finished"; readonly cell: string; readonly status: CellStatus }
  | { readonly kind: "state_lost"; readonly reason: StateLossReason; readonly cells: number }
  | { readonly kind: "kernel_closed"; readonly cells: number }
);

type EventBody = KernelEvent extends infer E ? E extends KernelEvent ? Omit<E, "session" | "generation"> : never : never;

export type KernelErrorCode = "kernel_closed" | "queue_full" | "code_too_large" | "invalid_cell";

export class KernelError extends Error {
  constructor(readonly code: KernelErrorCode, message: string) {
    super(`${code}: ${message}`);
    this.name = "KernelError";
  }
}

export interface CodeKernelOptions {
  readonly session: string;
  readonly tools: ToolBridge;
  readonly limits?: Partial<Omit<KernelLimits, "output">> & { readonly output?: Partial<OutputLimits> };
  /** Called synchronously for every event; it must not throw. */
  readonly onEvent?: (event: KernelEvent) => void;
  readonly worker?: string | URL;
}

export interface KernelStatus {
  readonly generation: number;
  readonly cells: number;
  readonly running: string | undefined;
  readonly queued: readonly string[];
  readonly worker: boolean;
  /** Terminated workers whose exit has not been observed yet. */
  readonly retiring: number;
  readonly closed: boolean;
}

interface Queued {
  readonly cell: string;
  readonly code: string;
  readonly timeoutMs: number;
  readonly resolve: (result: CellResult) => void;
}

interface Running extends Queued {
  readonly generation: number;
  readonly budget: OutputBudget;
  readonly outputs: KernelOutput[];
  readonly calls: Map<string, AbortController>;
  readonly deadline: ReturnType<typeof setTimeout>;
}

type Outcome = { readonly ok: true; readonly value: unknown } | { readonly ok: false; readonly error: unknown };

/** Unbounded error fields; OutputBudget.error clips them and records the truncation. */
function errorFields(error: unknown): WireError {
  if (!(error instanceof Error)) return { name: "Error", message: String(error) };
  const code = (error as { code?: unknown }).code;
  return { name: error.name, message: error.message, ...(typeof code === "string" ? { code } : {}) };
}

export class CodeKernel {
  readonly session: string;
  private readonly limits: KernelLimits;
  private generation = 0;
  private cells = 0;
  private sequence = 0;
  private worker: Worker | undefined;
  private running: Running | undefined;
  private readonly queue: Queued[] = [];
  private readonly exits = new Set<Promise<void>>();
  private closing: Promise<void> | undefined;
  private isClosed = false;

  constructor(private readonly options: CodeKernelOptions) {
    this.session = options.session;
    const limits = options.limits ?? {};
    this.limits = { ...DEFAULT_KERNEL_LIMITS, ...limits, output: { ...DEFAULT_KERNEL_LIMITS.output, ...limits.output } };
  }

  get closed(): boolean {
    return this.isClosed;
  }

  status(): KernelStatus {
    return {
      generation: this.generation, cells: this.cells, running: this.running?.cell, queued: this.queue.map(({ cell }) => cell),
      worker: this.worker !== undefined, retiring: this.exits.size, closed: this.isClosed,
    };
  }

  /** Queues a cell; it runs after every cell submitted before it. The result never rejects. */
  submit(code: string, options: { readonly cell?: string; readonly timeoutMs?: number } = {}): CellHandle {
    if (this.isClosed) throw new KernelError("kernel_closed", `the kernel of session ${this.session} is closed`);
    if (Buffer.byteLength(code) > this.limits.maxCodeBytes) {
      throw new KernelError("code_too_large", `cells are limited to ${this.limits.maxCodeBytes} bytes`);
    }
    const cell = options.cell ?? `cell-${++this.sequence}`;
    if (!CELL_ID.test(cell) || this.running?.cell === cell || this.queue.some((queued) => queued.cell === cell)) {
      throw new KernelError("invalid_cell", `cell ids are unique among live cells and match ${CELL_ID.source}`);
    }
    const timeoutMs = options.timeoutMs ?? this.limits.cellTimeoutMs;
    if (!Number.isFinite(timeoutMs) || timeoutMs <= 0 || timeoutMs > MAX_DELAY) {
      throw new KernelError("invalid_cell", `timeoutMs must be in (0, ${MAX_DELAY}]`);
    }
    if (this.queue.length >= this.limits.maxQueuedCells) {
      throw new KernelError("queue_full", `at most ${this.limits.maxQueuedCells} cells may wait`);
    }
    let resolve: (result: CellResult) => void = () => undefined;
    const result = new Promise<CellResult>((settle) => { resolve = settle; });
    this.queue.push({ cell, code, timeoutMs, resolve });
    this.emit({ kind: "cell_queued", cell });
    this.pump();
    return { cell, result };
  }

  /** Cancels a queued or active cell; false when no live cell has that id. */
  cancel(cell: string): boolean {
    const index = this.queue.findIndex((queued) => queued.cell === cell);
    const queued = index >= 0 ? this.queue.splice(index, 1)[0] : undefined;
    if (queued) {
      this.finishQueued(queued, "cancelled");
      return true;
    }
    if (this.running?.cell !== cell) return false;
    this.lose("cancelled");
    return true;
  }

  /** Discards the current generation and its state; the active cell settles as `reset`. */
  async reset(): Promise<void> {
    if (this.isClosed) throw new KernelError("kernel_closed", `the kernel of session ${this.session} is closed`);
    if (this.worker) this.lose("reset");
    else this.emit({ kind: "state_lost", reason: "reset", cells: 0 });
    await this.drainExits();
  }

  /** Settles every live cell as `closed`, terminates the worker and waits (bounded) for its exit. */
  close(): Promise<void> {
    this.closing ??= this.shutdown();
    return this.closing;
  }

  private async shutdown(): Promise<void> {
    this.isClosed = true;
    for (const queued of this.queue.splice(0)) this.finishQueued(queued, "closed");
    const worker = this.worker;
    this.worker = undefined;
    if (worker) this.retire(worker);
    if (this.running) this.settle(this.running, "closed", { stateLost: true });
    await this.drainExits();
    this.emit({ kind: "kernel_closed", cells: this.cells });
  }

  private emit(event: EventBody): void {
    this.options.onEvent?.({ session: this.session, generation: this.generation, ...event } as KernelEvent);
  }

  private send(worker: Worker, message: HostMessage): void {
    worker.postMessage(message);
  }

  private spawn(): Worker {
    this.generation++;
    this.cells = 0;
    const worker = new Worker(this.options.worker ?? KERNEL_WORKER, { env: {} });
    const failed = (message: string) => {
      if (this.worker === worker) this.lose("worker_failed", { name: "KernelError", message, code: "worker_failed" });
    };
    worker.on("message", (message: WorkerMessage) => {
      if (this.worker === worker) this.receive(worker, message);
    });
    worker.on("error", (error: Error) => failed(`the kernel worker failed: ${error.message}`));
    worker.on("exit", (code: number) => failed(`the kernel worker exited with code ${code}`));
    this.worker = worker;
    return worker;
  }

  private pump(): void {
    if (this.isClosed || this.running) return;
    const next = this.queue.shift();
    if (!next) return;
    const worker = this.worker ?? this.spawn();
    const running: Running = {
      ...next, generation: this.generation, budget: new OutputBudget(this.limits.output), outputs: [], calls: new Map(),
      deadline: setTimeout(() => { if (this.running === running) this.lose("timeout"); }, next.timeoutMs),
    };
    this.running = running;
    this.cells++;
    this.emit({ kind: "cell_started", cell: next.cell });
    this.send(worker, { op: "run", cell: next.cell, code: next.code, limits: this.limits.output });
  }

  private receive(worker: Worker, message: WorkerMessage): void {
    const running = this.running;
    switch (message.op) {
      case "ready":
        this.emit({ kind: "kernel_started", module: message.module });
        return;
      case "call":
        this.call(worker, message);
        return;
      case "output": {
        const item = running?.cell === message.cell ? running.budget.admit(message.item) : undefined;
        if (running && item) running.outputs.push(item);
        return;
      }
      case "done":
        if (running?.cell !== message.cell) return;
        this.settle(running, message.status, {
          stateLost: false, truncated: message.truncated,
          ...(message.value === undefined ? {} : { value: message.value }),
          ...(message.error ? { error: running.budget.error(message.error) } : {}),
        });
        return;
    }
  }

  private call(worker: Worker, message: Extract<WorkerMessage, { op: "call" }>): void {
    const { cell, call, tool } = message;
    const running = this.running;
    if (running?.cell !== cell || running.calls.has(call)) {
      this.emit({ kind: "call_discarded", cell, call, tool });
      return;
    }
    const controller = new AbortController();
    running.calls.set(call, controller);
    this.emit({ kind: "call_started", cell, call, tool });
    let input: unknown;
    try {
      input = message.input === undefined ? undefined : JSON.parse(message.input);
    } catch (error) {
      this.answer(worker, running, call, tool, { ok: false, error });
      return;
    }
    const request: ToolRequest = {
      session: this.session, generation: running.generation, cell, call, tool, input, signal: controller.signal,
    };
    void Promise.resolve().then(() => this.options.tools(request)).then(
      (value) => this.answer(worker, running, call, tool, { ok: true, value }),
      (error: unknown) => this.answer(worker, running, call, tool, { ok: false, error }),
    );
  }

  private answer(worker: Worker, running: Running, call: string, tool: string, outcome: Outcome): void {
    const live = this.worker === worker && this.running === running && running.calls.delete(call);
    if (!live) {
      this.emit({ kind: "call_discarded", cell: running.cell, call, tool });
      return;
    }
    let value: string | undefined;
    let error = outcome.ok ? undefined : running.budget.error(errorFields(outcome.error));
    if (outcome.ok) {
      try {
        value = outcome.value === undefined ? undefined : JSON.stringify(outcome.value);
      } catch (cause) {
        const message = `${tool} returned a value that is not JSON-serializable: ${errorFields(cause).message}`;
        error = running.budget.error({ name: "TypeError", message });
      }
    }
    this.send(worker, error
      ? { op: "failure", cell: running.cell, call, error }
      : { op: "result", cell: running.cell, call, ...(value === undefined ? {} : { value }) });
    this.emit({ kind: "call_finished", cell: running.cell, call, tool, ok: error === undefined });
  }

  private settle(running: Running, status: CellStatus, result: {
    readonly stateLost: boolean; readonly truncated?: boolean; readonly value?: string; readonly error?: WireError;
  }): void {
    if (this.running !== running) return;
    this.running = undefined;
    clearTimeout(running.deadline);
    for (const controller of running.calls.values()) controller.abort();
    running.calls.clear();
    running.resolve({
      cell: running.cell, status, generation: running.generation, outputs: running.outputs,
      truncated: result.truncated === true || running.budget.truncated, stateLost: result.stateLost,
      ...(result.value === undefined ? {} : { value: result.value }), ...(result.error ? { error: result.error } : {}),
    });
    this.emit({ kind: "cell_finished", cell: running.cell, status });
    this.pump();
  }

  private finishQueued(queued: Queued, status: "cancelled" | "closed"): void {
    queued.resolve({ cell: queued.cell, status, generation: this.generation, outputs: [], truncated: false, stateLost: false });
    this.emit({ kind: "cell_finished", cell: queued.cell, status });
  }

  private lose(reason: StateLossReason, error?: WireError): void {
    const worker = this.worker;
    if (!worker) return;
    this.worker = undefined;
    this.retire(worker);
    this.emit({ kind: "state_lost", reason, cells: this.cells });
    const running = this.running;
    if (!running) return;
    this.settle(running, reason === "worker_failed" ? "error" : reason, {
      stateLost: true, ...(error ? { error: running.budget.error(error) } : {}),
    });
  }

  // A retired worker never keeps the host alive and its late messages are ignored; a worker
  // blocked in native code may never exit, so waiting for it is bounded by exitWaitMs.
  private retire(worker: Worker): void {
    worker.removeAllListeners();
    worker.on("error", () => undefined);
    worker.unref();
    const exit: Promise<void> = worker.terminate().then(() => undefined, () => undefined)
      .finally(() => this.exits.delete(exit));
    this.exits.add(exit);
  }

  private async drainExits(): Promise<void> {
    if (this.exits.size === 0) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const bound = new Promise<void>((resolve) => { timer = setTimeout(resolve, this.limits.exitWaitMs); });
    try {
      await Promise.race([Promise.all(this.exits), bound]);
    } finally {
      clearTimeout(timer);
    }
  }
}

/** One kernel per bound host session: the same session reaches the same live kernel, others never share it. */
export class CodeKernels {
  private readonly kernels = new Map<string, CodeKernel>();

  constructor(private readonly create: (session: string) => CodeKernel) {}

  kernel(session: string): CodeKernel {
    const existing = this.kernels.get(session);
    if (existing && !existing.closed) return existing;
    const created = this.create(session);
    this.kernels.set(session, created);
    return created;
  }

  async close(session: string): Promise<void> {
    const kernel = this.kernels.get(session);
    this.kernels.delete(session);
    await kernel?.close();
  }

  async closeAll(): Promise<void> {
    await Promise.all([...this.kernels.keys()].map((session) => this.close(session)));
  }
}
