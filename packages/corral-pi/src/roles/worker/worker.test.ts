// Production worker loadout, local tools and engine commands with the offline provider.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../../bridge/events.ts";
import { loadModules } from "../../host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../../host/restrict.ts";
import { HostSession } from "../../host/session.ts";
import { CorralClient } from "../../tools/corral.ts";
import type { ChildSnapshot, FleetSnapshot } from "../../build/types.ts";
import { doneTool, workerDoneTool } from "./tools.ts";
import { fakeCodexTransport } from "../../../test/fixtures/auth/fake-transport.ts";
const provider = fakeCodexTransport();
import { type FakeEngine, writeFakeEngine } from "../../../test/support/fake-engine.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "../../../test/support/fixture-install.ts";
import { expectReplacementsRefused } from "../../../test/support/replacement.ts";
import { nextEvent, within } from "../../../test/support/wait.ts";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import worker, { CONTRACT_ASSET, SKILL_ASSET, kickoff, workerRole } from "./index.ts";

const identity = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const, task: "T-007", run: "R2" };
const UPSTREAM = ["read", "edit", "write", "bash", "grep", "find", "ls"];
const CORRAL_TOOLS = ["corral_checkpoint", "corral_artifact", "corral_done"];
const LSP_TOOLS = ["lsp_diagnostics", "lsp_definition", "lsp_references"];
const originalFetch = globalThis.fetch;

let assets: FixtureInstall;
let work: string;
let corral: FakeEngine;
let events: EventLog;
let host: HostSession;

/** A fixture installation with the offline provider and production worker and assets. */
function realWorkerInstall(): FixtureInstall {
  const built = buildFixtureInstall();
  rmSync(join(built.dir, "src/roles/worker"), { recursive: true, force: true });
  for (const tree of ["src/roles/worker", "src/roles/common", "assets"]) {
    cpSync(join(PACKAGE_ROOT, tree), join(built.dir, tree), { recursive: true });
  }
  return built;
}

beforeAll(async () => {
  assets = realWorkerInstall();
  work = mkdtempSync(join(tmpdir(), "corral-pi-worker-"));
  const cwd = join(work, "worktree");
  mkdirSync(join(cwd, ".pi/skills/corral-worker"), { recursive: true });
  writeFileSync(join(cwd, ".pi/skills/corral-worker/SKILL.md"), "---\nname: corral-worker\ndescription: worktree copy\n---\nLeak.\n");
  restrictEnvironment(process.env);
  const modules = await loadModules(assets.dir, "worker");
  events = new EventLog();
  const network = guardedFetch(provider.send);
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  corral = writeFakeEngine(work);
  const client = new CorralClient({ corral: corral.path, root: work, cwd, identity });
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(work, "pi-agent"), model: "gpt-6.1-sol", modules, client, events, network,
    sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
    capabilities: { catalog: 5, enabled: ["ask_user", "todo", "lsp"] },
  });
  // InteractiveMode binds extensions before its first frame; that is when resources_discover runs.
  await host.session.bindExtensions({});
  await host.startLocal();
}, 60_000);

afterAll(async () => {
  await host?.close();
  globalThis.fetch = originalFetch;
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
});

test("the worker loadout exposes only its task and local tools", () => {
  expect([...worker.upstreamTools]).toEqual(UPSTREAM);
  const context = { identity, cwd: "/tmp", model: "m" };
  const tools = worker.tools(context);
  expect(tools.map((tool) => tool.name)).toEqual(CORRAL_TOOLS);
  expect(tools.map((tool) => tool.op)).toEqual(["checkpoint", "artifact", "done"]);
  expect(worker.tools({ ...context, todo: true }).map((tool) => tool.name)).toEqual([...CORRAL_TOOLS, "corral_todo"]);
});

test("the contract is the installed worker-contract.md with the bound identity filled in", () => {
  const template = readFileSync(join(PACKAGE_ROOT, CONTRACT_ASSET), "utf8");
  const contract = worker.contract({ identity, cwd: "/tmp", model: "m" });
  expect(contract).toContain("corral task T-007, run R2, in project demo");
  expect(contract).not.toContain("{{");
  expect(contract.length).toBeGreaterThan(template.length / 2);
  const empty = mkdtempSync(join(tmpdir(), "corral-pi-noassets-"));
  try {
    expect(() => workerRole(empty).contract({ identity, cwd: "/tmp", model: "m" })).toThrow(`role_asset_missing: ${CONTRACT_ASSET}`);
  } finally {
    rmSync(empty, { recursive: true, force: true });
  }
});

test("a host session activates exactly the loadout and loads the installed worker and plan skills", () => {
  const state = host.state();
  const expected = [...UPSTREAM, ...CORRAL_TOOLS, "corral_todo", ...LSP_TOOLS, "ask_user", "agent_message", "agent_roster"];
  expect([...host.loadout]).toEqual(expected);
  expect([...(state.active_tools as string[])].sort()).toEqual(expected.sort());
  const skills = host.runtime.services.resourceLoader.getSkills().skills;
  expect(skills.map((skill) => [skill.name, skill.filePath])).toEqual([
    ["corral-worker", join(assets.dir, SKILL_ASSET)],
    ["corral-plan", join(assets.dir, "assets/skills/corral-plan/SKILL.md")],
  ]);
  expect(readFileSync(skills[0]?.filePath ?? "", "utf8")).toBe(readFileSync(join(PACKAGE_ROOT, SKILL_ASSET), "utf8"));
  expect(host.session.systemPrompt).toContain("# corral worker contract (corral-pi)");
});

test("worker todo and resume notes stay local; artifact and completion use bound engine argv", async () => {
  const summary = "Fixed the parser.\nVerified: focused tests\nNot verified: full suite";
  const note = "Parser fixed; re-run the frontend check if the input grammar changes.";
  const call = (name: string, arguments_: Record<string, unknown>, id = name) =>
    ({ toolCalls: [{ callId: id, name, arguments: arguments_ }] });
  provider.script([
    call("corral_todo", { op: "init", phases: [{ id: "build", label: "Build", steps: [
      { id: "fix", label: "Fix parser", criteria: ["C1"] }, { id: "extra", label: "Optional check" },
    ] }] }, "todo-init"),
    call("corral_todo", { op: "start", step: "fix" }, "todo-start"),
    call("corral_todo", { op: "done", step: "fix" }, "todo-done"),
    call("corral_todo", { op: "drop", step: "extra", reason: "Covered by the focused test" }, "todo-drop"),
    call("corral_checkpoint", { note }),
    call("corral_artifact", { path: ".evidence/test output.log" }),
    call("corral_done", { summary }),
  ]);
  const ended = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 1);
  expect(await host.submitLocal({ text: "Record your work, attach the output, then finish.", busyPolicy: "prompt", origin: "owner" }))
    .toEqual({ disposition: "started", turn: 1 });
  await ended;
  await host.drained();
  expect(host.state().last_error).toBeNull();
  const calls = corral.calls().filter((argv) => ["artifact", "done"].includes(argv[3] ?? ""));
  expect(calls).toEqual([
    ["--root", work, "--json", "artifact", "add", identity.task, ".evidence/test output.log"],
    ["--root", work, "--json", "done", identity.task, "--summary", summary],
  ]);
  const todo = host.session.messages.findLast((message) => message.role === "toolResult" && message.toolName === "corral_todo");
  expect(todo).toMatchObject({ isError: false, details: { progress: { revision: 4, phases: [{ steps: [
    { criteria: ["C1"], status: { state: "done" } },
    { status: { state: "dropped", reason: "Covered by the focused test" } },
  ] }] } } });
  const checkpoint = host.session.sessionManager.getEntries().find((entry) =>
    entry.type === "custom" && entry.customType === "corral_checkpoint_note");
  expect(checkpoint).toMatchObject({ data: { turn: 1, note } });
  expect(corral.calls().some((argv) => argv.includes("todo") || argv.includes("checkpoint"))).toBeFalse();
});

test("an owner steer joins the existing bridge turn and the next pane input gets its own turn", async () => {
  const transport = provider;
  transport.holdNext();
  const held = transport.whenHeld();
  const bridgeEnded = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 2);
  try {
    expect(await host.submitLocal({ text: "Reply only BRIDGE.", busyPolicy: "prompt", origin: "owner" }))
      .toEqual({ disposition: "started", turn: 2 });
    await within(held, 30_000, "the held bridge turn");
    await host.session.prompt("Reply only STEERED.", { source: "interactive", streamingBehavior: "steer" });
  } finally {
    transport.release();
  }
  expect(await bridgeEnded).toMatchObject({ turn: 2, owner_input: true });
  await host.drained();
  const ownerEnded = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 3);
  await host.session.prompt("Reply only OWNER.", { source: "interactive" });
  await ownerEnded;
  await host.drained();
  const started = events.since(0, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER).events
    .filter((event) => event.kind === "turn_started" && [2, 3].includes(event.turn as number));
  expect(started.map((event) => [event.turn, event.origin])).toEqual([
    [2, { kind: "bridge" }], [3, { kind: "owner_terminal" }],
  ]);
});

test("while bound, the worker refuses /new, /resume, /fork and import before the session changes", async () => {
  await expectReplacementsRefused(host, events);
});

test("a worker kickoff is sent once per run prompt, including on a retained transcript", () => {
  type Handler = (event: unknown, ctx: unknown) => unknown;
  const handlers: Handler[] = [];
  const sent: string[] = [];
  const pi = {
    on: (name: string, handler: Handler) => { if (name === "session_start") handlers.push(handler); },
    sendUserMessage: (text: string) => sent.push(text),
  } as unknown as ExtensionAPI;
  const context = { identity, cwd: "/tmp", model: "m", promptFile: "/store/prompts/T-007-R2.md",
    capabilities: { catalog: 5, enabled: [] } };
  const install = (bound: typeof context) => {
    handlers.length = 0;
    worker.extend?.(pi, bound);
  };
  const start = (reason: "startup" | "reload", texts: string[]) => {
    const ctx = { sessionManager: { getEntries: () => texts.map((text) => ({
      type: "message", message: { role: "user", content: [{ type: "text", text }] },
    })) } };
    for (const handler of handlers) handler({ type: "session_start", reason }, ctx);
  };
  install(context);
  start("reload", []);
  expect(sent).toEqual([]);
  start("startup", []);
  start("startup", []);
  expect(sent).toEqual([kickoff(context) ?? ""]);
  expect(sent[0]).toContain('`"$CORRAL" gate T-007` in bash');
  const prior = [...sent];
  sent.length = 0;
  install(context);
  start("startup", prior);
  expect(sent).toEqual([]);

  const next = { ...context, identity: { ...identity, run: "R3" }, promptFile: "/store/prompts/T-007-R3.md" };
  install(next);
  start("startup", prior);
  expect(sent).toEqual([kickoff(next) ?? ""]);
  prior.push(...sent);
  sent.length = 0;
  install(next);
  start("startup", prior);
  expect(sent).toEqual([]);
  expect(kickoff({ identity, cwd: "/tmp", model: "m" })).toBeUndefined();
});

test("the kickoff starts the first local turn when Pi binds extensions", async () => {
  const modules = await loadModules(assets.dir, "worker");
  const log = new EventLog();
  const network = guardedFetch(fakeCodexTransport().send);
  const sdk = await import("@earendil-works/pi-coding-agent");
  const dir = mkdtempSync(join(work, "kickoff-"));
  const promptFile = join(dir, "T-007-R2.md");
  writeFileSync(promptFile, "# Task\n\nReply only KICKED.\n");
  const cwd = join(work, "worktree");
  const client = new CorralClient({ corral: "", root: dir, cwd, identity });
  const started = await HostSession.create({
    sdk, identity, cwd, agentDir: join(dir, "pi-agent"), model: "gpt-6.1-sol", modules, client,
    events: log, network, promptFile, sessionManager: sdk.SessionManager.create(cwd, join(dir, "sessions")),
  });
  expect(log.since(0, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER).events.some((e) => e.kind === "turn_started")).toBeFalse();
  const ended = nextEvent(log, (event) => event.kind === "turn_ended" && event.turn === 1);
  await started.startLocal();
  await started.session.bindExtensions({});
  await ended;
  await started.drained();
  const first = started.session.messages.find((message) => message.role === "user");
  const text = first && typeof first.content !== "string" ? first.content.map((part) => part.type === "text" ? part.text : "").join("") : first?.content;
  expect(text).toBe(kickoff({ identity, cwd: "/tmp", model: "m", promptFile }));
  const lifecycle = log.since(0, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER).events
    .filter((event) => event.kind === "turn_started" || event.kind === "turn_ended");
  expect(lifecycle.map((event) => [event.kind, event.turn])).toEqual([["turn_started", 1], ["turn_ended", 1]]);
  await started.close();
});

const completedChild: ChildSnapshot = {
  id: "slice", kind: "slice", call: "task", title: "Implement", brief: "Implement", owns: ["file.ts"], after: [],
  profile: "default", model: "provider/model", state: "done", createdAt: 0, edits: [],
  usage: { input: 1, output: 2, turns: 1 }, steers: [], pulse: [], history: "d", messages: [], report: "Done",
};

function completionHarness(fleet: FleetSnapshot, recordDir: string, artifact: "committed" | "rejected" | "throw") {
  const calls: { op: string; payload: Record<string, unknown> }[] = [];
  const tool = workerDoneTool({ ...doneTool, async execute(_id, params) {
    calls.push({ op: "done", payload: params as Record<string, unknown> });
    return { content: [{ type: "text", text: "Done" }], details: {}, isError: false, terminate: true };
  } }, {
    recordDir, task: identity.task, run: identity.run, fleet: () => fleet,
    async attachArtifact(payload) {
      calls.push({ op: "artifact", payload });
      if (artifact === "rejected") throw new Error("artifact_refused");
      if (artifact === "throw") throw new Error("artifact transport unavailable");
    },
  });
  return { calls, tool };
}

test("worker done attaches fleet evidence before completing and attachment refusals or errors never fail done", async () => {
  const directory = mkdtempSync(join(tmpdir(), "corral-worker-record-"));
  const mail = [{ seq: 1, at: 0, from: "child", to: "parent", text: "Information", delivery: "delivered" as const }];
  try {
    for (const artifact of ["committed", "rejected", "throw"] as const) {
      const { calls, tool } = completionHarness({ children: artifact === "throw" ? [] : [completedChild], mail, maxParallel: 1 },
        join(directory, artifact), artifact);
      const result = await tool.execute("done-call", { summary: "Done.\nVerified: none\nNot verified: gate" }, undefined, undefined, {} as never);
      expect(calls.map(call => call.op)).toEqual(["artifact", "done"]);
      expect(calls[0]?.payload.title).toBe("Session record");
      expect(calls[0]?.payload.kind).toBe("report");
      const record = JSON.parse(readFileSync(String(calls[0]?.payload.path), "utf8"));
      expect(record.version).toBe(1);
      expect(record.slices.map((slice: { id: string }) => slice.id)).toEqual(artifact === "throw" ? [] : ["slice"]);
      expect(record.messages[0].from).toBe("child");
      expect(result.isError).toBeFalse();
      expect(result.terminate).toBeTrue();
      const text = result.content.filter(part => part.type === "text").map(part => part.text).join("\n");
      if (artifact === "committed") expect(text).not.toContain("Session record attachment failed");
      else expect(text).toContain(artifact === "rejected" ? "artifact_refused" : "artifact transport unavailable");
    }
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("worker done collects or cancels live reviews before sending any operation", async () => {
  for (const state of ["queued", "running", "needs"] as const) {
    const { calls, tool } = completionHarness({ children: [{ ...completedChild, id: "review", kind: "review", state }], mail: [], maxParallel: 1 },
      join(work, "live-review-record"), "committed");
    const result = await tool.execute("done-live", { summary: "Done" }, undefined, undefined, {} as never);
    expect(calls).toHaveLength(0);
    expect(result.isError).toBeTrue();
    const text = result.content.filter(part => part.type === "text").map(part => part.text).join("\n");
    expect(text).toContain("review");
    expect(text).toContain("still running");
    expect(text).toContain("agent_wait");
    expect(text).toContain("agent_cancel");
  }
});
