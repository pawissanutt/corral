// Background-jobs cross-lane fixture (T-565): the production worker host with background_shell,
// against the corral binary built from this checkout and the private plugin-free herdr server of
// scripts/test-herdr.sh. A fake provider scripts the model; the job barrier is a FIFO, so the
// exit happens only when the test releases it, after the monitor committed.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { spawn, spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { EventLog } from "../src/bridge/events.ts";
import { type BridgeServer, startBridge } from "../src/bridge/server.ts";
import { loadModules } from "../src/host/modules.ts";
import { loadPeerLookup } from "../src/host/peer.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession } from "../src/host/session.ts";
import { startTimeOf } from "../src/extensions/jobs/runtime.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { type FakeTransport, fakeCodexTransport } from "./fixtures/auth/fake-transport.ts";
import { type FixtureInstall, buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { nextEvent } from "./support/wait.ts";
import { corralBinary, generationDir, ok, privateHerdr, privateRoot, probeBinary, run } from "./support/workspace.ts";

const isolated = privateHerdr();
const CTL = "d".repeat(64);
const HOST_CAP = "e".repeat(64);
const originalFetch = globalThis.fetch;
const originalData = process.env.XDG_DATA_HOME;

interface Lane {
  task: string; run: string; generation: number; nonce: string; store: string; project: string;
  prompt_file: string; contract_sha256: string; sessions: string;
}

let corral = "";
let workspace = "";
let workspaceId = "";
let gate = "";
let assets: FixtureInstall | undefined;
let lane: Lane;
let transport: FakeTransport;
let events: EventLog;
let host: HostSession;
let bridge: BridgeServer | undefined;

const show = (id: string) => JSON.parse(ok([corral, "show", id, "--json"], { cwd: workspace }));
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

function syntheticAccess(): string {
  const part = (value: object) => Buffer.from(JSON.stringify(value)).toString("base64url");
  const claims = { exp: Math.floor(Date.now() / 1000) + 3600, "https://api.openai.com/auth": { chatgpt_account_id: "synthetic" } };
  return `${part({ alg: "none", typ: "JWT" })}.${part(claims)}.synthetic-signature`;
}

beforeAll(async () => {
  if (!isolated) {
    process.stdout.write("jobs cross-lane: skipped (needs the private corral-test server of scripts/test-herdr.sh)\n");
    return;
  }
  corral = corralBinary();
  const probe = probeBinary();
  workspace = mkdtempSync(join(privateRoot() ?? "", "pi-jobs-"));
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
  const line = /WORKER_LANE (\{.*\})/.exec(setup.stdout)?.[1];
  if (!line) throw new Error(`worker lane setup printed no line: ${setup.stdout.slice(-2000)} ${setup.stderr.slice(-2000)}`);
  lane = JSON.parse(line) as Lane;

  assets = buildFixtureInstall({ parent: workspace, overlays: false });
  cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
  const data = join(workspace, "data");
  const auth = join(data, "corral/pi-native/auth");
  mkdirSync(auth, { recursive: true, mode: 0o700 });
  writeFileSync(join(auth, "auth.json"), JSON.stringify({ "openai-codex": {
    type: "oauth", access: syntheticAccess(), refresh: "synthetic-refresh", expires: Date.now() + 3_600_000,
    accountId: "synthetic" } }), { mode: 0o600 });
  process.env.XDG_DATA_HOME = data;

  restrictEnvironment(process.env);
  const modules = await loadModules(assets.dir, "worker");
  transport = fakeCodexTransport();
  const network = guardedFetch(transport.send);
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  events = new EventLog(lane.generation);
  const identity = { store: lane.store, project: lane.project, role: "worker" as const, task: lane.task, run: lane.run,
    generation: lane.generation };
  const recorded = show(lane.task).runs[0].agent_host.generations[0].capabilities;
  expect(recorded.features.background_shell.enabled).toBeTrue();
  const client = new CorralClient({ corral, root: workspace, identity, hostCap: HOST_CAP, sessionId: () => host.sessionId() });
  const dir = generationDir(workspace, lane.task, lane.run, lane.generation);
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  const sdk = await import("@earendil-works/pi-coding-agent");
  host = await HostSession.create({
    sdk, identity, cwd: app, agentDir: join(workspace, "pi-agent"), model: "gpt-6.1-sol", modules, client, events, network,
    capabilities: { catalog: recorded.catalog, digest: recorded.digest, preset: recorded.preset, enabled: ["background_shell"] },
    guidance: { settings: { background_shell: recorded.features.background_shell.settings }, projectText: false },
    runtimeDir: dir, promptFile: lane.prompt_file, sessionManager: sdk.SessionManager.create(app, lane.sessions),
  });
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
  gate = join(workspace, "gate");
  expect(spawnSync("mkfifo", [gate]).status).toBe(0);
}, 900_000);

afterAll(async () => {
  globalThis.fetch = originalFetch;
  if (originalData === undefined) delete process.env.XDG_DATA_HOME;
  else process.env.XDG_DATA_HOME = originalData;
  await bridge?.close();
  if (workspaceId) run(["herdr", "--session", "corral-test", "workspace", "close", workspaceId]);
  if (workspace && lane) rmSync(join(generationDir(workspace, lane.task, lane.run, lane.generation), ".."), { recursive: true, force: true });
  assets?.cleanup();
  if (workspace) rmSync(workspace, { recursive: true, force: true });
});

const turnEnded = (turn: number) => nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === turn, 120_000);

test("a worker starts, peeks, kills and monitors jobs; one wake answers the exit after the barrier", async () => {
  if (!isolated) return;
  // Given the bound worker, whose first turn starts a gated job and a sleeper, reads the first,
  // kills the sleeper and monitors the first job's exit:
  transport.script([
    { toolCalls: [{ callId: "start-a", name: "job_start", arguments: { command: `cat ${gate} >/dev/null; echo BUILT` } }] },
    { toolCalls: [{ callId: "start-b", name: "job_start", arguments: { command: "sleep 30" } }] },
    { toolCalls: [{ callId: "peek-a", name: "job_output", arguments: { job: `j${lane.generation}-1` } }] },
    { toolCalls: [{ callId: "kill-b", name: "job_kill", arguments: { job: `j${lane.generation}-2` } }] },
    { toolCalls: [{ callId: "watch-a", name: "corral_job_monitor", arguments: { job: `j${lane.generation}-1`, on: "exit" } }] },
    { text: "WAITING_FOR_BUILD" },
  ]);
  await host.hello(lane.nonce, show(lane.task).runs[0].herdr.pane_id, sha256Hex(CTL));
  const first = turnEnded(1);
  await host.session.bindExtensions({});
  await first;
  await host.drained();
  // Then corral holds both jobs' identities, the monitor's pending wait, and a waiting turn:
  let state = show(lane.task).runs[0].agent_host;
  const [built, slept] = state.jobs;
  expect([built.state, slept.state]).toEqual(["running", "exited"]);
  expect(String(built.start_time)).toBe(startTimeOf(built.pid) ?? "gone");
  expect(slept.signal).toBe("SIGTERM");
  expect(state.turns[0].class).toBe("waiting_external");
  const wait = `j${lane.generation}-1.m1`;
  expect(show(lane.task).runs[0].obligations.waits.find((w: { request: { id: string } }) => w.request.id === wait)
    .observation.state).toBe("pending");
  process.stdout.write("PASS jobs: real host -> job identity, kill and armed monitor recorded by the corral binary\n");

  // When the barrier is released, the job exits and a fresh corral delivers the wake:
  transport.script([{ text: "BUILD_DONE" }]);
  const exited = nextEvent(events, (event) => event.kind === "job_exited" && event.job === built.id, 60_000);
  writeFileSync(gate, "go\n");
  await exited;
  const woken = turnEnded(2);
  await cliAsync("sync");
  await woken;
  await host.drained();
  await cliAsync("sync");
  // Then exactly one wait_wake reached the worker, naming the ready exit.
  const inbox = join(workspace, `.corral/inbox/${lane.task}_${lane.run}`);
  const wakes = readdirSync(inbox).filter((name) => /^\d+\.md$/.test(name))
    .map((name) => readFileSync(join(inbox, name), "utf8"))
    .filter((body) => body.includes("kind: wait_wake"));
  expect(wakes).toHaveLength(1);
  expect(wakes[0]).toContain(`Wait ${wait} (job ${built.id} exits) reached ready`);
  state = show(lane.task).runs[0].agent_host;
  expect(state.jobs[0].state).toBe("exited");
  expect(state.jobs[0].exit_code).toBe(0);
  expect(state.wakes.filter((w: { state: string }) => w.state === "fired")).toHaveLength(1);
  expect(state.turns.find((turn: { seq: number }) => turn.seq === 2).origin.kind).toBe("bridge");
  expect(existsSync(join(generationDir(workspace, lane.task, lane.run, lane.generation), "jobs", `${built.id}.log`))).toBeTrue();
  process.stdout.write("PASS jobs: FIFO barrier -> exit report -> one inbox wake -> woken bridge turn\n");
}, 600_000);
