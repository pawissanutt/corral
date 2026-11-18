// Both planning roles retain their real read-only loadout and supplied prompt in local host sessions.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { EventLog } from "../src/bridge/events.ts";
import { loadModules, missingModules, modulePaths } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession } from "../src/host/session.ts";
import planReviewer from "../src/roles/plan-reviewer/index.ts";
import planner from "../src/roles/planner/index.ts";
import type { HostIdentity } from "../src/roles/types.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { fakeCodexTransport } from "./fixtures/auth/fake-transport.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "./support/fixture-install.ts";
import { nextEvent } from "./support/wait.ts";

const READ_ONLY = ["read", "grep", "find", "ls"];
const originalFetch = globalThis.fetch;
type PlanningRole = "planner" | "plan_reviewer";
const identities: Record<PlanningRole, HostIdentity> = {
  planner: { store: "st-0123456789abcdef", project: "demo", role: "planner", task: "T-009", run: "D1" },
  plan_reviewer: {
    store: "st-0123456789abcdef", project: "demo", role: "plan_reviewer", task: "T-009", run: "P1", review: "PR1",
  },
};

let assets: FixtureInstall;
let work: string;

async function bind(role: PlanningRole, promptFile: string) {
  const identity = identities[role];
  const dir = join(work, role);
  const cwd = join(dir, "repo");
  mkdirSync(cwd, { recursive: true });
  const modules = await loadModules(assets.dir, role);
  const events = new EventLog();
  const transport = fakeCodexTransport();
  const network = guardedFetch(transport.send);
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const client = new CorralClient({ corral: "", root: work, cwd, identity });
  const host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(dir, "pi-agent"), model: "gpt-6.1-sol", modules, client, events, network, promptFile,
    sessionManager: sdk.SessionManager.create(cwd, join(dir, "sessions")),
  });
  return { host, events, cwd, transport };
}

beforeAll(() => {
  assets = buildFixtureInstall({ omit: ["role"] });
  work = mkdtempSync(join(tmpdir(), "corral-pi-plan-roles-"));
  restrictEnvironment(process.env);
});

afterAll(() => {
  globalThis.fetch = originalFetch;
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
});

test("both planning roles have read-only file tools and no board mutations", () => {
  const context = (role: PlanningRole) => ({ identity: identities[role], cwd: "/tmp", model: "m" });
  expect([...planner.upstreamTools]).toEqual(READ_ONLY);
  expect([...planReviewer.upstreamTools]).toEqual(READ_ONLY);
  expect(planner.tools(context("planner"))).toEqual([]);
  expect(planReviewer.tools(context("plan_reviewer"))).toEqual([]);
  expect(planner.contract(context("planner"))).toContain("design run D1");
  expect(planReviewer.contract(context("plan_reviewer"))).toContain("plan P1 of corral task T-009 (attempt PR1)");
});

test("both planning roles have loadable modules at their fixed paths", () => {
  expect(modulePaths("planner").role).toBe("src/roles/planner/index.ts");
  expect(modulePaths("plan_reviewer").role).toBe("src/roles/plan-reviewer/index.ts");
  for (const role of ["planner", "plan_reviewer"] as const) {
    expect(existsSync(join(PACKAGE_ROOT, modulePaths(role).role))).toBeTrue();
    expect(missingModules(PACKAGE_ROOT, role)).toEqual([]);
  }
});

for (const role of ["planner", "plan_reviewer"] as const) {
  test(`the ${role} reads its supplied prompt and artifacts without write or shell access`, async () => {
    const promptFile = join(work, `${role}-prompt.md`);
    writeFileSync(promptFile, "Inspect the supplied prior-work artifact.");
    const { host, events, cwd, transport } = await bind(role, promptFile);
    try {
      expect([...host.loadout]).toEqual([...READ_ONLY, "agent_message", "agent_roster"]);
      const artifact = join(cwd, "prior-work.md");
      writeFileSync(artifact, "Prior work: include a regression test.");
      const target = join(cwd, "edited.txt");
      transport.script([
        { toolCalls: [
          { callId: "call-prompt", name: "read", arguments: { path: promptFile } },
          { callId: "call-artifact", name: "read", arguments: { path: artifact } },
          { callId: "call-write", name: "write", arguments: { path: target, content: "x" } },
          { callId: "call-bash", name: "bash", arguments: { command: `touch '${target}'` } },
        ] },
        { text: "DONE" },
      ]);
      const ended = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 1);
      await host.startLocal();
      await host.session.bindExtensions({});
      await ended;
      await host.drained();
      const kickoff = host.session.messages.find((message) => message.role === "user");
      const kickoffText = typeof kickoff?.content === "string" ? kickoff.content : kickoff?.content[0]?.type === "text"
        ? kickoff.content[0].text : undefined;
      expect(kickoffText).toContain(promptFile);
      for (const text of ["Inspect the supplied prior-work artifact.", "Prior work: include a regression test."]) {
        expect(host.session.messages.some((message) => message.role === "toolResult" && message.toolName === "read"
          && !message.isError && message.content.some((part) => part.type === "text" && part.text.includes(text)))).toBeTrue();
      }
      for (const excludeFromContext of [false, true]) {
        const action = await host.session.extensionRunner.emitUserBash({
          type: "user_bash", command: `touch '${target}'`, cwd, excludeFromContext,
        });
        expect(action?.result?.exitCode).toBe(1);
        expect(action?.operations).toBeUndefined();
      }
      expect(existsSync(target)).toBeFalse();
    } finally {
      await host.close();
    }
  });
}

test("the kickoff is sent once per planning request and again for a rewritten round", () => {
  const handlers = new Map<string, (event: unknown, ctx: unknown) => unknown>();
  const sent: string[] = [];
  const pi = {
    on: (name: string, handler: (event: unknown, ctx: unknown) => unknown) => handlers.set(name, handler),
    sendUserMessage: (text: string) => sent.push(text),
  } as unknown as ExtensionAPI;
  const promptFile = join(work, "prompt.md");
  writeFileSync(promptFile, "Round 1 planning prompt.");
  const start = () => {
    planner.extend?.(pi, { identity: identities.planner, cwd: "/tmp", model: "m", promptFile });
    const handler = handlers.get("session_start");
    if (!handler) throw new Error("the planner handles session_start");
    return handler;
  };
  const session = (texts: string[]) => ({
    sessionManager: {
      getEntries: () => texts.map((content, index) => ({
        type: "message", id: `m${index}`, parentId: null, timestamp: new Date(0).toISOString(),
        message: { role: "user", content, timestamp: 0 },
      })),
    },
  });
  start()({ type: "session_start", reason: "startup" }, session([]));
  expect(sent.length).toBe(1);
  expect(sent[0]).toContain(promptFile);
  const first = sent[0] ?? "";
  sent.length = 0;
  start()({ type: "session_start", reason: "startup" }, session([first]));
  expect(sent).toEqual([]);
  writeFileSync(promptFile, "Round 2 planning prompt.\n\n## Required plan changes\n\nName the test.");
  start()({ type: "session_start", reason: "startup" }, session([first]));
  expect(sent.length).toBe(1);
});
