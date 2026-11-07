// Fixed managed-interface ceiling. This is not a shell parser or an OS sandbox. Metadata,
// capabilities, filesystem permissions and old owner answers cannot widen this policy.
import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { sha256Hex } from "../tools/corral.ts";

export interface WorkflowExpected {
  readonly workflow: string; readonly state_revision: number;
  readonly authority_epoch: number; readonly plan_revision: number | null;
}
export interface PlanningState {
  readonly phase: string;
  readonly expected?: WorkflowExpected;
  readonly boundary?: {
    readonly activities: readonly { readonly id: string; readonly tool: string; readonly uncertain: boolean }[];
    readonly active_count?: number; readonly uncertain?: boolean; readonly identities_digest?: string;
  };
}
const PHASES = new Set(["ordinary", "entering", "recovering", "planning", "executing", "candidate", "paused",
  "handoff_preparing", "handoff_reconciling", "board_owned", "closed"]);
const READS = new Set(["read", "grep", "find", "ls", "corral_assistant_inspect", "web_fetch", "web_read", "web_search", "tool_search", "explore",
  "corral_receipt_lookup", "corral_content_read", "corral_memory_list", "corral_memory_read", "codemode",
  "ask_user", "job_output", "job_wait", "job_kill", "corral_job_monitor"]);
const BOOKKEEPING: Readonly<Record<string, ReadonlySet<string>>> = {
  corral_assistant_plan: new Set(["read", "read_revision", "stage", "publish", "checkpoint", "pause", "candidate", "close"]),
  corral_assistant_plan_review: new Set(["read", "start", "cancel"]),
  corral_assistant_handoff: new Set(["read", "preview", "commit", "reconcile", "cancel", "return_preview", "return"]),
  corral_todo: new Set(["init", "append", "start", "done", "drop", "wait", "resume"]),
};

export function planningBookkeeping(name: string, input: Record<string, unknown>): boolean {
  return READS.has(name) || BOOKKEEPING[name]?.has(String(input.action ?? input.op)) === true;
}

/** Only canonical readback may open ordinary admission. Missing/future/corrupt state blocks. */
export function planningState(snapshot: unknown): PlanningState {
  if (!snapshot || typeof snapshot !== "object") throw new Error("planning recovery required: missing canonical state");
  const root = snapshot as Record<string, unknown>;
  const state = (root.workflow && typeof root.workflow === "object" ? root.workflow : root) as Record<string, unknown>;
  const container = root.container as { version?: unknown; workflow?: unknown } | undefined;
  if (container && ![1, 2].includes(Number(container.version))) throw new Error("planning recovery required: container version");
  if (state.workflow === null && state.phase === "ordinary") return { phase: "ordinary", boundary: state.boundary as PlanningState["boundary"] };
  if (container?.version === 1 && container.workflow === undefined && root.workflow === undefined) return { phase: "ordinary" };
  const phase = state.phase;
  if (typeof phase !== "string" || !PHASES.has(phase) || state.version !== 1
      || typeof state.workflow !== "string" || !/^W[1-9][0-9]*$/.test(state.workflow)
      || !Number.isSafeInteger(state.state_revision) || Number(state.state_revision) < 1
      || !Number.isSafeInteger(state.authority_epoch) || Number(state.authority_epoch) < 1) {
    throw new Error("planning recovery required: invalid workflow");
  }
  const owner = state.owner as { kind?: string } | undefined;
  // No artifact reference is itself execution authority. The later scoped execution consumer
  // must verify exact T-743 evidence/assumptions; until then executing resumes fenced.
  const effective = owner?.kind === "board" ? "board_owned" : owner?.kind === "local" ? phase : "recovering";
  const plan = state.current_plan as { revision?: number } | null | undefined;
  return { phase: effective, expected: { workflow: state.workflow, state_revision: Number(state.state_revision),
    authority_epoch: Number(state.authority_epoch), plan_revision: plan?.revision ?? null },
    ...(state.boundary ? { boundary: state.boundary as PlanningState["boundary"] } : {}) };
}

export class PlanningPolicy {
  private state: PlanningState = { phase: "recovering" };
  private fenced = false;
  private sequence = 0;
  private readonly active = new Set<string>();
  private queue: Promise<unknown> = Promise.resolve();
  constructor(private readonly port: {
    read(): Promise<unknown>;
    record(transition: Record<string, unknown>): Promise<unknown>;
    fence(): void;
  }) {}

  snapshot(): PlanningState { return this.state; }
  get restricted(): boolean { return this.fenced || !["ordinary", "closed"].includes(this.state.phase); }
  visible(name: string): boolean { return !this.restricted || READS.has(name) || Object.hasOwn(BOOKKEEPING, name); }
  refusal(name: string, input: Record<string, unknown> = {}): string | undefined {
    return this.restricted && !planningBookkeeping(name, input)
      ? `assistant_planning: ${name} is unavailable during ${this.fenced ? "entering/recovering" : this.state.phase}; no effect started`
      : undefined;
  }
  private accept(snapshot: unknown): void {
    const readback = planningState(snapshot);
    // A new host cannot infer settlement from an old process/generation disappearing.
    // Only starts this controller still owns may coexist with ordinary admission.
    const ledger = readback.boundary;
    const unknown = ledger?.uncertain || ledger?.activities.some(a => a.uncertain || !this.active.has(a.id))
      || (ledger?.identities_digest !== undefined && ledger.identities_digest !== sha256Hex([...this.active].sort().join("\0")));
    const next = unknown ? { ...readback, phase: "recovering" } : readback;
    const old = this.state.expected;
    if (old && (!next.expected || next.expected.authority_epoch < old.authority_epoch)) {
      throw new Error("planning recovery required: authority readback regressed");
    }
    if (next.expected?.authority_epoch !== old?.authority_epoch || !["ordinary", "closed"].includes(next.phase)) this.port.fence();
    this.state = next;
  }
  private ordered<T>(action: () => Promise<T>): Promise<T> {
    const result = this.queue.then(action);
    this.queue = result.catch(() => {});
    return result;
  }
  private async load(): Promise<void> {
    try { this.accept(await this.port.read()); }
    catch (error) { this.state = { ...this.state, phase: "recovering" }; this.port.fence(); throw error; }
  }
  refresh(): Promise<void> { return this.ordered(() => this.load()); }
  private record(action: Record<string, unknown>): Promise<void> {
    return this.ordered(async () => { this.accept(await this.port.record(action)); });
  }

  /** Called at actual dispatch, AFTER async permission dialogs and every tool_call handler. */
  async begin(id: string, tool: string): Promise<() => Promise<void>> {
    return this.ordered(async () => {
      await this.load();
      const reason = this.refusal(tool);
      if (reason) throw new Error(reason);
      const key = `${++this.sequence}:${id}`;
      this.active.add(key);
      try {
        this.accept(await this.port.record({ action: "start", id: key, tool, authority_epoch: this.state.expected?.authority_epoch ?? 0 }));
      } catch (error) {
        // Lost acknowledgements are not absence. Leave the local/canonical identity visible.
        this.fenced = true; this.port.fence(); throw error;
      }
      return () => this.ordered(async () => {
        const settled = await this.port.record({ action: "settle", id: key, uncertain: false });
        this.active.delete(key);
        this.accept(settled);
      });
    });
  }
  async execute<T>(name: string, input: Record<string, unknown>, id: string, effect: () => Promise<T>): Promise<T> {
    if (planningBookkeeping(name, input)) {
      const reason = this.refusal(name, input);
      if (reason) throw new Error(reason);
      return effect();
    }
    const settled = await this.begin(id, name);
    try {
      // Entry can close admission while the durable start is awaiting acknowledgement.
      if (this.fenced || this.refusal(name, input)) throw new Error("assistant_planning: admission changed before dispatch");
      return await effect();
    } finally { await settled(); }
  }
  wrap(tool: ToolDefinition): ToolDefinition {
    return { ...tool, execute: (id, input, signal, update, ctx) =>
      this.execute(tool.name, input as Record<string, unknown>, id, () => tool.execute(id, input, signal, update, ctx)) };
  }

  /** Host-authenticated seam only; public /plan and choice UI are a later member. */
  async enter(input: string, settleResources: () => Promise<void>): Promise<PlanningState> {
    this.fenced = true; this.port.fence(); // synchronous barrier, before the first await
    await this.refresh();
    await this.record({ action: "enter", input, expected: this.state.expected ?? null });
    await settleResources();
    return this.drained();
  }
  async drained(): Promise<PlanningState> {
    await this.refresh();
    if (this.active.size || this.state.boundary?.activities.length) return this.state;
    if (this.state.expected && ["entering", "recovering"].includes(this.state.phase)) {
      await this.record({ action: "drained", expected: this.state.expected });
      this.fenced = false;
    }
    return this.state;
  }
}
