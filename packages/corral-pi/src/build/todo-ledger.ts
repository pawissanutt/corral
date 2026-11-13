// Local assistant-session todo semantics, without a corral store or receipt history.
import { closeSync, fsyncSync, mkdirSync, openSync, readFileSync, renameSync, unlinkSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { randomUUID } from "node:crypto";
import { z } from "zod";

import type { CorralCall } from "../tools/corral.ts";

export type LocalOpHandler = (call: CorralCall) => unknown | Promise<unknown>;

const Step = z.object({ id: z.string(), label: z.string(), criteria: z.array(z.string()).default([]) }).strict();
const Phase = z.object({ id: z.string(), label: z.string(), steps: z.array(Step) }).strict();
const Plan = z.string().nullish();
const On = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("event"), event: z.string() }).strict(),
  z.object({ kind: z.literal("owner"), question: z.string() }).strict(),
]);
const Action = z.discriminatedUnion("op", [
  z.object({ op: z.literal("init"), phases: z.array(Phase) }).strict(),
  z.object({ op: z.literal("append"), phase: z.string(), label: z.string().nullish(), steps: z.array(Step), plan: Plan }).strict(),
  z.object({ op: z.literal("start"), step: z.string(), plan: Plan }).strict(),
  z.object({ op: z.literal("done"), step: z.string(), plan: Plan }).strict(),
  z.object({ op: z.literal("drop"), step: z.string(), reason: z.string(), plan: Plan }).strict(),
  z.object({ op: z.literal("wait"), step: z.string(), on: On, plan: Plan,
    deadline: z.string().datetime({ offset: true }).nullish(), when: z.array(z.string()).nullish() }).strict(),
]);
const Status = z.discriminatedUnion("state", [
  z.object({ state: z.literal("pending") }), z.object({ state: z.literal("in_progress") }),
  z.object({ state: z.literal("done") }), z.object({ state: z.literal("dropped"), reason: z.string() }),
]);
const Progress = z.object({
  revision: z.number().int().nonnegative().safe(), spec_revision: z.string(), plan: z.string(),
  phases: z.array(z.object({ id: z.string(), label: z.string(),
    steps: z.array(Step.extend({ status: Status })) })),
  waits: z.array(z.object({ step: z.string(), on: On })).default([]),
});
const State = z.object({
  version: z.literal(1), plans: z.number().int().nonnegative().safe(), progress: Progress.nullable(),
  history: z.array(z.object({ plan: z.string(), revision: z.number(), steps: z.number(),
    done: z.number(), dropped: z.number(), closed: z.string() })).max(16),
  deadlines: z.array(z.object({ step: z.string(), deadline: z.string(), when: z.array(z.string()) })),
});
type Progress = z.infer<typeof Progress>;
type State = z.infer<typeof State>;
type Step = Progress["phases"][number]["steps"][number];

function refuse(reason: string): never {
  throw Object.assign(new Error(reason), { code: "bad_request" });
}

function parseAction(payload: Record<string, unknown>): z.infer<typeof Action> {
  const result = Action.safeParse(payload);
  if (!result.success) {
    refuse("malformed todo payload (data error) at payload; expected fields: a todo init/append/start/done/drop/wait command");
  }
  return result.data;
}

const open = (step: Step) => step.status.state === "pending" || step.status.state === "in_progress";
const steps = (progress: Progress) => progress.phases.flatMap((phase) => phase.steps);

function openStep(progress: Progress, id: string): Step {
  const target = steps(progress).find((step) => step.id === id);
  if (!target) refuse("unknown_todo_step");
  if (!open(target)) refuse("terminal_todo_step");
  return target;
}

function validate(progress: Progress): void {
  if (!progress.phases.length) refuse("empty_todo_phases");
  const phases = new Set<string>();
  const ids = new Set<string>();
  let current = 0;
  for (const phase of progress.phases) {
    if (!phase.id.trim() || phases.has(phase.id) || !phase.label.trim()) refuse("invalid_todo_phase");
    phases.add(phase.id);
    for (const step of phase.steps) {
      if (!step.id.trim() || ids.has(step.id) || !step.label.trim()) refuse("invalid_todo_step");
      ids.add(step.id);
      if (step.status.state === "in_progress") current++;
    }
  }
  if (current > 1) refuse("multiple_current_steps");
}

function disposition(progress: Progress): unknown {
  const all = steps(progress);
  const wait = (id: string) => progress.waits.find((entry) => entry.step === id);
  const free = (state: string) => all.find((step) => step.status.state === state && !wait(step.id));
  const next = free("in_progress") ?? free("pending");
  if (next) return { state: "actionable", next: next.id };
  const waiting = all.filter(open).flatMap((step) => wait(step.id) ?? []);
  const owners = waiting.filter((entry) => entry.on.kind === "owner");
  const first = owners[0] ?? waiting[0];
  if (!first) return { state: "complete" };
  return first.on.kind === "owner"
    ? { state: "awaiting_owner", question: first.on.question, steps: owners.map((entry) => entry.step) }
    : { state: "waiting_event", event: first.on.event, steps: waiting.map((entry) => entry.step) };
}

function publicProgress(progress: Progress): unknown {
  const { waits, ...rest } = structuredClone(progress);
  return waits.length ? { ...rest, waits } : rest;
}

function normalizeTask(value: string): string {
  const digits = value.trim().replace(/^[Tt]-/, "");
  if (!/^\d+$/.test(digits) || BigInt(digits) > 18446744073709551615n) {
    refuse(`invalid task id ${JSON.stringify(value)}`);
  }
  return `T-${BigInt(digits).toString().padStart(3, "0")}`;
}

// Match parse_park_check's vocabulary and refusals. Checks are retained, not evaluated:
// a build session has no board/question records or PM wake-publication loop.
function validateCheck(input: string): void {
  const colon = input.indexOf(":");
  if (colon < 0) refuse("resume-when requires kind:target");
  const kind = input.slice(0, colon);
  const value = input.slice(colon + 1);
  let target = value;
  switch (kind) {
    case "status": {
      const equals = value.indexOf("=");
      if (equals < 0) refuse("status requires task=status");
      const status = value.slice(equals + 1).trim().toLowerCase().replace(/[^a-z]/g, "");
      if (!["idea", "ideas", "backlog", "ready", "running", "blocked", "needsme", "review", "done", "dropped", "drop"].includes(status)) {
        refuse("unknown resume status");
      }
      target = normalizeTask(value.slice(0, equals));
      break;
    }
    case "landed": target = normalizeTask(value); break;
    case "answered": {
      const at = value.lastIndexOf("@");
      if (at < 0) refuse("answered requires key@revision");
      target = value.slice(0, at);
      if (!target) refuse("answered requires a question key");
      const revision = value.slice(at + 1);
      if (!/^\+?\d+$/.test(revision) || BigInt(revision) > 4294967295n) refuse("invalid question revision");
      if (BigInt(revision) === 0n) refuse("question revision must be positive");
      break;
    }
    case "decision":
    case "hold-lifted": break;
    default: refuse(`unknown or Manual resume condition: ${kind}`);
  }
  if (!target || Buffer.byteLength(target) > 512 || Buffer.byteLength(input) > 1024) {
    refuse("park resume condition must be bounded and machine-checkable");
  }
}

function apply(state: State, action: z.infer<typeof Action>, spec: string): void {
  const previous = state.progress;
  if (previous && previous.spec_revision !== spec) refuse("stale_todo_spec");
  if (action.op !== "init" && action.plan != null && action.plan !== previous?.plan) refuse("stale_todo_plan");
  const pending = (step: z.infer<typeof Step>): Step => ({ ...step, status: { state: "pending" } });
  if (action.op === "init") {
    if (previous) {
      const all = steps(previous);
      if (all.some(open)) refuse("todo_plan_open");
      state.history.push({ plan: previous.plan, revision: previous.revision, steps: all.length,
        done: all.filter((step) => step.status.state === "done").length,
        dropped: all.filter((step) => step.status.state === "dropped").length, closed: new Date().toISOString() });
      state.history = state.history.slice(-16);
    }
    if (!Number.isSafeInteger(state.plans + 1)) refuse("progress_revision_exhausted");
    state.plans++;
    state.progress = { revision: previous?.revision ?? 0, spec_revision: spec, plan: `P${state.plans}`,
      phases: action.phases.map((phase) => ({ ...phase, steps: phase.steps.map(pending) })), waits: [] };
    state.deadlines = [];
  } else {
    if (!previous) refuse("todo_not_initialized");
    switch (action.op) {
      case "append": {
        const phase = previous.phases.find((phase) => phase.id === action.phase);
        if (phase) phase.steps.push(...action.steps.map(pending));
        else {
          if (action.label == null) refuse("unknown_todo_phase");
          previous.phases.push({ id: action.phase, label: action.label, steps: action.steps.map(pending) });
        }
        break;
      }
      case "wait": {
        const text = action.on.kind === "event" ? action.on.event : action.on.question;
        if (!text.trim() || Buffer.byteLength(text) > 256 || /[\u0000-\u001f\u007f-\u009f]/u.test(text)) refuse("invalid_todo_wait");
        const now = Date.now();
        const deadline = action.deadline == null ? now + (action.on.kind === "event" ? 2 : 24) * 3600000 : Date.parse(action.deadline);
        if (!Number.isFinite(deadline) || deadline < now || deadline - now > 7 * 24 * 3600000) refuse("todo_wait_deadline_range");
        if (action.when != null) {
          if (action.when.length < 1 || action.when.length > 8) refuse("todo_wait_requires_1_to_8_checks");
          for (const check of action.when) validateCheck(check);
        }
        openStep(previous, action.step);
        previous.waits = previous.waits.filter((wait) => wait.step !== action.step);
        previous.waits.push({ step: action.step, on: action.on });
        state.deadlines = state.deadlines.filter((wait) => wait.step !== action.step);
        state.deadlines.push({ step: action.step, deadline: new Date(deadline).toISOString(), when: action.when ?? [] });
        break;
      }
      case "start":
      case "done":
      case "drop": {
        if (action.op === "drop" && !action.reason.trim()) refuse("drop_reason_required");
        const step = openStep(previous, action.step);
        step.status = action.op === "drop" ? { state: "dropped", reason: action.reason }
          : { state: action.op === "start" ? "in_progress" : "done" };
        previous.waits = previous.waits.filter((wait) => wait.step !== action.step);
        state.deadlines = state.deadlines.filter((wait) => wait.step !== action.step);
        break;
      }
    }
  }
  const progress = state.progress!;
  validate(progress);
  if (!Number.isSafeInteger(progress.revision + 1)) refuse("progress_revision_exhausted");
  progress.revision++;
  if (steps(progress).some((step) => step.criteria.length)) {
    refuse("a PM or assistant todo cannot link task acceptance criteria");
  }
}

/**
 * Every action returns { revision, progress, plan, disposition, closed_plans }, matching
 * an assistant todo receipt's result. The directory names the local assistant spec scope.
 * Snapshot exposes container.state.todo and todo_requests with plans/history and explicit
 * turn: null, continued: null. No request record exists, so no continuation is due or stopped.
 * Wait deadlines/checks persist privately; wire waits contain only step/on, like corral.
 * There is no board or PM wake publisher here. Waits remain until start/done/drop clears them.
 */
export function createTodoLedger(dir: string): { handle: LocalOpHandler; snapshot(): unknown } {
  const path = join(dir, "todo.json");
  const spec = `assistant:${basename(resolve(dir))}`;
  let state: State;
  try {
    state = State.parse(JSON.parse(readFileSync(path, "utf8")));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    state = { version: 1, plans: 0, progress: null, history: [], deadlines: [] };
  }
  const handle: LocalOpHandler = (call) => {
    const next = structuredClone(state);
    apply(next, parseAction(call.payload), spec);
    mkdirSync(dir, { recursive: true, mode: 0o700 });
    const temporary = join(dir, `.todo-${randomUUID()}.tmp`);
    const fd = openSync(temporary, "wx", 0o600);
    try {
      try {
        writeFileSync(fd, `${JSON.stringify(next)}\n`);
        fsyncSync(fd);
      } finally { closeSync(fd); }
      renameSync(temporary, path);
    } finally {
      try { unlinkSync(temporary); }
      catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; }
    }
    state = next;
    const progress = state.progress!;
    return { revision: progress.revision, progress: publicProgress(progress), plan: progress.plan,
      disposition: disposition(progress), closed_plans: state.history.length };
  };
  return { handle, snapshot: () => ({ container: { state: {
    todo: state.progress ? publicProgress(state.progress) : null,
    todo_requests: { plans: state.plans, history: structuredClone(state.history), turn: null, continued: null },
  } } }) };
}
