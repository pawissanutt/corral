// The herdr projection of a bound host (T-605). One adapter serves every role: it reports the host
// to its pane's herdr server as agent `corral-pi`, names the bound corral identity in display
// metadata and tokens, and derives working, idle and blocked from the host's own turn events and
// corral's receipts, never from the terminal. Herdr only shows what this host last reported;
// corral stays the authority for runs, questions, receipts and recovery.
import { execFile } from "node:child_process";

import type { EventLog } from "../bridge/events.ts";
import type { HostIdentity } from "../roles/types.ts";

export const SOURCE = "custom:corral-pi";
export const AGENT = "corral-pi";
/**
 * A generation reports with sequence numbers in [epoch << 20, (epoch + 1) << 20), where epoch is
 * its hello time in milliseconds as corral recorded it. The last number of the range is corral's:
 * it releases a lost generation with it (`agent/herdr_status.rs`). Herdr ignores a report whose
 * number is not above the last it accepted from this source for the pane, so neither a late
 * report of an older generation nor that release can overwrite a later generation.
 */
export const SEQ_SHIFT = 20n;
export const TOKENS = [
  "corral_role", "corral_project", "corral_task", "corral_run", "corral_review", "corral_assistant",
  "corral_generation", "corral_session",
] as const;
const OWNER_KINDS = new Set(["owner_only", "permission"]);
/**
 * An answer or correction member of an inbox envelope: corral's member frame
 * (`[corral <recipient>#<seq> <kind>]`, inbox/storage.rs), then the answer's own header
 * (questions/triage.rs). Only corral's frame counts, never the same words inside other text.
 */
const ANSWER = /^\[corral \S+#\d+ question_(?:answer|correction)\]\n(?:Corrected answer|Answer) to (\S+) \(revision (\d+)\) by \w+:/gm;
/** Corral's pending question states (corral-core `QuestionState::pending`). */
const PENDING = new Set(["open", "escalated"]);
const CALL_TIMEOUT_MS = 10_000;

export type HerdrState = "working" | "idle" | "blocked";
export type HerdrRunner = (bin: string, args: readonly string[]) => Promise<void>;

export interface HerdrTarget {
  readonly bin: string;
  readonly pane: string;
}

export interface PendingQuestion {
  readonly key: string;
  readonly kind: string;
  readonly blocking: boolean;
  readonly state?: string;
  /** Corral bumps it on every transition; the projection keeps each key's newest. */
  readonly revision?: number;
}

export interface HerdrBinding {
  readonly sessionId: string;
  readonly sessionFile: string;
  readonly epochMs: unknown;
  readonly questions: unknown;
}

/**
 * Where this host reports, or why it does not: only a host inside herdr whose inherited pane is
 * the one corral bound it to. Herdr's CLI reaches that pane's server through `HERDR_SOCKET_PATH`.
 */
export function herdrTarget(env: NodeJS.ProcessEnv, pane: string): HerdrTarget | string {
  if (env.HERDR_ENV !== "1") return "not_in_herdr";
  const bin = env.HERDR_BIN_PATH;
  if (!bin || !env.HERDR_PANE_ID || !env.HERDR_SOCKET_PATH) return "herdr_environment_incomplete";
  if (env.HERDR_PANE_ID !== pane) return "pane_mismatch";
  return { bin, pane };
}

/**
 * The host's one adapter, as a HostSession option. A headless host and one outside its bound pane
 * get none; the bridge event names why, so the readback explains an absent projection.
 */
export function herdrLifecycle(
  env: NodeJS.ProcessEnv, pane: string, identity: HostIdentity, events: EventLog, headless: boolean,
): { herdr?: HerdrLifecycle } {
  const target = headless ? "headless" : herdrTarget(env, pane);
  if (typeof target === "string") {
    events.emit("herdr_reporting_off", { reason: target });
    return {};
  }
  return { herdr: new HerdrLifecycle(target, identity, (kind, data) => events.emit(kind, data)) };
}

/** An owner must answer it, or the asker said its next step depends on it. */
export function holds(question: PendingQuestion): boolean {
  return question.blocking || OWNER_KINDS.has(question.kind) || question.state === "escalated";
}

export function answeredKeys(text: string): { key: string; revision: number }[] {
  return [...text.matchAll(ANSWER)].flatMap((match) => match[1] && match[2] ? [{ key: match[1], revision: Number(match[2]) }] : []);
}

export function pendingQuestions(value: unknown): PendingQuestion[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item) => {
    const question = item as Partial<PendingQuestion> | null;
    return question && typeof question.key === "string" && typeof question.kind === "string"
      ? [{ key: question.key, kind: question.kind, blocking: question.blocking === true,
        ...(typeof question.state === "string" ? { state: question.state } : {}),
        ...(typeof question.revision === "number" ? { revision: question.revision } : {}) }]
      : [];
  });
}

export const runHerdr: HerdrRunner = (bin, args) => new Promise((resolve, reject) => {
  const child = execFile(bin, [...args], { timeout: CALL_TIMEOUT_MS, maxBuffer: 256 * 1024 }, (error, _stdout, stderr) => {
    if (error) reject(new Error(`${String(stderr).trim() || error.message}`.slice(-256)));
    else resolve();
  });
  child.stdin?.end();
});

function display(identity: HostIdentity): string {
  const run = identity.task && identity.run ? `${identity.task}/${identity.run}` : identity.task;
  const scope = identity.role === "pm" ? identity.project : undefined;
  return [AGENT, identity.role, scope, run, identity.review, identity.assistant, `g${identity.generation}`]
    .filter((part) => part !== undefined && part !== "").join(" ");
}

type Call = {
  readonly op: "state" | "identity" | "release" | "clear";
  readonly args: string[];
  readonly state?: HerdrState;
  /** A state call's state and message as sent. */
  readonly signature?: string;
};

export class HerdrLifecycle {
  private open = false;
  private readonly questions = new Map<string, PendingQuestion>();
  private readonly revisions = new Map<string, number>();
  private binding: { readonly sessionId: string; readonly sessionFile: string; readonly base: bigint } | undefined;
  private used = 0n;
  private identityDue = false;
  private reported: string | undefined;
  private running = false;
  private changes = 0;
  private flight: Promise<void> = Promise.resolve();
  private closed = false;
  private closing: Promise<void> | undefined;
  private readonly stats = {
    calls: 0, failed: 0, last_error: null as string | null, reported: null as HerdrState | null, seq: null as string | null,
  };

  constructor(
    private readonly target: HerdrTarget,
    private readonly identity: HostIdentity,
    private readonly emit: (kind: string, data: Record<string, unknown>) => void,
    private readonly run: HerdrRunner = runHerdr,
  ) {}

  /** Reporting starts once the generation's hello committed: its receipt fixes the sequence range. */
  bind(binding: HerdrBinding): void {
    if (this.closed || this.binding) return;
    const epoch = binding.epochMs;
    if (typeof epoch !== "number" || !Number.isSafeInteger(epoch) || epoch <= 0) {
      this.failed("bind", "the hello receipt carries no herdr_seq_epoch_ms; herdr reporting stays off");
      return;
    }
    this.binding = { sessionId: binding.sessionId, sessionFile: binding.sessionFile, base: BigInt(epoch) << SEQ_SHIFT };
    for (const question of pendingQuestions(binding.questions)) this.place(question);
    this.identityDue = true;
    this.kick();
  }

  turnStarted(): void {
    this.open = true;
    this.kick();
  }

  turnEnded(): void {
    this.open = false;
    this.kick();
  }

  /** A committed ask receipt: corral opened the question at `revision`. */
  asked(question: PendingQuestion): void {
    if (this.place({ ...question, state: question.state ?? "open", revision: question.revision ?? 1 })) this.kick();
  }

  /** Answered through the inbox or withdrawn; without a revision, corral holds it as not pending. */
  resolved(key: string, revision?: number): void {
    if (!this.newer(key, revision)) return;
    if (this.questions.delete(key)) this.kick();
  }

  /**
   * Corral's states of the container's questions (the bridge `questions` command), covering
   * escalations and cancellations this host did not make. Each key keeps its newest revision, so a
   * late or repeated update cannot undo a newer fact, and questions left out stay as they are.
   */
  apply(updates: readonly PendingQuestion[]): void {
    let changed = false;
    for (const update of updates) changed = this.place(update) || changed;
    if (changed) this.kick();
  }

  private newer(key: string, revision: number | undefined): boolean {
    if (revision === undefined) return true;
    if (revision <= (this.revisions.get(key) ?? 0)) return false;
    this.revisions.set(key, revision);
    return true;
  }

  /** Holds or releases one question at its revision; true when the held set changed. */
  private place(question: PendingQuestion): boolean {
    if (!this.newer(question.key, question.revision)) return false;
    const held = this.questions.has(question.key);
    if ((question.state === undefined || PENDING.has(question.state)) && holds(question)) {
      this.questions.set(question.key, question);
      return true;
    }
    this.questions.delete(question.key);
    return held;
  }

  get state(): HerdrState {
    if (this.questions.size > 0) return "blocked";
    return this.open ? "working" : "idle";
  }

  /**
   * Graceful exit: stops reporting, then releases the pane and clears this source's metadata. Every
   * caller awaits the same release, so none can let the process exit before herdr has it.
   */
  close(): Promise<void> {
    this.closing ??= this.release();
    return this.closing;
  }

  private async release(): Promise<void> {
    this.closed = true;
    await this.flight;
    if (!this.binding) return;
    const pane = this.target.pane;
    await this.call({ op: "release", args: ["pane", "release-agent", pane, "--source", SOURCE, "--agent", AGENT] });
    await this.call({ op: "clear", args: ["pane", "report-metadata", pane, "--source", SOURCE, "--clear-display-agent",
      ...TOKENS.flatMap((token) => ["--clear-token", token])] });
  }

  readback(): Record<string, unknown> {
    return {
      source: SOURCE, agent: AGENT, pane: this.target.pane, bound: this.binding !== undefined, closed: this.closed,
      state: this.state, blocked_on: [...this.questions.keys()], ...this.stats,
    };
  }

  private kick(): void {
    this.changes++;
    if (this.running || this.closed || !this.binding) return;
    this.running = true;
    this.flight = this.pump();
  }

  /**
   * One report at a time, always of the latest state: changes during a call coalesce. A failed call
   * is retried only for a change that arrived meanwhile, so a herdr that is down is not hammered.
   */
  private async pump(): Promise<void> {
    for (;;) {
      const next = this.closed ? undefined : this.next();
      if (!next) {
        this.running = false;
        return;
      }
      if (next.op === "identity") this.identityDue = false;
      const changes = this.changes;
      if (await this.call(next)) {
        if (next.signature !== undefined) this.reported = next.signature;
      } else if (this.changes === changes) {
        this.running = false;
        return;
      }
    }
  }

  private signature(state: HerdrState): string {
    return `${state}\n${this.message(state) ?? ""}`;
  }

  private message(state: HerdrState): string | undefined {
    if (state !== "blocked") return undefined;
    const [first] = this.questions.values();
    const more = this.questions.size > 1 ? ` and ${this.questions.size - 1} more` : "";
    return first ? `corral question ${first.key} (${first.kind})${more} awaits an answer` : undefined;
  }

  /** Identity first: herdr applies it as soon as this source holds the pane, so the agent never appears unnamed. */
  private next(): Call | undefined {
    const binding = this.binding;
    if (!binding) return undefined;
    if (this.identityDue) return this.identityCall(binding.sessionId);
    const state = this.state;
    const signature = this.signature(state);
    if (signature === this.reported) return undefined;
    const message = this.message(state);
    return { op: "state", state, signature, args: ["pane", "report-agent", this.target.pane, "--source", SOURCE, "--agent", AGENT,
      "--state", state, "--agent-session-id", binding.sessionId, "--agent-session-path", binding.sessionFile,
      ...(message ? ["--message", message] : [])] };
  }

  private identityCall(sessionId: string): Call {
    const id = this.identity;
    const values: Record<(typeof TOKENS)[number], string | undefined> = {
      corral_role: id.role, corral_project: id.project, corral_task: id.task, corral_run: id.run,
      corral_review: id.review, corral_assistant: id.assistant, corral_generation: String(id.generation),
      corral_session: sessionId,
    };
    return { op: "identity", args: ["pane", "report-metadata", this.target.pane, "--source", SOURCE, "--agent", AGENT,
      "--applies-to-source", SOURCE, "--display-agent", display(id),
      ...TOKENS.flatMap((token) => values[token] ? ["--token", `${token}=${values[token]}`] : ["--clear-token", token])] };
  }

  private async call(call: Call): Promise<boolean> {
    const binding = this.binding;
    if (!binding) return false;
    // The range's last number belongs to corral's release of a lost generation.
    if (this.used >= (1n << SEQ_SHIFT) - 2n) {
      this.failed(call.op, "this generation used its whole herdr sequence range");
      return false;
    }
    this.used++;
    const seq = (binding.base + this.used).toString();
    this.stats.calls++;
    try {
      await this.run(this.target.bin, [...call.args, "--seq", seq]);
    } catch (error) {
      this.failed(call.op, error instanceof Error ? error.message : String(error));
      return false;
    }
    this.stats.seq = seq;
    if (call.state) this.stats.reported = call.state;
    this.emit("herdr_reported", { op: call.op, seq, ...(call.state ? { state: call.state } : {}) });
    return true;
  }

  private failed(op: string, reason: string): void {
    this.stats.failed++;
    this.stats.last_error = reason.slice(0, 256);
    this.emit("herdr_report_failed", { op, reason: reason.slice(0, 256) });
  }
}
