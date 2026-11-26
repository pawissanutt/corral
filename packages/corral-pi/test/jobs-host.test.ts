import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { EventLog } from "../src/bridge/events.ts";
import { type JobRuntime, startTimeOf } from "../src/extensions/jobs/runtime.ts";
import type { Commands } from "../src/extensions/jobs/commands.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession } from "../src/host/session.ts";
import { reviewerRole } from "../src/roles/reviewer/index.ts";
import { workerRole } from "../src/roles/worker/index.ts";
import type { CapabilitySet, RoleContext } from "../src/roles/types.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { fakeCodexTransport, requestBody } from "./fixtures/auth/fake-transport.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeEngine } from "./support/fake-engine.ts";
import { nextEvent } from "./support/wait.ts";

const capabilities: CapabilitySet = {
  catalog: 2, digest: "b".repeat(64), preset: "standard", enabled: ["background_shell"],
};
const guidance = {
  settings: { background_shell: { max_jobs: 2, max_output_bytes: 4096, max_runtime_secs: 60 } }, projectText: false,
};

test("a worker starts, peeks and kills managed jobs; a nested monitor wakes it once after exit", async () => {
  const assets = buildFixtureInstall();
  const cwd = mkdtempSync(join(tmpdir(), "corral-pi-jobs-host-"));
  const gate = join(cwd, "gate"), ready = join(cwd, "ready");
  try {
    restrictEnvironment(process.env);
    const sdk = await import("@earendil-works/pi-coding-agent");
    const modules = await loadModules(assets.dir, "worker");
    const transport = fakeCodexTransport();
    const declared: { name: string; description: string }[][] = [];
    const network = guardedFetch(async (input, init) => {
      type DeclaredTool = { name: string; description: string };
      const body = requestBody(init) as { tools?: DeclaredTool[]; input?: { tools?: DeclaredTool[] }[] };
      declared.push([...(body.tools ?? []), ...(body.input ?? []).flatMap(item => item.tools ?? [])]);
      return transport.send(input, init);
    }, undefined, "openai-codex");
    const role = workerRole(PACKAGE_ROOT);
    const nested: ToolDefinition = {
      name: "fixture_nested", label: "Nested monitor", description: "Exercises nested execution",
      parameters: Type.Object({ job: Type.String() }),
      async execute(_id, { job }: { job: string }, signal, _update, ctx) {
        await readFile(ready, "utf8");
        const outcome = await ctx.executeTool("corral_job_monitor", { job, on: "exit" }, { signal });
        expect(outcome.isError).toBeFalse();
        return outcome.result;
      },
    };
    const identity = { store: "st-demo", project: "demo", role: "worker" as const, task: "T-565", run: "R1" };
    const events = new EventLog();
    const engine = writeFakeEngine(cwd);
    const client = new CorralClient({ corral: engine.path, root: cwd, cwd, identity });
    const options = {
      sdk, identity, cwd, agentDir: join(cwd, "agent"), model: "gpt-6.1-sol", ui: "plain" as const,
      capabilities: { ...capabilities, enabled: [...capabilities.enabled, "codemode"] }, guidance, client, events,
      network, runtimeDir: join(cwd, "runtime"),
      sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions")),
    };
    const host = await HostSession.create({
      ...options,
      modules: { ...modules, role: {
        ...role, localToolNames: [...role.localToolNames ?? [], nested.name],
        localTools: (context: RoleContext) => [...role.localTools?.(context) ?? [], nested],
      } },
    });
    for (const fifo of [gate, ready]) expect(spawnSync("mkfifo", [fifo]).status).toBe(0);
    try {
      await host.session.bindExtensions({});
      await host.startLocal();
      expect(["job_start", "job_output", "job_kill", "corral_job_monitor"].every((name) => host.loadout.includes(name)))
        .toBeTrue();
      // Hold the jobs behind FIFOs until their returned IDs can be used by the next turn.
      transport.script([
        { toolCalls: [{ callId: "start", name: "job_start", arguments: { command: `cat "${gate}" >/dev/null; echo BUILT` } }] },
        { toolCalls: [{ callId: "start-b", name: "bash", arguments: {
          command: `echo ready > "${ready}"; exec sleep 30`, wait_ms: 0,
        } }] },
        { text: "Jobs launched" },
      ]);
      const ended = nextEvent(events, (event) => event.kind === "turn_ended");
      expect((await host.submitLocal({ text: "go", busyPolicy: "prompt", origin: "owner" })).disposition).toBe("started");
      await ended;
      await host.drained();
      expect(host.controlState(false).activity).toMatchObject({ settled: true, waiting: true });
      const result = (id: string) => host.session.messages.find(message => message.role === "toolResult" &&
        message.toolCallId.startsWith(`${id}|`));
      const ids = ["start", "start-b"].map(id => {
        const message = result(id);
        const details = message?.role === "toolResult" && !message.isError ? message.details : undefined;
        return details && typeof details === "object" && "job" in details ? details.job : undefined;
      });
      const [job, second] = ids;
      if (typeof job !== "string" || typeof second !== "string") {
        throw new Error(`job IDs missing: ${JSON.stringify([result("start"), result("start-b")])}`);
      }

      transport.script([
        { toolCalls: [{ callId: "peek", name: "job_output", arguments: { job } }] },
        { toolCalls: [{ callId: "nested", name: nested.name, arguments: { job } }] },
        { toolCalls: [{ callId: "kill", name: "job_kill", arguments: { job: second } }] },
        { text: "Waiting for the job" },
      ]);
      const armed = nextEvent(events, event => event.kind === "turn_ended");
      expect((await host.submitLocal({ text: "Watch the first job", busyPolicy: "prompt", origin: "owner" })).disposition).toBe("started");
      await armed;
      // The legacy scripted caller still runs, but neither provider schemas nor Code Mode advertise it.
      expect(declared.length).toBeGreaterThan(0);
      for (const tools of declared) {
        expect(tools.map(tool => tool.name)).not.toContain("job_start");
        expect(tools.map(tool => tool.name)).toContain("bash");
        const description = tools.find(tool => tool.name === "codemode")?.description ?? "";
        expect(description).toContain("bash");
        expect(description).not.toContain("job_start");
      }
      expect(result("peek")).toMatchObject({ isError: false, details: { job, state: "running" } });
      expect(result("kill")).toMatchObject({ isError: false, details: { job: second, state: "exited", signal: "SIGTERM" } });
      const monitor = result("nested");
      expect(monitor).toMatchObject({ isError: false, details: { state: "armed", job, on: "exit" } });
      const details = monitor?.role === "toolResult" ? monitor.details : undefined;
      if (!details || typeof details !== "object" || !("wait" in details) || !("deadline" in details)) {
        throw new Error("monitor result missing its wait or deadline");
      }
      expect(typeof details.wait).toBe("string");
      expect(typeof details.deadline === "string" && Number.isFinite(Date.parse(details.deadline))).toBeTrue();
      expect(host.state().jobs).toMatchObject([
        { job, state: "running" }, { job: second, state: "exited", group_clear: true },
      ]);

      const exited = nextEvent(events, event => event.kind === "job_exited" && event.job === job);
      const woken = nextEvent(events, event => event.kind === "turn_ended" && event.turn === 3);
      transport.script([{ text: "Build complete" }]);
      await writeFile(gate, "go\n");
      await exited;
      await woken;
      await host.drained();
      expect(host.controlState(false).activity).toMatchObject({ settled: true, waiting: false });
      expect(readFileSync(join(cwd, "runtime", "jobs", `${job}.log`), "utf8")).toBe("BUILT\n");
      const notices = host.session.messages.flatMap(message => message.role === "user"
        ? (typeof message.content === "string" ? [message.content] : message.content.flatMap(part => part.type === "text" ? [part.text] : []))
        : []).filter(text => text.includes("[command monitor") || text.includes("[command completed"));
      expect(notices).toHaveLength(1);
      expect(notices[0]).toContain(`[command monitor ${job}] job exited`);
      expect(host.state().jobs).toMatchObject([
        { job, state: "exited", exit_code: 0, group_clear: true }, { job: second, state: "exited" },
      ]);
    } finally {
      await host.close();
    }
    // And a reviewer launched with the capability is refused as a mismatched install.
    const reviewerModules = await loadModules(assets.dir, "reviewer");
    await expect(HostSession.create({
      ...options, identity: { ...identity, role: "reviewer" },
      modules: { ...reviewerModules, role: reviewerRole(PACKAGE_ROOT) },
      sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions-reviewer")),
    })).rejects.toThrow("module_invalid");
    // Without the capability, a worker has none of the managed job tools.
    const without = await HostSession.create({
      ...options,
      capabilities: { ...capabilities, enabled: [] }, modules: { ...modules, role },
      sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions-plain")),
    });
    try {
      expect(without.loadout.some((name) => name.startsWith("job_") || name === "corral_job_monitor")).toBeFalse();
    } finally {
      await without.close();
    }
  } finally {
    assets.cleanup();
    rmSync(cwd, { recursive: true, force: true });
  }
}, 120_000);

test("owner Stop escalates on the chosen job and retains output; host close clears the other group", async () => {
  const assets = buildFixtureInstall();
  const cwd = mkdtempSync(join(tmpdir(), "corral-pi-close-start-"));
  let host: HostSession | undefined;
  try {
    restrictEnvironment(process.env);
    const sdk = await import("@earendil-works/pi-coding-agent");
    const modules = await loadModules(assets.dir, "worker");
    const transport = fakeCodexTransport();
    const identity = { store: "st-demo", project: "demo", role: "worker" as const, task: "T-565", run: "R1" };
    const events = new EventLog();
    const engine = writeFakeEngine(cwd);
    const client = new CorralClient({ corral: engine.path, root: cwd, cwd, identity });
    host = await HostSession.create({ sdk, identity, cwd, agentDir: join(cwd, "agent"), model: "gpt-6.1-sol",
      capabilities, guidance,
      client, events, network: guardedFetch(transport.send, undefined, "openai-codex"),
      runtimeDir: join(cwd, "runtime"), modules: { ...modules, role: workerRole(PACKAGE_ROOT) },
      sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions")) });
    await host.session.bindExtensions({});
    await host.startLocal();
    const jobs = (host as unknown as { jobs: JobRuntime }).jobs;
    const ready = join(cwd, "ready");
    expect(spawnSync("mkfifo", [ready]).status).toBe(0);
    const normal = await jobs.start("sleep 30");
    const commands = (host as unknown as { commands: Commands }).commands;
    const stubborn = await commands.run(`trap '' TERM; echo retained; echo ready > "${ready}"; exec sleep 30`, { wait_ms: 0 });
    await readFile(ready, "utf8");
    host.installRemoteParts();
    const stop = () => host!.remoteRegistry.get("job_act")!({ job: stubborn.job, action: "stop" });
    transport.holdNext();
    const ended = nextEvent(events, event => event.kind === "turn_ended");
    try {
      expect((await host.submitLocal({ text: "Wait for the fixture", busyPolicy: "prompt", origin: "owner" })).disposition).toBe("started");
      await transport.whenHeld();
      const delivered = nextEvent(events, event => event.kind === "job_control" && event.phase === "started");
      const stopping = stop();
      expect(await stop()).toMatchObject({ state: "denied" });
      await delivered;
      // A concurrent runtime caller joins the same escalation rather than resetting grace.
      const duplicate = jobs.kill(String(stubborn.job));
      expect(await stopping).toMatchObject({ state: "stopped" });
      expect(await duplicate).toMatchObject({ state: "exited", stop_outcome: "stopped", group_clear: true });
      expect(transport.held).toBeTrue();
      expect(jobs.peek(String(stubborn.job))).toMatchObject({ state: "exited", signal: "SIGKILL", group_clear: true, text: "retained\n" });
      expect(jobs.peek(String(normal.job))).toMatchObject({ state: "running" });
    } finally { transport.release(); await ended; }
    await host.drained();
    expect(await stop()).toMatchObject({ state: "already_finished" });
    // Independently retain the host-close escalation regression after exercising owner Stop.
    const closeReady = join(cwd, "close-ready");
    expect(spawnSync("mkfifo", [closeReady]).status).toBe(0);
    const closeStubborn = await jobs.start(`trap '' TERM; echo ready > "${closeReady}"; exec sleep 30`);
    await readFile(closeReady, "utf8");
    await host.close();
    expect(jobs.peek(String(closeStubborn.job))).toMatchObject({ state: "exited", signal: "SIGKILL", group_clear: true });
    expect(await stop()).toMatchObject({ state: "denied" });
    for (const { pid } of [normal, stubborn, closeStubborn]) {
      if (typeof pid !== "number") throw new Error("job did not report its launcher PID");
      expect(startTimeOf(pid)).toBeUndefined();
    }
  } finally {
    await host?.close();
    assets.cleanup();
    rmSync(cwd, { recursive: true, force: true });
  }
}, 120_000);

test("background jobs obey bash rules before a command is started", async () => {
  // Given a worker with background_shell and bash rules that deny rm and ask before touch.
  const assets = buildFixtureInstall();
  const cwd = mkdtempSync(join(tmpdir(), "corral-pi-jobs-rules-"));
  try {
    restrictEnvironment(process.env);
    const sdk = await import("@earendil-works/pi-coding-agent");
    const modules = await loadModules(assets.dir, "worker");
    const transport = fakeCodexTransport();
    const identity = { store: "st-demo", project: "demo", role: "worker" as const, task: "T-565", run: "R1" };
    const events = new EventLog();
    const engine = writeFakeEngine(cwd);
    const client = new CorralClient({ corral: engine.path, root: cwd, cwd, identity });
    const host = await HostSession.create({
      sdk, identity, cwd, agentDir: join(cwd, "agent"), model: "gpt-6.1-sol", client, events,
      network: guardedFetch(transport.send, undefined, "openai-codex"), runtimeDir: join(cwd, "runtime"),
      capabilities: { ...capabilities, enabled: ["background_shell", "bash_rules"] },
      guidance: { projectText: false, settings: {
        ...guidance.settings, bash_rules: { allow: [], ask: ["touch asked*"], deny: ["rm *"], default: "allow" },
      } },
      modules: { ...modules, role: workerRole(PACKAGE_ROOT) },
      sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions")),
    });
    try {
      await host.session.bindExtensions({});
      await host.startLocal();
      // When the model starts a denied, an ask-gated and an allowed job.
      transport.script([
        { toolCalls: [{ callId: "deny", name: "job_start", arguments: { command: "rm notes.txt" } }] },
        { toolCalls: [{ callId: "ask", name: "job_start", arguments: { command: "touch asked.txt" } }] },
        { toolCalls: [{ callId: "allow", name: "bash", arguments: { command: "true" } }] },
        { text: "Done" },
      ]);
      const ended = nextEvent(events, (event) => event.kind === "turn_ended");
      const exited = nextEvent(events, (event) => event.kind === "job_exited");
      expect((await host.submitLocal({ text: "go", busyPolicy: "prompt", origin: "owner" })).disposition).toBe("started");
      await ended;
      await exited;
      await host.drained();
      // Only the allowed command acquired a job, and the confirmation is visible to the owner.
      const results = host.session.messages.flatMap(message => message.role === "toolResult" ? [message] : []);
      const denied = results.find(message => message.toolCallId.startsWith("deny|"));
      expect(denied).toMatchObject({ isError: true, content: [
        { type: "text", text: 'bash rule "rm *" denies "rm notes.txt"; nothing ran' },
      ] });
      expect(results.find(message => message.toolCallId.startsWith("ask|"))?.isError).toBeTrue();
      expect(results.find(message => message.toolCallId.startsWith("allow|"))?.isError).toBeFalse();
      expect(host.pendingOwnerQuestions().map(question => question.options)).toEqual([["allow", "deny"]]);
      expect(host.state().jobs).toMatchObject([{ state: "exited", exit_code: 0 }]);
    } finally {
      await host.close();
    }
  } finally {
    assets.cleanup();
    rmSync(cwd, { recursive: true, force: true });
  }
}, 120_000);
