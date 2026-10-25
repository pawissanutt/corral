// The real reviewer role (T-252 reviewer-role) in a host session: the read-only loadout, the
// contract from the installed assets only, corral_verdict as one envelope, and the kickoff turn.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../../bridge/events.ts";
import type { SubmitInput } from "../../bridge/protocol.ts";
import { loadModules } from "../../host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../../host/restrict.ts";
import { HostSession, authContext } from "../../host/session.ts";
import { CorralClient, sha256Hex, toolRequestId } from "../../tools/corral.ts";
import { fixtureTransport } from "../../../test/fixtures/auth/index.ts";
import { type FakeCorral, writeFakeCorral } from "../../../test/support/fake-corral.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "../../../test/support/fixture-install.ts";
import { nextEvent } from "../../../test/support/wait.ts";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import reviewer, { CONTRACT_ASSET, kickoff, reviewerRole } from "./index.ts";

const identity = {
  store: "st-0123456789abcdef", project: "demo", role: "reviewer" as const, task: "T-007", run: "R2",
  review: "R2-review1", callback: "g1:c1", generation: 1,
};
const UPSTREAM = ["read", "grep", "find", "ls", "bash"];
const CORRAL_TOOLS = ["corral_ask", "corral_withdraw", "corral_evidence", "corral_artifact", "corral_verdict"];
const originalFetch = globalThis.fetch;

let assets: FixtureInstall;
let work: string;
let cwd: string;
let corral: FakeCorral;
let events: EventLog;
let host: HostSession;

const input = (text: string, key: string): SubmitInput => ({ text, busyPolicy: "prompt", idempotencyKey: key, origin: "test" });

/** A fixture installation (synthetic auth, fixture delivery) with the real reviewer role and assets. */
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
  cwd = join(work, "worktree");
  mkdirSync(cwd, { recursive: true });
  restrictEnvironment(process.env);
  const modules = await loadModules(assets.dir, "reviewer");
  events = new EventLog(identity.generation);
  const transport = modules.auth.transport?.(authContext());
  if (!transport) throw new Error("the fixture auth module supplies a transport");
  const network = guardedFetch(transport);
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  corral = writeFakeCorral(work);
  const client = new CorralClient({ corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host.sessionId() });
  host = await HostSession.create({
    sdk, identity, cwd, agentDir: join(work, "pi-agent"), model: "gpt-6.1-sol", modules, client, events, network,
    sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
  });
  await host.session.bindExtensions({});
  await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
}, 60_000);

afterAll(() => {
  globalThis.fetch = originalFetch;
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
});

test("the package's reviewer role is read-only and allows only the reviewer's corral ops", () => {
  expect([...reviewer.upstreamTools]).toEqual(UPSTREAM);
  expect(reviewer.upstreamTools).not.toContain("edit");
  expect(reviewer.upstreamTools).not.toContain("write");
  const tools = reviewer.tools({ identity, cwd: "/tmp", model: "m" });
  expect(tools.map((tool) => tool.name)).toEqual(CORRAL_TOOLS);
  expect([...reviewer.corralOps].sort()).toEqual(
    ["hello", "event", "receipt_lookup", "ask", "withdraw", "evidence", "artifact", "verdict"].sort(),
  );
  expect(tools.every((tool) => reviewer.corralOps.includes(tool.op))).toBeTrue();
});

test("the contract is the installed reviewer-contract.md with the bound identity filled in", () => {
  const contract = reviewer.contract({ identity, cwd: "/tmp", model: "m" });
  expect(contract).toContain("corral task T-007, run R2, review R2-review1, in\nproject demo");
  expect(contract).not.toContain("{{");
  const empty = mkdtempSync(join(tmpdir(), "corral-pi-noassets-"));
  try {
    expect(() => reviewerRole(empty).contract({ identity, cwd: "/tmp", model: "m" })).toThrow(`role_asset_missing: ${CONTRACT_ASSET}`);
  } finally {
    rmSync(empty, { recursive: true, force: true });
  }
});

test("a host session activates exactly the loadout and denies a write the model attempts", async () => {
  expect([...host.loadout]).toEqual([...UPSTREAM, ...CORRAL_TOOLS]);
  expect([...(host.state().active_tools as string[])].sort()).toEqual([...UPSTREAM, ...CORRAL_TOOLS].sort());
  expect(host.session.systemPrompt).toContain("# corral reviewer contract (Pi-native)");
  const target = join(cwd, "edited.txt");
  fixtureTransport().script([
    { toolCalls: [{ callId: "call-write", name: "write", arguments: { path: target, content: "x" } }] },
    { text: "DONE" },
  ]);
  const ended = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 1);
  expect(await host.submit(input("Write a file.", "k-write"), "rq-write")).toEqual({ disposition: "started", turn: 1 });
  await ended;
  await host.drained();
  expect(existsSync(target)).toBeFalse();
});

test("corral_verdict is one verdict envelope whose request ID derives from the session and tool call", async () => {
  const notes = "Changes needed:\n- add the regression test";
  fixtureTransport().script([
    { toolCalls: [{ callId: "call-verdict", name: "corral_verdict", arguments: { verdict: "changes", notes } }] },
    { text: "FINISHED" },
  ]);
  const ended = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 2);
  expect(await host.submit(input("Record changes.", "k-verdict"), "rq-verdict")).toEqual({ disposition: "started", turn: 2 });
  await ended;
  await host.drained();
  const callIds = host.session.messages.flatMap((message) =>
    message.role === "assistant" ? message.content.filter((part) => part.type === "toolCall") : [])
    .filter((part) => part.name === "corral_verdict").map((part) => part.id);
  expect(callIds.length).toBe(1);
  const verdicts = corral.calls().map((call) => call.envelope).filter((envelope) => envelope.op === "verdict");
  expect(verdicts.map((envelope) => [envelope.request_id, envelope.identity.turn, envelope.identity.review, envelope.identity.callback]))
    .toEqual([[toolRequestId(host.sessionId(), callIds[0] ?? ""), 2, "R2-review1", "g1:c1"]]);
  expect(verdicts[0]?.payload).toEqual({ verdict: "changes", notes });
});

test("a fresh session starts one first turn with the review prompt; a reload or a session with messages does not", () => {
  const handlers = new Map<string, (event: unknown, ctx: unknown) => unknown>();
  const sent: string[] = [];
  const pi = {
    on: (name: string, handler: (event: unknown, ctx: unknown) => unknown) => handlers.set(name, handler),
    sendUserMessage: (text: string) => sent.push(text),
  } as unknown as ExtensionAPI;
  const context = { identity, cwd: "/tmp", model: "m", promptFile: "/store/prompts/T-007-R2-review.md" };
  reviewer.extend?.(pi, context);
  const start = handlers.get("session_start");
  if (!start) throw new Error("the reviewer role handles session_start");
  const session = (entries: { type: string }[]) => ({ sessionManager: { getEntries: () => entries } });
  start({ type: "session_start", reason: "reload" }, session([]));
  start({ type: "session_start", reason: "startup" }, session([{ type: "message" }]));
  expect(sent).toEqual([]);
  start({ type: "session_start", reason: "startup" }, session([]));
  expect(sent).toEqual([kickoff(context) ?? ""]);
  expect(sent[0]?.startsWith("Read /store/prompts/T-007-R2-review.md in full and follow it")).toBeTrue();
  expect(sent[0]).toContain("call corral_verdict");
  expect(kickoff({ identity, cwd: "/tmp", model: "m" })).toBeUndefined();
  expect(readFileSync(join(PACKAGE_ROOT, CONTRACT_ASSET), "utf8")).toContain("corral_verdict");
});
