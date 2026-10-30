// The native planning roles (T-460 plan-roles) in real host sessions: read-only loadouts, typed
// corral operations sent with the launcher-bound identity, one kickoff per planning request,
// the module paths Rust preflight checks, and the role_enabled launch requirement.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { cpSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { EventLog } from "../src/bridge/events.ts";
import { LaunchMessageInvalid, parseLaunchMessage } from "../src/host/config.ts";
import { loadModules, missingModules, modulePaths } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import planReviewer from "../src/roles/plan-reviewer/index.ts";
import planner from "../src/roles/planner/index.ts";
import type { HostIdentity, Role } from "../src/roles/types.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { type FakeCorral, writeFakeCorral } from "./support/fake-corral.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "./support/fixture-install.ts";
import { nextEvent } from "./support/wait.ts";

const READ_ONLY = ["read", "grep", "find", "ls"];
const originalFetch = globalThis.fetch;
const identities: Record<"planner" | "plan_reviewer", HostIdentity> = {
  planner: { store: "st-0123456789abcdef", project: "demo", role: "planner", task: "T-009", run: "D1", generation: 1 },
  plan_reviewer: {
    store: "st-0123456789abcdef", project: "demo", role: "plan_reviewer", task: "T-009", run: "P1", review: "PR1",
    generation: 1,
  },
};

let assets: FixtureInstall;
let work: string;

interface Bound {
  readonly host: HostSession;
  readonly events: EventLog;
  readonly corral: FakeCorral;
  readonly cwd: string;
}

/** A fixture installation (synthetic auth and delivery) carrying the real planning role modules. */
function realPlanningInstall(): FixtureInstall {
  const built = buildFixtureInstall();
  for (const tree of ["src/roles/planner", "src/roles/plan-reviewer", "src/roles/common"]) {
    rmSync(join(built.dir, tree), { recursive: true, force: true });
    cpSync(join(PACKAGE_ROOT, tree), join(built.dir, tree), { recursive: true });
  }
  return built;
}

async function bind(role: "planner" | "plan_reviewer"): Promise<Bound> {
  const identity = identities[role];
  const dir = join(work, role);
  const cwd = join(dir, "repo");
  mkdirSync(cwd, { recursive: true });
  const modules = await loadModules(assets.dir, role);
  const events = new EventLog(identity.generation);
  const transport = modules.auth.transport?.(authContext());
  if (!transport) throw new Error("the fixture auth module supplies a transport");
  const network = guardedFetch(transport);
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const corral = writeFakeCorral(dir);
  let host: HostSession | undefined;
  const client = new CorralClient({
    corral: corral.path, root: dir, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "",
  });
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(dir, "pi-agent"), model: "gpt-6.1-sol", modules, client, events, network,
    sessionManager: sdk.SessionManager.create(cwd, join(dir, "sessions")),
  });
  await host.session.bindExtensions({});
  await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
  return { host, events, corral, cwd };
}

async function turn(bound: Bound, n: number, calls: { callId: string; name: string; arguments: Record<string, unknown> }[]) {
  fixtureTransport().script([{ toolCalls: calls }, { text: "DONE" }]);
  const ended = nextEvent(bound.events, (event) => event.kind === "turn_ended" && event.turn === n);
  await bound.host.submit({ text: `Turn ${n}.`, busyPolicy: "prompt", idempotencyKey: `k${n}`, origin: "test" }, `rq${n}`);
  await ended;
  await bound.host.drained();
}

beforeAll(() => {
  assets = realPlanningInstall();
  work = mkdtempSync(join(tmpdir(), "corral-pi-plan-roles-"));
  restrictEnvironment(process.env);
});

afterAll(() => {
  globalThis.fetch = originalFetch;
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
});

test("both planning roles are read-only and call only their own planning operations", () => {
  const context = (role: "planner" | "plan_reviewer") => ({ identity: identities[role], cwd: "/tmp", model: "m" });
  expect([...planner.upstreamTools]).toEqual(READ_ONLY);
  expect([...planReviewer.upstreamTools]).toEqual(READ_ONLY);
  expect(planner.tools(context("planner")).map((tool) => tool.name))
    .toEqual(["corral_ask", "corral_withdraw", "corral_plan_write", "corral_plan_submit"]);
  expect(planReviewer.tools(context("plan_reviewer")).map((tool) => tool.name))
    .toEqual(["corral_ask", "corral_withdraw", "corral_plan_write", "corral_plan_verdict"]);
  expect([...planner.corralOps].sort())
    .toEqual(["ask", "event", "hello", "plan_submit", "plan_write", "receipt_lookup", "withdraw"]);
  expect([...planReviewer.corralOps].sort())
    .toEqual(["ask", "event", "hello", "plan_verdict", "plan_write", "receipt_lookup", "withdraw"]);
  expect(planner.contract(context("planner"))).toContain("design run D1");
  expect(planReviewer.contract(context("plan_reviewer"))).toContain("plan P1 of corral task T-009 (attempt PR1)");
});

test("module paths match the directories corral preflight checks", () => {
  expect(modulePaths("planner").role).toBe("src/roles/planner/index.ts");
  expect(modulePaths("plan_reviewer").role).toBe("src/roles/plan-reviewer/index.ts");
  for (const role of ["planner", "plan_reviewer"] as Role[]) {
    expect(existsSync(join(PACKAGE_ROOT, modulePaths(role).role))).toBeTrue();
    expect(missingModules(PACKAGE_ROOT, role)).toEqual([]);
  }
});

test("a planning role launches only with an enabled role_enabled capability", () => {
  const message = {
    version: 1, store: "st-demo", project: "demo", role: "planner", task: "T-009", run: "D1", generation: 1,
    secrets: { host_cap: "h".repeat(64), ctl_cap: "c".repeat(64), bind_nonce: "n".repeat(64) },
    pane: "p1", cwd: "/work/repo", root: "/work", corral: "/bin/corral",
    assets: "/assets", runtime_dir: "/runtime", session_dir: "/sessions", model: "gpt-6.1-sol",
  };
  const capabilities = (enabled: string[]) => ({ catalog: 2, digest: "d".repeat(64), preset: "standard", enabled });
  expect(parseLaunchMessage(JSON.stringify({ ...message, capabilities: capabilities(["role_enabled"]) })).identity.role)
    .toBe("planner");
  for (const role of ["planner", "plan_reviewer"]) {
    for (const launch of [{ ...message, role }, { ...message, role, capabilities: capabilities([]) }]) {
      expect(() => parseLaunchMessage(JSON.stringify(launch))).toThrow(LaunchMessageInvalid);
    }
  }
});

test("a planner session activates exactly its loadout, refuses source writes and sends typed plan ops", async () => {
  const bound = await bind("planner");
  expect([...bound.host.loadout])
    .toEqual([...READ_ONLY, "corral_ask", "corral_withdraw", "corral_plan_write", "corral_plan_submit"]);
  const target = join(bound.cwd, "edited.txt");
  await turn(bound, 1, [
    { callId: "call-write", name: "write", arguments: { path: target, content: "x" } },
    { callId: "call-bash", name: "bash", arguments: { command: `touch ${target}` } },
  ]);
  expect(existsSync(target)).toBeFalse();
  await turn(bound, 2, [
    { callId: "call-plan", name: "corral_plan_write", arguments: { content: "# Plan\n", offset: 0 } },
    { callId: "call-submit", name: "corral_plan_submit", arguments: {} },
  ]);
  // Pi runs one message's tool calls concurrently, so their envelopes arrive in either order.
  const envelopes = bound.corral.calls().map((call) => call.envelope).filter((e) => e.op.startsWith("plan_"))
    .sort((a, b) => a.op.localeCompare(b.op));
  expect(envelopes.map((e) => [e.op, e.payload, e.identity.role, e.identity.run])).toEqual([
    ["plan_submit", {}, "planner", "D1"],
    ["plan_write", { content: "# Plan\n", offset: 0 }, "planner", "D1"],
  ]);
});

test("a plan reviewer's verdict carries only the verdict and notes; corral binds the attempt", async () => {
  const bound = await bind("plan_reviewer");
  await turn(bound, 1, [
    { callId: "call-verdict", name: "corral_plan_verdict", arguments: { verdict: "changes", notes: "Name the test." } },
  ]);
  const verdicts = bound.corral.calls().map((call) => call.envelope).filter((e) => e.op === "plan_verdict");
  expect(verdicts.map((e) => [e.payload, e.identity.run, e.identity.review])).toEqual([
    [{ verdict: "changes", notes: "Name the test." }, "P1", "PR1"],
  ]);
});

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
  expect(sent[0]).toContain("corral_plan_submit");
  const first = sent[0] ?? "";
  sent.length = 0;
  start()({ type: "session_start", reason: "startup" }, session([first]));
  expect(sent).toEqual([]);
  writeFileSync(promptFile, "Round 2 planning prompt.\n\n## Required plan changes\n\nName the test.");
  start()({ type: "session_start", reason: "startup" }, session([first]));
  expect(sent.length).toBe(1);
});
