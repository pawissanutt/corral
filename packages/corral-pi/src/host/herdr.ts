// Optional pane projection derived from the host’s turn and question state.
import { execFile } from "node:child_process";

import type { EventLog } from "../bridge/events.ts";
import type { HostIdentity } from "../roles/types.ts";

export const SOURCE = "custom:corral-pi";
export const AGENT = "corral-pi";
// Herdr accepts its source state updates in increasing order. Start near the wall clock.
export const SEQ_SHIFT = 20n;
export const TOKENS = [
  "corral_role", "corral_project", "corral_task", "corral_run", "corral_review", "corral_assistant",
  "corral_session",
] as const;
const CALL_TIMEOUT_MS = 10_000;

export type HerdrState = "working" | "idle" | "blocked";
export type HerdrRunner = (bin: string, args: readonly string[]) => Promise<void>;

export interface HerdrTarget {
  readonly bin: string;
  readonly pane: string;
}

export interface HerdrBinding {
  readonly sessionId: string;
  readonly sessionFile: string;
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
  return [AGENT, identity.role, scope, run, identity.review, identity.assistant]
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
  private readonly questions = new Set<string>();
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

  bind(binding: HerdrBinding): void {
    if (this.closed || this.binding) return;
    this.binding = { ...binding, base: BigInt(Date.now()) << SEQ_SHIFT };
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

  questionChanged(id: string, pending: boolean): void {
    const before = this.questions.has(id);
    if (pending) this.questions.add(id);
    else this.questions.delete(id);
    if (before !== pending) this.kick();
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
    return first ? `corral question ${first}${more} awaits an answer` : undefined;
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
      corral_review: id.review, corral_assistant: id.assistant,
      corral_session: sessionId,
    };
    return { op: "identity", args: ["pane", "report-metadata", this.target.pane, "--source", SOURCE, "--agent", AGENT,
      "--applies-to-source", SOURCE, "--display-agent", display(id),
      ...TOKENS.flatMap((token) => values[token] ? ["--token", `${token}=${values[token]}`] : ["--clear-token", token])] };
  }

  private async call(call: Call): Promise<boolean> {
    const binding = this.binding;
    if (!binding) return false;
    const seq = (binding.base + ++this.used).toString();
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
