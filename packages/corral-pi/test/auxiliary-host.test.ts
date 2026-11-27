import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
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
import { CorralClient } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { requestBody } from "./fixtures/auth/fake-transport.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeEngine } from "./support/fake-engine.ts";
import { nextEvent } from "./support/wait.ts";
import { RecordingTerminal } from "./support/panel-terminal.ts";
import { workerRole } from "../src/roles/worker/index.ts";
import { PACKAGE_ROOT } from "./support/fixture-install.ts";
import type { FleetSnapshot } from "../src/build/types.ts";

test("an old full snapshot cannot run oracle; bound explore works across reload", async () => {
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
    const identity = { store: "st-demo", project: "demo", role: "worker" as const, task: "T-571", run: "R1" };
    const events = new EventLog();
    const corral = writeFakeEngine(cwd);
    const client = new CorralClient({ corral: corral.path, root: cwd, cwd, identity });
    host = await HostSession.create({ sdk, identity, cwd, agentDir: join(cwd, "agent"), model: "gpt-6.1-sol", modules: { ...modules, role: workerRole(PACKAGE_ROOT) },
      client, events, network, capabilities: { catalog: 7, digest: "a".repeat(64), preset: "full", enabled: ["subagents", "oracle"] },
      guidance: { projectText: false, settings: { subagents: { max_parallel: 3, model: "gpt-6-luna", service_tier: "priority", max_turns: 12, timeout_secs: 30 },
        oracle: { model: "gpt-6.1-sol", timeout_secs: 30 } } },
      sessionDir: join(cwd, "agent"), sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions")),
    });
    await host.session.bindExtensions({});
    await host.startLocal();
    expect(host.loadout).toContain("explore");
    expect(host.loadout).not.toContain("oracle");
    const workbench = host.panels?.workbench;
    if (!workbench) throw new Error("Native workbench missing");
    sdk.initTheme("dark", false);
    const tui = new TuiAltScreen(new RecordingTerminal());
    const root = workbench.mount(new Text("PARENT_TRANSCRIPT", 0, 0), () => ({ tui, outputPad: 1,
      markdownTheme: sdk.getMarkdownTheme(), markdownTransformers: [], renderers: name => sdk.withBuiltInRenderers(name, undefined) }));
    const screen = () => stripTerminalSequences(renderLayoutFrame(root, 80, 70, () => {}).lines.join("\n"));
    let previousExplore: string | undefined;
    for (const stage of ["startup", "reload"]) {
      if (stage === "reload") {
        await host.session.reload();
        expect((host.state().fleet as FleetSnapshot | null)?.children.some(child => child.call === previousExplore)).toBeTrue();
      }
      expect(host.session.agent.state.tools.map(tool => tool.name)).not.toContain("oracle");
      fixtureTransport().script([
        { toolCalls: [{ callId: `${stage}-explore`, name: "explore", arguments: { prompt: "research only" } }] },
        { toolCalls: [{ callId: "write", name: "write", arguments: { path: "bad", content: "bad" } }] },
        { toolCalls: [{ callId: "recurse", name: "explore", arguments: { prompt: "no" } }] },
        { text: "child findings" },
        { toolCalls: [{ callId: `${stage}-oracle`, name: "oracle", arguments: { prompt: "consult only" } }] },
        { text: "parent finished" },
      ]);
      const ended = nextEvent(events, (e) => e.kind === "turn_ended");
      expect((await host.submitLocal({ text: stage, busyPolicy: "prompt", origin: "owner" })).disposition).toBe("started");
      await ended;
      await host.drained();
      const results = host.session.messages.filter((m) => m.role === "toolResult" && m.toolCallId.startsWith(`${stage}-`));
      expect(results.map((m) => m.role === "toolResult" && m.isError)).toEqual([false, true]);
      expect(results[1]).toMatchObject({ role: "toolResult", toolName: "oracle", content: [{ type: "text", text: "Tool oracle not found" }] });
      const prepared = events.since(0, 64 * 1024).events.filter(event => event.kind === "auxiliary" && event.action === "prepare");
      const beforeInspection = requestCount;
      const exploreReference = prepared.findLast(event => event.usage_kind === "explore")?.request;
      if (typeof exploreReference !== "string") throw new Error("Explore reference missing");
      const fleet = host.state().fleet as FleetSnapshot | null;
      const researchResult = results.find(message => message.role === "toolResult" && message.toolName === "explore");
      if (researchResult?.role !== "toolResult") throw new Error("Explore tool result missing");
      const explore = fleet?.children.find(child => child.call === researchResult.toolCallId);
      previousExplore = researchResult.toolCallId;
      expect(explore).toMatchObject({ kind: "explore", state: "done", brief: "research only", report: "child findings" });
      const transcript = join(cwd, "agent", "auxiliary", `${exploreReference}.jsonl`);
      expect(explore?.sessionFile).toBe(transcript);
      expect(existsSync(transcript)).toBeTrue();
      const file = host.session.sessionFile;
      if (!file) throw new Error("Parent Pi transcript missing");
      const recorded = sdk.SessionManager.open(file).getBranch().flatMap(entry => entry.type === "message"
        && entry.message.role === "toolResult" && entry.message.toolCallId.startsWith(`${stage}-`) ? [entry.message.details] : []);
      expect(recorded[0]).toMatchObject({ conversationId: explore?.id });
      expect(explore?.messages.some(message => message.text.includes("child findings"))).toBeTrue();
      expect(explore?.messages.some(message => message.tool === "write" && message.text.includes("Tool write not found"))).toBeTrue();
      workbench.openChild(explore!.id);
      expect(screen()).toContain("child findings");
      expect(requestCount).toBe(beforeInspection); // Inspecting never invokes a model or resumes a child.
    }
    const researchRequests = requests.filter(request => request.model === "gpt-6-luna");
    expect(researchRequests.length).toBeGreaterThan(0);
    expect(researchRequests.filter(request => request.service_tier !== "priority")).toEqual([]);
    expect(requests.filter(request => request.model !== "gpt-6-luna").every(request => request.service_tier === undefined)).toBeTrue();
    const lifecycle = events.since(0, 64 * 1024).events.filter(event => event.kind === "auxiliary");
    expect(lifecycle).toHaveLength(6);
    expect(lifecycle.every(event => event.usage_kind === "explore")).toBeTrue();
    expect(new Set(lifecycle.map(event => event.request)).size).toBe(2);
    expect(lifecycle.filter(event => event.action === "finish").map(event => event.outcome))
      .toEqual(["completed", "completed"]);
    // Fence/stop while the third child is actually streaming. Subscribe before the exact
    // provider barrier; final close aborts the parent too, so no follow-up request escapes.
    const before = requestCount;
    holdOnRequest = before + 2;
    fixtureTransport().script([
      { toolCalls: [{ callId: "fence-explore", name: "explore", arguments: { prompt: "hold child response" } }] },
      { text: "partial child findings" },
    ]);
    const held = fixtureTransport().whenHeld();
    expect((await host.submitLocal({ text: "hold child", busyPolicy: "prompt", origin: "owner" })).disposition).toBe("started");
    await held;
    await host.close();
    expect(requestCount - before).toBe(2);
    expect(host.state().auxiliary).toEqual({ explore: { running: 0, queued: 0 },
      web_search: { running: 0, queued: 0 }, stopped: true });
    expect((host.state().fleet as FleetSnapshot | null)?.children.every(child => !["running", "queued", "needs"].includes(child.state))).toBeTrue();
    const terminal = events.since(0, 64 * 1024).events.findLast(event => event.kind === "auxiliary" && event.action === "finish");
    expect(terminal).toMatchObject({ usage_kind: "explore", outcome: "uncertain" });
    await host.drained();
    expect(corral.calls().filter(argv => argv[3] === "usage" && argv[4] === "add")).toHaveLength(requestCount - 1);
    host = undefined;
  } finally {
    await host?.close(); globalThis.fetch = originalFetch; assets.cleanup(); rmSync(cwd, { recursive: true, force: true });
  }
}, 30_000);

test("bound worker task slices finish their local lifecycle before collection", async () => {
  const assets = buildFixtureInstall();
  const repo = mkdtempSync(join(tmpdir(), "corral-slice-host-"));
  const cwd = join(repo, ".worktrees", "T-571-r2");
  mkdirSync(cwd, { recursive: true });
  const originalFetch = globalThis.fetch;
  let host: HostSession | undefined;
  let unsubscribe: (() => void) | undefined;
  const collecting = Promise.withResolvers<void>();
  try {
    const modules = await loadModules(assets.dir, "worker");
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("fixture transport missing");
    const network = guardedFetch(transport);
    globalThis.fetch = network.fetch;
    const identity = { store: "st-demo", project: "demo", role: "worker" as const, task: "T-571", run: "R2" };
    const events = new EventLog();
    const client = new CorralClient({ corral: writeFakeEngine(repo).path, root: repo, cwd, identity });
    host = await HostSession.create({ sdk, identity, cwd, agentDir: join(cwd, "agent"), model: "gpt-6.1-sol",
      childProfiles: [{ name: "fixture", model: "openai-codex/gpt-6.1-sol" }],
      modules: { ...modules, role: workerRole(PACKAGE_ROOT) }, ui: "plain", client, events, network,
      capabilities: { catalog: 7, digest: "a".repeat(64), preset: "full", enabled: ["fleet"] },
      guidance: { projectText: false, settings: {} },
      sessionManager: sdk.SessionManager.create(cwd, join(cwd, "sessions")),
    });
    await host.session.bindExtensions({});
    await host.startLocal();
    expect(host.loadout).toContain("task");
    unsubscribe = host.session.subscribe(event => {
      if (event.type === "tool_execution_start" && event.toolName === "agent_wait") collecting.resolve();
    });
    fixtureTransport().script([
      { match: "slice lifecycle", toolCalls: [{ callId: "slice-task", name: "task", arguments: {
        agents: [{ id: "slice-one", kind: "slice", title: "One slice", brief: "slice child brief", owns: ["owned.txt"], profile: "fixture" }],
      } }] },
      { match: "slice lifecycle", toolCalls: [{ callId: "slice-wait", name: "agent_wait", arguments: { ids: ["slice-one"] } }] },
      { match: "slice child brief", toolCalls: [{ callId: "slice-write", name: "write", arguments: {
        path: "owned.txt", content: "written from the worker's rerun checkout",
      } }] },
      { match: "slice child brief", chunks: ["slice ", "completed"], afterFirstChunk: () => collecting.promise },
      { match: "slice lifecycle", text: "parent finished" },
    ]);
    const ended = nextEvent(events, event => event.kind === "turn_ended");
    expect((await host.submitLocal({ text: "slice lifecycle", busyPolicy: "prompt", origin: "owner" })).disposition).toBe("started");
    await ended;
    await host.drained();
    const records = events.since(0, 64 * 1024).events.filter(event => event.kind === "auxiliary");
    expect(records.map(event => event.action)).toEqual(["prepare", "started", "finish"]);
    expect(records.every(event => event.usage_kind === "slice")).toBeTrue();
    expect(new Set(records.map(event => event.request)).size).toBe(1);
    expect(records.at(-1)?.outcome).toBe("completed");
    const request = records[0]?.request;
    if (typeof request !== "string") throw new Error("Slice reference missing");
    const transcript = join(cwd, "agent", "auxiliary", `${request}.jsonl`);
    expect(existsSync(transcript)).toBeTrue();
    expect((host.state().fleet as FleetSnapshot | null)?.children).toMatchObject([
      { id: "slice-one", kind: "slice", state: "done", report: "slice completed", sessionFile: transcript },
    ]);
    const results = host.session.messages.filter(message => message.role === "toolResult" && message.toolCallId.startsWith("slice-"));
    expect(results.map(message => message.role === "toolResult" && message.isError)).toEqual([false, false]);
    expect(readFileSync(join(cwd, "owned.txt"), "utf8")).toBe("written from the worker's rerun checkout");
  } finally {
    collecting.resolve(); unsubscribe?.(); await host?.close();
    globalThis.fetch = originalFetch; assets.cleanup(); rmSync(repo, { recursive: true, force: true });
  }
}, 30_000);
