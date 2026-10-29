import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { reviewerRole } from "../src/roles/reviewer/index.ts";
import { workerRole } from "../src/roles/worker/index.ts";
import type { CapabilitySet, RoleContext } from "../src/roles/types.ts";
import { CorralClient, sha256Hex, toolRequestId } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";

const capabilities: CapabilitySet = {
  catalog: 2, digest: "b".repeat(64), preset: "standard", enabled: ["background_shell"],
};
const guidance = {
  settings: { background_shell: { max_jobs: 2, max_output_bytes: 4096, max_runtime_secs: 60 } }, projectText: false,
};

test("background_shell gives only the worker job tools, and the monitor runs only at top level", async () => {
  // Given a worker host with background_shell and a nested tool that tries the monitor.
  const assets = buildFixtureInstall();
  const cwd = mkdtempSync(join(tmpdir(), "corral-pi-jobs-host-"));
  try {
    restrictEnvironment(process.env);
    const sdk = await import("@earendil-works/pi-coding-agent");
    const modules = await loadModules(assets.dir, "worker");
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("fixture transport missing");
    const network = guardedFetch(transport, undefined, "openai-codex");
    const role = workerRole(PACKAGE_ROOT);
    const nested: ToolDefinition = {
      name: "fixture_nested", label: "Nested monitor", description: "Exercises nested execution",
      parameters: Type.Object({}),
      async execute(_id, _params, signal, _update, ctx) {
        return (await ctx.executeTool("corral_job_monitor", { job: "j1-1", on: "exit" }, { signal })).result;
      },
    };
    const identity = { store: "st-demo", project: "demo", role: "worker" as const, generation: 1, task: "T-565", run: "R1" };
    const events = new EventLog(1);
    const corral = writeFakeCorral(cwd);
    let host: HostSession | undefined;
    const client = new CorralClient({
      corral: corral.path, root: cwd, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "",
    });
    const options = {
      sdk, identity, cwd, agentDir: join(cwd, "agent"), model: "gpt-6.1-sol", capabilities, guidance, client, events,
      network, runtimeDir: join(cwd, "runtime"),
      sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions")),
    };
    host = await HostSession.create({
      ...options,
      modules: { ...modules, role: {
        ...role, localToolNames: [...role.localToolNames ?? [], nested.name],
        localTools: (context: RoleContext) => [...role.localTools?.(context) ?? [], nested],
      } },
    });
    const gate = join(cwd, "gate");
    expect(spawnSync("mkfifo", [gate]).status).toBe(0);
    try {
      await host.session.bindExtensions({});
      await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
      expect(["job_start", "job_output", "job_kill", "corral_job_monitor"].every((name) => host?.loadout.includes(name)))
        .toBeTrue();
      // When the model starts a job, tries the monitor nested, then registers it directly.
      fixtureTransport().script([
        { toolCalls: [{ callId: "start", name: "job_start", arguments: { command: `cat ${gate} >/dev/null` } }] },
        { toolCalls: [{ callId: "nested", name: nested.name, arguments: {} }] },
        { toolCalls: [{ callId: "monitor", name: "corral_job_monitor", arguments: { job: "j1-1", on: "exit" } }] },
        { text: "Waiting for the job" },
      ]);
      const ended = nextEvent(events, (event) => event.kind === "turn_ended");
      expect((await host.submit({ text: "go", busyPolicy: "prompt", idempotencyKey: "go", origin: "test" }, "rq-go"))
        .disposition).toBe("started");
      await ended;
      await host.drained();
      const exited = nextEvent(events, (event) => event.kind === "job_exited");
      await writeFile(gate, "go\n");
      await exited;
      // Then the nested attempt never reached corral, the direct monitor carried its tool-call request ID, and
      // the job was recorded before and after it ran.
      const jobCalls = corral.calls().map((call) => call.envelope).filter((e) => e.op === "job" || e.op === "job_monitor");
      expect(jobCalls.map((e) => e.op === "job" ? e.payload.action : e.op))
        .toEqual(["prepare", "started", "job_monitor", "exited"]);
      const sessionId = host.sessionId();
      const callId = (name: string) => host?.session.messages.flatMap((message) =>
        message.role === "assistant" ? message.content.filter((part) => part.type === "toolCall") : [])
        .find((part) => part.name === name)?.id ?? "";
      expect(jobCalls[2]?.request_id).toBe(toolRequestId(sessionId, callId("corral_job_monitor")));
      expect(jobCalls[0]?.request_id).toBe(toolRequestId(sessionId, `${callId("job_start")}/prepare`));
      expect(events.since(0, 64 * 1024).events
        .some((event) => event.kind === "tool_denied" && event.tool === "corral_job_monitor" && event.nested === true))
        .toBeTrue();
      expect((host.state().jobs as { job: string; state: string }[]).map((job) => [job.job, job.state]))
        .toEqual([["j1-1", "exited"]]);
    } finally {
      host.session.dispose();
    }
    // And a reviewer launched with the capability is refused as a mismatched install.
    const reviewerModules = await loadModules(assets.dir, "reviewer");
    await expect(HostSession.create({
      ...options, identity: { ...identity, role: "reviewer" },
      modules: { ...reviewerModules, role: reviewerRole(PACKAGE_ROOT) },
      sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions-reviewer")),
    })).rejects.toThrow("module_invalid");
    // And a worker module that allows no job ops (a fixture role) starts without the job tools.
    const without = await HostSession.create({
      ...options,
      modules: { ...modules, role: { ...role, corralOps: role.corralOps.filter((op) => op !== "job" && op !== "job_monitor") } },
      sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions-plain")),
    });
    try {
      expect(without.loadout.some((name) => name.startsWith("job_") || name === "corral_job_monitor")).toBeFalse();
    } finally {
      without.session.dispose();
    }
  } finally {
    assets.cleanup();
    rmSync(cwd, { recursive: true, force: true });
  }
}, 120_000);
