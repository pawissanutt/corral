// Background jobs (T-565) against a fake corral: the reports a job sends, in order, with real
// processes. Barriers are FIFOs the test opens, so no step depends on timing.
import { afterEach, expect, test } from "bun:test";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  type JobCall, type JobReceipt, JobRuntime, type Schedule, processGroup, startTimeOf,
} from "../src/extensions/jobs/runtime.ts";

const dirs: string[] = [];

function fields(value: Record<string, unknown>, keys: readonly string[]): Record<string, unknown> {
  return Object.fromEntries(keys.map((key) => [key, value[key]]));
}
afterEach(() => {
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

interface Sent {
  readonly op: string;
  readonly key: string;
  readonly payload: Record<string, unknown>;
}

function fixture(options: {
  maxOutput?: number; maxJobs?: number; schedule?: Schedule; answer?: (sent: Sent) => Promise<JobReceipt>;
} = {}) {
  const dir = mkdtempSync(join(tmpdir(), "corral-jobs-"));
  dirs.push(dir);
  const sent: Sent[] = [];
  const committed = (result: unknown = {}): JobReceipt => ({ state: "committed", result });
  const call: JobCall = async (op, key, payload) => {
    const item = { op, key, payload };
    sent.push(item);
    return options.answer ? options.answer(item) : committed();
  };
  const runtime = new JobRuntime({
    cwd: dir, runtimeDir: join(dir, "runtime"), generation: 3, call, ...(options.schedule ? { schedule: options.schedule } : {}),
    settings: { max_jobs: options.maxJobs ?? 4, max_output_bytes: options.maxOutput ?? 1024, max_runtime_secs: 60 },
  });
  const fifo = (name: string) => {
    const path = join(dir, name);
    expect(spawnSync("mkfifo", [path]).status).toBe(0);
    return path;
  };
  const actions = () => sent.filter((s) => s.op === "job").map((s) => s.payload.action);
  const identity = (job: string) => {
    const started = sent.find((s) => s.payload.action === "started" && s.payload.job === job)?.payload;
    return { pid: Number(started?.pid), start: String(started?.start_time) };
  };
  return { dir, sent, runtime, fifo, actions, identity };
}

test("a job is recorded before it spawns, with its pid and kernel start time, and output is bounded", async () => {
  // Given a job that prints more than the retained output bound.
  const fx = fixture({ maxOutput: 1024 });
  const gate = fx.fifo("gate");
  // When it is released after its identity was recorded and runs to completion.
  const started = await fx.runtime.start(`cat ${gate} >/dev/null; head -c 3000 /dev/zero | tr '\\0' x`, "call-1");
  await writeFile(gate, "go\n");
  await fx.runtime.settled(started.job as string);
  // Then corral saw prepare, started and exited in order, and output was capped and pageable.
  expect(fx.actions()).toEqual(["prepare", "started", "exited"]);
  expect(fx.sent[0]?.key).toBe("call-1/prepare");
  const identity = fx.sent[1]?.payload;
  expect(identity?.pid).toBe(started.pid);
  expect(String(identity?.start_time)).toMatch(/^\d+$/);
  expect(fx.sent[2]?.payload).toEqual({ action: "exited", job: "j3-1", exit_code: 0, group_clear: true });
  const output = fx.runtime.output("j3-1", 1000, 100);
  expect(fields(output, ["bytes_total", "bytes_stored", "truncated", "offset", "next_offset"]))
    .toEqual({ bytes_total: 3000, bytes_stored: 1024, truncated: true, offset: 1000, next_offset: 1024 });
  expect(output.text).toBe("x".repeat(24));
});

test("a line monitor armed before its line reports one match, before the exit", async () => {
  // Given a job blocked on a barrier and a committed line monitor.
  const fx = fixture();
  const gate = fx.fifo("gate");
  const { job } = await fx.runtime.start(`cat ${gate} >/dev/null; echo READY; echo READY again`, "c1");
  const receipt = await fx.runtime.monitor(job as string, "line", "^READY", 60, "c2");
  expect(receipt.state).toBe("committed");
  const registered = fx.sent.find((s) => s.op === "job_monitor");
  expect(registered?.key).toBe("c2");
  expect(fields(registered?.payload ?? {}, ["wait", "job", "on", "pattern"]))
    .toEqual({ wait: "j3-1.m1", job: "j3-1", on: "line", pattern: "^READY" });
  // When the barrier is released and the job exits.
  await writeFile(gate, "go\n");
  await fx.runtime.settled(job as string);
  // Then exactly one match was reported, and only then the exit.
  expect(fx.actions()).toEqual(["prepare", "started", "line", "exited"]);
  expect(fx.sent.find((s) => s.payload.action === "line")?.payload).toEqual({
    action: "line", job: "j3-1", wait: "j3-1.m1", line: "READY",
  });
});

test("a line printed while the monitor commits still matches, and the exit report waits for it", async () => {
  // Given a monitor whose corral commit is held open.
  let release!: (receipt: JobReceipt) => void;
  const held = new Promise<JobReceipt>((resolve) => { release = resolve; });
  const fx = fixture({
    answer: async (sent) => (sent.op === "job_monitor" ? held : { state: "committed", result: {} }),
  });
  const gate = fx.fifo("gate");
  const { job } = await fx.runtime.start(`cat ${gate} >/dev/null; echo READY`, "c1");
  const monitoring = fx.runtime.monitor(job as string, "line", "READY", 60, "c2");
  // When the line prints and the process exits before the commit answers.
  await writeFile(gate, "go\n");
  await fx.runtime.closed(job as string);
  // Then nothing is reported until the monitor commits; afterwards the match precedes the exit.
  expect(fx.actions()).toEqual(["prepare", "started"]);
  release({ state: "committed", result: {} });
  await monitoring;
  await fx.runtime.settled(job as string);
  expect(fx.actions()).toEqual(["prepare", "started", "line", "exited"]);
});

test("a refused monitor reports no match; a refused prepare never spawns", async () => {
  // Given corral refusing the first job's monitor and the second job's prepare.
  let refusePrepare = false;
  const fx = fixture({
    answer: async (sent) => (sent.op === "job_monitor" || (refusePrepare && sent.payload.action === "prepare")
      ? { state: "rejected", result: { code: "op_not_allowed" } } : { state: "committed", result: {} }),
  });
  const gate = fx.fifo("gate");
  const { job } = await fx.runtime.start(`cat ${gate} >/dev/null; echo READY`, "c1");
  expect((await fx.runtime.monitor(job as string, "line", "READY", 60, "c2")).state).toBe("rejected");
  await writeFile(gate, "go\n");
  await fx.runtime.settled(job as string);
  refusePrepare = true;
  const refused = await fx.runtime.start("echo never", "c3");
  // Then no line report was sent and the refused job has no process.
  expect(fx.actions()).toEqual(["prepare", "started", "exited", "prepare"]);
  expect(refused.state).toBe("refused");
});

test("job_kill stops the whole process group and its exit carries the signal", async () => {
  // Given a job whose shell started a child process.
  const fx = fixture();
  const { job, pid } = await fx.runtime.start("sleep 30 & wait", "c1");
  // When it is killed.
  const killed = await fx.runtime.kill(job as string);
  // Then the group is gone and corral was told how it ended.
  expect(fields(killed, ["state", "signal"])).toEqual({ state: "exited", signal: "SIGTERM" });
  expect(startTimeOf(pid as number)).toBeUndefined();
  expect(fx.sent.at(-1)?.payload).toEqual({ action: "exited", job: "j3-1", signal: "SIGTERM", group_clear: true });
});

test("losing the runtime output file falls back to the in-memory tail", async () => {
  // Given a finished job.
  const fx = fixture();
  const { job } = await fx.runtime.start("echo hello from the job", "c1");
  await fx.runtime.settled(job as string);
  // When its runtime directory disappears.
  rmSync(join(fx.dir, "runtime"), { recursive: true, force: true });
  // Then output still answers from memory and says the file is missing.
  expect(fields(fx.runtime.output(job as string), ["state", "output_file", "text"])).toEqual({
    state: "exited", output_file: "missing", text: "hello from the job\n",
  });
});

test("the host refuses jobs beyond max_jobs and unknown job IDs", async () => {
  // Given a one-job limit with a job running.
  const fx = fixture({ maxJobs: 1 });
  const { job } = await fx.runtime.start("sleep 30", "c1");
  // When another job starts and an unknown job is read, then both refuse without a corral call.
  await expect(fx.runtime.start("true", "c2")).rejects.toThrow("job_limit");
  expect(() => fx.runtime.output("j3-9")).toThrow("unknown job");
  expect(fx.actions()).toEqual(["prepare", "started"]);
  await fx.runtime.kill(job as string);
});

/** A job whose child ignores SIGTERM, redirects its output and reports readiness on `ready`. */
function stubborn(ready: string, leader: string): string {
  return `(trap '' TERM; exec >/dev/null 2>&1 </dev/null; echo up > ${ready}; exec sleep 300) & ${leader}`;
}

for (const path of ["job_kill", "runtime limit", "leader exit", "shutdown"] as const) {
  test(`${path} leaves no member of the job's group alive, even one that ignores SIGTERM`, async () => {
    // Given a job whose descendant ignores SIGTERM and has redirected its output, and is ready.
    const deadlines: (() => void)[] = [];
    const schedule: Schedule = (run, ms) => {
      if (ms === 60_000) deadlines.push(run);
      return () => undefined;
    };
    const fx = fixture({ schedule });
    const ready = fx.fifo("ready");
    const gate = fx.fifo("gate");
    const up = readFile(ready, "utf8");
    const leader = path === "leader exit" ? `cat ${gate} >/dev/null` : "wait";
    const { job } = await fx.runtime.start(stubborn(ready, leader), "c1");
    expect(await up).toBe("up\n");
    const { pid, start } = fx.identity(job as string);
    expect(processGroup(pid, start)).toBe("owned");
    // When the job is ended through this path.
    if (path === "job_kill") await fx.runtime.kill(job as string);
    if (path === "runtime limit") deadlines.splice(0).forEach((run) => run());
    if (path === "leader exit") await writeFile(gate, "go\n");
    if (path === "shutdown") fx.runtime.shutdown();
    await fx.runtime.settled(job as string);
    // Then the whole group is gone and the exit report says so.
    expect(processGroup(pid, start)).toBe("empty");
    expect(fx.sent.at(-1)?.payload.group_clear).toBeTrue();
  });
}

const PIDFD_WAIT = "import os, select, sys\nfd = os.pidfd_open(int(sys.argv[1]))\nprint('ARMED', flush=True)\n"
  + "ready, _, _ = select.select([fd], [], [], 30)\nprint('EXITED' if ready else 'TIMEOUT', flush=True)\n";

function lines(stream: NodeJS.ReadableStream): (pattern: RegExp) => Promise<string> {
  let buffer = "";
  const waiting: { pattern: RegExp; resolve: (value: string) => void }[] = [];
  const check = () => {
    for (const entry of [...waiting]) {
      const match = buffer.split("\n").map((line) => entry.pattern.exec(line)).find((found) => found);
      if (match) {
        waiting.splice(waiting.indexOf(entry), 1);
        entry.resolve(match[1] ?? match[0]);
      }
    }
  };
  stream.on("data", (chunk: Buffer) => { buffer += chunk.toString("utf8"); check(); });
  return (pattern) => new Promise((resolve) => { waiting.push({ pattern, resolve }); check(); });
}

test("a host killed between spawn and the identity commit leaves no command running", async () => {
  // Given a host stalled inside the started commit of a job whose command would leave a marker.
  const dir = mkdtempSync(join(tmpdir(), "corral-jobs-crash-"));
  dirs.push(dir);
  const marker = join(dir, "ran");
  const runtimeDir = join(dir, "runtime");
  const fixturePath = fileURLToPath(new URL("./fixtures/jobs/crash-host.ts", import.meta.url));
  const host = spawn(process.execPath, [fixturePath, runtimeDir, marker], { stdio: ["ignore", "pipe", "inherit"] });
  const pid = await lines(host.stdout)(/^STARTED (\d+)$/);
  const waiter = spawn("python3", ["-I", "-c", PIDFD_WAIT, pid], { stdio: ["ignore", "pipe", "inherit"] });
  const next = lines(waiter.stdout);
  expect(await next(/^(ARMED)$/)).toBe("ARMED");
  // When the host is killed in that window and its runtime files are removed.
  const gone = new Promise((resolve) => host.once("close", resolve));
  host.kill("SIGKILL");
  await gone;
  rmSync(runtimeDir, { recursive: true, force: true });
  // Then the gated launcher exits by itself and the command never ran.
  expect(await next(/^(EXITED|TIMEOUT)$/)).toBe("EXITED");
  expect(existsSync(marker)).toBeFalse();
});
