// The real worker role (T-252 worker-role) in a host session: the OD3 loadout, the contract and
// skill from the installed assets only, each corral tool as one envelope, and turn attribution.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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
import { expectReplacementsRefused } from "../../../test/support/replacement.ts";
import { nextEvent } from "../../../test/support/wait.ts";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import worker, { CONTRACT_ASSET, SKILL_ASSET, kickoff, workerRole } from "./index.ts";

const identity = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const, task: "T-007", run: "R2", generation: 1 };
const UPSTREAM = ["read", "edit", "write", "bash", "grep", "find", "ls"];
const CORRAL_TOOLS = [
  "corral_ask", "corral_withdraw", "corral_wait", "corral_progress", "corral_checkpoint", "corral_evidence",
  "corral_artifact", "corral_gate", "corral_done",
];
const LSP_TOOLS = ["lsp_diagnostics", "lsp_definition", "lsp_references"];
const originalFetch = globalThis.fetch;

let assets: FixtureInstall;
let work: string;
let corral: FakeCorral;
let events: EventLog;
let host: HostSession;

const input = (text: string, key: string): SubmitInput => ({ text, busyPolicy: "prompt", idempotencyKey: key, origin: "test" });

/** A fixture installation (synthetic auth, fixture delivery) with the real worker role and assets. */
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
  // InteractiveMode binds extensions before its first frame; that is when resources_discover runs.
  await host.session.bindExtensions({});
  await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
}, 60_000);

afterAll(() => {
  globalThis.fetch = originalFetch;
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
});

test("the package's worker role is the OD3 loadout and allows only the worker's corral ops", () => {
  expect([...worker.upstreamTools]).toEqual(UPSTREAM);
  const context = { identity, cwd: "/tmp", model: "m" };
  const tools = worker.tools(context);
  expect(tools.map((tool) => tool.name)).toEqual(CORRAL_TOOLS);
  expect([...worker.corralOps].sort()).toEqual(
    ["hello", "event", "receipt_lookup", "ask", "withdraw", "wait", "progress", "checkpoint", "evidence", "artifact",
      "gate", "done"].sort(),
  );
  expect(tools.every((tool) => worker.corralOps.includes(tool.op))).toBeTrue();
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

test("a host session activates exactly the loadout and loads only the installed corral-worker skill", () => {
  const state = host.state();
  expect([...host.loadout]).toEqual([...UPSTREAM, ...CORRAL_TOOLS, ...LSP_TOOLS]);
  expect([...(state.active_tools as string[])].sort()).toEqual([...UPSTREAM, ...CORRAL_TOOLS, ...LSP_TOOLS].sort());
  const skills = host.runtime.services.resourceLoader.getSkills().skills;
  expect(skills.map((skill) => [skill.name, skill.filePath])).toEqual([["corral-worker", join(assets.dir, SKILL_ASSET)]]);
  expect(readFileSync(skills[0]?.filePath ?? "", "utf8")).toBe(readFileSync(join(PACKAGE_ROOT, SKILL_ASSET), "utf8"));
  expect(host.session.systemPrompt).toContain("# corral worker contract (Pi-native)");
});

test("each corral tool call is one envelope whose request ID derives from the session and tool call", async () => {
  const transport = fixtureTransport();
  transport.script([
    { toolCalls: [{ callId: "call-gate", name: "corral_gate", arguments: {} }] },
    { toolCalls: [{ callId: "call-done", name: "corral_done", arguments: {
      summary: "Did it.\nVerified: x\nNot verified: none", contract_sha256: "ab".repeat(32),
      gate_ref: { attempt: "ag-0123456789abcdef", tier: "quick", receipt_sha256: "cd".repeat(32) },
    } }] },
    { text: "FINISHED" },
  ]);
  const ended = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 1);
  expect(await host.submit(input("Start the gate, then finish.", "k-tools"), "rq-tools")).toEqual({ disposition: "started", turn: 1 });
  await ended;
  await host.drained();
  const callIds = host.session.messages.flatMap((message) =>
    message.role === "assistant" ? message.content.filter((part) => part.type === "toolCall") : [])
    .map((part) => [part.name, part.id]);
  expect(callIds.map(([name]) => name)).toEqual(["corral_gate", "corral_done"]);
  const tools = corral.calls().map((call) => call.envelope).filter((envelope) => ["gate", "done"].includes(envelope.op));
  expect(tools.map((envelope) => [envelope.op, envelope.request_id, envelope.identity.turn, envelope.identity.run])).toEqual([
    ["gate", toolRequestId(host.sessionId(), callIds[0]?.[1] ?? ""), 1, "R2"],
    ["done", toolRequestId(host.sessionId(), callIds[1]?.[1] ?? ""), 1, "R2"],
  ]);
  expect(tools[0]?.payload).toEqual({});
  expect(tools[1]?.payload).toEqual({
    summary: "Did it.\nVerified: x\nNot verified: none", contract_sha256: "ab".repeat(32),
    gate_ref: { attempt: "ag-0123456789abcdef", tier: "quick", receipt_sha256: "cd".repeat(32) },
  });
});

test("bridge input and owner terminal input are attributed per turn ID", async () => {
  const bridgeEnded = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 2);
  expect(await host.submit(input("Reply only BRIDGE.", "k-bridge"), "rq-bridge")).toEqual({ disposition: "started", turn: 2 });
  await bridgeEnded;
  const ownerEnded = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 3);
  await host.session.prompt("Reply only OWNER.", { source: "interactive" });
  await ownerEnded;
  await host.drained();
  const started = corral.calls().map((call) => call.envelope.payload)
    .filter((payload) => payload.kind === "turn_started" && [2, 3].includes(payload.turn as number));
  expect(started).toEqual([
    { kind: "turn_started", turn: 2, origin: { kind: "bridge", idempotency_key: "k-bridge" } },
    { kind: "turn_started", turn: 3, origin: { kind: "owner_terminal" } },
  ]);
});

test("while bound, the worker refuses /new, /resume, /fork and import before the session changes", async () => {
  await expectReplacementsRefused(host, events);
});

test("a fresh session starts one first turn with the run prompt; a reload or a session with messages does not", () => {
  const handlers = new Map<string, (event: unknown, ctx: unknown) => unknown>();
  const sent: string[] = [];
  const pi = {
    on: (name: string, handler: (event: unknown, ctx: unknown) => unknown) => handlers.set(name, handler),
    sendUserMessage: (text: string) => sent.push(text),
  } as unknown as ExtensionAPI;
  const context = { identity, cwd: "/tmp", model: "m", promptFile: "/store/prompts/T-007-R2.md" };
  worker.extend?.(pi, context);
  const start = handlers.get("session_start");
  if (!start) throw new Error("the worker role handles session_start");
  const session = (entries: { type: string }[]) => ({ sessionManager: { getEntries: () => entries } });
  start({ type: "session_start", reason: "reload" }, session([]));
  start({ type: "session_start", reason: "startup" }, session([{ type: "message" }]));
  expect(sent).toEqual([]);
  start({ type: "session_start", reason: "startup" }, session([{ type: "model_change" }]));
  expect(sent).toEqual([kickoff(context) ?? ""]);
  expect(sent[0]?.startsWith("Read /store/prompts/T-007-R2.md in full and follow it")).toBeTrue();
  expect(sent[0]).toContain("call corral_gate");
  expect(kickoff({ identity, cwd: "/tmp", model: "m" })).toBeUndefined();
});

test("in a real host the kickoff turn starts after hello, when extensions bind, and reaches corral", async () => {
  const modules = await loadModules(assets.dir, "worker");
  const log = new EventLog(identity.generation);
  const network = guardedFetch(modules.auth.transport?.(authContext()) ?? globalThis.fetch);
  const sdk = await import("@earendil-works/pi-coding-agent");
  const dir = mkdtempSync(join(work, "kickoff-"));
  const fake = writeFakeCorral(dir);
  const promptFile = join(dir, "T-007-R2.md");
  writeFileSync(promptFile, "# Task\n\nReply only KICKED.\n");
  let started: HostSession | undefined;
  const client = new CorralClient({ corral: fake.path, root: dir, identity, hostCap: "h".repeat(64), sessionId: () => started?.sessionId() ?? "" });
  started = await HostSession.create({
    sdk, identity, cwd: join(work, "worktree"), agentDir: join(dir, "pi-agent"), model: "gpt-6.1-sol", modules, client,
    events: log, network, promptFile, sessionManager: sdk.SessionManager.create(join(work, "worktree"), join(dir, "sessions")),
  });
  await started.hello("n".repeat(64), "pane-2", sha256Hex("c".repeat(64)));
  expect(log.since(0, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER).events.some((e) => e.kind === "turn_started")).toBeFalse();
  const ended = nextEvent(log, (event) => event.kind === "turn_ended" && event.turn === 1);
  await started.session.bindExtensions({});
  await ended;
  await started.drained();
  const first = started.session.messages.find((message) => message.role === "user");
  const text = first && typeof first.content !== "string" ? first.content.map((part) => part.type === "text" ? part.text : "").join("") : first?.content;
  expect(text).toBe(kickoff({ identity, cwd: "/tmp", model: "m", promptFile }));
  const lifecycle = fake.calls().map((call) => call.envelope).filter((envelope) => envelope.op === "event").map((envelope) => envelope.payload);
  expect(lifecycle).toEqual([
    { kind: "turn_started", turn: 1, origin: { kind: "unknown" } },
    { kind: "turn_ended", turn: 1 },
  ]);
});
