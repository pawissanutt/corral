// The common explore/oracle admission and accounting seam. Every invocation is recorded before
// billable work; an interrupted request is uncertain, never silently charged as zero.
import { createHash } from "node:crypto";
import type { AuxiliarySettings } from "./settings.ts";

export type AuxiliaryKind = "explore" | "oracle";
export type Outcome = "completed" | "uncertain" | "not_started";
export interface Invocation {
  readonly request: string;
  readonly kind: AuxiliaryKind;
  readonly model: string;
  readonly prompt: string;
  readonly sessionFile: string;
  readonly sessionId: string;
  readonly maxTurns: number;
}
export interface AuxiliaryResult { readonly text: string; readonly outcome: "completed" | "uncertain" }
export interface AuxiliaryOptions {
  readonly settings: AuxiliarySettings;
  readonly generation: number;
  readonly provider: string;
  readonly model: string;
  sessionId(): string;
  record(payload: Record<string, unknown>): Promise<Record<string, unknown>>;
  execute(invocation: Invocation, signal: AbortSignal): Promise<AuxiliaryResult>;
}

// Abortable FIFO semaphore. Taking a released slot happens before waking a waiter, so a new
// arrival cannot overtake queued work. A queued cancellation never launches a model request.
class Slots {
  private active = 0;
  private readonly queue: (() => void)[] = [];
  constructor(private readonly max: number) {}
  async take(signal: AbortSignal): Promise<() => void> {
    signal.throwIfAborted();
    if (this.active < this.max) this.active++;
    else await new Promise<void>((resolve, reject) => {
      const abort = () => {
        const index = this.queue.indexOf(wake);
        if (index >= 0) this.queue.splice(index, 1);
        reject(signal.reason);
      };
      const wake = () => { signal.removeEventListener("abort", abort); resolve(); };
      this.queue.push(wake);
      signal.addEventListener("abort", abort, { once: true });
    });
    return () => {
      const next = this.queue.shift();
      if (next) next(); else this.active--;
    };
  }
  state() { return { running: this.active, queued: this.queue.length }; }
}

export class AuxiliaryRuntime {
  private readonly explore: Slots;
  private readonly oracle = new Slots(1);
  private readonly controllers = new Set<AbortController>();
  private readonly requests = new Map<string, Promise<AuxiliaryResult & { request: string; generation: number }>>();
  private stopped = false;
  constructor(private readonly options: AuxiliaryOptions) {
    this.explore = new Slots(options.settings.explore?.max_parallel ?? 1);
  }
  run(kind: AuxiliaryKind, id: string, prompt: string, signal?: AbortSignal) {
    const key = createHash("sha256").update(JSON.stringify([this.options.sessionId(), this.options.generation, kind, id])).digest("hex");
    const previous = this.requests.get(key);
    if (previous) return previous;
    const request = this.perform(kind, key, prompt, signal);
    this.requests.set(key, request);
    return request;
  }
  private async perform(kind: AuxiliaryKind, request: string, prompt: string, outer?: AbortSignal) {
    const { options } = this;
    const settings = options.settings[kind];
    if (!settings) throw new Error(`${kind} is disabled`);
    if (this.stopped) throw new Error("auxiliary session is fenced/closed");
    if (!prompt.trim() || prompt.length > 32768) throw new Error("auxiliary prompt must be 1-32768 characters");
    const configured = settings.model ?? (kind === "explore" ? options.model : undefined);
    if (!configured) throw new Error("Oracle is not configured: set agent.capabilities.<role>.oracle.model to a model in the bound provider");
    // A second provider would need another explicit credential/transport binding. Never borrow
    // ambient auth or create a fresh unrestricted ModelRuntime for an auxiliary request.
    const prefix = `${options.provider}/`;
    const model = configured.startsWith(prefix) ? configured.slice(prefix.length) : configured;
    const controller = new AbortController();
    this.controllers.add(controller);
    const timeout = AbortSignal.timeout(settings.timeout_secs * 1000);
    const signal = AbortSignal.any([controller.signal, timeout, ...(outer ? [outer] : [])]);
    const common = { kind: "auxiliary", request, usage_kind: kind };
    let release: (() => void) | undefined;
    let prepared = false;
    let started = false;
    let outcome: Outcome = "not_started";
    let result: AuxiliaryResult | undefined;
    try {
      signal.throwIfAborted();
      const reference = await options.record({ ...common, action: "prepare", model: `${options.provider}/${model}` });
      prepared = true;
      if (typeof reference.session_file !== "string" || reference.session_id !== request) throw new Error("invalid auxiliary reference");
      release = await (kind === "explore" ? this.explore : this.oracle).take(signal);
      signal.throwIfAborted();
      await options.record({ ...common, action: "started" });
      started = true;
      outcome = "uncertain";
      signal.throwIfAborted();
      result = await options.execute({ request, kind, model, prompt,
        sessionFile: reference.session_file, sessionId: request,
        maxTurns: kind === "explore" ? options.settings.explore?.max_turns ?? 1 : 1 }, signal);
      outcome = result.outcome;
    } finally {
      // Release only after the terminal record; failure to record must not re-admit work while
      // its canonical record still says running. Admission will refuse rather than double-spend.
      try {
        if (prepared) await options.record({ ...common, action: "finish", outcome: started ? outcome : "not_started" });
      } finally { release?.(); this.controllers.delete(controller); }
    }
    if (!result) throw new Error("auxiliary request did not finish");
    return { ...result, text: result.text.slice(0, 32768), request, generation: options.generation };
  }
  /** Fence/close is permanent. Ordinary turn cancellation uses each tool's operation signal. */
  async close(): Promise<void> {
    this.stopped = true;
    await this.cancelAll();
  }
  async cancelAll(): Promise<void> {
    for (const controller of this.controllers) controller.abort(new Error("parent cancelled/reloaded/fenced"));
    await Promise.allSettled(this.requests.values());
  }
  state() { return { explore: this.explore.state(), oracle: this.oracle.state(), stopped: this.stopped }; }
}
