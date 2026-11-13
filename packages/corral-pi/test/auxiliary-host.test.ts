import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import { Text, TuiAltScreen, stripTerminalSequences } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { z } from "zod";
import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { requestBody } from "./fixtures/auth/fake-transport.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";
import { RecordingTerminal } from "./support/panel-terminal.ts";
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
    const requests: { model: string; service_tier?: string }[] = [];
    let holdOnRequest: number | undefined;
    const network = guardedFetch((input, init) => {
      requests.push(z.object({ model: z.string(), service_tier: z.string().optional() }).parse(requestBody(init)));
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
      guidance: { projectText: false, settings: { subagents: { max_parallel: 3, model: "gpt-6-luna", service_tier: "priority", max_turns: 12, timeout_secs: 30 },
        oracle: { model: "gpt-6.1-sol", timeout_secs: 30 } } },
      sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions")),
    });
    await host.session.bindExtensions({});
    await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
    expect(host.loadout).toContain("explore");
    expect(host.loadout).toContain("oracle");
    const workbench = host.panels?.workbench;
    if (!workbench) throw new Error("Native workbench missing");
    sdk.initTheme("dark", false);
    const tui = new TuiAltScreen(new RecordingTerminal());
    const root = workbench.mount(new Text("PARENT_TRANSCRIPT", 0, 0), () => ({ tui, outputPad: 1,
      markdownTheme: sdk.getMarkdownTheme(), markdownTransformers: [], renderers: name => sdk.withBuiltInRenderers(name, undefined) }));
    const screen = () => stripTerminalSequences(renderLayoutFrame(root, 80, 70, () => {}).lines.join("\n"));
    workbench.openAgents(); expect(screen()).toContain("No agents yet.");
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
      const reference = corral.calls().filter(call => call.envelope.payload.usage_kind === "oracle" && call.envelope.payload.action === "prepare").at(-1)?.envelope.payload.request;
      if (typeof reference !== "string") throw new Error("Oracle reference missing");
      const beforeInspection = requestCount;
      workbench.openAgent(`helper:${reference}`);
      expect(screen()).toContain("oracle advice"); expect(screen()).toContain("consult only");
      const exploreReference = corral.calls().filter(call => call.envelope.payload.usage_kind === "explore" && call.envelope.payload.action === "prepare").at(-1)?.envelope.payload.request;
      if (typeof exploreReference !== "string") throw new Error("Explore reference missing");
      workbench.openAgent(`helper:${exploreReference}`);
      expect(screen()).toContain("child findings"); expect(screen()).toContain("research only");
      // Ctrl+O expands the native tool cards, then folds them again for the next stage.
      workbench.handleInput("\x0f");
      expect(screen()).toContain("Tool write not found");
      workbench.handleInput("\x0f");
      expect(requestCount).toBe(beforeInspection); // Inspecting never invokes a model or resumes a child.
    }
    const researchRequests = requests.filter(request => request.model === "gpt-6-luna");
    expect(researchRequests.length).toBeGreaterThan(0);
    expect(researchRequests.filter(request => request.service_tier !== "priority")).toEqual([]);
    expect(requests.filter(request => request.model !== "gpt-6-luna").every(request => request.service_tier === undefined)).toBeTrue();
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
    expect(host.state().auxiliary).toEqual({ explore: { running: 0, queued: 0 }, oracle: { running: 0, queued: 0 },
      web_search: { running: 0, queued: 0 }, stopped: true });
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
