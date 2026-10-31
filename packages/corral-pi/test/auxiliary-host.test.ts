import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";
import { workerRole } from "../src/roles/worker/index.ts";
import { PACKAGE_ROOT } from "./support/fixture-install.ts";

test("host wires explore/oracle to bound runtime and keeps child restrictions after reload", async () => {
  const assets = buildFixtureInstall();
  const cwd = mkdtempSync(join(tmpdir(), "corral-aux-host-"));
  const originalFetch = globalThis.fetch;
  let host: HostSession | undefined;
  try {
    const modules = await loadModules(assets.dir, "worker");
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("fixture transport missing");
    let requestCount = 0;
    let holdOnRequest: number | undefined;
    const network = guardedFetch((input, init) => {
      if (++requestCount === holdOnRequest) fixtureTransport().holdNext();
      return transport(input, init);
    });
    globalThis.fetch = network.fetch;
    const identity = { store: "st-demo", project: "demo", role: "worker" as const, generation: 1, task: "T-571", run: "R1" };
    const events = new EventLog(1);
    const corral = writeFakeCorral(cwd);
    // The real operation's path-selection/admission/replay are tested in Rust; this fake supplies
    // canonical-shaped references while exercising the complete SDK host/child wiring offline.
    writeFileSync(corral.path, readFileSync(corral.path, "utf8").replace("result: { op: envelope.op,", `result: { op: envelope.op,
      ...(envelope.payload.kind === "auxiliary" ? { session_id: envelope.payload.request,
        session_file: join(dir, envelope.payload.request + ".jsonl"), generation: 1 } : {}),`));
    const client = new CorralClient({ corral: corral.path, root: cwd, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
    host = await HostSession.create({ sdk, identity, cwd, agentDir: join(cwd, "agent"), model: "gpt-6.1-sol", modules: { ...modules, role: workerRole(PACKAGE_ROOT) },
      client, events, network, capabilities: { catalog: 7, digest: "a".repeat(64), preset: "full", enabled: ["subagents", "oracle"] },
      guidance: { projectText: false, settings: { subagents: { max_parallel: 3, model: null, max_turns: 12, timeout_secs: 30 },
        oracle: { model: "gpt-6.1-sol", timeout_secs: 30 } } },
      sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions")),
    });
    await host.session.bindExtensions({});
    await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
    expect(host.loadout).toContain("explore");
    expect(host.loadout).toContain("oracle");
    for (const stage of ["startup", "reload"]) {
      if (stage === "reload") await host.session.reload();
      fixtureTransport().script([
        { toolCalls: [{ callId: `${stage}-explore`, name: "explore", arguments: { prompt: "research only" } }] },
        { toolCalls: [{ callId: "write", name: "write", arguments: { path: "bad", content: "bad" } }] },
        { toolCalls: [{ callId: "recurse", name: "explore", arguments: { prompt: "no" } }] },
        { text: "child findings" },
        { toolCalls: [{ callId: `${stage}-oracle`, name: "oracle", arguments: { prompt: "consult only" } }] },
        { text: "oracle advice" },
        { text: "parent finished" },
      ]);
      const ended = nextEvent(events, (e) => e.kind === "turn_ended");
      await host.submit({ text: stage, busyPolicy: "prompt", idempotencyKey: stage, origin: "test" }, `rq-${stage}`);
      await ended;
      await host.drained();
      const results = host.session.messages.filter((m) => m.role === "toolResult" && m.toolCallId.startsWith(`${stage}-`));
      expect(results.map((m) => m.role === "toolResult" && m.isError)).toEqual([false, false]);
    }
    const calls = corral.calls().filter((c) => c.envelope.payload.kind === "auxiliary");
    expect(calls).toHaveLength(12);
    expect(new Set(calls.map((c) => c.envelope.payload.request)).size).toBe(4);
    expect(calls.every((c) => c.envelope.identity.generation === 1)).toBeTrue();
    expect(calls.filter((c) => c.envelope.payload.action === "finish").every((c) => c.envelope.payload.outcome === "completed")).toBeTrue();
    // Fence/stop while the fifth child is actually streaming. Subscribe before the exact
    // provider barrier; final close aborts the parent too, so no follow-up request escapes.
    const before = requestCount;
    holdOnRequest = before + 2;
    fixtureTransport().script([
      { toolCalls: [{ callId: "fence-explore", name: "explore", arguments: { prompt: "hold child response" } }] },
      { text: "partial child findings" },
    ]);
    const held = fixtureTransport().whenHeld();
    await host.submit({ text: "fence", busyPolicy: "prompt", idempotencyKey: "fence", origin: "test" }, "rq-fence");
    await held;
    await host.close();
    expect(requestCount - before).toBe(2);
    expect(host.state().auxiliary).toEqual({ explore: { running: 0, queued: 0 }, oracle: { running: 0, queued: 0 }, stopped: true });
    const terminal = corral.calls().filter((c) => c.envelope.payload.kind === "auxiliary" && c.envelope.payload.action === "finish");
    expect(terminal.at(-1)?.envelope.payload.outcome).toBe("uncertain");
    const budget = corral.calls().filter((c) => c.envelope.payload.kind === "budget");
    expect(budget.filter((c) => c.envelope.payload.action === "reserve")).toHaveLength(requestCount);
    expect(budget.filter((c) => c.envelope.payload.action === "settle")).toHaveLength(requestCount - 1);
    host = undefined;
    console.log("host: explore/oracle and read-only child guard survive reload; parent stop cancels streaming child without follow-up; generation-linked terminal records");
  } finally {
    await host?.close(); globalThis.fetch = originalFetch; assets.cleanup(); rmSync(cwd, { recursive: true, force: true });
  }
}, 30_000);
