// Production PM role in a real upstream session, with offline auth and a wire-level corral fake.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { cpSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Check } from "typebox/value";

import { EventLog } from "../../bridge/events.ts";
import { loadModules } from "../../host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../../host/restrict.ts";
import { HostSession, authContext } from "../../host/session.ts";
import { canonicalJson, CorralClient, sha256Hex, toolRequestId } from "../../tools/corral.ts";
import { fixtureTransport } from "../../../test/fixtures/auth/index.ts";
import { type FakeCorral, writeFakeCorral } from "../../../test/support/fake-corral.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "../../../test/support/fixture-install.ts";
import { nextEvent } from "../../../test/support/wait.ts";

import pm, { CONTEXT_ASSET, pmRole } from "./index.ts";

const identity = { store: "st-0123456789abcdef", project: "demo", role: "pm" as const, generation: 1 };
const context = { identity, cwd: "/tmp", model: "m" };
const cases = [
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
  { name: "capture", op: "capture", payload: {
    project: "demo", title: "Follow-up", body: "Investigate the adapter.", status: "backlog",
  } },
] as const;
const names = cases.map(({ name }) => `corral_${name}`);
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
  for (const tree of ["src/roles/pm", "src/roles/common", "assets"]) {
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
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(work, "pi-agent"), model: "gpt-6.1-sol", modules, client, events, network,
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

test("the PM exposes only its scoped operations and requires the installed context asset", () => {
  // Given the production role, when constructing its tools and contract:
  const tools = pm.tools(context);
  // Then neither lifecycle-only operations nor owner authority become model tools.
  expect(pm.upstreamTools).toEqual([]);
  expect(tools.map((item) => item.name)).toEqual(names);
  expect([...pm.corralOps].sort()).toEqual(["hello", "event", "receipt_lookup", ...cases.map(({ op }) => op)].sort());
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
];
for (const [index, [name, payload]] of invalidCases.entries()) test(`the ${name} schema refuses invalid case ${index}`, () => {
  // Given malformed input, when checking the schema, then it cannot reach the client.
  expect(Check(spec(name).parameters, payload)).toBeFalse();
});

for (const stage of ["startup", "reload", "replacement"]) test(
  `the PM retains its enforced loadout and receipt tools after ${stage}`,
  async () => {
    // Given the actual host lifecycle transition:
    if (stage === "reload") await host.session.reload();
    if (stage === "replacement") {
      const old = host.session.sessionId;
      expect((await host.runtime.newSession()).cancelled).toBeFalse();
      expect(host.session.sessionId).not.toBe(old);
    }
    const state = host.state();
    expect(host.session.getActiveToolNames().sort()).toEqual([...names].sort());
    expect(state.loadout).toEqual(names);
    const resources = host.runtime.services.resourceLoader;
    expect(resources.getSkills().skills).toHaveLength(0);
    expect(resources.getPrompts().prompts).toHaveLength(0);
    expect(resources.getAgentsFiles().agentsFiles).toHaveLength(0);
    expect(resources.getExtensions().extensions).toHaveLength(1);
    expect(host.session.settingsManager.getEnableSkillCommands()).toBeFalse();

    // When the offline model calls each permitted tool and attempts forbidden ones:
    const target = join(cwd, `forbidden-${stage}`);
    const forbidden = [
      { callId: "write", name: "write", arguments: { path: target, content: "forbidden" } },
      { callId: "bash", name: "bash", arguments: { command: `touch ${target}` } },
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
    expect(calls).toHaveLength(14);
    const results = host.session.messages.filter((message) => message.role === "toolResult");
    for (const { name } of forbidden) {
      expect(results.findLast((message) => message.toolName === name)?.isError).toBeTrue();
    }
    expect(existsSync(target)).toBeFalse();
  }, 60_000,
);
