// The plan's facts for reading: one step's detail for the sidebar and the whole ledger for the shared pager.
// Pure. Every line restates what the session's plan recorded (ids, criteria, waits, drop reasons, spec
// revision, auto-continue); nothing here infers acceptance or a state the ledger did not record.
import { plainLabel } from "./panels-view.ts";
import type { ContinuedState, TodoProgress } from "./todo.ts";

/** The sidebar's and the pager's plan input. */
export interface PlanSource {
  readonly progress: TodoProgress | null;
  /** The held copy is not confirmed current: its read failed or a later outcome is unconfirmed. */
  readonly stale: boolean;
  /** The ledger's auto-continue record: null when none is recorded, absent when the source does not report it. */
  readonly continued?: ContinuedState | null;
}

type Step = TodoProgress["phases"][number]["steps"][number];
export type PlanStepState = Step["status"]["state"];
export interface PlanWait { readonly kind: "owner" | "event"; readonly text: string }
export interface PlanStepDetail {
  readonly id: string;
  readonly label: string;
  /** The label of the phase the step belongs to. */
  readonly phase: string;
  readonly state: PlanStepState;
  readonly criteria: readonly string[];
  readonly droppedReason?: string;
  /** Every wait the ledger records on this step, in its order. */
  readonly waits: readonly PlanWait[];
}

/** The still glyph of each state; a live surface draws the shared spinner for `in_progress` instead. */
export const STEP_GLYPH: Readonly<Record<PlanStepState, string>> = { done: "✓", in_progress: "●", pending: "○", dropped: "–" };
const STATE_WORD: Readonly<Record<PlanStepState, string>> = { done: "done", in_progress: "in progress", pending: "pending", dropped: "dropped" };
/** The wait's mark: `?` the owner answers it, `…` an event ends it. */
export const WAIT_GLYPH: Readonly<Record<PlanWait["kind"], string>> = { owner: "?", event: "…" };

const waitsOf = (progress: TodoProgress, step: string): PlanWait[] => (progress.waits ?? []).filter(wait => wait.step === step)
  .map((wait): PlanWait => wait.on.kind === "owner" ? { kind: "owner", text: plainLabel(wait.on.question) } : { kind: "event", text: plainLabel(wait.on.event) });

function detail(progress: TodoProgress, phase: string, step: Step): PlanStepDetail {
  return {
    id: plainLabel(step.id), label: plainLabel(step.label), phase: plainLabel(phase), state: step.status.state,
    criteria: step.criteria.map(plainLabel),
    ...(step.status.state === "dropped" ? { droppedReason: plainLabel(step.status.reason) } : {}),
    waits: waitsOf(progress, step.id),
  };
}

/** One step's facts, by its ledger id; undefined when the plan has no such step. */
export function planStepDetail(progress: TodoProgress, step: string): PlanStepDetail | undefined {
  for (const phase of progress.phases) {
    const found = phase.steps.find(entry => entry.id === step);
    if (found) return detail(progress, phase.label, found);
  }
  return undefined;
}

/** `waits for you: <question>` or `waits for <event>`, after the wait's mark; one wording for steps and stray waits. */
function waitWords(wait: PlanWait): string {
  return wait.kind === "owner" ? `waits for you: ${wait.text}` : `waits for ${wait.text}`;
}

/** The pager's title: `Plan <plan> · revision <n>`, the plan id only where the ledger names one. */
export function planTitle(progress: TodoProgress | null): string {
  if (!progress) return "Plan";
  const plan = progress.plan === undefined ? "" : plainLabel(progress.plan);
  return `Plan${plan ? ` ${plan}` : ""} · revision ${progress.revision}`;
}

function continuation(continued: ContinuedState): string {
  const facts = `attempt ${continued.attempt} · plan ${plainLabel(continued.plan)} revision ${continued.revision} · key ${plainLabel(continued.key)}`;
  switch (continued.stopped) {
    case "no_progress": return `! auto-continue stopped: no progress · ${facts}`;
    case "limit": return `! auto-continue stopped: attempt limit reached · ${facts}`;
    case undefined: return `→ auto-continue · ${facts}`;
  }
}

export interface PlanLedger {
  /** `Plan <plan> · revision <n>` for the pager's border. */
  readonly title: string;
  /** Plain text, one fact per line; the pager draws them as they are. */
  readonly lines: readonly string[];
  /** The line of a step's entry, so the pager opens on it; the top for the plan itself or an unknown step. */
  startOf(step: string | undefined): number;
}

/**
 * The whole ledger for the shared pager: plan id, revision and spec revision, the stale mark, every phase with
 * every step's state, label, id, criteria, waits and drop reason, waits on steps the phases no longer list, and the
 * auto-continue record.
 */
export function planLedger(source: PlanSource): PlanLedger {
  const { progress, stale } = source;
  const lines: string[] = [], starts = new Map<string, number>();
  if (progress) ledgerLines(progress, stale, source.continued, lines, starts);
  else lines.push(stale ? "! No confirmed plan. /todo refresh rereads it." : "No plan yet.");
  return { title: planTitle(progress), lines, startOf: step => step === undefined ? 0 : starts.get(step) ?? 0 };
}

/** Appends the ledger's lines and records the line each step's entry starts on. */
function ledgerLines(progress: TodoProgress, stale: boolean, continued: ContinuedState | null | undefined,
  lines: string[], starts: Map<string, number>): void {
  const steps = progress.phases.flatMap(phase => phase.steps);
  const count = (state: PlanStepState) => steps.filter(step => step.status.state === state).length;
  lines.push(`${planTitle(progress)} · spec revision ${plainLabel(progress.spec_revision) || "(empty)"}`);
  lines.push([`${steps.length} ${steps.length === 1 ? "step" : "steps"}`,
    ...(["done", "in_progress", "pending", "dropped"] as const).flatMap(state => count(state) ? [`${count(state)} ${STATE_WORD[state]}`] : [])].join(" · "));
  if (stale) lines.push("! stale: this is the last confirmed revision; /todo refresh rereads the plan");
  for (const phase of progress.phases) {
    const done = phase.steps.filter(step => step.status.state === "done").length;
    lines.push("", `${plainLabel(phase.label)} · id ${plainLabel(phase.id)} · ${done}/${phase.steps.length} done`);
    if (!phase.steps.length) lines.push("  no steps");
    for (const step of phase.steps) {
      const facts = detail(progress, phase.label, step);
      if (!starts.has(step.id)) starts.set(step.id, lines.length);
      lines.push(`  ${STEP_GLYPH[facts.state]} ${facts.label}`);
      lines.push(`      id ${facts.id} · ${STATE_WORD[facts.state]}`
        + (facts.criteria.length ? ` · criteria ${facts.criteria.join(", ")}` : " · no criteria recorded"));
      for (const wait of facts.waits) lines.push(`      ${WAIT_GLYPH[wait.kind]} ${waitWords(wait)}`);
      if (facts.droppedReason !== undefined) lines.push(`      – dropped: ${facts.droppedReason}`);
    }
  }
  const listed = new Set(steps.map(step => step.id));
  const orphans = [...new Set((progress.waits ?? []).map(wait => wait.step).filter(step => !listed.has(step)))];
  if (orphans.length) {
    lines.push("", "Waits on steps this revision does not list");
    for (const step of orphans) {
      for (const wait of waitsOf(progress, step)) lines.push(`  ${WAIT_GLYPH[wait.kind]} ${plainLabel(step)} ${waitWords(wait)}`);
    }
  }
  if (continued !== undefined) lines.push("", "Auto-continue", `  ${continued ? continuation(continued) : "○ none recorded"}`);
}
