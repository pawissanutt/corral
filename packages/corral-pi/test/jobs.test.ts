// Local process-supervisor regressions with FIFO barriers instead of timing sleeps.
import { afterEach, expect, test } from "bun:test";
import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { PassThrough } from "node:stream";

import {
  JobRuntime, type JobRuntimeOptions, type Schedule, processGroup, startTimeOf,
} from "../src/extensions/jobs/runtime.ts";
import { openGroupAnchor } from "../src/extensions/jobs/group-anchor.ts";

const dirs: string[] = [];
const runtimes: JobRuntime[] = [];

function fields(value: Record<string, unknown>, keys: readonly string[]): Record<string, unknown> {
  return Object.fromEntries(keys.map((key) => [key, value[key]]));
}
afterEach(async () => {
  await Promise.all(runtimes.splice(0).map(runtime => runtime.shutdown()));
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});


function fixture(options: {
  maxOutput?: number; maxJobs?: number; schedule?: Schedule;
  registerCapability?: JobRuntimeOptions["registerCapability"];
  onPrepared?: JobRuntimeOptions["onPrepared"];
} = {}) {
  const dir = mkdtempSync(join(tmpdir(), "corral-jobs-"));
  dirs.push(dir);
  const events: { kind: string; data: Record<string, unknown> }[] = [];
  const prepared: string[] = [];
  const runtime = new JobRuntime({
    cwd: dir, runtimeDir: join(dir, "runtime"), ...(options.schedule ? { schedule: options.schedule } : {}),
    registerCapability: options.registerCapability,
    onPrepared(job) { prepared.push(job); options.onPrepared?.(job); },
    onEvent: (kind, data) => events.push({ kind, data }),
    settings: { max_jobs: options.maxJobs ?? 4, max_output_bytes: options.maxOutput ?? 1024, max_runtime_secs: 60 },
  });
  runtimes.push(runtime);
  const fifo = (name: string) => {
    const path = join(dir, name);
    expect(spawnSync("mkfifo", [path]).status).toBe(0);
    return path;
  };
  return { dir, events, prepared, runtime, fifo };
}

test("the supervisor is registered until exit, not the command it launches", async () => {
  const pids = new Set<number>();
  const fx = fixture({ registerCapability: (pid) => { pids.add(pid); return () => { pids.delete(pid); }; } });
  const gate = fx.fifo("registered-gate");
  const started = await fx.runtime.start(`cat ${gate} >/dev/null`);
  expect([...pids]).toEqual([started.pid]);
  await writeFile(gate, "go\n");
  await fx.runtime.settled(started.job as string);
  expect([...pids]).toEqual([]);
});

test("a locally prepared job reports start and exit and bounds retained output", async () => {
  const fx = fixture({ maxOutput: 1024 });
  const gate = fx.fifo("gate");
  const started = await fx.runtime.start(`cat ${gate} >/dev/null; head -c 3000 /dev/zero | tr '\\0' x`);
  const { job } = started;
  if (typeof job !== "string") throw new Error("job missing");
  expect(fx.prepared).toEqual([job]);
  expect(fx.events).toEqual([{ kind: "job_started", data: { job, pid: started.pid } }]);
  await writeFile(gate, "go\n");
  await fx.runtime.settled(job);
  expect(fx.events[1]).toEqual({ kind: "job_exited", data: { job, exit_code: 0, signal: null, group_clear: true } });
  const output = fx.runtime.output(job, 1000, 100);
  expect(fields(output, ["bytes_total", "bytes_stored", "truncated", "offset", "next_offset"]))
    .toEqual({ bytes_total: 3000, bytes_stored: 1024, truncated: true, offset: 1000, next_offset: 1024 });
  expect(output.text).toBe("x".repeat(24));
  expect(fx.runtime.inspection()[0]).toMatchObject({ job, state: "exited", exitCode: 0, groupClear: true, bytes: 3000, truncated: true });
});

test("failed local preparation never spawns the command", async () => {
  const fx = fixture({ onPrepared: () => { throw new Error("transcript unavailable"); } });
  const marker = join(fx.dir, "ran");
  await expect(fx.runtime.start(`touch "${marker}"`)).rejects.toThrow("transcript unavailable");
  expect(fx.runtime.state()).toEqual([]);
  expect(fx.events).toEqual([]);
  expect(existsSync(marker)).toBeFalse();
});

test("job_kill stops the whole process group and its exit carries the signal", async () => {
  // Given a job whose shell started a child process.
  const fx = fixture();
  const ready = fx.fifo("kill-ready");
  const up = readFile(ready, "utf8");
  const { job, pid } = await fx.runtime.start(`sleep 30 & echo ready > ${ready}; wait`);
  // The command shell must have launched its child before the group is signalled.
  expect(await up).toBe("ready\n");
  // When it is killed.
  const killed = await fx.runtime.kill(job as string);
  // The group is gone and the local exit event carries its signal.
  expect(fields(killed, ["state", "signal"])).toEqual({ state: "exited", signal: "SIGTERM" });
  expect(startTimeOf(pid as number)).toBeUndefined();
  expect(fx.events.at(-1)).toEqual({ kind: "job_exited", data: { job, exit_code: null, signal: "SIGTERM", group_clear: true } });
}, 30_000); // Includes the runtime's 5s sweep, grace, startup and report handling.

test("job_kill waits for a TERM-ignoring command and reports the forced exit", async () => {
  const fx = fixture();
  const ready = fx.fifo("slow-kill-ready");
  const up = readFile(ready, "utf8");
  const { job, pid } = await fx.runtime.start(`trap '' TERM; echo ready > ${ready}; exec sleep 300`);
  expect(await up).toBe("ready\n");
  const killed = await fx.runtime.kill(job as string);
  expect(fields(killed, ["state", "signal", "group_clear"])).toEqual({
    state: "exited", signal: "SIGKILL", group_clear: true,
  });
  expect(startTimeOf(pid as number)).toBeUndefined();
}, 30_000);

test("forced stop clears a suspended group without relying on its control reader", async () => {
  const fx = fixture({ schedule(run, ms) { if (ms < 60_000) queueMicrotask(run); return () => {}; } });
  const ready = fx.fifo("suspend-ready");
  const started = readFile(ready, "utf8");
  const { job, pid } = await fx.runtime.start(`trap '' TERM; echo ready > ${ready}; exec sleep 30`);
  expect(await started).toBe("ready\n");
  if (typeof job !== "string" || typeof pid !== "number") throw new Error("job identity missing");
  const start = startTimeOf(pid);
  const anchor = openGroupAnchor(pid, () => start !== undefined && startTimeOf(pid) === start);
  if (!anchor) { console.log("process-group pidfd unavailable; suspended stop not exercised"); return; }
  anchor.close();
  try {
    process.kill(-pid, "SIGSTOP");
    expect(await fx.runtime.kill(job)).toMatchObject({ stop_outcome: "stopped", signal: "SIGKILL", group_clear: true });
  } finally {
    if (startTimeOf(pid) === start && processGroup(pid, start).owned) {
      try { process.kill(-pid, "SIGCONT"); } catch { /* already exited */ }
    }
  }
}, 12_000);

test("losing the runtime output file falls back to the in-memory tail", async () => {
  // Given a finished job.
  const fx = fixture();
  const { job } = await fx.runtime.start("echo hello from the job");
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
  const { job } = await fx.runtime.start("sleep 30");
  await expect(fx.runtime.start("true")).rejects.toThrow("job_limit");
  expect(() => fx.runtime.output("foreign")).toThrow("unknown job");
  expect(fx.prepared).toEqual([job]);
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
      else queueMicrotask(run); // Deliver the forced-stop timer without waiting through the grace period.
      return () => undefined;
    };
    const fx = fixture({ schedule });
    const ready = fx.fifo("ready");
    const gate = fx.fifo("gate");
    const up = readFile(ready, "utf8");
    const leader = path === "leader exit" ? `cat ${gate} >/dev/null` : "wait";
    const { job, pid } = await fx.runtime.start(stubborn(ready, leader));
    expect(await up).toBe("up\n");
    if (typeof job !== "string" || typeof pid !== "number") throw new Error("job identity missing");
    const start = startTimeOf(pid);
    expect(processGroup(pid, start).owned).toBeTrue();
    // When the job is ended through this path.
    if (path === "job_kill") await fx.runtime.kill(job);
    if (path === "runtime limit") deadlines.splice(0).forEach((run) => run());
    if (path === "leader exit") await writeFile(gate, "go\n");
    if (path === "shutdown") await fx.runtime.shutdown();
    await fx.runtime.settled(job);
    // Then the whole group is gone and the exit report says so.
    expect(processGroup(pid, start).members).toBe(0);
    expect(fx.events.at(-1)?.data.group_clear).toBeTrue();
  });
}

const PIDFD_WAIT = "import os, select, sys, time\nfds = {os.pidfd_open(int(pid)) for pid in sys.argv[1:]}\n"
  + "print('ARMED', flush=True)\nend = time.monotonic() + 30\nwhile fds:\n"
  + " ready, _, _ = select.select(list(fds), [], [], max(0, end - time.monotonic()))\n if not ready: break\n fds.difference_update(ready)\n"
  + "print('TIMEOUT' if fds else 'EXITED', flush=True)\n";

function lines(stream: NodeJS.ReadableStream): (pattern: RegExp) => Promise<string> {
  let buffer = "";
  const complete: string[] = [];
  let ended: Error | undefined;
  const waiting: { pattern: RegExp; resolve: (value: string) => void; reject: (error: Error) => void }[] = [];
  const check = () => {
    for (const entry of [...waiting]) {
      const match = complete.map((line) => entry.pattern.exec(line)).find((found) => found);
      if (match || ended) {
        waiting.splice(waiting.indexOf(entry), 1);
        if (match) entry.resolve(match[1] ?? match[0]);
        else if (ended) entry.reject(ended);
      }
    }
  };
  stream.on("data", (chunk: Buffer) => {
    buffer += chunk.toString("utf8");
    const parts = buffer.split("\n");
    buffer = parts.pop() ?? "";
    complete.push(...parts);
    check();
  });
  const end = (error?: Error) => {
    ended = error ?? new Error(`handshake ended before a matching line: ${JSON.stringify([...complete, buffer])}`);
    check();
  };
  stream.once("end", () => end());
  stream.once("error", end);
  stream.once("close", () => end());
  return (pattern) => new Promise((resolve, reject) => { waiting.push({ pattern, resolve, reject }); check(); });
}

test("crash handshakes wait for the complete identity line, not a chunk ending in digits", async () => {
  const stream = new PassThrough();
  const next = lines(stream);
  const identity = next(/^STARTED (\d+)$/);
  stream.write("STARTED 12");
  stream.write("345\n");
  expect(await identity).toBe("12345");
  stream.end();
});

test("crash handshakes reject a truncated identity or a failed peer instead of hanging", async () => {
  const stream = new PassThrough();
  const next = lines(stream);
  const identity = next(/^STARTED (\d+)$/);
  stream.end("STARTED 12");
  await expect(identity).rejects.toThrow("handshake ended");
  await expect(next(/^(ARMED)$/)).rejects.toThrow("handshake ended");
  const failed = new PassThrough();
  const armed = lines(failed)(/^(ARMED)$/);
  failed.destroy(new Error("peer failed"));
  await expect(armed).rejects.toThrow("peer failed");
});

for (const phase of ["gated", "active"]) test(`a crash during the ${phase} phase leaves no owned command running`, async () => {
  // Synchronize either before run admission or after the TERM-ignoring command has started.
  const dir = mkdtempSync(join(tmpdir(), "corral-jobs-crash-"));
  dirs.push(dir);
  const marker = join(dir, "ran");
  const runtimeDir = join(dir, "runtime");
  const fixturePath = fileURLToPath(new URL("./fixtures/jobs/crash-host.ts", import.meta.url));
  const host = spawn(process.execPath, [fixturePath, runtimeDir, marker, ...(phase === "active" ? [phase] : [])],
    { stdio: ["ignore", "pipe", "inherit"] });
  // Arm exit observations at spawn, before waiting on either peer's handshake.
  const gone = new Promise((resolve) => host.once("close", resolve));
  let waiter: ChildProcess | undefined;
  let waiterGone: Promise<unknown> | undefined;
  const identities = new Map<number, string>();
  try {
    const pids = (await lines(host.stdout)(/^STARTED (\d+(?: \d+)?)$/)).split(" ");
    expect(pids).toHaveLength(phase === "active" ? 2 : 1);
    for (const raw of pids) {
      const pid = Number(raw), start = startTimeOf(pid);
      if (start === undefined) throw new Error("owned process exited before readiness handshake");
      identities.set(pid, start);
    }
    waiter = spawn("python3", ["-I", "-c", PIDFD_WAIT, ...pids], { stdio: ["ignore", "pipe", "inherit"] });
    waiterGone = new Promise((resolve) => waiter?.once("close", resolve));
    const next = lines(waiter.stdout!);
    expect(await next(/^(ARMED)$/)).toBe("ARMED");
    // When the host is killed in that window and its runtime files are removed.
    host.kill("SIGKILL");
    await gone;
    rmSync(runtimeDir, { recursive: true, force: true });
    // Then EOF clears the launcher and the running command, if it had been admitted.
    expect(await next(/^(EXITED|TIMEOUT)$/)).toBe("EXITED");
    await waiterGone;
    for (const pid of identities.keys()) expect(startTimeOf(pid)).toBeUndefined();
    expect(existsSync(marker)).toBeFalse();
  } finally {
    host.kill("SIGKILL");
    waiter?.kill("SIGKILL");
    await Promise.all([gone, waiterGone]);
    // Only on failure: never leave an observed fixture process behind after a broken handshake.
    for (const [pid, start] of identities) if (startTimeOf(pid) === start) {
      try { process.kill(pid, "SIGKILL"); } catch { /* already exited */ }
    }
  }
});

test("only the recorded leader anchors a group; a leaderless or stale identity is never owned", async () => {
  // Given a disposable group whose leader blocks on stdin beside a TERM-ignoring descendant.
  const leader = spawn("/bin/bash", ["-c", "(trap '' TERM; exec sleep 300) & echo $!; read -r _; exit 0"], {
    detached: true, stdio: ["pipe", "pipe", "inherit"],
  });
  const descendant = Number(await lines(leader.stdout)(/^(\d+)$/));
  const pgid = leader.pid as number;
  const real = startTimeOf(pgid);
  try {
    // Then while the leader lives only its real start time owns the group.
    expect(processGroup(pgid, real)).toEqual({ owned: true, reused: false, members: 2 });
    expect(processGroup(pgid, "1")).toEqual({ owned: false, reused: true, members: 2 });
    // When the leader exits and is reaped, leaving the descendant in the group.
    // "exit", not "close": the descendant still holds the leader's stdout pipe.
    const gone = new Promise((resolve) => leader.once("exit", resolve));
    leader.stdin.end();
    await gone;
    // Then neither the real nor a stale identity owns it any more, and the descendant lives on.
    expect(processGroup(pgid, real)).toEqual({ owned: false, reused: false, members: 1 });
    expect(processGroup(pgid, "1")).toEqual({ owned: false, reused: false, members: 1 });
    expect(startTimeOf(descendant)).toBeDefined();
  } finally {
    try { process.kill(-pgid, "SIGKILL"); } catch { /* already gone */ }
  }
});

test("a command's own output never stands in for the launcher's status line", async () => {
  // Given a command that prints a status-shaped line and exits with another code.
  const fx = fixture();
  const { job } = await fx.runtime.start('echo "exit 7"; exit 3');
  await fx.runtime.settled(job as string);
  // Then the launcher's status wins and the line is ordinary job output.
  expect(fx.events.at(-1)).toEqual({ kind: "job_exited", data: { job, exit_code: 3, signal: null, group_clear: true } });
  expect(fx.runtime.output(job as string).text).toBe("exit 7\n");
});
