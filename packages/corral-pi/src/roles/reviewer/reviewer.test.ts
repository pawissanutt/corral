// Production review loadout and its verdict command with the offline provider.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../../bridge/events.ts";
import { loadModules } from "../../host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../../host/restrict.ts";
import { HostSession } from "../../host/session.ts";
import { CorralClient } from "../../tools/corral.ts";
import { fakeCodexTransport } from "../../../test/fixtures/auth/fake-transport.ts";
import { writeFakeEngine } from "../../../test/support/fake-engine.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "../../../test/support/fixture-install.ts";
import { expectReplacementsRefused } from "../../../test/support/replacement.ts";
import { nextEvent } from "../../../test/support/wait.ts";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { RoleContext } from "../types.ts";

import reviewer, { CONTRACT_ASSET, kickoff, reviewerRole } from "./index.ts";

const identity = {
  store: "st-0123456789abcdef", project: "demo", role: "reviewer" as const, task: "T-007", run: "R2",
};
const UPSTREAM = ["read", "grep", "find", "ls", "bash"];
const CORRAL_TOOLS = ["corral_artifact", "corral_verdict"];
const originalFetch = globalThis.fetch;

let assets: FixtureInstall;
let work: string;

/** A fixture installation with the offline provider and production reviewer and assets. */
function realReviewerInstall(): FixtureInstall {
  const built = buildFixtureInstall();
  rmSync(join(built.dir, "src/roles/reviewer"), { recursive: true, force: true });
  for (const tree of ["src/roles/reviewer", "src/roles/common", "assets"]) {
    cpSync(join(PACKAGE_ROOT, tree), join(built.dir, tree), { recursive: true });
  }
  return built;
}

beforeAll(async () => {
  assets = realReviewerInstall();
  work = mkdtempSync(join(tmpdir(), "corral-pi-reviewer-"));
  mkdirSync(join(work, "worktree"));
  restrictEnvironment(process.env);
}, 60_000);

async function startReviewer() {
  const dir = mkdtempSync(join(work, "run-"));
  const cwd = join(work, "worktree");
  const modules = await loadModules(assets.dir, "reviewer");
  const events = new EventLog();
  const transport = fakeCodexTransport();
  const network = guardedFetch(transport.send);
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const corral = writeFakeEngine(dir, { "review-verdict": { result: { recorded: true } } });
  const client = new CorralClient({ corral: corral.path, root: dir, cwd, sessionDir: dir, identity });
  const host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(dir, "pi-agent"), model: "gpt-6.1-sol", modules, client, events, network,
    sessionManager: sdk.SessionManager.create(cwd, join(dir, "sessions")),
    capabilities: { catalog: 5, enabled: ["ask_user"] },
  });
  expect(corral.calls()).toEqual([]);
  await host.startLocal();
  await host.session.bindExtensions({});
  return { host, events, cwd, corral, dir, transport };
}

afterAll(() => {
  globalThis.fetch = originalFetch;
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
});

test("the reviewer has read-only file tools plus task artifact and verdict", () => {
  expect([...reviewer.upstreamTools]).toEqual(UPSTREAM);
  expect(reviewer.upstreamTools).not.toContain("edit");
  expect(reviewer.upstreamTools).not.toContain("write");
  const tools = reviewer.tools({ identity, cwd: "/tmp", model: "m" });
  expect(tools.map((tool) => tool.name)).toEqual(CORRAL_TOOLS);
  expect(tools.map((tool) => tool.op)).toEqual(["artifact", "verdict"]);
});

test("the contract is the installed reviewer-contract.md with the bound identity filled in", () => {
  const contract = reviewer.contract({ identity, cwd: "/tmp", model: "m" });
  expect(contract).toContain("corral task T-007 in project demo");
  expect(contract).not.toContain("{{");
  const empty = mkdtempSync(join(tmpdir(), "corral-pi-noassets-"));
  try {
    expect(() => reviewerRole(empty).contract({ identity, cwd: "/tmp", model: "m" })).toThrow(`role_asset_missing: ${CONTRACT_ASSET}`);
  } finally {
    rmSync(empty, { recursive: true, force: true });
  }
});

test("a host session activates exactly the loadout and denies a write the model attempts", async () => {
  const { host, events, cwd, transport } = await startReviewer();
  try {
    const expected = [...UPSTREAM, ...CORRAL_TOOLS, "ask_user", "agent_message", "agent_roster"];
    expect([...host.loadout]).toEqual(expected);
    expect([...(host.state().active_tools as string[])].sort()).toEqual(expected.sort());
    expect(host.session.systemPrompt).toContain("# corral reviewer contract (corral-pi)");
    const target = join(cwd, "edited.txt");
    transport.script([
      { toolCalls: [{ callId: "call-write", name: "write", arguments: { path: target, content: "x" } }] },
      { text: "DONE" },
    ]);
    const ended = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 1);
    expect(await host.submitLocal({ text: "Write a file.", busyPolicy: "prompt", origin: "owner" }))
      .toEqual({ disposition: "started", turn: 1 });
    await ended;
    await host.drained();
    expect(host.session.messages.find((message) => message.role === "toolResult" && message.toolName === "write"))
      .toMatchObject({ isError: true });
    expect(existsSync(target)).toBeFalse();
  } finally {
    await host.close();
  }
});

test("corral_verdict saves the notes and sends the bound task, run and request to the engine", async () => {
  const { host, events, corral, dir, transport } = await startReviewer();
  try {
    const request = `15-0-${"a".repeat(40)}`;
    const notes = "Changes needed:\n- add the regression test";
    transport.script([
      { toolCalls: [{ callId: "call-verdict", name: "corral_verdict", arguments: { request, verdict: "changes", notes } }] },
    ]);
    const ended = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 1);
    expect(await host.submitLocal({ text: "Record changes.", busyPolicy: "prompt", origin: "owner" }))
      .toEqual({ disposition: "started", turn: 1 });
    await ended;
    await host.drained();
    expect(corral.calls().filter((argv) => argv[3] === "review-verdict")).toEqual([
      ["--root", dir, "--json", "review-verdict", identity.task, identity.run, request],
    ]);
    expect(JSON.parse(readFileSync(join(dir, `verdict-${identity.run}-${request}.json`), "utf8"))).toEqual({ verdict: "changes", notes });
  } finally {
    await host.close();
  }
});

test("review kickoff is keyed by round and prompt content and retries only unobserved requests", () => {
  const handlers = new Map<string, (event: unknown, ctx: unknown) => unknown>();
  const sent: string[] = [];
  const pi = {
    on: (name: string, handler: (event: unknown, ctx: unknown) => unknown) => handlers.set(name, handler),
    sendUserMessage: (text: string) => sent.push(text),
  } as unknown as ExtensionAPI;
  const promptFile = join(work, "T-007-R2-review.md");
  writeFileSync(promptFile, "First candidate\n\n## Review round 0\n");
  const context = { identity, cwd: "/tmp", model: "m", promptFile };
  const install = (bound: RoleContext) => {
    reviewer.extend?.(pi, bound);
    const start = handlers.get("session_start");
    if (!start) throw new Error("the reviewer role handles session_start");
    return start;
  };
  const session = (contents: (string | { type: "text"; text: string }[])[]) => ({
    sessionManager: {
      getEntries: () => contents.map((content, index) => ({
        type: "message", id: `m${index}`, parentId: null, timestamp: new Date(0).toISOString(),
        message: { role: "user", content, timestamp: 0 },
      })),
    },
  });
  const start = install(context);
  start({ type: "session_start", reason: "reload" }, session([]));
  expect(sent).toEqual([]);
  start({ type: "session_start", reason: "startup" }, session(["a previous review request"]));
  expect(sent.length).toBe(1);
  const observed = sent[0];
  if (!observed) throw new Error("review request not emitted");
  expect(observed.startsWith(`${kickoff(context)}\n\n[corral review request `)).toBeTrue();
  // Rebinding the same host cannot enqueue another request before its first one is persisted.
  start({ type: "session_start", reason: "startup" }, session([]));
  expect(sent.length).toBe(1);

  sent.length = 0;
  install(context)({ type: "session_start", reason: "startup" }, session([[{ type: "text", text: observed }]]));
  expect(sent).toEqual([]);
  const copiedPrompt = join(work, "copied-review.md");
  writeFileSync(copiedPrompt, readFileSync(promptFile, "utf8"));
  install({ ...context, promptFile: copiedPrompt })({ type: "session_start", reason: "startup" }, session([observed]));
  expect(sent).toEqual([]);
  // Recovery after a crash before observation still needs to deliver the current attempt.
  install(context)({ type: "session_start", reason: "startup" }, session(["a previous review request"]));
  expect(sent.length).toBe(1);

  sent.length = 0;
  writeFileSync(promptFile, "First candidate\n\n## Review round 1\n");
  install(context)({ type: "session_start", reason: "startup" }, session([observed]));
  expect(sent.length).toBe(1);

  sent.length = 0;
  writeFileSync(promptFile, "Rewritten candidate\n\n## Review round 0\n");
  install(context)({ type: "session_start", reason: "startup" }, session([observed]));
  expect(sent.length).toBe(1);
  sent.length = 0;
  install(context)({ type: "session_start", reason: "startup" }, session([]));
  expect(sent.length).toBe(1);
  expect(kickoff({ identity, cwd: "/tmp", model: "m" })).toBeUndefined();
});

test("while bound, the reviewer refuses /new, /resume, /fork and import before the session changes", async () => {
  const { host, events, transport } = await startReviewer();
  try {
    transport.script([{ text: "Ready." }]);
    const ended = nextEvent(events, (event) => event.kind === "turn_ended");
    expect((await host.submitLocal({ text: "Confirm ready.", busyPolicy: "prompt", origin: "owner" })).disposition).toBe("started");
    await ended;
    await host.drained();
    await expectReplacementsRefused(host, events);
  } finally {
    await host.close();
  }
});
