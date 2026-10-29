// Background jobs of a Pi-native worker (T-565). A job is its whole process group. It is recorded
// with corral before it is spawned, and its process identity (pid, kernel start time) is
// committed before the command runs: the spawned launcher waits on its stdin for "run", so a host
// that dies first leaves no executable work behind. When the leader exits, every remaining
// verified member of its group is killed before the exit is reported. Output goes to a runtime
// file capped at max_output_bytes plus an in-memory tail that survives the file's loss. Line
// monitors match in this process; corral settles every monitor's wait and posts its one wake.
import { type ChildProcess, spawn } from "node:child_process";
import { closeSync, existsSync, mkdirSync, openSync, readFileSync, readSync, readdirSync, writeSync } from "node:fs";
import { join } from "node:path";

import type { JobSettings } from "./settings.ts";

export const OUTPUT_READ_DEFAULT = 16 * 1024;
export const OUTPUT_READ_MAX = 64 * 1024;
const TAIL_BYTES = 64 * 1024;
const LINE_MATCH_MAX = 4096;
const KILL_GRACE_MS = 2000;
const REPORT_ATTEMPTS = 4;
const REPORT_BACKOFF_MS = 250;
/** How long the host waits for SIGKILLed group members to disappear before reporting. */
const SWEEP_WAIT_MS = 5000;
const SWEEP_STEP_MS = 20;
/** `$0` is the shell and `$1` the command; nothing runs unless the host writes "run". */
const LAUNCHER = 'IFS= read -r go || exit 125; [ "$go" = run ] || exit 125; exec </dev/null; exec "$0" -c "$1"';

export interface JobReceipt {
  readonly state: string;
  readonly result: unknown;
}

/** One corral call: `key` is the stable request key the caller turns into a request ID. */
export type JobCall = (op: "job" | "job_monitor", key: string, payload: Record<string, unknown>) => Promise<JobReceipt>;

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
  readonly generation: number;
  readonly settings: JobSettings;
  readonly call: JobCall;
  readonly shell?: string;
  readonly schedule?: Schedule;
  /** Bridge events: `job_started` once identity is recorded, `job_exited` once the exit is. */
  readonly onEvent?: (kind: "job_started" | "job_exited", data: Record<string, unknown>) => void;
}

interface Watcher {
  readonly wait: string;
  readonly regex: RegExp;
  readonly afterLine: number;
  armed: boolean;
  matched?: string;
  reported: boolean;
}

interface Job {
  readonly id: string;
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
  monitors: number;
  inflight: number;
  drained: (() => void)[];
  reports: Promise<unknown>;
  readonly watchers: Watcher[];
  readonly closed: Promise<void>;
  readonly exited: Promise<void>;
  cancelKill?: () => void;
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

/**
 * The job's process group now. Linux never reuses a pid while it is still some group's ID, so
 * live members of a group whose leader is gone are the job's descendants; a live leader must
 * still have the recorded start time. Without /proc a group is never claimed.
 */
export function processGroup(pgid: number, start: string | undefined): "empty" | "owned" | "reused" {
  if (start === undefined || !existsSync("/proc/self/stat")) return "reused";
  let members = 0;
  for (const name of readdirSync("/proc")) {
    if (!/^\d+$/.test(name)) continue;
    const found = stat(Number(name));
    if (!found || found.pgrp !== pgid || found.state === "Z" || found.state === "X") continue;
    if (Number(name) === pgid && found.start !== start) return "reused";
    members++;
  }
  return members === 0 ? "empty" : "owned";
}

export class JobError extends Error {}

export class JobRuntime {
  private readonly jobs = new Map<string, Job>();
  private seq = 0;
  private readonly schedule: Schedule;

  constructor(private readonly options: JobRuntimeOptions) {
    this.schedule = options.schedule ?? realSchedule;
  }

  private live(): number {
    return [...this.jobs.values()].filter((job) => job.state === "running").length;
  }

  private job(id: string): Job {
    const job = this.jobs.get(id);
    if (!job) throw new JobError(`unknown job ${id}; job_start returns the job ID`);
    return job;
  }

  // The request key keeps a retried report idempotent; a report that never lands leaves the
  // job for corral's reaper and the monitor for its deadline wake.
  private async report(key: string, payload: Record<string, unknown>): Promise<void> {
    for (let attempt = 0; attempt < REPORT_ATTEMPTS; attempt++) {
      if (attempt > 0) await new Promise((resolve) => setTimeout(resolve, REPORT_BACKOFF_MS * 2 ** (attempt - 1)));
      try {
        await this.options.call("job", key, payload);
        return;
      } catch {
        continue;
      }
    }
  }

  private enqueue(job: Job, key: string, payload: Record<string, unknown>): void {
    job.reports = job.reports.then(() => this.report(key, payload));
  }

  async start(command: string, key: string): Promise<Record<string, unknown>> {
    const { settings } = this.options;
    if (this.live() >= settings.max_jobs) {
      throw new JobError(`job_limit: ${settings.max_jobs} jobs are already running; wait for one or kill it`);
    }
    const id = `j${this.options.generation}-${++this.seq}`;
    const prepared = await this.options.call("job", `${key}/prepare`, { action: "prepare", job: id, command });
    if (prepared.state !== "committed") return { job: id, state: "refused", result: prepared.result };
    const dir = join(this.options.runtimeDir, "jobs");
    mkdirSync(dir, { recursive: true, mode: 0o700 });
    const file = join(dir, `${id}.log`);
    const fd = openSync(file, "w", 0o600);
    const shell = this.options.shell ?? "/bin/bash";
    const child = spawn(shell, ["-c", LAUNCHER, shell, command], {
      cwd: this.options.cwd, detached: true, stdio: ["pipe", "pipe", "pipe"],
    });
    const spawned = await new Promise<Error | undefined>((resolve) => {
      child.once("spawn", () => resolve(undefined));
      child.once("error", (error) => resolve(error));
    });
    if (spawned || child.pid === undefined) {
      closeSync(fd);
      await this.options.call("job", `${key}/exited`, { action: "exited", job: id, exit_code: 127, group_clear: true });
      throw new JobError(`job ${id} did not start: ${spawned?.message ?? "no pid"}`);
    }
    let finish!: () => void;
    let close!: () => void;
    const job: Job = {
      id, command, file, child, pid: child.pid, start: startTimeOf(child.pid), state: "running", exitCode: null,
      signal: null, groupClear: false, bytes: 0, stored: 0, fd, tail: Buffer.alloc(0), partial: "", lines: 0,
      monitors: 0, inflight: 0, drained: [], reports: Promise.resolve(), watchers: [],
      closed: new Promise((resolve) => { close = resolve; }),
      exited: new Promise((resolve) => { finish = resolve; }),
    };
    this.jobs.set(id, job);
    child.stdin?.on("error", () => undefined);
    child.stdout?.on("data", (chunk: Buffer) => this.capture(job, chunk));
    child.stderr?.on("data", (chunk: Buffer) => this.capture(job, chunk));
    const cancelDeadline = this.schedule(() => void this.kill(id), settings.max_runtime_secs * 1000);
    child.once("close", (code, signal) => {
      cancelDeadline();
      if (job.partial) this.line(job, job.partial);
      job.partial = "";
      job.state = "exited";
      job.exitCode = code;
      job.signal = signal;
      if (job.fd !== undefined) closeSync(job.fd);
      job.fd = undefined;
      void this.sweep(job).then(() => {
        close();
        return this.whenDrained(job);
      }).then(() => {
        const payload: Record<string, unknown> = { action: "exited", job: id };
        if (code !== null) payload.exit_code = code;
        if (signal !== null) payload.signal = signal;
        if (job.groupClear) payload.group_clear = true;
        this.enqueue(job, `job:${id}:exited`, payload);
        return job.reports;
      }).then(() => this.options.onEvent?.("job_exited", { job: id, exit_code: code, signal })).finally(finish);
    });
    // The launcher is blocked on stdin: the command runs only once corral holds its identity.
    let recorded = false;
    if (job.start !== undefined) {
      try {
        const receipt = await this.options.call("job", `${key}/started`, {
          action: "started", job: id, pid: child.pid, start_time: job.start,
        });
        recorded = receipt.state === "committed";
      } catch {
        recorded = false;
      }
    }
    await new Promise<void>((resolve) => {
      if (!child.stdin) resolve();
      else if (recorded) child.stdin.end("run\n", () => resolve());
      else child.stdin.end(() => resolve());
    });
    if (!recorded) return { job: id, state: "not_started", reason: "corral did not record the job's process identity" };
    this.options.onEvent?.("job_started", { job: id, pid: child.pid });
    return { job: id, state: job.state, pid: child.pid, output_file: file };
  }

  /**
   * After the leader exits, kills every remaining verified member of its group and waits for
   * them to go, so a descendant that ignored SIGTERM or redirected its output cannot outlive
   * the job. A group that cannot be verified is never signalled.
   */
  private async sweep(job: Job): Promise<void> {
    let group = processGroup(job.pid, job.start);
    if (group === "owned") {
      this.groupSignal(job, "SIGKILL");
      for (let waited = 0; waited < SWEEP_WAIT_MS && group === "owned"; waited += SWEEP_STEP_MS) {
        await new Promise((resolve) => setTimeout(resolve, SWEEP_STEP_MS));
        group = processGroup(job.pid, job.start);
      }
    }
    job.groupClear = group === "empty";
    job.cancelKill?.();
  }

  private whenDrained(job: Job): Promise<void> {
    return job.inflight === 0 ? Promise.resolve() : new Promise((resolve) => job.drained.push(resolve));
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
    for (const watcher of job.watchers) {
      if (watcher.matched !== undefined || job.lines <= watcher.afterLine || !watcher.regex.test(candidate)) continue;
      watcher.matched = candidate;
      this.reportMatch(job, watcher);
    }
  }

  private reportMatch(job: Job, watcher: Watcher): void {
    if (!watcher.armed || watcher.matched === undefined || watcher.reported) return;
    watcher.reported = true;
    this.enqueue(job, `job:${job.id}:line:${watcher.wait}`, {
      action: "line", job: job.id, wait: watcher.wait, line: watcher.matched.slice(0, 1024),
    });
  }

  /**
   * Registers a monitor through `corral_job_monitor`'s receipted wait. A line watcher arms
   * before the call, so a line printed while corral commits still matches; its match is
   * reported only after the commit, and the job's exit report waits for both.
   */
  async monitor(
    id: string, on: "exit" | "line", pattern: string | undefined, timeoutSecs: number, key: string,
  ): Promise<JobReceipt> {
    const job = this.job(id);
    const wait = `${id}.m${++job.monitors}`;
    let watcher: Watcher | undefined;
    if (on === "line") {
      if (!pattern) throw new JobError("a line monitor needs a pattern");
      let regex: RegExp;
      try {
        regex = new RegExp(pattern);
      } catch (error) {
        throw new JobError(`invalid pattern: ${error instanceof Error ? error.message : error}`);
      }
      watcher = { wait, regex, afterLine: job.lines, armed: false, reported: false };
      job.watchers.push(watcher);
    }
    const deadline = new Date(Date.now() + timeoutSecs * 1000).toISOString();
    job.inflight++;
    try {
      const receipt = await this.options.call("job_monitor", key, {
        wait, job: id, on, ...(pattern !== undefined && on === "line" ? { pattern } : {}), deadline,
      });
      if (watcher && receipt.state === "committed") {
        watcher.armed = true;
        this.reportMatch(job, watcher);
      } else if (watcher) {
        job.watchers.splice(job.watchers.indexOf(watcher), 1);
      }
      return receipt;
    } finally {
      job.inflight--;
      if (job.inflight === 0) for (const resolve of job.drained.splice(0)) resolve();
    }
  }

  output(id: string, offset?: number, maxBytes = OUTPUT_READ_DEFAULT): Record<string, unknown> {
    const job = this.job(id);
    const max = Math.min(Math.max(1, maxBytes), OUTPUT_READ_MAX);
    const base = {
      job: id, state: job.state, exit_code: job.exitCode, signal: job.signal,
      bytes_total: job.bytes, bytes_stored: job.stored, truncated: job.bytes > job.stored,
    };
    let fd: number | undefined;
    try {
      fd = openSync(job.file, "r");
    } catch {
      const start = job.bytes - job.tail.length;
      return {
        ...base, output_file: "missing", offset: start, next_offset: job.bytes,
        text: job.tail.subarray(Math.max(0, job.tail.length - max)).toString("utf8"),
      };
    }
    try {
      const from = offset === undefined ? Math.max(0, job.stored - max) : Math.min(Math.max(0, offset), job.stored);
      const buffer = Buffer.alloc(Math.min(max, job.stored - from));
      const read = buffer.length === 0 ? 0 : readSync(fd, buffer, 0, buffer.length, from);
      return { ...base, offset: from, next_offset: from + read, text: buffer.subarray(0, read).toString("utf8") };
    } finally {
      closeSync(fd);
    }
  }

  /** Signals the job's group only while it is verifiably the job's: never a reused pid. */
  private groupSignal(job: Job, signal: NodeJS.Signals): void {
    if (processGroup(job.pid, job.start) !== "owned") return;
    try {
      process.kill(-job.pid, signal);
    } catch {
      return;
    }
  }

  async kill(id: string): Promise<Record<string, unknown>> {
    const job = this.job(id);
    if (!job.groupClear) {
      this.groupSignal(job, "SIGTERM");
      job.cancelKill?.();
      job.cancelKill = this.schedule(() => this.groupSignal(job, "SIGKILL"), KILL_GRACE_MS);
      await job.exited;
    }
    return { job: id, state: job.state, exit_code: job.exitCode, signal: job.signal, group_clear: job.groupClear };
  }

  shutdown(): void {
    for (const job of this.jobs.values()) if (!job.groupClear) this.groupSignal(job, "SIGKILL");
  }

  state(): Record<string, unknown>[] {
    return [...this.jobs.values()].map((job) => ({
      job: job.id, pid: job.pid, state: job.state, exit_code: job.exitCode, signal: job.signal,
      group_clear: job.groupClear, bytes_total: job.bytes, monitors: job.monitors,
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
