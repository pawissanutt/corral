// Reviewer-role cross-lane fixture (T-252 reviewer-role): the production reviewer, auth and
// delivery modules in one host, against the corral binary built from this checkout and the private
// plugin-free herdr server of scripts/test-herdr.sh. The credential is synthetic and the fake
// provider transport sits at the host's network boundary, so nothing leaves the machine.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { cpSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { EventLog } from "../../bridge/events.ts";
import { loadModules } from "../../host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../../host/restrict.ts";
import { HostSession } from "../../host/session.ts";
import { CorralClient, sha256Hex } from "../../tools/corral.ts";
import { type FakeTransport, fakeCodexTransport } from "../../../test/fixtures/auth/fake-transport.ts";
import { type FixtureInstall, buildFixtureInstall, PACKAGE_ROOT } from "../../../test/support/fixture-install.ts";
import { nextEvent } from "../../../test/support/wait.ts";
import { corralBinary, ok, privateHerdr, privateRoot, probeBinary, run } from "../../../test/support/workspace.ts";

const isolated = privateHerdr();
const CTL = "d".repeat(64);
const HOST_CAP = "e".repeat(64);
const NOTES = "Changes needed:\n- cover the stale-attempt path";
const originalFetch = globalThis.fetch;
const originalData = process.env.XDG_DATA_HOME;

interface Lane {
  task: string; run: string; review: string; callback: string; generation: number; nonce: string; store: string;
  project: string; prompt_file: string; contract_sha256: string; sessions: string; generation_dir: string;
}

let corral = "";
let workspace = "";
let workspaceId = "";
let pane = "";
let assets: FixtureInstall | undefined;
let lane: Lane;
let transport: FakeTransport;
let events: EventLog;
let host: HostSession;

const show = (id: string) => JSON.parse(ok([corral, "show", id, "--json"], { cwd: workspace }));

/** A synthetic OpenAI Codex OAuth access token: unsigned, accepted only by the fake transport. */
function syntheticAccess(): string {
  const part = (value: object) => Buffer.from(JSON.stringify(value)).toString("base64url");
  const claims = { exp: Math.floor(Date.now() / 1000) + 3600, "https://api.openai.com/auth": { chatgpt_account_id: "synthetic" } };
  return `${part({ alg: "none", typ: "JWT" })}.${part(claims)}.synthetic-signature`;
}

beforeAll(async () => {
  if (!isolated) {
    process.stdout.write("reviewer cross-lane: skipped (needs the private corral-test server of scripts/test-herdr.sh)\n");
    return;
  }
  corral = corralBinary();
  const probe = probeBinary();
  workspace = mkdtempSync(join(privateRoot() ?? "", "pi-reviewer-"));
  const app = join(workspace, "app");
  mkdirSync(app);
  for (const argv of [["init", "-q", "-b", "main"], ["config", "user.name", "corral test"],
    ["config", "user.email", "corral@test.invalid"]]) ok(["git", "-C", app, ...argv]);
  writeFileSync(join(app, "README.md"), "# app\n");
  ok(["git", "-C", app, "add", "README.md"]);
  ok(["git", "-C", app, "commit", "-q", "-m", "initial commit"]);
  const created = JSON.parse(ok(["herdr", "--session", "corral-test", "workspace", "create", "--cwd", app, "--no-focus"]));
  workspaceId = created.result.workspace.workspace_id;
  pane = created.result.root_pane.pane_id;
  const setup = run([probe, "--exact", "agent::hooks::review::tests::reviewer_cross_lane_setup", "--ignored",
    "--nocapture", "--test-threads", "1"],
  { env: { ...process.env, CORRAL_REVIEWER_LANE: JSON.stringify({ root: workspace, workspace: workspaceId, pane }) } });
  const line = /REVIEWER_LANE (\{.*\})/.exec(setup.stdout)?.[1];
  if (!line) throw new Error(`reviewer lane setup printed no line: ${setup.stdout.slice(-2000)} ${setup.stderr.slice(-2000)}`);
  lane = JSON.parse(line) as Lane;

  // The production installation: real reviewer, delivery and auth modules, no fixture.json.
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
  const modules = await loadModules(assets.dir, "reviewer");
  transport = fakeCodexTransport();
  const network = guardedFetch(transport.send);
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  events = new EventLog(lane.generation);
  const identity = { store: lane.store, project: lane.project, role: "reviewer" as const, task: lane.task, run: lane.run,
    review: lane.review, callback: lane.callback, generation: lane.generation };
  const client = new CorralClient({ corral, root: workspace, identity, hostCap: HOST_CAP, sessionId: () => host.sessionId() });
  const sdk = await import("@earendil-works/pi-coding-agent");
  host = await HostSession.create({
    sdk, identity, cwd: app, agentDir: join(workspace, "pi-agent"), model: "gpt-6.1-sol", modules, client, events,
    network, promptFile: lane.prompt_file, sessionManager: sdk.SessionManager.create(app, lane.sessions),
  });
}, 900_000);

afterAll(() => {
  globalThis.fetch = originalFetch;
  if (originalData === undefined) delete process.env.XDG_DATA_HOME;
  else process.env.XDG_DATA_HOME = originalData;
  if (workspaceId) run(["herdr", "--session", "corral-test", "workspace", "close", workspaceId]);
  if (lane) rmSync(join(lane.generation_dir, ".."), { recursive: true, force: true });
  assets?.cleanup();
  if (workspace) rmSync(workspace, { recursive: true, force: true });
});

test("the production reviewer records its verdict in its first turn, and the turn end fences the child", async () => {
  if (!isolated) return;
  // Given the bound production reviewer, whose fresh session starts its first turn with the review prompt:
  transport.script([
    { toolCalls: [{ callId: "verdict-1", name: "corral_verdict", arguments: { verdict: "changes", notes: NOTES } }] },
    { text: "REVIEWED" },
  ]);
  await host.hello(lane.nonce, pane, sha256Hex(CTL));
  const first = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 1, 120_000);
  await host.session.bindExtensions({});
  await first;
  await host.drained();

  // Then corral recorded the verdict on the review child, with its receipt, VerdictReport and the
  // gate section's digest, and the verdict's own turn closed and fenced the binding:
  const task = show(lane.task);
  const parent = task.runs[0];
  expect([parent.review.verdict, parent.review.notes]).toEqual(["changes", NOTES]);
  const state = parent.review_runs[0].agent_host;
  const verdict = state.receipts.find((receipt: { op: string }) => receipt.op === "verdict");
  expect([verdict.state, verdict.result.verdict, verdict.identity.turn]).toEqual(["committed", "changes", 1]);
  const report = state.reports[0];
  expect([report.kind, report.verdict, report.turn, report.contract_sha256])
    .toEqual(["verdict", "changes", 1, lane.contract_sha256]);
  const turn = state.turns.find((t: { seq: number }) => t.seq === 1);
  expect(turn.class).toBe("completed");
  expect(state.binding.health).toBe("fenced");
}, 600_000);
