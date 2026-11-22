// Host-owned review and oracle/expert admission and accounting. Every invocation is recorded before
// billable work; an interrupted request is uncertain, never silently charged as zero.
import { randomUUID } from "node:crypto";
import type { AuxiliarySettings } from "./settings.ts";
import { AuxiliaryHistory, type HelperEvent, type HelperInspection } from "./inspection.ts";
import { researchBudget, type ResearchOptions, type ResearchReport } from "./research.ts";
import { RetainedConversationError } from "./conversation.ts";

export type AuxiliaryKind = "explore" | "oracle" | "web_search" | "expert";
export type Outcome = "completed" | "uncertain" | "not_started";
export interface Invocation {
  readonly request: string;
  readonly kind: AuxiliaryKind;
  readonly provider: string;
  readonly model: string;
  readonly prompt: string;
  readonly sessionFile: string;
  readonly sessionId: string;
  readonly maxTurns: number;
  readonly research?: ResearchOptions;
  readonly deadlineAt?: number;
  readonly serviceTier?: "priority";
  readonly observe?: (event: HelperEvent) => void;
}
export interface AuxiliaryResult {
  readonly text: string;
  readonly outcome: "completed" | "uncertain";
  readonly report?: ResearchReport;
  readonly turns?: number;
}
export interface AuxiliaryResponse extends AuxiliaryResult {
  readonly request: string;
  /** Exact parent-local child lookup key, not necessarily the accounting request. */
  readonly conversationId?: string;
  readonly truncated: boolean;
  readonly sessionFile?: string;
  readonly model?: string;
}
export interface AuxiliaryOptions {
  readonly settings: AuxiliarySettings;
  readonly provider: string;
  readonly model: string;
  sessionId(): string;
  /** Search follows the current approved route, including an owner model switch. */
  searchModel?(): { provider: string; model: string } | undefined;
  record(payload: Record<string, unknown>): Promise<Record<string, unknown>>;
  execute(invocation: Invocation, signal: AbortSignal): Promise<AuxiliaryResult>;
  changed?(): void;
  retain?(helper: HelperInspection, file: string, report?: string): boolean;
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

export interface AuxiliaryObservation {
  readonly request: string; readonly parent: string; readonly kind: AuxiliaryKind;
  readonly model: string; state: string; sessionFile?: string;
}
export class AuxiliaryRuntime {
  private readonly observations = new Map<string, AuxiliaryObservation>();
  inspection(): readonly AuxiliaryObservation[] { return [...this.observations.values()].slice(-100).map(item => ({ ...item })); }
  private readonly history: AuxiliaryHistory;
  inspections() { return this.history.snapshot(); }
  private readonly explore: Slots;
  private readonly oracle = new Slots(1);
  private readonly webSearch = new Slots(1);
  private readonly expert: Slots;
  private readonly controllers = new Set<AbortController>();
  private readonly requests = new Map<string, { id: string; done: Promise<AuxiliaryResponse> }>();
  private stopped = false;
  constructor(private readonly options: AuxiliaryOptions) {
    this.explore = new Slots(options.settings.explore?.max_parallel ?? 1);
    this.expert = new Slots(options.settings.expert?.max_parallel ?? 1);
    this.history = new AuxiliaryHistory(options.changed);
  }
  /** Observe only this parent tool call, including when subscribed before it starts. */
  watch(kind: AuxiliaryKind, id: string, changed: (inspection: HelperInspection) => void): () => void {
    const key = `${kind}:${id}`;
    return this.history.subscribe(() => {
      const request = this.requests.get(key);
      if (!request) return;
      const inspection = this.history.snapshot().find(entry => entry.request === request.id);
      if (inspection) changed(inspection);
    });
  }
  run(kind: Exclude<AuxiliaryKind, "explore">, id: string, prompt: string, signal?: AbortSignal) {
    if (kind === ("explore" as AuxiliaryKind)) return Promise.reject(new Error("explore must use FleetRuntime.explore"));
    const key = `${kind}:${id}`;
    const previous = this.requests.get(key);
    if (previous) return previous.done;
    // Retries share this operation in memory; the transcript name must be fresh on relaunch.
    const request = randomUUID();
    const { promise, resolve, reject } = Promise.withResolvers<AuxiliaryResponse>();
    this.requests.set(key, { id: request, done: promise });
    let retained = false;
    void this.perform(kind, request, prompt, () => { retained = true; }, signal).then(result => {
      resolve(retained ? { ...result, conversationId: request } : result);
    }, error => reject(retained ? new RetainedConversationError(request, error) : error));
    return promise;
  }
  private slots(kind: AuxiliaryKind): Slots {
    switch (kind) {
      case "explore": return this.explore;
      case "oracle": return this.oracle;
      case "web_search": return this.webSearch;
      case "expert": return this.expert;
    }
  }
  private async perform(kind: AuxiliaryKind, request: string, prompt: string, saved: () => void, outer?: AbortSignal): Promise<AuxiliaryResponse> {
    const { options } = this;
    const settings = options.settings[kind];
    if (!settings) throw new Error(`${kind} is disabled`);
    if (this.stopped) throw new Error("auxiliary session is closed");
    if (!prompt.trim() || prompt.length > 32768) throw new Error("auxiliary prompt must be 1-32768 characters");
    const binding = kind === "web_search" ? options.searchModel?.() : undefined;
    if (kind === "web_search" && !binding) throw new Error("web_search_unavailable: select an approved Codex or Claude subscription model");
    const provider = binding?.provider ?? options.provider;
    const configured = binding?.model ?? settings.model ?? (kind === "explore" ? options.model : undefined);
    if (!configured) throw new Error(`${kind} is not configured: set agent.capabilities.<role>.${kind}.model in the bound provider`);
    // A second provider would need another explicit credential/transport binding. Never borrow
    // ambient auth or create a fresh unrestricted ModelRuntime for an auxiliary request.
    const serviceTier = kind === "explore" && options.settings.explore?.service_tier === "priority" ? "priority" : undefined;
    if (serviceTier && provider !== "openai-codex") throw new Error("Explore priority service tier requires the openai-codex provider");
    const prefix = `${provider}/`;
    const model = configured.startsWith(prefix) ? configured.slice(prefix.length) : configured;
    this.history.begin(request, kind, `${provider}/${model}`, prompt, serviceTier);
    const controller = new AbortController();
    this.controllers.add(controller);
    const deadlineAt = Date.now() + settings.timeout_secs * 1000;
    const timeout = AbortSignal.timeout(settings.timeout_secs * 1000);
    const signal = AbortSignal.any([controller.signal, timeout, ...(outer ? [outer] : [])]);
    const observation: AuxiliaryObservation = { request, parent: options.sessionId(), kind, model: `${provider}/${model}`, state: "preparing" };
    this.observations.set(request, observation);
    const common = { kind: "auxiliary", request, usage_kind: kind };
    let release: (() => void) | undefined;
    let prepared = false;
    let started = false;
    let outcome: Outcome = "not_started";
    let result: AuxiliaryResult | undefined;
    const retain = () => {
      if (!observation.sessionFile || !options.retain) return false;
      try {
        const helper = this.history.snapshot().find(entry => entry.request === request);
        return !!helper && options.retain(helper, observation.sessionFile, result?.text);
      } catch { return false; /* Readback does not change auxiliary execution or accounting. */ }
    };
    try {
      signal.throwIfAborted();
      const reference = await options.record({ ...common, action: "prepare", model: `${provider}/${model}` });
      prepared = true;
      if (typeof reference.session_file !== "string" || reference.session_id !== request) throw new Error("invalid auxiliary reference");
      // Search uses a different transcript format; only SDK native children are eligible.
      if (kind !== "web_search") observation.sessionFile = reference.session_file;
      observation.state = "queued";
      this.history.phase(request, "queued");
      release = await this.slots(kind).take(signal);
      signal.throwIfAborted();
      await options.record({ ...common, action: "started" });
      started = true;
      observation.state = "running";
      outcome = "uncertain";
      this.history.phase(request, "running");
      retain();
      signal.throwIfAborted();
      result = await options.execute({ request, kind, provider, model, prompt,
        sessionFile: reference.session_file, sessionId: request,
        maxTurns: kind === "explore" ? researchBudget({}, options.settings.explore?.max_turns ?? 1).maxTurns
          : kind === "expert" ? options.settings.expert?.max_turns ?? 24 : 1,
        ...(kind === "explore" ? { research: {}, deadlineAt } : kind === "expert" ? { deadlineAt } : {}),
        ...(serviceTier ? { serviceTier } : {}),
        observe: event => { if (!signal.aborted) this.history.observe(request, event); } }, signal);
      outcome = result.outcome;
    } catch (error) {
      this.history.error(request, error);
      if (kind === "explore") {
        const reason = timeout.aborted ? "timeout" : signal.aborted ? "cancelled" : "failed";
        throw new Error(`Research ${reason} during ${observation.state}; request ${request}. `
          + `${error instanceof Error ? error.message : String(error)}`, { cause: error });
      }
      throw error;
    } finally {
      // Release only after the terminal record; failure to record must not re-admit work while
      // its canonical record still says running. Admission will refuse rather than double-spend.
      let recorded = false;
      try {
        if (prepared) await options.record({ ...common, action: "finish", outcome: started ? outcome : "not_started" });
        recorded = true;
      } catch (error) {
        this.history.error(request, error); throw error;
      } finally {
        const settled: Outcome = started ? recorded ? outcome : "uncertain" : "not_started";
        observation.state = settled;
        try {
          this.history.finish(request, settled, result?.text);
          if (retain()) saved();
        }
        finally { try { release?.(); } finally { this.controllers.delete(controller); } }
      }
    }
    if (!result) throw new Error("auxiliary request did not finish");
    return { ...result, text: kind === "web_search" ? result.text : result.text.slice(0, 32768),
      truncated: kind !== "web_search" && result.text.length > 32768,
      request,
      ...(kind === "expert" ? { model: `${provider}/${model}` } : {}),
      ...(observation.sessionFile ? { sessionFile: observation.sessionFile } : {}),
    };
  }
  /** Close is permanent. Ordinary turn cancellation uses each tool's operation signal. */
  async close(): Promise<void> {
    this.stopped = true;
    try { await this.cancelAll(); }
    finally { this.history.dispose(); }
  }
  async cancelAll(): Promise<void> {
    for (const controller of this.controllers) controller.abort(new Error("parent cancelled or reloaded"));
    await Promise.allSettled([...this.requests.values()].map(({ done }) => done));
  }
  state() {
    return { explore: this.explore.state(), oracle: this.oracle.state(),
      web_search: this.webSearch.state(),
      ...(this.options.settings.expert ? { expert: this.expert.state() } : {}),
      stopped: this.stopped };
  }
}
