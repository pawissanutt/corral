// PM and assistant request execution (T-679). Canonical receipts carry normal plan changes;
// persisted checkpoints repair missing visibility without rewriting a previous request.
// Corral alone decides continuation, once per continuation key.
import type { ExtensionAPI, ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { z } from "zod";

import type { TodoProgress } from "../ui/todo-widget.ts";
import type { ContinuedState, TodoLedger } from "../ui/todo.ts";
import { requestCheckpoint, type RequestCheckpoint } from "./request-checkpoint.ts";

export const SUMMARY_MAX_BYTES = 2048;
const STEPS_SHOWN = 8;
const LABEL_MAX = 120;

export type Disposition =
  | { state: "complete" }
  | { state: "actionable"; next: string }
  | { state: "awaiting_owner"; question: string; steps: string[] }
  | { state: "waiting_event"; event: string; steps: string[] };

type Wait = NonNullable<TodoProgress["waits"]>[number];

const open = (state: string) => state === "pending" || state === "in_progress";

/** corral-core's `request::disposition`, over the same canonical progress. */
export function disposition(progress: TodoProgress | null): Disposition {
  if (!progress) return { state: "complete" };
  const steps = progress.phases.flatMap((phase) => phase.steps);
  const waits = (id: string) => progress.waits?.find((wait) => wait.step === id);
  const free = (state: string) => steps.find((step) => step.status.state === state && !waits(step.id));
  const next = free("in_progress") ?? free("pending");
  if (next) return { state: "actionable", next: next.id };
  const waiting = steps.filter((step) => open(step.status.state)).flatMap((step) => waits(step.id) ?? []);
  const text = (wait: Wait) => wait.on.kind === "owner" ? wait.on.question : wait.on.event;
  const owner = waiting.filter((wait) => wait.on.kind === "owner");
  const first = owner[0] ?? waiting[0];
  if (!first) return { state: "complete" };
  return owner[0]
    ? { state: "awaiting_owner", question: text(first), steps: owner.map((wait) => wait.step) }
    : { state: "waiting_event", event: text(first), steps: waiting.map((wait) => wait.step) };
}

export interface RequestSummary {
  readonly plan: string | null;
  readonly revision: number;
  readonly disposition: Disposition;
  readonly outstanding: readonly { id: string; label: string; state: string; wait?: string }[];
  readonly omitted: number;
  readonly next: { id: string; label: string } | null;
  readonly stopped: "no_progress" | "limit" | null;
}

const clip = (text: string) => {
  const plain = text.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ").replace(/\s+/g, " ").trim();
  return plain.length > LABEL_MAX ? `${plain.slice(0, LABEL_MAX - 1)}…` : plain;
};

export function summarize(progress: TodoProgress | null, continued: ContinuedState | null): RequestSummary | null {
  if (!progress) return null;
  const outcome = disposition(progress);
  const steps = progress.phases.flatMap((phase) => phase.steps).filter((step) => open(step.status.state));
  const waitText = (id: string) => {
    const wait = progress.waits?.find((entry) => entry.step === id)?.on;
    return wait && (wait.kind === "owner" ? `the owner: ${clip(wait.question)}` : clip(wait.event));
  };
  const next = outcome.state === "actionable" ? steps.find((step) => step.id === outcome.next) : undefined;
  const current = continued && continued.plan === progress.plan && continued.revision === progress.revision;
  return {
    plan: progress.plan ?? null, revision: progress.revision, disposition: outcome,
    outstanding: steps.slice(0, STEPS_SHOWN).map((step) => {
      const wait = waitText(step.id);
      return { id: clip(step.id), label: clip(step.label), state: step.status.state, ...(wait ? { wait } : {}) };
    }),
    omitted: Math.max(0, steps.length - STEPS_SHOWN),
    next: next ? { id: clip(next.id), label: clip(next.label) } : null,
    stopped: current ? continued.stopped ?? null : null,
  };
}

function utf8Prefix(text: string, max: number): string {
  const bytes = Buffer.from(text);
  if (bytes.length <= max) return text;
  return `${bytes.subarray(0, max - 3).toString("utf8").replace(/\uFFFD+$/, "")}…`;
}

export function summaryText(summary: RequestSummary): string {
  const plan = summary.plan ?? "plan";
  const state = summary.disposition.state.replace("_", " ");
  const lines = [`[corral request ${plan} revision ${summary.revision}: ${state}${
    summary.next ? `; next step ${summary.next.id}` : ""}]`];
  if (summary.outstanding.length > 0) {
    lines.push("Outstanding steps:");
    for (const step of summary.outstanding) {
      lines.push(`- [${step.state}] ${step.id}: ${step.label}${step.wait ? ` (waits on ${step.wait})` : ""}`);
    }
    if (summary.omitted > 0) lines.push(`- ... ${summary.omitted} more open steps`);
  }
  if (summary.stopped) lines.push(`Automatic continuation stopped for this revision: ${summary.stopped.replace("_", " ")}.`);
  lines.push("This checkpoint reflects the canonical ledger; later committed corral_todo results supersede it.");
  return utf8Prefix(lines.join("\n"), SUMMARY_MAX_BYTES);
}

const Due = z.object({ state: z.literal("due"), key: z.string(), attempt: z.number(), max: z.number() });
const TurnRequest = z.object({
  plan: z.string(), revision: z.number(), continuation: Due,
  next: z.object({ id: z.string(), label: z.string() }).nullable(),
});
export type DueContinuation = z.infer<typeof TurnRequest>;

export function continuationText(due: DueContinuation): string {
  const next = due.next ? ` The next step is ${clip(due.next.id)}: ${clip(due.next.label)}.` : "";
  return `[corral continuation ${due.plan} revision ${due.revision}, ${due.continuation.attempt} of `
    + `${due.continuation.max}]\nYour request plan ${due.plan} still has actionable steps.${next} This reminder `
    + "is not an owner instruction and grants no new authority. Re-read the state the step depends on before "
    + "acting, and never repeat a mutation whose outcome was uncertain. Continue the plan and keep corral_todo "
    + "current; if a step now waits on an event or the owner, record it with wait; if the request is finished or "
    + "no longer wanted, mark its remaining steps done or dropped.";
}

export class RequestContext {
  private readonly acted = new Set<string>();
  private readonly reported = new Set<string>();
  private ui: ExtensionUIContext | undefined;

  constructor(private readonly ledger: TodoLedger, private readonly persist: (message: RequestCheckpoint) => void) {}

  /** The current plan as the model sees it now: the ledger the widget renders. */
  summary(): string | undefined {
    const summary = summarize(this.ledger.current, this.ledger.continued);
    return summary ? summaryText(summary) : undefined;
  }

  extend(pi: ExtensionAPI): void {
    pi.on("session_start", async (_event, ctx) => {
      this.ui = ctx.ui;
      // Startup and resume read the canonical ledger before the first model request.
      await this.ledger.restore().catch(() => false);
    });
    pi.on("context_with_system", (event, ctx) => {
      const checkpoint = requestCheckpoint(event.messages, this.ledger.current, this.summary(),
        this.ledger.unavailable, summarize(this.ledger.current, this.ledger.continued)?.stopped != null);
      if (!checkpoint) return undefined;
      // This boundary follows finalized tool results. Persist first, so replay/resume sees the
      // same record. Using the full-transcript hook preserves chronological system/tool patches.
      try { this.persist(checkpoint); }
      catch (error) { ctx.abort(); throw error; }
      return { messages: [...event.messages, checkpoint] };
    });
  }

  /** The due continuation a committed turn_ended result grants, the first time its key is seen. */
  due(result: unknown): DueContinuation | undefined {
    const request = z.object({ request: TurnRequest }).safeParse(result);
    if (!request.success) return undefined;
    const due = request.data.request;
    if (this.acted.has(due.continuation.key)) return undefined;
    this.acted.add(due.continuation.key);
    this.ledger.noteContinued({ key: due.continuation.key, plan: due.plan, revision: due.revision,
      attempt: due.continuation.attempt });
    return due;
  }

  notify(text: string): void {
    this.ui?.notify(text, "warning");
  }

  stopped(result: unknown): string | undefined {
    const stop = z.object({ request: z.object({ plan: z.string(), revision: z.number(), continuation: z.object({
      state: z.literal("stopped"), reason: z.enum(["no_progress", "limit"]) }) }) }).safeParse(result);
    if (!stop.success) return undefined;
    const { plan, revision, continuation } = stop.data.request;
    const key = `continue:${plan}:${revision}`;
    this.ledger.noteContinued({ key, plan, revision, attempt: 0, stopped: continuation.reason });
    // Later turns at a stopped revision report the stop again; the owner is told once.
    if (this.reported.has(key)) return undefined;
    this.reported.add(key);
    return continuation.reason;
  }
}
