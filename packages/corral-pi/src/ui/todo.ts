// One host-side copy of the canonical ledger, read by the sidebar's Plan tab and, for PM and
// assistant sessions, by the model's request summary, so both name the same plan and revision.
// The Rust ledger owns every status; this module only reads, holds and re-reads it.
import type { ExtensionAPI, ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { z } from "zod";

import type { HostIdentity } from "../roles/types.ts";
import { observedCorralResult } from "./receipt-source.ts";

const Status = z.discriminatedUnion("state", [
  z.object({ state: z.literal("pending") }),
  z.object({ state: z.literal("in_progress") }),
  z.object({ state: z.literal("done") }),
  z.object({ state: z.literal("dropped"), reason: z.string() }),
]);
const Wait = z.object({
  step: z.string(),
  on: z.discriminatedUnion("kind", [
    z.object({ kind: z.literal("event"), event: z.string() }),
    z.object({ kind: z.literal("owner"), question: z.string() }),
  ]),
});
export const ProgressSchema = z.object({
  revision: z.number().int().nonnegative(),
  spec_revision: z.string(),
  phases: z.array(z.object({
    id: z.string(), label: z.string(),
    steps: z.array(z.object({
      id: z.string(), label: z.string(), status: Status,
      criteria: z.array(z.string()).default([]),
    })),
  })),
  /** PM and assistant plans only: the plan identity the model's request summary names. */
  plan: z.string().optional(),
  waits: z.array(Wait).optional(),
});
export type TodoProgress = z.infer<typeof ProgressSchema>;

const Receipt = z.object({
  state: z.literal("committed"),
  result: z.object({ progress: ProgressSchema }),
});
const Continued = z.object({
  key: z.string(), plan: z.string(), revision: z.number(), attempt: z.number(),
  stopped: z.enum(["no_progress", "limit"]).optional(),
});
export type ContinuedState = z.infer<typeof Continued>;
const Requests = z.object({ continued: Continued.nullable().optional() }).nullable().optional();
const WorkerSnapshot = z.object({
  runs: z.array(z.object({ id: z.string(), progress: ProgressSchema.nullable().optional() })),
});
const PmSnapshot = z.object({ container: z.object({ todo: ProgressSchema.nullable(), todo_requests: Requests }) });
const AssistantSnapshot = z.object({
  container: z.object({ state: z.object({ todo: ProgressSchema.nullable().optional(), todo_requests: Requests }) }),
});

export interface LedgerSnapshot {
  readonly progress: TodoProgress | null;
  readonly continued: ContinuedState | null;
}

export function snapshotLedger(value: unknown, identity: HostIdentity): LedgerSnapshot {
  if (identity.role === "pm") {
    const { todo, todo_requests } = PmSnapshot.parse(value).container;
    return { progress: todo, continued: todo_requests?.continued ?? null };
  }
  if (identity.role === "assistant" || identity.role === "build") {
    const { todo, todo_requests } = AssistantSnapshot.parse(value).container.state;
    return { progress: todo ?? null, continued: todo_requests?.continued ?? null };
  }
  const run = WorkerSnapshot.parse(value).runs.find((run) => run.id === identity.run);
  if (!run) throw new Error(`todo_read: missing bound run ${identity.run}`);
  return { progress: run.progress ?? null, continued: null };
}

export function snapshotProgress(value: unknown, identity: HostIdentity): TodoProgress | null {
  return snapshotLedger(value, identity).progress;
}

export class TodoLedger {
  private progress: TodoProgress | null = null;
  private continuedState: ContinuedState | null = null;
  private stale = true;
  // A receipt arriving during a read is newer than that read's snapshot.
  private version = 0;
  private readonly listeners = new Set<(failure?: string) => void>();

  constructor(private readonly identity: HostIdentity, private readonly load: () => Promise<unknown>) {}

  get current(): TodoProgress | null { return this.progress; }
  get continued(): ContinuedState | null { return this.continuedState; }
  get unavailable(): boolean { return this.stale; }

  onChange(listener: (failure?: string) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** Canonical progress replaces the held copy unless it is an older revision of it. */
  apply(progress: TodoProgress | null, continued?: ContinuedState | null): boolean {
    if (progress && this.progress && progress.revision < this.progress.revision) return false;
    this.version++;
    this.progress = progress;
    if (continued !== undefined) this.continuedState = continued;
    this.stale = false;
    for (const listener of this.listeners) listener();
    return true;
  }

  noteContinued(continued: ContinuedState): void {
    this.continuedState = continued;
  }

  /** Re-reads the canonical ledger; false when a newer receipt superseded the read. */
  async restore(): Promise<boolean> {
    const started = ++this.version;
    try {
      const snapshot = snapshotLedger(await this.load(), this.identity);
      if (this.version !== started) return false;
      if (!this.apply(snapshot.progress, snapshot.continued)) {
        throw new Error("todo_read: older ledger revision; retained newer progress");
      }
      return true;
    } catch (error) {
      if (this.version !== started) return false;
      this.stale = true;
      const reason = error instanceof Error ? error.message : String(error);
      for (const listener of this.listeners) listener(reason);
      throw error;
    }
  }

  /** A committed corral_todo or corral_progress tool result; one without progress re-reads. */
  async receipt(details: unknown): Promise<void> {
    const receipt = Receipt.safeParse(details);
    if (receipt.success) this.apply(receipt.data.result.progress);
    else if (z.object({ state: z.literal("committed") }).safeParse(details).success) await this.restore().catch(() => false);
    else if (z.object({ state: z.enum(["uncertain", "reserved", "queued"]) }).safeParse(details).success
      || z.object({ error: z.object({ effect: z.literal("unknown") }) }).safeParse(details).success) {
      // A read started before this outcome cannot establish post-mutation freshness.
      this.version++;
      this.stale = true;
      for (const listener of this.listeners) listener("Todo outcome unconfirmed; refresh canonical ledger");
    }
  }

  extend(pi: ExtensionAPI): void {
    pi.on("tool_result", async (event) => {
      if (!["corral_todo", "corral_progress"].includes(event.toolName) || !observedCorralResult(event)) return;
      await this.receipt(event.details);
    });
  }
}

/** The one plan presentation: the sidebar's Plan tab, in the rail or, below the rail's width, the drawer. */
export interface PlanSurface {
  /** Shows the Plan tab with the keys: the rail when it is on screen, else the open drawer. */
  show(): void;
  /** Shows the Plan tab and turns its page: negative back, positive forward. */
  scroll(delta: number): void;
  /** A panel notice in its tone; never a transcript line. */
  notice(text: string, tone: "text" | "warning" | "error"): void;
}

/** What a failed or unconfirmed read leaves on screen, and why. */
function staleNotice(ledger: TodoLedger, failure: string): string {
  const reason = failure.split("\n")[0]?.trim() ?? "";
  const lead = ledger.current ? `Plan is stale · showing revision ${ledger.current.revision}` : "Plan could not be read";
  return reason ? `${lead} · ${reason}` : lead;
}

/**
 * `/todo` shows the Plan tab, `/todo up|down` turn its page and `/todo refresh` rereads the canonical ledger.
 * `restoreOnStart` rereads the ledger when the terminal UI opens. A failed or unconfirmed read is a warning
 * notice on the surface; the last confirmed revision stays shown, marked stale.
 */
export function installPlanCommand(
  pi: ExtensionAPI, ledger: TodoLedger, restoreOnStart: boolean, surface: () => PlanSurface | undefined,
): void {
  // Before the panels exist (or without them) a failure still reaches the owner through the SDK's notify.
  let ui: ExtensionUIContext | undefined;
  ledger.onChange((failure) => {
    if (failure === undefined) return;
    const text = staleNotice(ledger, failure), shown = surface();
    if (shown) shown.notice(text, "warning");
    else ui?.notify(text, "warning");
  });
  pi.on("session_start", async (_event, ctx) => {
    if (ctx.mode !== "tui") return;
    ui = ctx.ui;
    if (restoreOnStart) await ledger.restore().catch(() => false);
  });
  pi.registerCommand("todo", {
    description: "Show the plan in the sidebar; /todo up, down, or refresh",
    handler: async (args, ctx) => {
      const shown = surface();
      if (!shown) {
        ctx.ui.notify("The plan is shown in the terminal UI's sidebar.", "info");
        return;
      }
      switch (args.trim()) {
        case "": shown.show(); break;
        case "up": shown.scroll(-1); break;
        case "down": shown.scroll(1); break;
        case "refresh": {
          // A failed read, or an unconfirmed outcome that overtook it, already said so through `onChange`.
          const read = await ledger.restore().then(() => true, () => false);
          if (!read || ledger.unavailable) break;
          const current = ledger.current;
          shown.notice(current ? `Plan refreshed · revision ${current.revision}` : "Plan refreshed · no plan yet", "text");
          break;
        }
        default: shown.notice("Use /todo, /todo up, /todo down or /todo refresh", "warning");
      }
    },
  });
}
