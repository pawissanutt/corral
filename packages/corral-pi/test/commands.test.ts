// Managed observation/delivery regressions. Real process barriers, never timing sleeps.
import { afterEach, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readOutput } from "../src/extensions/jobs/output.ts";
import { runChild } from "../src/extensions/codemode/child.ts";
import { jobTools } from "../src/extensions/jobs/index.ts";
import { Commands, type CommandFact } from "../src/extensions/jobs/commands.ts";
import { type MonitorFact } from "../src/extensions/jobs/monitors.ts";
import { JobRuntime, type Schedule } from "../src/extensions/jobs/runtime.ts";
import { jobControl } from "../src/host/job-controls.ts";

const cleanup: (() => Promise<void>)[] = [];
afterEach(async () => { for (const stop of cleanup.splice(0)) await stop(); });
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
function fixture(options: { worker?: boolean; schedule?: Schedule; waitMs?: number; started?: () => Promise<void> } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "corral-commands-"));
  const facts: CommandFact[] = [], monitors: MonitorFact[] = [], notices: string[] = [];
  const tracked = deferred<string>();
  const jobs = new JobRuntime({ cwd: dir, runtimeDir: join(dir, "runtime"), generation: 3,
    settings: { max_jobs: 2, max_output_bytes: 1024, max_runtime_secs: 60 },
    call: async (_op, _key, payload) => {
      if (payload.action === "started") await options.started?.();
      return { state: "committed", result: {} };
    }, ...(options.schedule ? { schedule: options.schedule } : {}) });
  const commands = new Commands({ jobs, generation: 3, worker: options.worker ?? false,
    waitMs: options.waitMs ?? 10000, journal: fact => { facts.push(fact); tracked.resolve(fact.job); },
    monitorJournal: fact => monitors.push(fact), notify: async (_key, text) => { notices.push(text); return "queued"; } });
  const fifo = (name: string) => {
    const path = join(dir, name);
    expect(spawnSync("mkfifo", [path]).status).toBe(0);
    return path;
  };
  cleanup.push(async () => { commands.pause(); await jobs.shutdown(); rmSync(dir, { recursive: true, force: true }); });
  const line = (value: string) => {
    const done = deferred<void>();
    const remove = jobs.onLine((_job, text) => { if (text === value) { remove(); done.resolve(); } });
    return done.promise;
  };
  return { dir, jobs, commands, facts, monitors, notices, fifo, tracked: tracked.promise, line };
}

test("background promotion releases one Bash observer without relaunching or resetting its hard deadline", async () => {
  const schedules: { ms: number; run: () => void }[] = [];
  const fx = fixture({ schedule: (run, ms) => { schedules.push({ run, ms }); return () => {}; } });
  fx.commands.bound();
  const gate = fx.fifo("release"), before = fx.line("before");
  const foreground = fx.commands.run(`echo before; cat ${gate} >/dev/null; echo after`, "call", { timeout: 20 });
  const job = await fx.tracked;
  await before;
  const pid = fx.jobs.inspection()[0]?.pid;
  expect(fx.jobs.peek(job, { tail_lines: 1 })).toMatchObject({ state: "running", text: "before\n" });
  expect(fx.commands.background(job)).toMatchObject({ background: true });
  expect(await foreground).toMatchObject({ job, state: "running", background: true });
  expect(schedules.map(item => item.ms)).toEqual([20000]);
  expect(fx.jobs.inspection()[0]?.pid).toBe(pid);
  await writeFile(gate, "go");
  await fx.jobs.settled(job);
  await fx.commands.notified();
  fx.commands.flush();
  await fx.commands.notified();
  expect(fx.notices).toHaveLength(1);
  expect(fx.notices[0]).toContain("before\nafter\n");
  expect(fx.jobs.peek(job).text).toBe("before\nafter\n");
  expect(await fx.commands.wait(undefined, 0)).toMatchObject({ state: "none" });
  expect(await fx.commands.wait(job, 0)).toMatchObject({ state: "exited", exit_code: 0 });
});

test("foreground and nested Bash return real completion; Code Mode sequencing is not silently backgrounded", async () => {
  const fx = fixture({ waitMs: 0 });
  fx.commands.bound();
  const gate = fx.fifo("nested"), ready = fx.line("ready");
  let advanced = false;
  const cell = fx.commands.run(`echo ready; cat ${gate} >/dev/null; echo done`, "nested", { nested: true })
    .then(result => { advanced = true; return result; });
  await ready;
  const job = await fx.tracked;
  expect(advanced).toBeFalse();
  await writeFile(gate, "go");
  expect(await cell).toMatchObject({ job, state: "exited", text: "ready\ndone\n" });
  await fx.commands.notified();
  expect(fx.notices).toEqual([]);
  const bash = jobTools(fx.jobs, true, fx.commands).find(tool => tool.name === "bash");
  if (!bash) throw new Error("managed Bash missing");
  const failure = await bash.execute("fail", { command: "exit 7", wait_ms: 10000 }, undefined, undefined, {} as never);
  expect(failure.isError).toBeTrue();
  expect(failure.content.flatMap(part => part.type === "text" ? [part.text] : []).join("\n")).toContain("exit_code: 7");
  const raw = await bash.execute("raw", { command: "seq 1 60", wait_ms: 10000 }, undefined, undefined, {} as never);
  expect(raw.content).toEqual([{ type: "text", text: Array.from({ length: 60 }, (_, i) => `${i + 1}\n`).join("") }]);
  const output = jobTools(fx.jobs, true, fx.commands).find(tool => tool.name === "job_output");
  if (!output) throw new Error("job_output missing");
  const peek = await output.execute("peek", { job: "j3-3" }, undefined, undefined, {} as never);
  const visible = peek.content.flatMap(part => part.type === "text" ? [part.text] : []).join("\n");
  expect(visible).toContain("59\n60\n");
  expect(peek.details).toMatchObject({ state: "exited", exit_code: 0 });
  const nested = await runChild({ tool: "job_output", key: "parent/cell-1.1",
    parent: { tool_call_id: "parent", cell: "cell-1", call: "cell-1.1" } },
  () => output.execute("parent/1", { job: "j3-3" }, undefined, undefined, {} as never));
  const nestedText = nested.content[0];
  if (nestedText?.type !== "text") throw new Error("nested output missing");
  expect(JSON.parse(nestedText.text)).toEqual(peek.details);
  expect(nested.structuredContent).toBeUndefined();
});

test("wait-next claims exactly one completion, while cancellation releases waiting without killing the command", async () => {
  const fx = fixture();
  const gate = fx.fifo("wait"), ready = fx.line("waiting");
  const started = await fx.commands.run(`echo waiting; cat ${gate} >/dev/null; echo done`, "bg", { wait_ms: 0 });
  const job = String(started.job);
  await ready;
  const abort = new AbortController();
  const cancelled = fx.commands.wait(job, 60000, abort.signal);
  abort.abort();
  expect(await cancelled).toMatchObject({ state: "wait_cancelled" });
  expect(fx.jobs.peek(job).state).toBe("running");
  const next = fx.commands.wait(undefined);
  await writeFile(gate, "go");
  expect(await next).toMatchObject({ job, state: "exited" });
  fx.commands.bound();
  await fx.commands.notified();
  expect(fx.notices).toEqual([]);
  expect(await fx.commands.wait(undefined, 0)).toMatchObject({ state: "none" });
  expect(() => fx.jobs.peek("j999-1")).toThrow("unknown job");
});

test("Background releases wait-next across its snapshot without consuming or stopping either job", async () => {
  const fx = fixture();
  const gates = [fx.fifo("next-a"), fx.fifo("next-b")] as const;
  const starts = await Promise.all(gates.map((gate, i) => fx.commands.run(`cat ${gate} >/dev/null; echo ${i}`, `next-${i}`, { wait_ms: 0 })));
  const waiting = fx.commands.wait();
  expect(fx.commands.state().every(job => job.observation === "attached")).toBeTrue();
  fx.commands.background(String(starts[1]?.job));
  await Promise.resolve();
  expect(fx.commands.state().every(job => job.observation === "background")).toBeTrue();
  expect(await waiting).toMatchObject({ state: "waiting" });
  expect(fx.jobs.state().every(job => job.state === "running")).toBeTrue();
  const next = fx.commands.wait();
  await writeFile(gates[0], "go");
  expect(await next).toMatchObject({ job: starts[0]?.job, state: "exited" });
  await writeFile(gates[1], "go");
  expect(await fx.commands.wait()).toMatchObject({ job: starts[1]?.job, state: "exited" });
});

test("runtime deadline starts at executable release, not during ownership acknowledgement", async () => {
  const entered = deferred<void>(), acknowledgement = deferred<void>();
  const schedules: { ms: number; run: () => void }[] = [];
  const fx = fixture({ started: async () => { entered.resolve(); await acknowledgement.promise; },
    schedule: (run, ms) => { schedules.push({ run, ms }); return () => {}; } });
  const gate = fx.fifo("deadline"), executed = fx.line("EXECUTED");
  const launch = fx.commands.run(`echo EXECUTED; cat ${gate} >/dev/null`, "delayed", { timeout: 10, wait_ms: 0 });
  await entered.promise;
  const before = schedules.length;
  const startedAt = fx.jobs.inspection()[0]?.startedAt;
  acknowledgement.resolve();
  const { job } = await launch;
  await executed;
  expect(startedAt).toBeUndefined();
  expect(before).toBe(0);
  expect(schedules.map(timer => timer.ms)).toEqual([10000]);
  schedules[0]?.run();
  await fx.jobs.settled(String(job));
  expect(fx.jobs.peek(String(job))).toMatchObject({ termination: "deadline", text: "EXECUTED\n", group_clear: true });
});

test("peeks page independent UTF-8 cursors, expose gaps, and show the rolling tail beyond the file cap", () => {
  const fx = fixture(), file = join(fx.dir, "output");
  writeFileSync(file, "prefix");
  const source = { id: "j3-1", file, bytes: 100000, stored: 6, tail: Buffer.from("A\n€\nZ\n") };
  expect(readOutput(source, {}).text).toBe("A\n€\nZ\n");
  const first = readOutput(source, { offset: 0, max_bytes: 4 });
  expect(first.text).toBe("pref");
  expect(readOutput(source, { offset: 0, max_bytes: 1 }).text).toBe("p");
  expect(readOutput(source, { cursor: String(first.cursor), max_bytes: 4 }).text).toBe("ix");
  expect(readOutput(source, { cursor: String(first.cursor), max_bytes: 4 }).text).toBe("ix");
  const gap = readOutput(source, { offset: 20 });
  expect(gap.gap).toEqual({ from: 20, to: source.bytes - source.tail.length });
  expect(gap.text).toBe("A\n€\nZ\n");
  expect(readOutput(source, { offset: source.bytes - source.tail.length + 2, max_bytes: 4 }).text).toBe("€\n");
  expect(readOutput(source, { tail_lines: 1 }).text).toBe("Z\n");
  expect(() => readOutput(source, { cursor: "j2-1:0" })).toThrow("foreign output cursor");
  expect(() => readOutput(source, { cursor: "j3-1:0", tail_lines: 1 })).toThrow("cannot be combined");
});

test("session monitors wake once for a line and exit; an explicit exit watch suppresses the automatic duplicate", async () => {
  const fx = fixture();
  fx.commands.bound();
  const gate = fx.fifo("monitor");
  const { job } = await fx.commands.run(`cat ${gate} >/dev/null; echo READY; echo READY`, "bg", { wait_ms: 0 });
  await fx.commands.monitor(String(job), "line", "^READY", 60, "line");
  await fx.commands.monitor(String(job), "exit", undefined, 60, "exit");
  await writeFile(gate, "go");
  await fx.jobs.settled(String(job));
  // Notify stubs settle on the next microtask; no real timers are involved.
  await Promise.resolve(); await Promise.resolve();
  expect(fx.notices).toHaveLength(2);
  expect(fx.notices.filter(text => text.includes("matched:"))).toHaveLength(1);
  expect(fx.notices.filter(text => text.includes("job exited"))).toHaveLength(1);
  expect(fx.monitors.filter(fact => fact.delivered)).toHaveLength(2);
});

test("already-ready completions coalesce without delaying another command or consuming peek output", async () => {
  const fx = fixture();
  const gates = [fx.fifo("a"), fx.fifo("b")];
  const starts = await Promise.all(gates.map((gate, index) => fx.commands.run(`cat ${gate} >/dev/null; echo result-${index}`, `batch-${index}`, { wait_ms: 0 })));
  await Promise.all(gates.map(gate => writeFile(gate, "go")));
  await Promise.all(starts.map(start => fx.jobs.settled(String(start.job))));
  fx.commands.bound();
  await fx.commands.notified();
  expect(fx.notices).toHaveLength(1);
  for (const start of starts) {
    expect(fx.notices[0]).toContain(`[command completed ${String(start.job)}]`);
    expect(fx.jobs.peek(String(start.job)).state).toBe("exited");
  }
});

test("recovery reports a lost process and pending monitor without replay; queued completion remains retryable", async () => {
  const fx = fixture();
  fx.commands.bound([
    { job: "j2-1", generation: 2, consumed: false },
    { job: "j2-2", generation: 2, consumed: true, delivery: "notice", result: { job: "j2-2", state: "exited", text: "done" } },
    { job: "j2-3", generation: 2, consumed: true, delivery: "result" },
  ], [{ key: "old-line", job: "j2-1", pattern: "READY", deadline: new Date(0).toISOString(), delivered: false }]);
  await fx.commands.notified();
  expect(fx.jobs.state()).toEqual([]);
  expect(fx.notices.filter(text => text.includes("not replayed"))).toHaveLength(2);
  expect(fx.notices.filter(text => text.includes("j2-2"))).toHaveLength(1);
  expect(fx.notices.some(text => text.includes("j2-3"))).toBeFalse();
  expect(fx.monitors.at(-1)?.delivered).toBeTrue();
});

test("inspector refuses stale/foreign/disabled selections; stop runs the shared guard once and uncertainty is never retried", async () => {
  const fx = fixture();
  const gate = fx.fifo("control"), ready = fx.line("alive");
  const { job } = await fx.commands.run(`echo alive; cat ${gate} >/dev/null`, "control", { wait_ms: 0 });
  await ready;
  let allowed = true, before = 0;
  const records: unknown[] = [];
  const control = jobControl({ jobs: fx.jobs, commands: fx.commands, generation: 3, session: () => "session",
    allowed: () => allowed, record: entry => records.push(entry),
    before: async () => { before++; return "denied by pre-tool hook"; } });
  const target = { job: String(job), session: "session", generation: 3 };
  expect(await control({ ...target, generation: 2 }, "stop")).toMatchObject({ state: "denied" });
  expect(await control({ ...target, job: "j3-foreign" }, "stop")).toMatchObject({ state: "denied" });
  allowed = false;
  expect(await control(target, "stop")).toMatchObject({ state: "denied" });
  allowed = true;
  expect(await control(target, "peek")).toMatchObject({ state: "running", text: "alive\n" });
  expect(await control(target, "stop")).toMatchObject({ state: "denied" });
  expect(before).toBe(1);
  expect(fx.jobs.peek(String(job)).state).toBe("running");
  const uncertain = jobControl({ jobs: fx.jobs, commands: fx.commands, generation: 3, session: () => "session",
    allowed: () => true, record: entry => records.push(entry), before: async () => { throw new Error("hook acknowledgement lost"); } });
  expect(await uncertain(target, "stop")).toMatchObject({ state: "outcome_unknown" });
  expect(fx.jobs.peek(String(job)).state).toBe("running");
  const authorized = jobControl({ jobs: fx.jobs, commands: fx.commands, generation: 3, session: () => "session",
    allowed: () => true, record: entry => records.push(entry) });
  expect(await authorized(target, "background")).toMatchObject({ state: "running", background: true });
  expect(fx.jobs.peek(String(job)).state).toBe("running");
  expect(await authorized(target, "stop")).toMatchObject({ state: "exited", termination: "cancelled", group_clear: true });
  expect(fx.jobs.peek(String(job)).text).toBe("alive\n");
});
