// Background jobs are local process groups. The launcher waits on stdin until the host
// has recorded the job, and kills its group if the host disappears. Output is bounded.
import { type ChildProcess, spawn } from "node:child_process";
import { closeSync, existsSync, mkdirSync, openSync, readFileSync, readdirSync, rmSync, writeSync } from "node:fs";
import { constants } from "node:os";
import { join } from "node:path";

import type { JobSettings } from "./settings.ts";
import { readOutput, type OutputOptions } from "./output.ts";
import type { RegisterCapability } from "../registry.ts";

export const OUTPUT_READ_DEFAULT = 16 * 1024;
export const OUTPUT_READ_MAX = 64 * 1024;
const TAIL_BYTES = 64 * 1024;
const LINE_MATCH_MAX = 4096;
const KILL_GRACE_MS = 2000;
/** How long the host waits for SIGKILLed group members to disappear before reporting. */
const SWEEP_WAIT_MS = 5000;
const SWEEP_STEP_MS = 20;
/**
 * `$0` is the shell and `$1` the command. Nothing runs unless the host writes "run". SIGTERM
 * and SIGINT only interrupt the launcher's wait (the command keeps default handlers), so the
 * leader outlives the command; the host's group SIGKILL or its own EOF-triggered `kill 0` ends it.
 */
const LAUNCHER = [
  'IFS= read -r go || exit 125', '[ "$go" = run ] || exit 125', "trap : TERM INT",
  '"$0" -c "$1" </dev/null >&2 &', "c=$!",
  'while :; do wait "$c"; s=$?; [ "$s" -gt 128 ] && kill -0 "$c" 2>/dev/null || break; done',
  'printf "exit %s\\n" "$s"', "IFS= read -r _", "kill -KILL 0",
].join("\n");


/** Schedules `run` after `ms` and returns its cancellation; tests drive the runtime limit with it. */
export type Schedule = (run: () => void, ms: number) => () => void;

const realSchedule: Schedule = (run, ms) => {
  const timer = setTimeout(run, ms);
  timer.unref();
  return () => clearTimeout(timer);
};

export interface JobRuntimeOptions {
  readonly cwd: string;
  readonly runtimeDir: string;
  readonly settings: JobSettings;
  readonly shell?: string;
  readonly environment?: () => NodeJS.ProcessEnv;
  /** Durable session fact before the launcher can execute anything. */
  readonly onPrepared?: (job: string) => void;
  readonly schedule?: Schedule;
  readonly registerCapability?: RegisterCapability;
  /** Bridge events: `job_started` once identity is recorded, `job_exited` once the exit is. */
  readonly onEvent?: (kind: "job_started" | "job_exited", data: Record<string, unknown>) => void;
}

export interface JobInspection {
  readonly job: string; readonly command: string; readonly state: "running" | "exited";
  readonly startedAt: number | undefined; readonly endedAt: number | undefined;
  readonly exitCode: number | null; readonly signal: string | null; readonly groupClear: boolean;
  readonly bytes: number; readonly truncated: boolean; readonly tail: string; readonly monitorAttempts: number;
  /** Process-group leader, finished output lines and launch directory; absent from legacy observations. */
  readonly pid?: number; readonly lines?: number; readonly cwd?: string;
  readonly watches: readonly { wait: string; pattern: string; armed: boolean; matched: string | undefined; reportQueued: boolean; deadline: string }[];
}

interface Job {
  readonly id: string;
  startedAt: number | undefined;
  endedAt: number | undefined;
  readonly command: string;
  readonly file: string;
  readonly child: ChildProcess;
  readonly pid: number;
  readonly start: string | undefined;
  state: "running" | "exited";
  exitCode: number | null;
  signal: string | null;
  groupClear: boolean;
  bytes: number;
  stored: number;
  fd: number | undefined;
  tail: Buffer;
  partial: string;
  lines: number;

  readonly closed: Promise<void>;
  readonly exited: Promise<void>;
  cancelKill?: () => void;
  status: string;
  commandStatus?: number;
  reason?: "deadline" | "cancelled" | "host_stopped";
}

interface Stat {
  readonly state: string;
  readonly pgrp: number;
  readonly start: string;
}

function stat(pid: number): Stat | undefined {
  try {
    const text = readFileSync(`/proc/${pid}/stat`, "utf8");
    const fields = text.slice(text.lastIndexOf(")") + 2).split(" ");
    const [state, pgrp, start] = [fields[0], Number(fields[2]), fields[19]];
    return state && start && /^\d+$/.test(start) ? { state, pgrp, start } : undefined;
  } catch {
    return undefined;
  }
}

/** Kernel start time of `pid` in clock ticks since boot; `undefined` once it is gone. */
export function startTimeOf(pid: number): string | undefined {
  if (!existsSync("/proc/self/stat")) return "0";
  const found = stat(pid);
  return found && found.state !== "Z" && found.state !== "X" ? found.start : undefined;
}

export interface GroupView {
  /** The recorded leader still holds the pid (running or an unreaped zombie) with its start time. */
  readonly owned: boolean;
  /** The pid is held by a process with another start time or in another group. */
  readonly reused: boolean;
  /** Live (non-zombie) processes whose group ID is the job's pid; counting signals nothing. */
  readonly members: number;
}

/**
 * The job's process group now. Ownership is anchored on the recorded leader alone: a group whose
 * leader is gone cannot be told apart from a later group that reused the ID, so it is never
 * claimed, whatever its members. Without /proc nothing is claimed.
 */
export function processGroup(pgid: number, start: string | undefined): GroupView {
  if (!existsSync("/proc/self/stat")) return { owned: false, reused: false, members: 0 };
  const leader = stat(pgid);
  const reused = leader !== undefined && (leader.start !== start || leader.pgrp !== pgid);
  let members = 0;
  for (const name of readdirSync("/proc")) {
    if (!/^\d+$/.test(name)) continue;
    const found = stat(Number(name));
    if (found && found.pgrp === pgid && found.state !== "Z" && found.state !== "X") members++;
  }
  return { owned: leader !== undefined && !reused && start !== undefined, reused, members };
}

const SIGNAL_NAMES = Object.fromEntries(Object.entries(constants.signals).map(([name, number]) => [number, name]));

export class JobError extends Error {}

export class JobRuntime {
  private readonly jobs = new Map<string, Job>();
  private readonly lineListeners = new Set<(job: string, line: string) => void>();
  private shutdownEpoch = 0;
  private shutdownOperation?: Promise<void>;
  private readonly starting = new Set<Promise<unknown>>();
  private readonly schedule: Schedule;

  constructor(private readonly options: JobRuntimeOptions) {
    this.schedule = options.schedule ?? realSchedule;
  }

  private live(): number {
    return [...this.jobs.values()].filter((job) => job.state === "running").length;
  }

  private job(id: string): Job {
    const job = this.jobs.get(id);
    if (!job) {
      throw new JobError(`unknown job ${id}; bash returns the job ID`);
    }
    return job;
  }

  async start(command: string, timeoutSecs?: number): Promise<Record<string, unknown>> {
    if (this.shutdownOperation) throw new JobError("job runtime is shutting down");
    if (this.live() + this.starting.size >= this.options.settings.max_jobs) throw new JobError("job_limit: wait for a job to finish");
    if (timeoutSecs !== undefined && (!Number.isFinite(timeoutSecs) || timeoutSecs <= 0)) throw new JobError("timeout must be positive seconds");
    const starting = this.startJob(command, this.shutdownEpoch, timeoutSecs);
    this.starting.add(starting);
    try {
      return await starting;
    } finally {
      this.starting.delete(starting);
    }
  }

  private async startJob(command: string, epoch: number, timeoutSecs?: number): Promise<Record<string, unknown>> {
    const { settings } = this.options;
    if (this.live() >= settings.max_jobs) {
      throw new JobError(`job_limit: ${settings.max_jobs} jobs are already running; wait for one or kill it`);
    }
    const id = `j${crypto.randomUUID()}`;
    if (epoch !== this.shutdownEpoch) return { job: id, state: "not_started", reason: "job runtime shut down" };
    this.options.onPrepared?.(id);
    const dir = join(this.options.runtimeDir, "jobs");
    mkdirSync(dir, { recursive: true, mode: 0o700 });
    const file = join(dir, `${id}.log`);
    const fd = openSync(file, "w", 0o600);
    const shell = this.options.shell ?? "/bin/bash";
    const child = spawn(shell, ["-c", LAUNCHER, shell, command], {
      cwd: this.options.cwd, detached: true, stdio: ["pipe", "pipe", "pipe"],
      ...(this.options.environment ? { env: this.options.environment() } : {}),
    });
    const spawned = await new Promise<Error | undefined>((resolve) => {
      child.once("spawn", () => resolve(undefined));
      child.once("error", (error) => resolve(error));
    });
    if (spawned || child.pid === undefined) {
      closeSync(fd);
      this.options.onEvent?.("job_exited", { job: id, exit_code: 127 });
      throw new JobError(`job ${id} did not start: ${spawned?.message ?? "no pid"}`);
    }
    try {
      const remove = this.options.registerCapability?.(child.pid);
      if (remove) child.once("exit", remove);
    } catch (error) {
      child.kill("SIGKILL");
      closeSync(fd);
      this.options.onEvent?.("job_exited", { job: id, exit_code: 127 });
      throw error;
    }
    let finish!: () => void;
    let close!: () => void;
    const job: Job = {
      id, startedAt: undefined, endedAt: undefined, command, file, child, pid: child.pid, start: startTimeOf(child.pid), state: "running", exitCode: null,
      signal: null, groupClear: false, status: "", bytes: 0, stored: 0, fd, tail: Buffer.alloc(0), partial: "", lines: 0,
      closed: new Promise((resolve) => { close = resolve; }),
      exited: new Promise((resolve) => { finish = resolve; }),
    };
    this.jobs.set(id, job);
    child.stdin?.on("error", () => undefined);
    child.stdout?.on("data", (chunk: Buffer) => this.statusLine(job, chunk));
    child.stderr?.on("data", (chunk: Buffer) => this.capture(job, chunk));
    let cancelDeadline = () => {};
    child.once("close", (code, signal) => {
      cancelDeadline();
      if (job.partial) this.line(job, job.partial);
      job.partial = "";
      job.state = "exited";
      job.endedAt = Date.now();
      const status = job.commandStatus;
      job.exitCode = status === undefined ? code : status < 128 ? status : null;
      job.signal = status === undefined ? signal : status > 128 ? SIGNAL_NAMES[status - 128] ?? `SIG${status - 128}` : null;
      if (job.fd !== undefined) closeSync(job.fd);
      job.fd = undefined;
      void this.sweep(job).then(() => {
        this.options.onEvent?.("job_exited", { job: id, exit_code: job.exitCode, signal: job.signal, group_clear: job.groupClear });
      }).finally(() => { close(); finish(); });
    });
    // The open stdin is the launcher's lifeline.
    await new Promise<void>((resolve) => {
      if (!child.stdin) resolve();
      else if (job.start !== undefined && epoch === this.shutdownEpoch && job.state === "running") {
        job.startedAt = Date.now();
        cancelDeadline = this.schedule(() => void this.kill(id, "deadline"), Math.min(timeoutSecs ?? settings.max_runtime_secs, settings.max_runtime_secs) * 1000);
        child.stdin.write("run\n", () => resolve());
      }
      else child.stdin.end(() => resolve());
    });
    if (job.startedAt === undefined) return { job: id, state: "not_started", reason: "owning host stopped" };
    this.options.onEvent?.("job_started", { job: id, pid: child.pid });
    return { job: id, state: job.state, pid: child.pid, output_file: file };
  }

  /** The launcher reports the command's status; the still-anchored group is then cleared. */
  private statusLine(job: Job, chunk: Buffer): void {
    job.status = (job.status + chunk.toString("utf8")).slice(-256);
    const found = /^exit (\d+)$/m.exec(job.status);
    if (job.commandStatus !== undefined || !found) return;
    job.commandStatus = Number(found[1]);
    this.groupSignal(job, "SIGKILL");
  }

  /**
   * Once the leader has gone, waits for the members its group SIGKILL ended to disappear. This
   * only reads /proc: with the leader gone the group can no longer be verified as this job's,
   * so nothing is signalled here, and a group that did not empty stays for corral's reaper.
   */
  private async sweep(job: Job): Promise<void> {
    let members = processGroup(job.pid, job.start).members;
    for (let waited = 0; waited < SWEEP_WAIT_MS && members > 0; waited += SWEEP_STEP_MS) {
      await new Promise((resolve) => setTimeout(resolve, SWEEP_STEP_MS));
      members = processGroup(job.pid, job.start).members;
    }
    job.groupClear = members === 0;
    job.cancelKill?.();
  }

  private capture(job: Job, chunk: Buffer): void {
    job.bytes += chunk.length;
    const room = this.options.settings.max_output_bytes - job.stored;
    if (room > 0 && job.fd !== undefined) {
      const part = chunk.subarray(0, room);
      try {
        writeSync(job.fd, part);
        job.stored += part.length;
      } catch {
        closeSync(job.fd);
        job.fd = undefined;
      }
    }
    const tail = Buffer.concat([job.tail, chunk]);
    job.tail = tail.length > TAIL_BYTES ? tail.subarray(tail.length - TAIL_BYTES) : tail;
    const text = job.partial + chunk.toString("utf8");
    const parts = text.split("\n");
    job.partial = (parts.pop() ?? "").slice(-LINE_MATCH_MAX);
    for (const part of parts) this.line(job, part);
  }

  private line(job: Job, text: string): void {
    job.lines++;
    const candidate = text.slice(0, LINE_MATCH_MAX);
    for (const listener of this.lineListeners) listener(job.id, candidate);
  }

  output(id: string, offset?: number, maxBytes = OUTPUT_READ_DEFAULT): Record<string, unknown> {
    return this.peek(id, { ...(offset === undefined ? {} : { offset }), max_bytes: maxBytes });
  }

  peek(id: string, options: OutputOptions = {}): Record<string, unknown> {
    const job = this.job(id);
    return { job: id, state: job.state, exit_code: job.exitCode, signal: job.signal,
      group_clear: job.groupClear, termination: job.reason ?? (job.state === "exited" ? "exit" : null),
      bytes_total: job.bytes, bytes_stored: job.stored, truncated: job.bytes > job.stored,
      ...readOutput(job, options) };
  }

  onLine(listener: (job: string, line: string) => void): () => void {
    this.lineListeners.add(listener);
    return () => this.lineListeners.delete(listener);
  }

  /** Signals the job's group only while its recorded leader anchors it: never a reused pid. */
  private groupSignal(job: Job, signal: NodeJS.Signals): void {
    if (!processGroup(job.pid, job.start).owned) return;
    try {
      process.kill(-job.pid, signal);
    } catch {
      return;
    }
  }

  async kill(id: string, reason: "cancelled" | "deadline" = "cancelled"): Promise<Record<string, unknown>> {
    const job = this.job(id);
    if (job.state === "running" && job.commandStatus === undefined) job.reason ??= reason;
    if (!job.groupClear) {
      this.groupSignal(job, "SIGTERM");
      job.cancelKill?.();
      job.cancelKill = this.schedule(() => this.groupSignal(job, "SIGKILL"), KILL_GRACE_MS);
      await job.exited;
    }
    return { ...this.peek(id), group_clear: job.groupClear };
  }

  /** Only acknowledged terminal results are eligible for bounded cache eviction. */
  forget(id: string): void {
    const job = this.jobs.get(id);
    if (!job) return;
    if (job.state !== "exited" || !job.groupClear) throw new JobError(`cannot evict live or unsettled job ${id}`);
    rmSync(job.file, { force: true });
    this.jobs.delete(id);
  }

  async shutdown(): Promise<void> {
    if (this.shutdownOperation) return this.shutdownOperation;
    this.shutdownEpoch++;
    const operation = this.stopJobs();
    this.shutdownOperation = operation;
    try {
      await operation;
    } finally {
      this.shutdownOperation = undefined;
    }
  }

  private async stopJobs(): Promise<void> {
    await Promise.allSettled([...this.starting]);
    await Promise.all([...this.jobs.values()].filter((job) => !job.groupClear).map(async (job) => {
      if (job.state === "running" && job.commandStatus === undefined) job.reason ??= "host_stopped";
      this.groupSignal(job, "SIGTERM");
      job.cancelKill?.();
      job.cancelKill = this.schedule(() => this.groupSignal(job, "SIGKILL"), KILL_GRACE_MS);
      await job.closed;
    }));
  }

  /** Local observations only: exited/report-queued never means canonical settlement. */
  inspection(): readonly JobInspection[] {
    return [...this.jobs.values()].map(job => ({
      job: job.id, command: job.command, state: job.state, startedAt: job.startedAt, endedAt: job.endedAt,
      exitCode: job.exitCode, signal: job.signal, groupClear: job.groupClear,
      bytes: job.bytes, truncated: job.bytes > job.stored, tail: job.tail.subarray(-8192).toString("utf8"), monitorAttempts: 0,
      pid: job.pid, lines: job.lines, cwd: this.options.cwd,
      watches: [],
    }));
  }
  state(): Record<string, unknown>[] {
    return [...this.jobs.values()].map((job) => ({
      job: job.id, pid: job.pid, state: job.state, exit_code: job.exitCode, signal: job.signal,
      group_clear: job.groupClear, bytes_total: job.bytes,
    }));
  }

  settled(id: string): Promise<void> {
    return this.job(id).exited;
  }

  /** Resolves when the process has exited and its group was swept, before any report. */
  closed(id: string): Promise<void> {
    return this.job(id).closed;
  }
}
