// Production PM role with offline auth, local todo, and an argv-level engine fixture.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../../bridge/events.ts";
import { loadModules } from "../../host/modules.ts";
import { workspaceSettings } from "../../host/config.ts";
import { guardedFetch, restrictEnvironment } from "../../host/restrict.ts";
import { startTimeOf } from "../../extensions/jobs/runtime.ts";
import { HostSession } from "../../host/session.ts";
import { CorralClient } from "../../tools/corral.ts";
import { fakeCodexTransport } from "../../../test/fixtures/auth/fake-transport.ts";
import { type FakeEngine, writeFakeEngine } from "../../../test/support/fake-engine.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "../../../test/support/fixture-install.ts";
import { expectReplacementsRefused } from "../../../test/support/replacement.ts";
import { nextEvent } from "../../../test/support/wait.ts";

import pm, { CONTEXT_ASSET, pmRole } from "./index.ts";

const identity = { store: "st-0123456789abcdef", project: "demo", role: "pm" as const };
const context = { identity, cwd: "/tmp", model: "m" };
const originalFetch = globalThis.fetch;
let assets: FixtureInstall;
let work: string;
let cwd: string;
let corral: FakeEngine;

beforeAll(() => {
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
  corral = writeFakeEngine(work);
}, 60_000);

async function startPm() {
  const dir = mkdtempSync(join(work, "run-"));
  const modules = await loadModules(assets.dir, "pm");
  const transport = fakeCodexTransport();
  const network = guardedFetch(transport.send);
  globalThis.fetch = network.fetch;
  const events = new EventLog();
  const client = new CorralClient({ corral: corral.path, root: work, cwd, identity });
  const sdk = await import("@earendil-works/pi-coding-agent");
  const promptFile = join(dir, "pm-pi.json");
  writeFileSync(promptFile, JSON.stringify({ version: 1, workspace: cwd, projects: { demo: cwd },
    policy: { revision: "launch-policy", authorization: "approved" } }));
  const host = await HostSession.create({
    sdk, identity, cwd, promptFile, agentDir: join(dir, "pi-agent"), model: "gpt-6.1-sol", modules, client, events, network,
    capabilities: { catalog: 5, enabled: ["ask_user", "todo"] },
    sessionManager: sdk.SessionManager.create(cwd, join(dir, "sessions")),
  });
  await host.startLocal();
  await host.session.bindExtensions({});
  return { host, events, transport };
}

afterAll(() => {
  globalThis.fetch = originalFetch;
  assets?.cleanup();
  if (work) rmSync(work, { recursive: true, force: true });
});

test("PM and assistant hosts keep shared capabilities and PM guidance", async () => {
  mkdirSync(join(cwd, ".agents/skills/pm-fixture"), { recursive: true });
  writeFileSync(join(cwd, ".agents/skills/pm-fixture/SKILL.md"), "---\nname: pm-fixture\ndescription: fixture\n---\nGuidance.\n");
  mkdirSync(join(cwd, ".corral-next"), { recursive: true });
  writeFileSync(join(cwd, ".corral-next/pi.toml"),
    '[web]\nhosts = ["*"]\nmax_bytes = 1048576\ntimeout_ms = 15000\nmax_redirects = 5\n[mcp.servers]\n');
  const settings = workspaceSettings(cwd, "assistant");
  const sdk = await import("@earendil-works/pi-coding-agent");
  const hosts: HostSession[] = [];
  const hostEvents: EventLog[] = [];
  const pmTransport = fakeCodexTransport();
  for (const role of ["pm", "assistant"] as const) {
    const bound = role === "pm" ? identity : { ...identity, role, project: "", assistant: "A-1" };
    const modules = await loadModules(assets.dir, role);
    const transport = role === "pm" ? pmTransport : fakeCodexTransport();
    if (!settings.capabilities.web) throw new Error("missing web settings");
    const network = guardedFetch(transport.send, undefined, "openai-codex", undefined, { settings: settings.capabilities.web, send: transport.send });
    globalThis.fetch = network.fetch;
    const client = new CorralClient({ corral: corral.path, root: cwd, cwd, identity: bound });
    const eventLog = new EventLog();
    const current = await HostSession.create({ sdk, identity: bound, cwd, root: cwd, home: work,
      agentDir: join(work, `${role}-agent`), runtimeDir: join(work, role), model: "gpt-6.1-sol", modules, client,
      events: eventLog, network, ...settings,
      sessionManager: sdk.SessionManager.create(cwd, join(work, `${role}-sessions`)) });
    await current.session.bindExtensions({});
    await current.startLocal();
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
  const pmEvents = hostEvents[0];
  if (!pmEvents) throw new Error("missing PM event log");
  pmTransport.script([{ toolCalls: [{ callId: "start-job", name: "job_start", arguments: { command: "sleep 30" } }] }, { text: "STARTED" }]);
  const started = nextEvent(pmEvents, (event) => event.kind === "turn_ended");
  await pmHost.submitLocal({ text: "Start a job", busyPolicy: "prompt", origin: "owner" });
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
  pmTransport.script([{ toolCalls: [
    { callId: "output-job", name: "job_output", arguments: { job } },
    { callId: "kill-job", name: "job_kill", arguments: { job } },
  ] }, { text: "STOPPED" }]);
  const stopped = nextEvent(pmEvents, (event) => event.kind === "turn_ended");
  await pmHost.submitLocal({ text: "Read and stop it", busyPolicy: "prompt", origin: "owner" });
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
  pmTransport.script([{ toolCalls: [{ callId: "shutdown-job", name: "job_start", arguments: { command: "sleep 30" } }] }, { text: "STARTED" }]);
  const again = nextEvent(pmEvents, (event) => event.kind === "turn_ended");
  await pmHost.submitLocal({ text: "Start another job", busyPolicy: "prompt", origin: "owner" });
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

test("the PM role exposes only local todo and requires the installed context asset", () => {
  expect(pm.upstreamTools).toEqual(["read", "edit", "write", "grep", "find", "ls", "bash"]);
  expect(pm.tools(context)).toEqual([]);
  expect(pm.tools({ ...context, todo: true }).map((item) => item.name)).toEqual(["corral_todo"]);
  const empty = mkdtempSync(join(tmpdir(), "corral-pi-pm-noassets-"));
  try {
    expect(() => pmRole(empty).contract(context)).toThrow(`role_asset_missing: ${CONTEXT_ASSET}`);
  } finally {
    rmSync(empty, { recursive: true, force: true });
  }
});

for (const stage of ["startup", "reload", "refused replacement"]) test(
  `the PM retains its enforced loadout and local todo after ${stage}`,
  async () => {
    const { host, events, transport } = await startPm();
    try {
      if (stage === "reload") await host.session.reload();
      if (stage === "refused replacement") {
        transport.script([{ text: "Ready." }]);
        const seeded = nextEvent(events, (event) => event.kind === "turn_ended");
        expect((await host.submitLocal({ text: "Confirm ready.", busyPolicy: "prompt", origin: "owner" })).disposition).toBe("started");
        await seeded;
        await host.drained();
        await expectReplacementsRefused(host, events);
      }
      const state = host.state();
      const prompt = host.session.systemPrompt;
      const start = prompt.indexOf('{\n  "version"');
      expect(start).toBeGreaterThanOrEqual(0);
      const snapshot = JSON.parse(prompt.slice(start, prompt.indexOf('\n}', start) + 2));
      expect(snapshot.workspace).toBe(cwd);
      expect(snapshot.projects).toEqual({ demo: cwd });
      expect(snapshot.policy.revision).toBe("launch-policy");
      const expected = [...pm.upstreamTools, "corral_todo", "ask_user", "agent_message", "agent_roster"];
      expect(host.session.getActiveToolNames().sort()).toEqual([...expected].sort());
      expect(state.loadout).toEqual(expected);
      const resources = host.runtime.services.resourceLoader;
      if (stage === "startup") expect(resources.getSkills().skills.map((skill) => skill.name)).toEqual(["corral-pm"]);
      expect(resources.getPrompts().prompts).toHaveLength(0);
      expect(resources.getAgentsFiles().agentsFiles).toHaveLength(0);
      expect(resources.getExtensions().extensions).toHaveLength(1);
      expect(host.session.settingsManager.getEnableSkillCommands()).toBeFalse();

      // A board write runs via bash; todo stays in the host and removed tools stay unavailable.
      const forbidden = [
        { callId: "act", name: "corral_pm_act", arguments: {} },
        { callId: "deadline", name: "corral_question_deadline", arguments: {} },
        { callId: "mcp", name: "mcp__leak", arguments: {} },
      ];
      transport.script([
        { toolCalls: [{ callId: `${stage}-comment`, name: "bash", arguments: {
          command: `${JSON.stringify(corral.path)} --root ${JSON.stringify(work)} --json comment T-007 "Evidence recorded"`,
        } }] },
        { toolCalls: [{ callId: `${stage}-todo-init`, name: "corral_todo", arguments: { op: "init",
          phases: [{ id: "record", label: "Record", steps: [{ id: "comment", label: "Leave comment" }] }] } }] },
        { toolCalls: [{ callId: `${stage}-todo-done`, name: "corral_todo", arguments: { op: "done", step: "comment" } }] },
        { toolCalls: forbidden },
        { text: "RECORDED" },
      ]);
      const before = corral.calls().length;
      const ended = nextEvent(events, (event) => event.kind === "turn_ended");
      expect((await host.submitLocal({ text: "Use the scripted tools.", busyPolicy: "prompt", origin: "owner" })).disposition).toBe("started");
      await ended;
      await host.drained();

      expect(corral.calls().slice(before).filter((argv) => argv[3] === "comment" || argv[3] === "todo"))
        .toEqual([["--root", work, "--json", "comment", "T-007", "Evidence recorded"]]);
      const todo = host.session.messages.findLast((message) => message.role === "toolResult" && message.toolName === "corral_todo");
      expect(todo).toMatchObject({ isError: false, details: { progress: { phases: [{ steps: [{ status: { state: "done" } }] }] } } });
      const results = host.session.messages.filter((message) => message.role === "toolResult");
      for (const { name } of forbidden) {
        expect(results.findLast((message) => message.toolName === name)?.isError).toBeTrue();
      }
    } finally {
      await host.close();
    }
  }, 60_000,
);
