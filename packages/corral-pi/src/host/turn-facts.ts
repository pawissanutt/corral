// Presentation-only facts. No session messages, board reads or execution decisions live here.
import type { MarkSink, TurnEndMark, TurnStopCause, TurnWait } from "./transcript-marks.ts";

export interface TurnOutcome {
  readonly stopReason?: string;
  readonly ownerStopped?: boolean;
  readonly error?: boolean;
  readonly stopCause?: TurnStopCause;
  readonly continuationStopped?: string;
  readonly waits?: readonly TurnWait[];
}

export function turnReason(input: TurnOutcome): TurnEndMark["reason"] {
  if (input.stopCause === "host") return "error";
  if (input.stopCause === "owner" || input.ownerStopped) return "stopped";
  if (input.error || input.stopReason === "error") return "error";
  if (input.stopReason === "aborted") return "stopped";
  if (input.stopReason === "length") return "length";
  if (input.continuationStopped) return "continue_stopped";
  return input.waits?.length ? "waiting" : "done";
}

/** Local observations only. Settled, failed and unknown resources are not invented waits. */
export function turnWaits(input: {
  readonly jobs?: readonly { readonly job: string; readonly state: string; readonly command: string;
    readonly watches?: readonly { readonly wait: string; readonly armed: boolean; readonly matched: string | undefined }[] }[];
  readonly questions?: readonly { readonly id: string; readonly status: string }[];
  readonly ownerKeys?: readonly string[];
  readonly children?: readonly { readonly id: string; readonly state: string; readonly label: string }[];
  readonly cells?: readonly { readonly cell: string; readonly detached: boolean }[];
  readonly events?: readonly TurnWait[];
}): readonly TurnWait[] {
  const waits: TurnWait[] = [];
  const label = (text: string) => text.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ").replace(/\s+/g, " ").trim().slice(0, 120);
  for (const job of input.jobs ?? []) {
    if (job.state === "running") waits.push({ kind: "job", id: job.job, label: label(`job ${job.job}: ${job.command}`) });
    for (const watch of job.watches ?? []) {
      if (watch.armed && !watch.matched) waits.push({ kind: "event", id: watch.wait, label: label(`monitor ${watch.wait} on ${job.job}`) });
    }
  }
  for (const question of input.questions ?? []) {
    if (question.status === "pending") waits.push({ kind: "owner", id: question.id, label: label(`your answer to ${question.id}`) });
  }
  for (const key of input.ownerKeys ?? []) waits.push({ kind: "owner", id: key, label: label(`answer to ${key}`) });
  for (const child of input.children ?? []) {
    if (["preparing", "queued", "running", "needs"].includes(child.state)) {
      waits.push({ kind: "child", id: child.id, label: label(child.label) });
    }
  }
  for (const cell of input.cells ?? []) {
    if (cell.detached) waits.push({ kind: "cell", id: cell.cell, label: label(`Code Mode cell ${cell.cell}`) });
  }
  for (const event of input.events ?? []) waits.push({ ...event, label: label(event.label) });
  return waits.filter((wait, index) => waits.findIndex(other => other.kind === wait.kind && other.id === wait.id) === index);
}

interface RunFacts {
  readonly startedAt: number;
  readonly startCost: number;
  readonly calls: Set<string>;
  readonly started: Set<string>;
  stopReason?: string;
  rebilledTokens: number;
  end?: { readonly at: number; readonly cost: number; readonly waits: readonly TurnWait[];
    readonly outcome: Pick<TurnOutcome, "ownerStopped" | "error" | "stopCause"> };
}

/** Holds a completed run until its existing lifecycle receipt supplies the continuation decision. */
export class TurnFacts {
  private readonly runs = new Map<number, RunFacts>();
  private lastRun = 0;
  constructor(private readonly sink?: MarkSink) {}

  start(run: number, at: number, cost: number): void {
    if (!this.sink || run <= this.lastRun) return;
    this.lastRun = run;
    this.runs.set(run, { startedAt: at, startCost: cost, calls: new Set(), started: new Set(), rebilledTokens: 0 });
  }

  assistant(run: number, stopReason: string, calls: readonly string[], rebilledTokens = 0): void {
    const facts = this.runs.get(run);
    if (!facts || facts.end) return;
    facts.stopReason = stopReason;
    for (const call of calls) facts.calls.add(call);
    facts.rebilledTokens += rebilledTokens;
  }

  toolStarted(run: number, id: string): void {
    const facts = this.runs.get(run);
    if (!facts || facts.end) return;
    facts.calls.add(id);
    facts.started.add(id);
  }

  end(run: number, at: number, cost: number, waits: readonly TurnWait[],
    outcome: Pick<TurnOutcome, "ownerStopped" | "error" | "stopCause"> = {}): void {
    const facts = this.runs.get(run);
    if (!facts) return;
    if (!facts.end) facts.end = { at, cost, waits, outcome };
    else facts.end = { ...facts.end, outcome: { ...facts.end.outcome, ...outcome } };
  }

  finish(run: number, outcome: Pick<TurnOutcome, "ownerStopped" | "continuationStopped" | "error" | "stopCause"> = {}): boolean {
    const facts = this.runs.get(run);
    if (!facts?.end) return false;
    // Delete before calling the collaborator: reentrancy or a failed sink cannot duplicate a run.
    this.runs.delete(run);
    const { end } = facts;
    const final = { ...end.outcome, ...outcome };
    const reason = turnReason({ stopReason: facts.stopReason, waits: end.waits, ...final });
    const notRun = reason === "stopped" || final.stopCause
      ? [...facts.calls].filter(id => !facts.started.has(id)).length : 0;
    const note = final.stopCause === "host" ? "the host stopped the run" : final.continuationStopped;
    return this.sink?.add({ kind: "turn_end", at: end.at, durationMs: Math.max(0, end.at - facts.startedAt),
      reason, toolCalls: facts.calls.size, waits: end.waits, cost: Math.max(0, end.cost - facts.startCost),
      ...(notRun ? { notRun } : {}), ...(facts.rebilledTokens ? { rebilledTokens: facts.rebilledTokens } : {}),
      ...(note ? { note } : {}) }) ?? false;
  }
}
