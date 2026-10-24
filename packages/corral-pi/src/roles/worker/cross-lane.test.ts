// Worker-role cross-lane fixture (T-252 worker-role): the production worker, auth and delivery
// modules in one host, against the corral binary built from this checkout and the private
// plugin-free herdr server of scripts/test-herdr.sh. The credential is synthetic and the fake
// provider transport sits at the host's network boundary (its guarded fetch), so nothing leaves
// the machine. The host runs in this process so that boundary can be substituted; it binds
// through hello and serves its bridge socket exactly as `src/host/main.ts` does.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, watch, writeFileSync } from "node:fs";
import { spawn } from "node:child_process";
import { join } from "node:path";

import { EventLog } from "../../bridge/events.ts";
import { type BridgeServer, startBridge } from "../../bridge/server.ts";
import { loadModules } from "../../host/modules.ts";
import { loadPeerLookup } from "../../host/peer.ts";
import { guardedFetch, restrictEnvironment } from "../../host/restrict.ts";
import { HostSession } from "../../host/session.ts";
import { CorralClient, sha256Hex } from "../../tools/corral.ts";
import { type FakeTransport, fakeCodexTransport } from "../../../test/fixtures/auth/fake-transport.ts";
import { type FixtureInstall, buildFixtureInstall, PACKAGE_ROOT } from "../../../test/support/fixture-install.ts";
import { within, nextEvent } from "../../../test/support/wait.ts";
import { EventFollower, corralBinary, generationDir, ok, privateHerdr, privateRoot, probeBinary, run } from "../../../test/support/workspace.ts";

const isolated = privateHerdr();
const CTL = "d".repeat(64);
const HOST_CAP = "e".repeat(64);
const SUMMARY = "Shipped it.\nVerified: worker cross-lane fixture\nNot verified: none";
const originalFetch = globalThis.fetch;
const originalData = process.env.XDG_DATA_HOME;

interface Lane {
  task: string; run: string; generation: number; nonce: string; store: string; project: string;
  prompt_file: string; contract_sha256: string; sessions: string;
}

let corral = "";
let workspace = "";
let workspaceId = "";
let assets: FixtureInstall | undefined;
let lane: Lane;
let transport: FakeTransport;
let events: EventLog;
let host: HostSession;
let bridge: BridgeServer | undefined;
let follower: EventFollower | undefined;

const cli = (...args: string[]) => ok([corral, ...args], { cwd: workspace });
/** A corral process that may call back into this process's bridge, so the event loop keeps serving it. */
function cliAsync(...args: string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    const child = spawn(corral, args, { cwd: workspace, stdio: ["ignore", "pipe", "pipe"] });
    const out: Buffer[] = [];
    const err: Buffer[] = [];
    child.stdout.on("data", (chunk: Buffer) => out.push(chunk));
    child.stderr.on("data", (chunk: Buffer) => err.push(chunk));
    child.on("error", reject);
    child.on("close", (code) => code === 0 ? resolve(Buffer.concat(out).toString("utf8"))
      : reject(new Error(`corral ${args.join(" ")} exited ${code}: ${Buffer.concat(err).toString("utf8").slice(-2000)}`)));
  });
}
const show = (id: string) => JSON.parse(cli("show", id, "--json"));

/** A synthetic OpenAI Codex OAuth access token: unsigned, accepted only by the fake transport. */
function syntheticAccess(): string {
  const part = (value: object) => Buffer.from(JSON.stringify(value)).toString("base64url");
  const claims = { exp: Math.floor(Date.now() / 1000) + 3600, "https://api.openai.com/auth": { chatgpt_account_id: "synthetic" } };
  return `${part({ alg: "none", typ: "JWT" })}.${part(claims)}.synthetic-signature`;
}

beforeAll(async () => {
  if (!isolated) {
    process.stdout.write("worker cross-lane: skipped (needs the private corral-test server of scripts/test-herdr.sh)\n");
    return;
  }
  corral = corralBinary();
  const probe = probeBinary();
  workspace = mkdtempSync(join(privateRoot() ?? "", "pi-worker-"));
  const app = join(workspace, "app");
  mkdirSync(app);
  for (const argv of [["init", "-q", "-b", "main"], ["config", "user.name", "corral test"],
    ["config", "user.email", "corral@test.invalid"]]) ok(["git", "-C", app, ...argv]);
  writeFileSync(join(app, "README.md"), "# app\n");
  ok(["git", "-C", app, "add", "README.md"]);
  ok(["git", "-C", app, "commit", "-q", "-m", "initial commit"]);
  const created = JSON.parse(ok(["herdr", "--session", "corral-test", "workspace", "create", "--cwd", app, "--no-focus"]));
  workspaceId = created.result.workspace.workspace_id;
  const pane: string = created.result.root_pane.pane_id;
  const setup = run([probe, "--exact", "agent::wake::tests::worker_cross_lane_setup", "--ignored", "--nocapture",
    "--test-threads", "1"], { env: { ...process.env, CORRAL_WORKER_LANE: JSON.stringify({ root: workspace, workspace: workspaceId, pane }) } });
  // libtest prints the test name on the same line as the probe's output.
  const line = /WORKER_LANE (\{.*\})/.exec(setup.stdout)?.[1];
  if (!line) throw new Error(`worker lane setup printed no line: ${setup.stdout.slice(-2000)} ${setup.stderr.slice(-2000)}`);
  lane = JSON.parse(line) as Lane;

  // The production installation: real worker, delivery and auth modules, no fixture.json.
  assets = buildFixtureInstall({ parent: workspace, overlays: false });
  cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
  expect(existsSync(join(assets.dir, "fixture.json"))).toBeFalse();
  const data = join(workspace, "data");
  const auth = join(data, "corral/pi-native/auth");
  mkdirSync(auth, { recursive: true, mode: 0o700 });
  writeFileSync(join(auth, "auth.json"), JSON.stringify({ "openai-codex": {
    type: "oauth", access: syntheticAccess(), refresh: "synthetic-refresh", expires: Date.now() + 3_600_000,
    accountId: "synthetic" } }), { mode: 0o600 });
  process.env.XDG_DATA_HOME = data;

  restrictEnvironment(process.env);
  const modules = await loadModules(assets.dir, "worker");
  expect(modules.auth.state?.()).toEqual({ module: "corral-pi", credential: "absent" });
  expect(modules.delivery.state).toBeDefined();
  transport = fakeCodexTransport();
  const network = guardedFetch(transport.send);
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  events = new EventLog(lane.generation);
  const identity = { store: lane.store, project: lane.project, role: "worker" as const, task: lane.task, run: lane.run,
    generation: lane.generation };
  const client = new CorralClient({ corral, root: workspace, identity, hostCap: HOST_CAP, sessionId: () => host.sessionId() });
  const sdk = await import("@earendil-works/pi-coding-agent");
  host = await HostSession.create({
    sdk, identity, cwd: app, agentDir: join(workspace, "pi-agent"), model: "gpt-6.1-sol", modules, client, events,
    network, promptFile: lane.prompt_file, sessionManager: sdk.SessionManager.create(app, lane.sessions),
  });
  const dir = generationDir(workspace, lane.task, lane.run, lane.generation);
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  writeFileSync(join(dir, "ctl.cap"), CTL, { mode: 0o600 });
  const session = host;
  bridge = await startBridge({
    path: join(dir, "host.sock"), ctlCap: CTL, generation: lane.generation, uid: process.getuid?.() ?? -1,
    peer: await loadPeerLookup(), events,
    handlers: {
      hello: () => ({ protocol: 1, generation: lane.generation, role: "worker", bound: session.bound,
        session_id: session.sessionId(), last_seq: events.last,
        commands: ["hello", "get_state", "submit_input", "cancel", "events_since", "shutdown"] }),
      state: () => session.state(),
      submit: (input, requestId) => session.submit(input, requestId),
      cancel: (turn) => session.cancel(turn),
      shutdown: () => undefined,
    },
  });
  follower = new EventFollower(join(workspace, ".corral/events.jsonl"));
}, 900_000);

afterAll(async () => {
  globalThis.fetch = originalFetch;
  if (originalData === undefined) delete process.env.XDG_DATA_HOME;
  else process.env.XDG_DATA_HOME = originalData;
  follower?.close();
  await bridge?.close();
  if (workspaceId) run(["herdr", "--session", "corral-test", "workspace", "close", workspaceId]);
  if (workspace && lane) rmSync(join(generationDir(workspace, lane.task, lane.run, lane.generation), ".."), { recursive: true, force: true });
  assets?.cleanup();
  if (workspace) rmSync(workspace, { recursive: true, force: true });
});

const turnEnded = (turn: number) => nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === turn, 120_000);

test("the production worker gates, owner input steers a bridge turn, and a fresh corral wakes it to report done", async () => {
  if (!isolated) return;
  // Given the bound production worker, whose fresh session starts its first turn with the prompt:
  transport.script([{ toolCalls: [{ callId: "gate-1", name: "corral_gate", arguments: {} }] }, { text: "GATE_STARTED" }]);
  await host.hello(lane.nonce, show(lane.task).runs[0].herdr.pane_id, sha256Hex(CTL));
  const first = turnEnded(1);
  await host.session.bindExtensions({});
  await first;
  await host.drained();
  const gateReceipt = show(lane.task).runs[0].agent_host.receipts.find((receipt: { op: string }) => receipt.op === "gate");
  expect([gateReceipt.state, gateReceipt.result.outcome]).toEqual(["committed", "started"]);
  const attempt: string = gateReceipt.result.attempt;
  expect(show(lane.task).runs[0].agent_host.turns[0].class).toBe("waiting_external");

  // When the owner types into the pane while a bridge turn is held inside the fake provider:
  transport.holdNext();
  const held = transport.whenHeld();
  const steered = turnEnded(2);
  expect(await host.submit({ text: "Reply only BRIDGE.", busyPolicy: "prompt", idempotencyKey: "xl-bridge",
    origin: "cross-lane" }, "rq-bridge")).toEqual({ disposition: "started", turn: 2 });
  await within(held, 60_000, "the held bridge turn");
  await host.session.prompt("Reply only OWNER.", { source: "interactive", streamingBehavior: "steer" });
  transport.release();
  await steered;
  await host.drained();

  // Then corral records turn 2, by its ID, as owner-typed and owner_engaged:
  const engaged = show(lane.task).runs[0].agent_host.turns.find((turn: { seq: number }) => turn.seq === 2);
  expect(engaged.origin).toEqual({ kind: "owner_terminal", idempotency_key: "xl-bridge" });
  expect(engaged.class).toBe("owner_engaged");

  // When the gate's real pane finishes and a fresh corral process syncs:
  const gates = join(workspace, ".corral/state/gates");
  const result = `${lane.task}-${lane.run}-${attempt}.json`;
  await within(new Promise<void>((resolve) => {
    const watcher = watch(gates, () => {
      if (existsSync(join(gates, result))) { watcher.close(); resolve(); }
    });
    if (existsSync(join(gates, result))) { watcher.close(); resolve(); }
  }), 120_000, "the gate result file");
  const readback = show(lane.task).gate_ref;
  expect(readback.attempt).toBe(attempt);
  transport.script([{ toolCalls: [{ callId: "done-1", name: "corral_done", arguments: {
    summary: SUMMARY, gate_ref: readback, contract_sha256: lane.contract_sha256 } }] }, { text: "DONE" }]);
  const woken = turnEnded(3);
  const done = follower!.next((event) => event.kind === "done" && event.task === lane.task, 120_000);
  await cliAsync("sync");
  await woken;
  await done;
  await host.drained();
  await cliAsync("sync");

  // Then exactly one wait_wake reached the worker by submit_input, no nudge was posted, and its
  // done moved the task to Review with AgentReport v1:
  const inbox = join(workspace, `.corral/inbox/${lane.task}_${lane.run}`);
  const kinds = readdirSync(inbox).filter((name) => /^\d+\.md$/.test(name))
    .map((name) => readFileSync(join(inbox, name), "utf8").split("\n").find((l) => l.startsWith("kind: ")));
  expect(kinds).toEqual(["kind: wait_wake"]);
  const wake = readFileSync(join(inbox, "000001.md"), "utf8");
  expect(wake).toContain(`Gate attempt ${attempt} finished: pass.`);
  expect(JSON.parse(/gate_ref: (\{.*\})/.exec(wake)?.[1] ?? "null")).toEqual(readback);
  expect(wake).toContain(`contract_sha256: ${lane.contract_sha256}`);
  const task = show(lane.task);
  expect(task.status).toBe("review");
  expect(task.runs[0].outcome).toBe("done");
  const state = task.runs[0].agent_host;
  const report = state.reports[0];
  expect([report.kind, report.turn, report.gate_ref, report.contract_sha256])
    .toEqual(["done", 3, readback, lane.contract_sha256]);
  expect(state.wakes.filter((w: { state: string }) => w.state === "fired")).toHaveLength(1);
  const wokenTurn = state.turns.find((turn: { seq: number }) => turn.seq === 3);
  expect([wokenTurn.origin.kind, wokenTurn.class]).toEqual(["bridge", "completed"]);
}, 600_000);
