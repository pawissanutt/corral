// One host-side copy of the canonical ledger, read by the rich-mode widget and, for PM and
// assistant sessions, by the model's request summary, so both name the same plan and revision.
import type { ExtensionAPI, ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { z } from "zod";

import type { HostIdentity } from "../roles/types.ts";
import { ProgressSchema, TodoWidget, type TodoProgress } from "./todo-widget.ts";

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
  if (identity.role === "assistant") {
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
  private stale = false;
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
      this.apply(snapshot.progress, snapshot.continued);
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
  }

  extend(pi: ExtensionAPI): void {
    pi.on("tool_result", async (event) => {
      if (event.isError || event.parentToolCallId !== undefined
        || !["corral_todo", "corral_progress"].includes(event.toolName)) return;
      await this.receipt(event.details);
    });
  }
}

/** The rich-mode widget and `/todo`; `restoreOnStart` re-reads the ledger when the pane opens. */
export function installTodo(pi: ExtensionAPI, ledger: TodoLedger, restoreOnStart: boolean): void {
  let widget: TodoWidget | undefined;
  let ui: ExtensionUIContext | undefined;
  ledger.onChange((failure) => {
    if (failure === undefined) {
      widget?.update(ledger.current);
      return;
    }
    widget?.failed();
    ui?.notify(`Todo refresh failed: ${failure}`, "warning");
  });
  pi.on("session_start", async (_event, ctx) => {
    if (ctx.mode !== "tui") return;
    ui = ctx.ui;
    ui.setWidget("corral-todo", (tui, theme) => {
      widget = new TodoWidget(theme, () => tui.terminal.rows, () => tui.requestRender());
      if (ledger.current) widget.update(ledger.current);
      else if (ledger.unavailable) widget.failed();
      return widget;
    }, { placement: "aboveEditor" });
    if (restoreOnStart) await ledger.restore().catch(() => false);
  });
  pi.registerCommand("todo", {
    description: "Toggle todo details; /todo up, down, or refresh",
    handler: async (args, ctx) => {
      if (!widget) {
        ctx.ui.notify("Todo visualization is available in the terminal UI.", "info");
        return;
      }
      switch (args.trim()) {
        case "": widget.toggle(); break;
        case "up": widget.scroll(-3); break;
        case "down": widget.scroll(3); break;
        case "refresh": await ledger.restore().catch(() => false); break;
        default: ctx.ui.notify("Use /todo, /todo up, /todo down, or /todo refresh.", "info");
      }
    },
  });
}
