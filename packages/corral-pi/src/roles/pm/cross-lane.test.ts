// Production PM host -> Rust CLI -> isolated Store. Run only inside scripts/test-herdr.sh.
// The Rust setup probe's contract is documented in fixture-contract.md beside this file.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
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

interface Lane {
  readonly store: string;
  readonly project: string;
  readonly task: string;
  readonly outside_task: string;
  readonly generation: number;
  readonly nonce: string;
  readonly sessions: string;
}

interface TaskReadback {
  readonly comments?: readonly { readonly author: string; readonly text: string }[];
  readonly agent_pm_receipts?: readonly {
    readonly op: string;
    readonly state: string;
    readonly identity: { readonly role: string; readonly task: string | null; readonly run: string | null };
  }[];
}

interface ToolOutcome {
  readonly state: string;
  readonly result?: { readonly code?: string };
}

const isolated = privateHerdr();
const originalFetch = globalThis.fetch;
const originalData = process.env.XDG_DATA_HOME;
const NOTE = "PM cross-lane comment";
let workspace = "";
let workspaceId = "";
let pane = "";
let corral = "";
let lane: Lane;
let assets: FixtureInstall | undefined;
let transport: FakeTransport;
let events: EventLog;
let host: HostSession;

function show(task: string): TaskReadback {
  return JSON.parse(ok([corral, "show", task, "--json"], { cwd: workspace }));
}

function syntheticAccess(): string {
  const part = (value: object) => Buffer.from(JSON.stringify(value)).toString("base64url");
  return `${part({ alg: "none", typ: "JWT" })}.${part({
    exp: Math.floor(Date.now() / 1000) + 3600,
    "https://api.openai.com/auth": { chatgpt_account_id: "synthetic" },
  })}.synthetic-signature`;
}

beforeAll(async () => {
  if (!isolated) {
    process.stdout.write("PM cross-lane: skipped (requires the private server from scripts/test-herdr.sh)\n");
    return;
  }
  corral = corralBinary();
  const probe = probeBinary();
  workspace = mkdtempSync(join(privateRoot() ?? "", "pi-pm-"));
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
  const modules = await loadModules(assets.dir, "pm");
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
    modules, client, events, network, sessionManager: sdk.SessionManager.create(workspace, lane.sessions),
  });
  await host.session.bindExtensions({});
  await host.hello(lane.nonce, pane, sha256Hex("d".repeat(64)));
}, 900_000);

afterAll(() => {
  globalThis.fetch = originalFetch;
  if (originalData === undefined) delete process.env.XDG_DATA_HOME;
  else process.env.XDG_DATA_HOME = originalData;
  if (workspaceId) run(["herdr", "--session", "corral-test", "workspace", "close", workspaceId]);
  assets?.cleanup();
  if (workspace) rmSync(workspace, { recursive: true, force: true });
});

test("a model-issued PM comment commits once in scope and cannot mutate another project", async () => {
  if (!isolated) return;
  // Given eligible tasks in two projects and a project-bound PM:
  const before = show(lane.task);
  const outside = show(lane.outside_task);
  transport.script([
    { toolCalls: [
      { callId: "comment-inside", name: "corral_comment", arguments: { task: lane.task, text: NOTE } },
      { callId: "comment-outside", name: "corral_comment", arguments: { task: lane.outside_task, text: NOTE } },
    ] },
    { text: "RECORDED" },
  ]);
  const ended = nextEvent(events, (event) => event.kind === "turn_ended" && event.turn === 1, 120_000);
  // When the real host sends both mutations through the Rust binary:
  expect((await host.submit({
    text: "Record the comments.", busyPolicy: "prompt", idempotencyKey: "pm-comments", origin: "test",
  }, "rq-pm-comments")).disposition).toBe("started");
  await ended;
  await host.drained();
  // Then only the scoped target changes, with a durable PM receipt and no task/run identity.
  const after = show(lane.task);
  const comments = (task: TaskReadback) => (task.comments ?? []).map(({ author, text }) => ({ author, text }));
  expect(comments(after)).toEqual([...comments(before), { author: "pm", text: NOTE }]);
  expect(show(lane.outside_task).comments ?? []).toEqual(outside.comments ?? []);
  const receipts = after.agent_pm_receipts?.filter((receipt) => receipt.op === "comment") ?? [];
  expect(receipts).toHaveLength(1);
  expect(receipts[0]?.state).toBe("committed");
  expect(receipts[0]?.identity.role).toBe("pm");
  expect(receipts[0]?.identity.task ?? null).toBeNull();
  expect(receipts[0]?.identity.run ?? null).toBeNull();
  const results: ToolOutcome[] = host.session.messages.flatMap((message) => message.role === "toolResult"
    ? message.content.flatMap((part) => part.type === "text" ? [JSON.parse(part.text)] : []) : []);
  expect(results.some((result) => result.state === "rejected" && result.result?.code === "out_of_scope")).toBeTrue();
}, 600_000);
