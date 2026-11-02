// Code Mode and request continuation (T-683 review F2): the production PM host, in process, over the
// real corral binary and an isolated store from the Rust PM lane probe, inside scripts/test-herdr.sh.
// Corral's own T-679 ledger decides every continuation. The PM opens a plan, detaches a cell and
// records that step's event wait; corral continues the independent step once and, with only the
// waiting step left, wakes nothing; the cell's settlement then arrives as exactly one bridge-origin
// turn, after which the plan is complete and nothing continues.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession } from "../src/host/session.ts";
import type { RoleContext } from "../src/roles/types.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { type FakeTransport, fakeCodexTransport } from "./fixtures/auth/fake-transport.ts";
import { type FixtureInstall, buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { nextEvent } from "./support/wait.ts";
import { corralBinary, ok, privateHerdr, privateRoot, probeBinary, run } from "./support/workspace.ts";

interface Lane {
  readonly store: string;
  readonly project: string;
  readonly generation: number;
  readonly nonce: string;
  readonly sessions: string;
}

const isolated = privateHerdr();
const originalFetch = globalThis.fetch;
const originalData = process.env.XDG_DATA_HOME;
let workspace = "";
let workspaceId = "";
let pane = "";
let corral = "";
let lane: Lane;
let assets: FixtureInstall | undefined;
let transport: FakeTransport;
let events: EventLog;
let host: HostSession;
let release: (text: string) => void = () => undefined;
let held: Promise<void> = Promise.resolve();

const pmState = (): any => JSON.parse(ok([corral, "agent", "state", "--pm", "--json"], { cwd: workspace }));

function syntheticAccess(): string {
  const part = (value: object) => Buffer.from(JSON.stringify(value)).toString("base64url");
  return `${part({ alg: "none", typ: "JWT" })}.${part({
    exp: Math.floor(Date.now() / 1000) + 3600,
    "https://api.openai.com/auth": { chatgpt_account_id: "synthetic" },
  })}.synthetic-signature`;
}

beforeAll(async () => {
  if (!isolated) {
    process.stdout.write("codemode continuation cross-lane: skipped (requires the private server from scripts/test-herdr.sh)\n");
    return;
  }
  corral = corralBinary();
  const probe = probeBinary();
  workspace = mkdtempSync(join(privateRoot() ?? "", "pi-codemode-continuation-"));
  const created = JSON.parse(ok([
    "herdr", "--session", "corral-test", "workspace", "create", "--cwd", workspace, "--no-focus",
  ]));
  workspaceId = created.result.workspace.workspace_id;
  pane = created.result.root_pane.pane_id;
  const setup = run([
    probe, "--exact", "agent::hooks::pm::tests::pm_cross_lane_setup", "--ignored", "--nocapture", "--test-threads", "1",
  ], { env: { ...process.env, CORRAL_PM_LANE: JSON.stringify({ root: workspace, workspace: workspaceId, pane }) } });
  const output = /PM_LANE (\{.*\})/.exec(setup.stdout)?.[1];
  if (setup.status !== 0 || !output) {
    throw new Error(`PM fixture contract not fulfilled: ${setup.stdout.slice(-2000)} ${setup.stderr.slice(-2000)}`);
  }
  lane = JSON.parse(output);
  assets = buildFixtureInstall({ parent: workspace, overlays: false });
  cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
  const data = join(workspace, "data");
  const auth = join(data, "corral/pi-native/auth");
  mkdirSync(auth, { recursive: true, mode: 0o700 });
  writeFileSync(join(auth, "auth.json"), JSON.stringify({
    "openai-codex": {
      type: "oauth", access: syntheticAccess(), refresh: "synthetic-refresh",
      expires: Date.now() + 3_600_000, accountId: "synthetic",
    },
  }), { mode: 0o600 });
  process.env.XDG_DATA_HOME = data;
  restrictEnvironment(process.env);
  const loaded = await loadModules(assets.dir, "pm");
  let modules = loaded;
  // The cell's nested call waits on this in-process gate, so its settlement is driven by the test.
  held = new Promise<void>((started) => {
    const hold: ToolDefinition = {
      name: "fixture_hold", label: "Hold", description: "Waits until the test releases it.", parameters: Type.Object({}),
      async execute() {
        started();
        return { content: [{ type: "text", text: await new Promise<string>((resolve) => { release = resolve; }) }], details: undefined };
      },
    };
    const base = loaded.role;
    modules = { ...loaded, role: { ...base, localToolNames: [...(base.localToolNames ?? []), hold.name],
      localTools: (context: RoleContext) => [...(base.localTools?.(context) ?? []), hold] } };
  });
  transport = fakeCodexTransport();
  const network = guardedFetch(transport.send);
  globalThis.fetch = network.fetch;
  events = new EventLog(lane.generation);
  const identity = { store: lane.store, project: lane.project, role: "pm" as const, generation: lane.generation };
  const client = new CorralClient({
    corral, root: workspace, identity, hostCap: "e".repeat(64), sessionId: () => host.sessionId(),
  });
  const sdk = await import("@earendil-works/pi-coding-agent");
  host = await HostSession.create({
    sdk, identity, cwd: workspace, agentDir: join(workspace, "pi-agent"), model: "gpt-6.1-sol",
    capabilities: { catalog: 4, digest: "c".repeat(64), preset: "full", enabled: ["todo", "codemode"] },
    modules, client, events, network, sessionManager: sdk.SessionManager.create(workspace, lane.sessions),
  });
  await host.session.bindExtensions({});
  await host.hello(lane.nonce, pane, sha256Hex("d".repeat(64)));
}, 900_000);

afterAll(() => {
  release("released at teardown");
  globalThis.fetch = originalFetch;
  if (originalData === undefined) delete process.env.XDG_DATA_HOME;
  else process.env.XDG_DATA_HOME = originalData;
  if (workspaceId) run(["herdr", "--session", "corral-test", "workspace", "close", workspaceId]);
  assets?.cleanup();
  if (workspace) rmSync(workspace, { recursive: true, force: true });
});

const todo = (callId: string, args: Record<string, unknown>) => ({ toolCalls: [{ callId, name: "corral_todo", arguments: args }] });

/** Every turn report, the continuation decision it led to, and that decision's own reports. */
async function quiet(): Promise<void> {
  await host.drained();
  await host.continued();
  await host.drained();
}

test("a detached cell's waiting step suspends continuation while independent work continues, then one turn reports it", async () => {
  if (!isolated) return;
  // The bound generation keeps a canonical plan, so corral itself decides continuation for its turns.
  const initial = pmState().container;
  expect([initial.generation, initial.todo_enabled]).toEqual([lane.generation, true]);
  const kinds = () => events.since(0, Number.MAX_SAFE_INTEGER, Number.MAX_SAFE_INTEGER).events.map((event) => event);
  // Given a PM turn that opens a plan, detaches a cell and records that step's wait, and the model's
  // next replies for the independent step and for the cell's result:
  transport.script([
    todo("open", { op: "init", phases: [{ id: "work", label: "Work", steps: [
      { id: "cell", label: "Use the cell result" }, { id: "side", label: "Independent side work" },
    ] }] }),
    { toolCalls: [{ callId: "detach", name: "codemode", arguments: {
      code: "const value = await tools.fixture_hold({}); return value;", wait_ms: 0,
    } }] },
    todo("wait", { op: "wait", step: "cell", on: { kind: "event", event: "code cell cell-1" } }),
    { text: "The cell runs in the background; the side work remains." },
    todo("side-start", { op: "start", step: "side" }),
    todo("side-done", { op: "done", step: "side" }),
    { text: "Side work done; waiting for the cell." },
    todo("cell-start", { op: "start", step: "cell" }),
    todo("cell-done", { op: "done", step: "cell" }),
    { text: "Used the cell result." },
  ]);
  const continued = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 2, 120_000);
  // When the owner's turn ends:
  expect((await host.submit({ text: "Plan and start.", busyPolicy: "prompt", idempotencyKey: "pm-plan", origin: "test" },
    "rq-pm-plan")).disposition).toBe("started");
  await held;
  await continued;
  await quiet();

  // Then corral continued the independent step exactly once, and with only the waiting step open it
  // decided waiting_event with no continuation, so nothing woke the model while the cell ran:
  const waiting = pmState().container;
  expect(waiting.todo.phases[0].steps.map((step: any) => [step.id, step.status.state])).toEqual([["cell", "pending"], ["side", "done"]]);
  expect(waiting.todo_requests.turn).toMatchObject({ turn: 2, disposition: { state: "waiting_event", event: "code cell cell-1" },
    continuation: { state: "not_due" } });
  expect(host.state().turn).toEqual({ current: 2, open: false });
  expect(kinds().filter((event) => event.kind === "continuation")).toHaveLength(1);

  // When the cell settles:
  const reported = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 3, 120_000);
  const notified = nextEvent(events, (event) => event.kind === "code_cell_notified", 120_000);
  release("the cell's value");
  expect(await notified).toMatchObject({ cell: "cell-1", key: `code:g${lane.generation}:cell-1`, disposition: "started" });
  await reported;
  await quiet();

  // Then exactly one bridge-origin turn reported it and finished the plan, after which corral
  // decided the request complete with no continuation:
  const origins = kinds().filter((event) => event.kind === "turn_started").map((event) => event.origin);
  expect(origins.map((origin: any) => origin.kind)).toEqual(["bridge", "continuation", "bridge"]);
  expect(origins[0]).toEqual({ kind: "bridge", idempotency_key: "pm-plan" });
  expect(/^continue:P\d+:\d+$/.test((origins[1] as any).idempotency_key)).toBe(true);
  expect(origins[2]).toEqual({ kind: "bridge", idempotency_key: `code:g${lane.generation}:cell-1` });
  const done = pmState().container;
  expect(done.todo.phases[0].steps.map((step: any) => step.status.state)).toEqual(["done", "done"]);
  expect(done.todo_requests.turn).toMatchObject({ turn: 3, disposition: { state: "complete" }, continuation: { state: "not_due" } });
  expect(kinds().filter((event) => event.kind === "continuation")).toHaveLength(1);
  expect(kinds().filter((event) => event.kind === "continuation_stopped")).toEqual([]);
  const notices = host.session.messages.filter((message) => message.role === "user").map((message) =>
    typeof message.content === "string" ? message.content : message.content.map((part) => part.type === "text" ? part.text : "").join(""))
    .filter((text) => text.startsWith("[corral code cell cell-1 settled: ok]"));
  expect(notices).toHaveLength(1);
  expect(notices[0]).toContain("Return value: the cell's value");
  expect(host.state().turn).toEqual({ current: 3, open: false });
}, 600_000);
