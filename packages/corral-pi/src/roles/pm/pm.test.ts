// Production PM role in a real upstream session, with offline auth and a wire-level corral fake.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { cpSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Check } from "typebox/value";

import { EventLog } from "../../bridge/events.ts";
import { loadModules } from "../../host/modules.ts";
import { HOST_CAPABILITIES, parseLaunchMessage } from "../../host/config.ts";
import defaultSnapshots from "../../../test/fixtures/default-capabilities.json";
import { guardedFetch, restrictEnvironment } from "../../host/restrict.ts";
import { startTimeOf } from "../../extensions/jobs/runtime.ts";
import { HostSession, authContext } from "../../host/session.ts";
import { canonicalJson, CorralClient, sha256Hex, toolRequestId } from "../../tools/corral.ts";
import { fixtureTransport } from "../../../test/fixtures/auth/index.ts";
import { type FakeCorral, writeFakeCorral } from "../../../test/support/fake-corral.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "../../../test/support/fixture-install.ts";
import { expectReplacementsRefused } from "../../../test/support/replacement.ts";
import { nextEvent } from "../../../test/support/wait.ts";

import pm, { CONTEXT_ASSET, pmRole } from "./index.ts";

const identity = { store: "st-0123456789abcdef", project: "demo", role: "pm" as const, generation: 1 };
const context = { identity, cwd: "/tmp", model: "m" };
const cases = [
  { name: "receipt_lookup", op: "receipt_lookup", payload: { request_id: `rq-${"a".repeat(32)}`, task: "T-007" } },
  { name: "brief", op: "read_brief", payload: {} },
  { name: "next", op: "read_next", payload: {} },
  { name: "show", op: "read_show", payload: { task: "T-007" } },
  { name: "list", op: "read_list", payload: { offset: 2, limit: 3, status: "backlog" } },
  { name: "search", op: "read_search", payload: { query: "bound project", limit: 4 } },
  { name: "route", op: "read_route", payload: { task: "T-007" } },
  { name: "artifacts", op: "read_artifacts", payload: { task: "T-007" } },
  { name: "lessons", op: "read_lessons", payload: {} },
  { name: "question_answer", op: "question_answer", payload: {
    key: "T-007/R2/Q1", rev: 3, text: "Use the approved adapter.", option: "adapter",
    basis_refs: ["T-007"], scope_note: "Existing write scope.",
  } },
  { name: "question_escalate", op: "question_escalate", payload: {
    key: "T-007/R2/Q2", rev: 1, note: "Owner authority required.", kind: "permission",
  } },
  { name: "question_cancel", op: "question_cancel", payload: {
    key: "T-007/R2/Q3", rev: 2, reason: "No longer needed.",
  } },
  { name: "reply", op: "reply", payload: { task: "T-007", text: "Continue within scope." } },
  { name: "comment", op: "comment", payload: { task: "T-007", text: "Evidence recorded." } },
  { name: "inbox_read", op: "inbox_read", payload: { id: "PMI-1", offset: 0, limit: 512 } },
  { name: "inbox_resolve", op: "inbox_resolve", payload: {
    id: "PMI-1", expected_revision: 1, action: "resolve", outcome: "done", text: "Filed T-9.", refs: ["T-9"],
  } },
  { name: "capture", op: "capture", payload: {
    project: "demo", title: "Follow-up", body: "Investigate the adapter.", status: "backlog",
  } },
] as const;
const memoryOps = ["memory_list", "memory_read", "memory_put", "memory_remove"];
const names = [...cases.map(({ name }) => `corral_${name}`), ...memoryOps.map((op) => `corral_${op}`)];
const originalFetch = globalThis.fetch;
let assets: FixtureInstall;
let work: string;
let cwd: string;
let corral: FakeCorral;
let events: EventLog;
let host: HostSession;

function spec(name: string) {
  const found = pm.tools(context).find((item) => item.name === `corral_${name}`);
  if (!found) throw new Error(`missing PM tool ${name}`);
  return found;
}

beforeAll(async () => {
  assets = buildFixtureInstall();
  rmSync(join(assets.dir, "src/roles/pm"), { recursive: true, force: true });
  for (const tree of ["src/roles/pm", "src/roles/assistant", "src/roles/common", "assets"]) {
    cpSync(join(PACKAGE_ROOT, tree), join(assets.dir, tree), { recursive: true });
  }
  work = mkdtempSync(join(tmpdir(), "corral-pi-pm-"));
  cwd = join(work, "workspace");
  mkdirSync(join(cwd, ".pi/skills/leak"), { recursive: true });
  mkdirSync(join(cwd, ".pi/prompts"), { recursive: true });
  writeFileSync(join(cwd, "AGENTS.md"), "Inherited context must not load.\n");
  writeFileSync(join(cwd, ".pi/skills/leak/SKILL.md"), "---\nname: leak\ndescription: leak\n---\n");
  writeFileSync(join(cwd, ".pi/prompts/leak.md"), "Inherited prompt must not load.\n");
  restrictEnvironment(process.env);
  const modules = await loadModules(assets.dir, "pm");
  const transport = modules.auth.transport?.(authContext());
  if (!transport) throw new Error("fixture auth must supply its offline transport");
  const network = guardedFetch(transport);
  globalThis.fetch = network.fetch;
  events = new EventLog(identity.generation);
  corral = writeFakeCorral(work);
  const client = new CorralClient({
    corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host.sessionId(),
  });
  const sdk = await import("@earendil-works/pi-coding-agent");
  const promptFile = join(work, "pm-pi.json");
  writeFileSync(promptFile, JSON.stringify({ version: 1, workspace: cwd, projects: { demo: cwd },
    policy: { revision: "launch-policy", authorization: "approved" } }));
  host = await HostSession.create({
    sdk, identity, cwd, promptFile, agentDir: join(work, "pi-agent"), model: "gpt-6.1-sol", modules, client, events, network,
    sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
  });
  await host.session.bindExtensions({});
  await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
}, 60_000);

afterAll(() => {
  globalThis.fetch = originalFetch;
  assets?.cleanup();
  if (work) rmSync(work, { recursive: true, force: true });
});

test("default-resolved PM and assistant hosts have the same capability tools and PM guidance", async () => {
  mkdirSync(join(cwd, ".agents/skills/pm-fixture"), { recursive: true });
  writeFileSync(join(cwd, ".agents/skills/pm-fixture/SKILL.md"), "---\nname: pm-fixture\ndescription: fixture\n---\nGuidance.\n");
  const sdk = await import("@earendil-works/pi-coding-agent");
  const hosts: HostSession[] = [];
  const hostEvents: EventLog[] = [];
  for (const role of ["pm", "assistant"] as const) {
    // This snapshot is generated by Rust's default resolver and checked against it in corral-core.
    const snapshot = defaultSnapshots[role];
    const entries = Object.entries(snapshot.features).filter(([, state]) => state.enabled);
    const payload = { catalog: snapshot.catalog, digest: snapshot.digest, preset: snapshot.preset,
      enabled: entries.map(([key]) => key).filter((key) => HOST_CAPABILITIES.includes(key)),
      settings: Object.fromEntries(entries.flatMap(([key, state]) => "settings" in state ? [[key, state.settings]] : [])),
      project_text: true, web: snapshot.web, mcp: snapshot.mcp };
    const parsed = parseLaunchMessage(JSON.stringify({ version: 1, store: identity.store, project: role === "pm" ? "demo" : "",
      role, ...(role === "assistant" ? { assistant: "A-1" } : {}), generation: 1,
      secrets: { host_cap: "h".repeat(64), ctl_cap: "c".repeat(64), bind_nonce: "n".repeat(64) },
      pane: "p", cwd, root: cwd, corral: corral.path, assets: assets.dir, runtime_dir: join(work, role),
      session_dir: join(work, `${role}-sessions`), model: "gpt-6.1-sol", capabilities: payload }));
    const modules = await loadModules(assets.dir, role);
    const transport = modules.auth.transport?.(authContext());
    if (!transport || !parsed.capabilities?.web) throw new Error("missing fixture transport or web settings");
    const network = guardedFetch(transport, undefined, "openai-codex", undefined, { settings: parsed.capabilities.web, send: transport });
    let current: HostSession;
    const client = new CorralClient({ corral: corral.path, root: work, identity: parsed.identity,
      hostCap: "h".repeat(64), sessionId: () => current.sessionId() });
    const eventLog = new EventLog(1);
    current = await HostSession.create({ sdk, identity: parsed.identity, cwd, root: cwd, home: work,
      agentDir: join(work, `${role}-agent`), runtimeDir: parsed.runtimeDir, model: parsed.model, modules, client,
      events: eventLog, network, capabilities: parsed.capabilities, guidance: parsed.guidance,
      sessionManager: sdk.SessionManager.create(cwd, parsed.sessionDir) });
    await current.session.bindExtensions({});
    await current.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
    hosts.push(current);
    hostEvents.push(eventLog);
  }
  const [pmHost, assistantHost] = hosts;
  if (!pmHost || !assistantHost) throw new Error("missing host");
  const capabilityTools = (session: HostSession) => session.session.getActiveToolNames()
    .filter((name) => !name.startsWith("corral_") || name === "corral_todo").sort();
  expect(capabilityTools(pmHost)).toEqual(capabilityTools(assistantHost));
  for (const name of ["ask_user", "corral_todo", "job_start", "job_output", "job_kill", "web_fetch", "web_search", "lsp_diagnostics", "lsp_definition", "lsp_references", "explore", "mcp_connect", "tool_search", "codemode"]) {
    expect(capabilityTools(pmHost)).toContain(name);
  }
  expect(pmHost.session.getActiveToolNames()).toContain("corral_job_monitor");
  const pmEvents = hostEvents[0];
  if (!pmEvents) throw new Error("missing PM event log");
  fixtureTransport().script([{ toolCalls: [{ callId: "start-job", name: "job_start", arguments: { command: "sleep 30" } }] }, { text: "STARTED" }]);
  const started = nextEvent(pmEvents, (event) => event.kind === "turn_ended");
  await pmHost.submit({ text: "Start a job", busyPolicy: "prompt", idempotencyKey: "job-start", origin: "test" }, "job-start");
  await started;
  await pmHost.drained();
  const result = pmHost.session.messages.findLast((message) => message.role === "toolResult" && message.toolName === "job_start");
  if (!result || result.role !== "toolResult") throw new Error("missing job result");
  expect(result.isError).toBeFalse();
  const output = result.content.find((part) => part.type === "text");
  if (!output || output.type !== "text") throw new Error("missing job result");
  const startedJob = result.details as Record<string, unknown>;
  expect(startedJob.state).toBe("running");
  const job = startedJob.job;
  fixtureTransport().script([{ toolCalls: [
    { callId: "output-job", name: "job_output", arguments: { job } },
    { callId: "kill-job", name: "job_kill", arguments: { job } },
  ] }, { text: "STOPPED" }]);
  const stopped = nextEvent(pmEvents, (event) => event.kind === "turn_ended");
  await pmHost.submit({ text: "Read and stop it", busyPolicy: "prompt", idempotencyKey: "job-stop", origin: "test" }, "job-stop");
  await stopped;
  await pmHost.drained();
  for (const name of ["job_output", "job_kill"]) {
    const result = pmHost.session.messages.findLast((message) => message.role === "toolResult" && message.toolName === name);
    if (!result || result.role !== "toolResult") throw new Error(`missing ${name} result`);
    expect(result.isError).toBeFalse();
    const text = result.content.find((part) => part.type === "text");
    if (!text || text.type !== "text") throw new Error(`missing ${name} output`);
    expect((result.details as Record<string, unknown>).error).toBeUndefined();
  }
  const resources = pmHost.runtime.services.resourceLoader;
  expect(resources.getSkills().skills.map((skill) => skill.name)).toContain("pm-fixture");
  expect(resources.getSkills().skills.map((skill) => skill.name)).toContain("corral-pm");
  expect(resources.getAgentsFiles().agentsFiles.map((file) => file.path)).toContain(join(cwd, "AGENTS.md"));
  fixtureTransport().script([{ toolCalls: [{ callId: "shutdown-job", name: "job_start", arguments: { command: "sleep 30" } }] }, { text: "STARTED" }]);
  const again = nextEvent(pmEvents, (event) => event.kind === "turn_ended");
  await pmHost.submit({ text: "Start another job", busyPolicy: "prompt", idempotencyKey: "job-shutdown", origin: "test" }, "job-shutdown");
  await again;
  await pmHost.drained();
  const live = pmHost.session.messages.findLast((message) => message.role === "toolResult" && message.toolName === "job_start");
  if (!live || live.role !== "toolResult") throw new Error("missing live job result");
  const liveText = live.content.find((part) => part.type === "text");
  if (!liveText || liveText.type !== "text") throw new Error("missing live job output");
  const pid = Number((live.details as Record<string, unknown>).pid);
  expect(startTimeOf(pid)).toBeDefined();
  await pmHost.close();
  await assistantHost.close();
  expect(startTimeOf(pid)).toBeUndefined();
}, 60_000);

test("the PM exposes only its scoped operations and requires the installed context asset", () => {
  // Given the production role, when constructing its tools and contract:
  const tools = pm.tools(context);
  // Then neither lifecycle-only operations nor owner authority become model tools.
  expect(pm.upstreamTools).toEqual(["read", "edit", "write", "grep", "find", "ls", "bash"]);
  expect(tools.map((item) => item.name)).toEqual(names);
  expect([...pm.corralOps].sort()).toEqual(["hello", "event", ...cases.map(({ op }) => op), ...memoryOps, "todo"].sort());
  const empty = mkdtempSync(join(tmpdir(), "corral-pi-pm-noassets-"));
  try {
    expect(() => pmRole(empty).contract(context)).toThrow(`role_asset_missing: ${CONTEXT_ASSET}`);
  } finally {
    rmSync(empty, { recursive: true, force: true });
  }
});

for (const { name, payload } of cases) test(`the ${name} schema and payload preserve the Rust wire contract`, () => {
  // Given a Rust-compatible payload, when it crosses the model-tool boundary:
  const tool = spec(name);
  // Then valid input survives and undeclared identity or owner authority is refused.
  expect(Check(tool.parameters, payload)).toBeTrue();
  expect(tool.payload(payload)).toEqual(payload);
  expect(Check(tool.parameters, { ...payload, owner: true })).toBeFalse();
  expect(Check(tool.parameters, { ...payload, generation: 99 })).toBeFalse();
  expect(tool.payload({ ...payload, owner: true, generation: 99 })).toEqual(payload);
});

test("policy wake keeps the native ceiling while independent coordination crosses the host wire", async () => {
  const call = (callId: string, name: string, args: Record<string, unknown>) =>
    ({ toolCalls: [{ callId, name: `corral_${name}`, arguments: args }] });
  // Scripted offline model: no paid call, no prose grading. Rust tests verify the effects.
  fixtureTransport().script([
    call("wake-brief", "brief", {}), call("wake-next", "next", {}),
    call("wake-inbox", "inbox_read", { id: "PMI-1" }),
    call("wake-owner", "question_escalate", { key: "T-007/R2/Q2", rev: 1,
      note: "Keep scope (recommended) or expand; owner must decide." }),
    call("wake-claim", "inbox_resolve", { id: "PMI-1", expected_revision: 0, action: "claim" }),
    call("wake-question", "show", { task: "T-007", section: "questions" }),
    call("wake-answer", "question_answer", { key: "T-007/R2/Q1", rev: 3,
      text: "Use the approved adapter", basis_refs: ["spec:T-007@approved"], scope_note: "Existing scope" }),
    call("wake-verify", "show", { task: "T-007", section: "questions" }),
    call("wake-resolve", "inbox_resolve", { id: "PMI-1", expected_revision: 1, action: "resolve",
      outcome: "done", text: "Answered canonical question", refs: ["T-007/R2/Q1"] }),
    call("wake-outcome", "inbox_read", { id: "PMI-1" }),
    { text: "" },
  ]);
  const before = corral.calls().length;
  const ended = nextEvent(events, (event) => event.kind === "turn_ended");
  await host.submit({ text: "[corral-policy] Effective policy replacement: current executor scope authorized.\n"
    + "[question_wake] T-007/R2/Q2\n[pm_inbox_wake] PMI-1", busyPolicy: "prompt",
    idempotencyKey: "operational-wake", origin: "corral-inbox" }, "operational-wake");
  await ended;
  await host.drained();
  const ops = corral.calls().slice(before).map(({ envelope }) => envelope.op).filter((op) => op !== "event");
  expect(ops).toEqual(["read_brief", "read_next", "inbox_read", "question_escalate", "inbox_resolve",
    "read_show", "question_answer", "read_show", "inbox_resolve", "inbox_read"]);
  expect(host.session.getActiveToolNames()).not.toContain("corral_pm_act");
  expect(host.session.getActiveToolNames()).not.toContain("corral_owner_action");
}, 60_000);

const invalidCases: readonly (readonly [string, Record<string, unknown>])[] = [
  ["brief", { task: "T-007" }], ["next", { project: "other" }], ["lessons", { limit: 5 }],
  ["show", {}], ["route", {}], ["artifacts", {}], ["search", {}],
  ["list", { limit: 1.5 }], ["list", { offset: "2" }], ["list", { status: 2 }],
  ["search", { query: "q", limit: 1.5 }],
  ["question_answer", { key: "T-007/R2/Q1", rev: 1, answer: "wrong field" }],
  ["question_cancel", { key: "T-007/R2/Q1", reason: "missing revision" }],
  ["question_escalate", { key: "T-007/R2/Q1", rev: 1.5 }],
  ["question_escalate", { key: "T-007/R2/Q1", rev: 1, kind: "priority" }],
  ["reply", { task: "T-007", message: "wrong field" }],
  ["comment", { text: "missing task" }],
  ["capture", { project: "demo", title: "x", body: "y", status: "ready" }],
  ["inbox_read", { state: "stale" }],
  ["inbox_resolve", { id: "PMI-1", action: "claim" }],
  ["inbox_resolve", { id: "PMI-1", expected_revision: 0, action: "approve" }],
  ["inbox_resolve", { id: "PMI-1", expected_revision: 1, action: "resolve", outcome: "landed" }],
];
for (const [index, [name, payload]] of invalidCases.entries()) test(`the ${name} schema refuses invalid case ${index}`, () => {
  // Given malformed input, when checking the schema, then it cannot reach the client.
  expect(Check(spec(name).parameters, payload)).toBeFalse();
});

for (const stage of ["startup", "reload", "refused replacement"]) test(
  `the PM retains its enforced loadout and receipt tools after ${stage}`,
  async () => {
    // Given the actual host lifecycle transition:
    if (stage === "reload") await host.session.reload();
    if (stage === "refused replacement") await expectReplacementsRefused(host, events);
    const state = host.state();
    const prompt = host.session.systemPrompt;
    const start = prompt.indexOf('{\n  "version"');
    expect(start).toBeGreaterThanOrEqual(0);
    const snapshot = JSON.parse(prompt.slice(start, prompt.indexOf('\n}', start) + 2));
    expect(snapshot.workspace).toBe(cwd);
    expect(snapshot.projects).toEqual({ demo: cwd });
    expect(snapshot.policy.revision).toBe("launch-policy");
    const upstream = ["read", "edit", "write", "grep", "find", "ls", "bash"];
    expect(host.session.getActiveToolNames().sort()).toEqual([...upstream, ...names].sort());
    expect(state.loadout).toEqual([...upstream, ...names]);
    const resources = host.runtime.services.resourceLoader;
    if (stage === "startup") expect(resources.getSkills().skills.map((skill) => skill.name)).toEqual(["corral-pm"]);
    expect(resources.getPrompts().prompts).toHaveLength(0);
    expect(resources.getAgentsFiles().agentsFiles).toHaveLength(0);
    expect(resources.getExtensions().extensions).toHaveLength(1);
    expect(host.session.settingsManager.getEnableSkillCommands()).toBeFalse();

    // When the offline model calls each permitted tool and attempts forbidden ones:
    const target = join(cwd, `forbidden-${stage}`);
    const forbidden = [
      { callId: "act", name: "corral_pm_act", arguments: {} },
      { callId: "deadline", name: "corral_question_deadline", arguments: {} },
      { callId: "mcp", name: "mcp__leak", arguments: {} },
    ];
    fixtureTransport().script([
      { toolCalls: cases.map(({ name, payload }) => ({ callId: `${stage}-${name}`, name: `corral_${name}`, arguments: payload })) },
      { toolCalls: forbidden },
      { text: "RECORDED" },
    ]);
    const before = corral.calls().length;
    const ended = nextEvent(events, (event) => event.kind === "turn_ended");
    expect((await host.submit({
      text: "Use the scripted tools.", busyPolicy: "prompt", idempotencyKey: stage, origin: "test",
    }, `rq-${stage}`)).disposition).toBe("started");
    await ended;
    await host.drained();

    // Then actual envelopes preserve operation, payload, identity, digest and request ID.
    const calls = corral.calls().slice(before).map(({ envelope }) => envelope).filter(({ op }) => op !== "event");
    const byOp = (a: { readonly op: string }, b: { readonly op: string }) => a.op.localeCompare(b.op);
    expect(calls.map(({ op, payload }) => ({ op, payload })).sort(byOp))
      .toEqual(cases.map(({ op, payload }) => ({ op, payload })).sort(byOp));
    const modelCalls = host.session.messages.flatMap((message) =>
      message.role === "assistant" ? message.content.filter((part) => part.type === "toolCall") : []);
    for (const call of calls) {
      const modelCall = modelCalls.find((item) => item.name === `corral_${call.op.replace(/^read_/, "")}`
        && toolRequestId(host.sessionId(), item.id) === call.request_id);
      expect(modelCall).toBeDefined();
      expect(call.payload_sha256).toBe(sha256Hex(canonicalJson(call.payload)));
      expect({
        store: call.identity.store, project: call.identity.project, role: call.identity.role,
        generation: call.identity.generation, session_id: call.identity.session_id,
      }).toEqual({ ...identity, session_id: host.sessionId() });
      expect(call.identity.task).toBeUndefined();
      expect(call.identity.run).toBeUndefined();
    }
    expect(calls).toHaveLength(cases.length);
    const results = host.session.messages.filter((message) => message.role === "toolResult");
    for (const { name } of forbidden) {
      expect(results.findLast((message) => message.toolName === name)?.isError).toBeTrue();
    }
    expect(existsSync(target)).toBeFalse();
  }, 60_000,
);
