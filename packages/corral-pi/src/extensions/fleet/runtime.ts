import { join, relative, resolve, sep } from "node:path";
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ModelRuntime, ToolDefinition } from "@earendil-works/pi-coding-agent";
import type { BuildContext, BuildProfile, ChildQuestion, ChildSnapshot, ChildSpec, FleetControl, FleetSnapshot } from "../../build/types.ts";
import { AuxiliaryHistory } from "../subagent/inspection.ts";
import { createSdkRunner, type ChildHandle, type ChildRunner } from "./child.ts";
import { resolveModel, resolveProfiles, type ResolvedModel } from "./profiles.ts";

export interface FleetOptions {
  readonly sdk: typeof Sdk;
  readonly models: ModelRuntime;
  readonly approved: readonly string[];
  readonly parent: { provider: string; model: string; effort?: string };
  readonly context: BuildContext;
  readonly agentDir: string;
  readonly localTools: readonly ToolDefinition[];
  readonly shellRefusal?: (command: string) => string | undefined;
  readonly changed: () => void;
  readonly now?: () => number;
  readonly runner?: ChildRunner;
}
type Mutable<T> = { -readonly [K in keyof T]: T[K] };
interface Entry {
  readonly spec: ChildSpec;
  readonly controller: AbortController;
  readonly history: AuxiliaryHistory;
  readonly settled: PromiseWithResolvers<void>;
  readonly value: Mutable<ChildSnapshot>;
  model: ResolvedModel;
  handle?: ChildHandle;
  question?: PromiseWithResolvers<string>;
  pending?: ResolvedModel;
  modelVersion: number;
  pulseBucket: number;
  historyBucket: number;
}
const STATE_CHAR = { queued: "q", running: "r", needs: "n", done: "d", failed: "f", cancelled: "c" } as const;
function live(entry: Entry): boolean { return entry.value.state === "queued" || entry.value.state === "running" || entry.value.state === "needs"; }

/** One FIFO over ready slices; dependency waiters never consume a running slot. */
export class FleetRuntime implements FleetControl {
  private readonly entries = new Map<string, Entry>();
  private readonly listeners = new Set<() => void>();
  private readonly jobs = new Set<Promise<void>>();
  private readonly choices: BuildProfile[];
  private readonly runner: ChildRunner;
  private readonly now: () => number;
  private active = 0;
  private pumping = false;
  private closed = false;
  private startedAt?: number;

  constructor(private readonly options: FleetOptions) {
    if (!Number.isInteger(options.context.settings.maxParallel) || options.context.settings.maxParallel < 1 || options.context.settings.maxParallel > 8) throw new Error("Child parallel cap must be between 1 and 8");
    this.choices = resolveProfiles(options.context.settings, options.parent);
    this.now = options.now ?? Date.now;
    this.runner = options.runner ?? createSdkRunner(options);
  }

  private path(path: string): string {
    return relative(this.options.context.session.worktree, resolve(this.options.context.session.worktree, path)).split(sep).join("/") || ".";
  }
  private notify(): void {
    // Observers do not own execution or cleanup.
    for (const listener of [this.options.changed, ...this.listeners]) { try { listener(); } catch { /* Keep other observers and execution alive. */ } }
  }
  private touch(entry: Entry, tool = false): void {
    const at = this.now();
    const value = entry.value;
    const bucket = Math.max(entry.historyBucket, Math.floor((at - this.startedAt!) / 5000));
    const history = value.history + (value.history.at(-1) ?? STATE_CHAR[value.state]).repeat(Math.min(240, Math.max(0, bucket - entry.historyBucket)));
    value.history = (history.slice(0, -1) + STATE_CHAR[value.state]).slice(-240);
    entry.historyBucket = bucket;
    if (value.startedAt !== undefined) {
      const pulseBucket = Math.max(entry.pulseBucket, Math.floor((at - value.startedAt) / 5000));
      const pulse = [...value.pulse, ...Array<number>(Math.min(120, Math.max(0, pulseBucket - entry.pulseBucket))).fill(0)].slice(-120);
      if (!pulse.length) pulse.push(0);
      if (tool) pulse[pulse.length - 1] = Math.min(7, pulse[pulse.length - 1]! + 1);
      value.pulse = pulse;
      entry.pulseBucket = pulseBucket;
    }
    this.notify();
  }

  async start(call: string, specs: readonly ChildSpec[], signal?: AbortSignal): Promise<readonly ChildSnapshot[]> {
    if (this.closed) throw new Error("Fleet is closed");
    if (specs.length > 12) throw new Error("A task call supports at most 12 children");
    const batch = new Map<string, ChildSpec>();
    const models = new Map<string, ResolvedModel>();
    for (const spec of specs) {
      if (!/^[a-z][a-z0-9-]{0,31}$/.test(spec.id)) throw new Error(`Invalid child id: ${spec.id}`);
      if (this.entries.has(spec.id) || batch.has(spec.id)) throw new Error(`Duplicate child id: ${spec.id}`);
      if (typeof spec.brief !== "string" || spec.brief.length < 1 || spec.brief.length > 32768) throw new Error(`Child ${spec.id} brief must contain 1 to 32,768 characters`);
      batch.set(spec.id, spec);
      models.set(spec.id, resolveModel(spec.profile, this.choices, this.options.approved));
    }
    for (const spec of specs) for (const id of spec.after ?? []) {
      if (!batch.has(id) && !this.entries.has(id)) throw new Error(`Child ${spec.id} has unknown dependency: ${id}`);
    }
    const visiting = new Set<string>();
    const visited = new Set<string>();
    const visit = (id: string) => {
      if (visiting.has(id)) throw new Error(`Child dependency cycle includes ${id}`);
      if (visited.has(id)) return;
      visiting.add(id);
      for (const dependency of batch.get(id)?.after ?? []) if (batch.has(dependency)) visit(dependency);
      visiting.delete(id); visited.add(id);
    };
    for (const id of batch.keys()) visit(id);
    // All validation precedes recording children, invoking observers or starting work.
    const at = this.now();
    if (specs.length && this.startedAt === undefined) this.startedAt = at;
    const added: Entry[] = [];
    for (const input of specs) {
      const spec = { ...input, owns: input.owns.map(path => this.path(path)), after: [...input.after ?? []] };
      const model = models.get(spec.id)!;
      const history = new AuxiliaryHistory();
      history.begin(spec.id, "explore", model.model, 0, spec.brief);
      const bucket = Math.max(0, Math.floor((at - this.startedAt!) / 5000));
      const entry: Entry = { spec, model, controller: new AbortController(), history, settled: Promise.withResolvers<void>(), modelVersion: 0, pulseBucket: 0, historyBucket: bucket,
        value: { id: spec.id, call, title: spec.title, brief: spec.brief, owns: spec.owns, after: spec.after, profile: model.profile,
          model: `${model.provider}/${model.model}`, effort: model.effort, state: "queued", createdAt: at, edits: [], steers: [], usage: { input: 0, output: 0, turns: 0 }, pulse: [], history: " ".repeat(Math.min(239, bucket)) + "q", messages: [] } };
      this.entries.set(spec.id, entry); added.push(entry);
    }
    // Register the complete batch before a notification can reenter the runtime.
    for (const entry of added) this.touch(entry);
    const abort = () => {
      const wasPumping = this.pumping;
      this.pumping = true;
      try { for (const entry of added) this.cancel(entry.spec.id); }
      finally { this.pumping = wasPumping; }
      this.pump();
    };
    signal?.addEventListener("abort", abort, { once: true });
    try {
      if (signal?.aborted) abort();
      this.pump();
      await Promise.all(added.map(entry => entry.settled.promise));
      return Object.freeze(added.map(entry => this.copy(entry)));
    } finally { signal?.removeEventListener("abort", abort); }
  }

  private finish(entry: Entry, state: "done" | "failed" | "cancelled", error?: string, report?: string): void {
    if (!live(entry)) return;
    entry.value.state = state; entry.value.endedAt = this.now();
    entry.value.activity = undefined; entry.value.question = undefined;
    entry.value.pendingModel = undefined; entry.pending = undefined;
    if (error !== undefined) entry.value.error = error;
    if (report !== undefined) entry.value.report = report.slice(0, 32768);
    if (entry.handle) entry.value.usage = entry.handle.usage();
    entry.question?.reject(new Error(error ?? "Child finished")); entry.question = undefined;
    entry.history.finish(entry.spec.id, state === "done" ? "completed" : "uncertain", report);
    this.touch(entry); entry.settled.resolve();
  }
  private pump(): void {
    if (this.pumping || this.closed) return;
    this.pumping = true;
    try {
      // Repeat because a failed dependency can cancel an earlier queue member.
      let again = true;
      while (again) {
        again = false;
        for (const entry of this.entries.values()) {
          if (entry.value.state !== "queued") continue;
          const dependencies = entry.value.after.map(id => this.entries.get(id)!);
          const failed = dependencies.find(dependency => dependency.value.state === "failed" || dependency.value.state === "cancelled");
          if (failed) { this.finish(entry, "cancelled", `Dependency ${failed.spec.id} ${failed.value.state}`); again = true; continue; }
          if (dependencies.some(dependency => dependency.value.state !== "done") || this.active >= this.options.context.settings.maxParallel) continue;
          this.launch(entry);
        }
      }
    } finally { this.pumping = false; }
  }
  private launch(entry: Entry): void {
    this.active++;
    entry.value.state = "running"; entry.value.startedAt = this.now();
    entry.value.sessionFile = join(this.options.context.dir, "children", `${entry.spec.id}.jsonl`);
    entry.history.phase(entry.spec.id, "running"); this.touch(entry);
    const job = (async () => {
      try {
        entry.handle = this.runner.run(entry.spec, entry.model, {
          onEvent: event => {
            if (!live(entry)) return;
            entry.history.observe(entry.spec.id, event);
            if (event.type === "tool" && (event.phase === "completed" || event.phase === "error")) entry.value.activity = undefined;
            this.refreshUsage(entry); this.touch(entry);
          },
          onToolStart: (name, args) => {
            if (!live(entry)) return;
            const input = args && typeof args === "object" ? args as Record<string, unknown> : {};
            const short = input.path ?? input.command ?? input.pattern ?? input.question ?? "";
            entry.value.activity = `${name}${short ? ` ${String(short).replace(/\s+/g, " ").slice(0, 120)}` : ""}`;
            if ((name === "edit" || name === "write") && typeof input.path === "string") {
              const path = this.path(input.path);
              const outside = entry.spec.owns.length > 0 && !entry.spec.owns.some(owned => owned === "." || path === owned || path.startsWith(`${owned}/`));
              entry.value.edits = [...entry.value.edits.filter(edit => edit.path !== path), { path, outside, at: this.now() }];
            }
            this.refreshUsage(entry); this.touch(entry, true);
          },
          onTurn: () => {
            if (!live(entry)) return;
            if (entry.pending) { this.adopt(entry, entry.pending); entry.pending = undefined; entry.value.pendingModel = undefined; }
            this.refreshUsage(entry); this.touch(entry);
          },
          ask: question => this.ask(entry, question),
        }, entry.controller.signal);
        // A synchronous observer may have cancelled this child during setup.
        if (!live(entry)) entry.controller.abort();
        const result = await entry.handle.done;
        if (live(entry)) this.finish(entry, result.failed ? "failed" : "done", result.failed, result.report);
        else { entry.value.usage = entry.handle.usage(); entry.value.report = result.report.slice(0, 32768); this.touch(entry); }
      } catch (error) { this.finish(entry, "failed", error instanceof Error ? error.message : String(error)); }
      finally { this.active--; this.pump(); }
    })();
    this.jobs.add(job);
    void job.finally(() => { this.jobs.delete(job); });
  }
  private refreshUsage(entry: Entry): void { if (entry.handle) entry.value.usage = entry.handle.usage(); }
  private ask(entry: Entry, question: ChildQuestion): Promise<string> {
    if (!live(entry) || entry.controller.signal.aborted) return Promise.reject(new Error("Child is cancelled"));
    if (entry.question) return Promise.reject(new Error("Child already has an open question"));
    const pending = Promise.withResolvers<string>();
    entry.question = pending;
    entry.value.question = { ...question, options: question.options.map(option => ({ ...option })) };
    entry.value.state = "needs"; this.touch(entry);
    return pending.promise;
  }
  answer(id: string, text: string): boolean {
    const entry = this.entries.get(id);
    if (!entry?.question || entry.value.state !== "needs") return false;
    const question = entry.question; entry.question = undefined;
    entry.value.question = undefined; entry.value.state = "running"; this.touch(entry);
    question.resolve(text); return true;
  }
  async steer(id: string, text: string): Promise<boolean> {
    const entry = this.entries.get(id);
    if (!entry?.handle || !live(entry)) return false;
    try { await entry.handle.steer(text); }
    catch (error) { if (!live(entry)) return false; throw error; }
    entry.value.steers = [...entry.value.steers, text]; this.touch(entry); return true;
  }
  cancel(id: string): boolean {
    const entry = this.entries.get(id);
    if (!entry || !live(entry)) return false;
    // The state says it was cancelled; `error` is kept for a reason the owner did not cause.
    this.finish(entry, "cancelled"); entry.controller.abort(); this.pump(); return true;
  }
  private adopt(entry: Entry, model: ResolvedModel): void {
    entry.model = model; entry.value.profile = model.profile;
    entry.value.model = `${model.provider}/${model.model}`; entry.value.effort = model.effort;
  }
  setModel(id: string, model: string): void {
    const entry = this.entries.get(id);
    if (!entry || !live(entry)) return;
    const choice = resolveModel(model, this.choices, this.options.approved);
    if (!entry.handle) { this.adopt(entry, choice); this.touch(entry); return; }
    const version = ++entry.modelVersion;
    entry.pending = choice; entry.value.pendingModel = `${choice.provider}/${choice.model}`; this.touch(entry);
    void entry.handle.setModel(choice.provider, choice.model, choice.effort).then(result => {
      if (!live(entry) || entry.modelVersion !== version) return;
      if (result === "applied") { this.adopt(entry, choice); entry.pending = undefined; entry.value.pendingModel = undefined; }
      this.touch(entry);
    }, error => {
      if (!live(entry) || entry.modelVersion !== version) return;
      entry.pending = undefined; entry.value.pendingModel = undefined; entry.value.error = error instanceof Error ? error.message : String(error); this.touch(entry);
    });
  }
  profiles(): readonly BuildProfile[] { return Object.freeze(this.choices.map(profile => Object.freeze({ ...profile }))); }
  models(): readonly string[] {
    return Object.freeze([...new Set([...this.options.models.getModels().filter(model => this.options.approved.includes(model.provider)).map(model => `${model.provider}/${model.id}`),
      ...this.choices.filter(profile => this.options.approved.includes(profile.model.split("/")[0]!)).map(profile => profile.model)])]);
  }
  subscribe(listener: () => void): () => void { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
  private copy(entry: Entry): ChildSnapshot {
    const value = entry.value;
    return Object.freeze({ ...value, owns: Object.freeze([...value.owns]), after: Object.freeze([...value.after]),
      edits: Object.freeze(value.edits.map(edit => Object.freeze({ ...edit }))), usage: Object.freeze({ ...value.usage }),
      steers: Object.freeze([...value.steers]), pulse: Object.freeze([...value.pulse]),
      question: value.question ? Object.freeze({ ...value.question, options: Object.freeze(value.question.options.map(option => Object.freeze({ ...option }))) }) : undefined,
      messages: Object.freeze((entry.history.snapshot()[0]?.messages ?? []).map(item => Object.freeze({ ...item, ...(item.textBlocks ? { textBlocks: Object.freeze([...item.textBlocks]) } : {}) }))) });
  }
  snapshot(): FleetSnapshot { return Object.freeze({ children: Object.freeze([...this.entries.values()].map(entry => this.copy(entry))), maxParallel: this.options.context.settings.maxParallel, startedAt: this.startedAt }); }
  async close(): Promise<void> {
    this.closed = true;
    for (const entry of this.entries.values()) this.cancel(entry.spec.id);
    await Promise.all([...this.jobs]);
    this.listeners.clear();
  }
}
