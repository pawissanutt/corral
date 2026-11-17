import { afterEach, expect, jest, test } from "bun:test";
import type { AgentToolResult } from "@earendil-works/pi-coding-agent";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { runChild } from "../codemode/child.ts";
import { INPUT_QUEUED_NOTICE, InputWake } from "../input-wake.ts";
import { Commands } from "./commands.ts";
import { jobTools } from "./index.ts";
import { JobRuntime } from "./runtime.ts";

const cleanup: (() => Promise<void>)[] = [];
afterEach(async () => {
  jest.useRealTimers();
  for (const stop of cleanup.splice(0)) await stop();
});

function fixture(wake?: InputWake) {
  const dir = mkdtempSync(join(tmpdir(), "corral-command-progress-"));
  const jobs = new JobRuntime({ cwd: dir, runtimeDir: join(dir, "runtime"), generation: 3,
    settings: { max_jobs: 2, max_output_bytes: 4096, max_runtime_secs: 60 },
    call: async () => ({ state: "committed", result: {} }) });
  const commands = new Commands({ jobs, generation: 3, worker: true, waitMs: 0,
    journal: () => {}, monitorJournal: () => {}, notify: async () => "queued" });
  commands.bound();
  const tools = jobTools(jobs, false, commands, wake);
  const bash = tools.find(tool => tool.name === "bash");
  const wait = tools.find(tool => tool.name === "job_wait");
  if (!bash || !wait) throw new Error("managed command tools missing");
  const fifo = (name: string) => {
    const path = join(dir, name);
    expect(spawnSync("mkfifo", [path]).status).toBe(0);
    return path;
  };
  const line = (value: string) => {
    const { promise, resolve } = Promise.withResolvers<void>();
    const remove = jobs.onLine((_job, text) => { if (text === value) { remove(); resolve(); } });
    return promise;
  };
  cleanup.push(async () => { commands.pause(); await jobs.shutdown(); rmSync(dir, { recursive: true, force: true }); });
  return { jobs, commands, bash, wait, fifo, line };
}

test("managed nested Bash streams growing tails without changing its final result or repeating idle output", async () => {
  const fx = fixture();
  const first = fx.line("one"), second = fx.line("two"), third = fx.line("three");
  const two = fx.fifo("two"), three = fx.fifo("three"), done = fx.fifo("done");
  const updates: AgentToolResult<unknown>[] = [];
  jest.useFakeTimers();
  const call = runChild({ tool: "bash", key: "parent/cell-1.1",
    parent: { tool_call_id: "parent", cell: "cell-1", call: "cell-1.1" } },
  () => fx.bash.execute("parent/1", {
    command: `echo one; cat ${two} >/dev/null; echo two; cat ${three} >/dev/null; echo three; cat ${done} >/dev/null`,
  }, undefined, update => updates.push(update), undefined as never));
  await first;
  jest.advanceTimersByTime(150);
  await writeFile(two, "go");
  await second;
  jest.advanceTimersByTime(150);
  await writeFile(three, "go");
  await third;
  jest.advanceTimersByTime(150);
  expect(updates.map(update => update.content)).toEqual([
    [{ type: "text", text: "one\n" }],
    [{ type: "text", text: "one\ntwo\n" }],
    [{ type: "text", text: "one\ntwo\nthree\n" }],
  ]);
  expect(updates.map(update => update.details)).toEqual(Array.from({ length: 3 }, () => ({ job: "j3-1", state: "running" })));
  jest.advanceTimersByTime(300);
  expect(updates).toHaveLength(3);
  await writeFile(done, "go");
  const result = await call;
  jest.advanceTimersByTime(500);
  expect(updates).toHaveLength(3);
  jest.useRealTimers();
  const baseline = fixture();
  expect(result).toEqual(await baseline.bash.execute("plain", { command: "printf 'one\\ntwo\\nthree\\n'", wait_ms: 10000 },
    undefined, undefined, undefined as never));
});

test("job_wait streams one job and removes progress on cancellation and observation expiry", async () => {
  const fx = fixture();
  const ready = fx.line("ready"), after = fx.line("after"), late = fx.line("late");
  const next = fx.fifo("next"), last = fx.fifo("last"), done = fx.fifo("done");
  const launched = await fx.bash.execute("background", {
    command: `echo ready; cat ${next} >/dev/null; echo after; cat ${last} >/dev/null; echo late; cat ${done} >/dev/null`, wait_ms: 0,
  }, undefined, undefined, undefined as never);
  const details = launched.details;
  if (!details || typeof details !== "object" || !("job" in details) || typeof details.job !== "string") throw new Error("job missing");
  const job = details.job;
  await ready;
  const updates: AgentToolResult<unknown>[] = [];
  const controller = new AbortController();
  jest.useFakeTimers();
  const waiting = fx.wait.execute("wait", { job }, controller.signal, update => updates.push(update), undefined as never);
  jest.advanceTimersByTime(100);
  expect(updates).toEqual([{ content: [{ type: "text", text: "ready\n" }], details: { job, state: "running" } }]);
  controller.abort();
  expect((await waiting).details).toEqual({ job, state: "wait_cancelled" });
  await writeFile(next, "go");
  await after;
  jest.advanceTimersByTime(300);
  expect(updates).toHaveLength(1);
  const timed = fx.wait.execute("timed", { job, wait_ms: 150 }, undefined, update => updates.push(update), undefined as never);
  jest.advanceTimersByTime(150);
  expect((await timed).details).toEqual({ job, state: "running" });
  expect(updates.at(-1)).toEqual({ content: [{ type: "text", text: "ready\nafter\n" }], details: { job, state: "running" } });
  await writeFile(last, "go");
  await late;
  jest.advanceTimersByTime(300);
  expect(updates).toHaveLength(2);
  await writeFile(done, "go");
  await fx.jobs.settled(job);
});

test("queued input ends a top-level job_wait at once but never a Code Mode cell's nested wait", async () => {
  let queued = false;
  const wake = new InputWake(() => queued);
  const fx = fixture(wake);
  const ready = fx.line("ready"), done = fx.fifo("done");
  const launched = await fx.bash.execute("background", { command: `echo ready; cat ${done} >/dev/null`, wait_ms: 0 },
    undefined, undefined, undefined as never);
  const details = launched.details;
  if (!details || typeof details !== "object" || !("job" in details) || typeof details.job !== "string") throw new Error("job missing");
  const job = details.job;
  await ready;
  const waiting = fx.wait.execute("wait", { job, wait_ms: 600000 }, undefined, undefined, undefined as never);
  wake.notify();
  const interrupted = { job, state: "running", interrupted: INPUT_QUEUED_NOTICE };
  expect((await waiting).details).toEqual(interrupted);
  queued = true;
  expect((await fx.wait.execute("again", { job, wait_ms: 600000 }, undefined, undefined, undefined as never)).details).toEqual(interrupted);
  const nested = await runChild({ tool: "job_wait", key: "cell/1", parent: { tool_call_id: "cell", cell: "c1", call: "c1.1" } },
    () => fx.wait.execute("cell/1", { job, wait_ms: 50 }, undefined, undefined, undefined as never));
  expect(nested.details).toEqual({ job, state: "running" });
  await writeFile(done, "go");
  await fx.jobs.settled(job);
});

test("managed Bash stops streaming when its observation budget backgrounds the job", async () => {
  const fx = fixture();
  const ready = fx.line("ready"), after = fx.line("after");
  const next = fx.fifo("next"), done = fx.fifo("done");
  const updates: AgentToolResult<unknown>[] = [];
  jest.useFakeTimers();
  const call = fx.bash.execute("foreground", {
    command: `echo ready; cat ${next} >/dev/null; echo after; cat ${done} >/dev/null`, wait_ms: 150,
  }, undefined, update => updates.push(update), undefined as never);
  await ready;
  jest.advanceTimersByTime(150);
  const result = await call;
  expect(result.details).toMatchObject({ job: "j3-1", state: "running", background: true });
  expect(updates).toEqual([{ content: [{ type: "text", text: "ready\n" }], details: { job: "j3-1", state: "running" } }]);
  await writeFile(next, "go");
  await after;
  jest.advanceTimersByTime(500);
  expect(updates).toHaveLength(1);
  await writeFile(done, "go");
  await fx.jobs.settled("j3-1");
});
