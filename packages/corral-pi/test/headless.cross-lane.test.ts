// Headless cross-lane fixture (T-577): the real corral binary dispatches a pi-native worker whose
// configuration enables `headless`, onto the package's production worker role and delivery module
// with the fixture provider scripted from a steps file, in the private plugin-free herdr server of
// scripts/test-herdr.sh. Its kickoff asks a real question; the host is killed and explicitly
// resumed; the owner's answer then reaches it through the bridge and inbox alone, and it reports
// done. Nothing is ever typed into its pane.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { connect } from "./support/bridge-client.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "./support/fixture-install.ts";
import { within } from "./support/wait.ts";
import {
  EventFollower, corralBinary, fileAppears, generationDir, ok, privateHerdr, privateRoot, run,
} from "./support/workspace.ts";

const isolated = privateHerdr();
let corral = "";
let workspace = "";
let steps = "";
let assets: FixtureInstall | undefined;
let events: EventFollower | undefined;
const workspaces: string[] = [];

const cli = (...args: string[]) => ok([corral, "--root", workspace, ...args], { cwd: workspace });
const show = (task: string): any => JSON.parse(cli("show", task, "--json"));
const agentState = (): any => JSON.parse(cli("agent", "state", "T-001", "--json")).container;

async function hostState(generation: number): Promise<any> {
  const dir = generationDir(workspace, "T-001", "R1", generation);
  const client = await connect(join(dir, "host.sock"));
  try {
    const cap = readFileSync(join(dir, "ctl.cap"), "utf8");
    for (;;) {
      const reply = await client.request({ version: 1, id: `hl-state-${Math.random().toString(36).slice(2, 10)}`,
        ctl_cap: cap, generation, command: "get_state", args: {} });
      if (reply.result?.bound === true) return reply.result;
    }
  } finally {
    client.close();
  }
}

beforeAll(() => {
  if (!isolated) {
    console.log("headless cross-lane: skipped (needs the private corral-test server of scripts/test-herdr.sh)");
    return;
  }
  corral = corralBinary();
  workspace = mkdtempSync(join(privateRoot() ?? "", "pi-headless-"));
  const app = join(workspace, "app");
  mkdirSync(app);
  for (const argv of [["init", "-q", "-b", "main"], ["config", "user.name", "corral test"],
    ["config", "user.email", "corral@test.invalid"]]) ok(["git", "-C", app, ...argv]);
  writeFileSync(join(app, "README.md"), "# app\n");
  ok(["git", "-C", app, "add", "README.md"]);
  ok(["git", "-C", app, "commit", "-q", "-m", "initial commit"]);
  cli("init");
  // The kickoff asks one blocking question; every later request gets the default reply until the
  // test scripts the answer turn.
  steps = join(workspace, "steps.json");
  writeFileSync(steps, JSON.stringify([
    { toolCalls: [{ callId: "call_ask", name: "corral_ask", arguments: {
      kind: "clarification", text: "Which adapter should the reply use?", blocking: true, options: ["inbox", "pane"],
    } }] },
  ]));
  assets = buildFixtureInstall({ parent: workspace, omit: ["role", "delivery"] });
  cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
  const auth = join(assets.dir, "src/auth");
  copyFileSync(join(auth, "index.ts"), join(auth, "fixture-auth.ts"));
  writeFileSync(join(auth, "index.ts"), `import { readFileSync, writeFileSync } from "node:fs";
import auth, { fixtureTransport } from "./fixture-auth.ts";
const STEPS = ${JSON.stringify(steps)};
function next() {
  const steps = JSON.parse(readFileSync(STEPS, "utf8"));
  const step = steps.shift();
  writeFileSync(STEPS, JSON.stringify(steps));
  return step;
}
export default {
  ...auth,
  transport() {
    const transport = fixtureTransport();
    return async (input, init) => {
      const step = next();
      if (step) transport.script([step]);
      return transport.send(input, init);
    };
  },
};
`);
  const config = join(workspace, ".corral/corral.toml");
  writeFileSync(config, `${readFileSync(config, "utf8")}
[profiles.pi]
harness = "pi-native"
model = "gpt-6.1-sol"
vendor = "openai"
effort = "high"

[agent]
assets_dir = ${JSON.stringify(assets.dir)}

[agent.capabilities.worker]
headless = true
`);
  cli("new", "Pi headless worker asks, resumes and completes", "-p", "app", "--ready");
  events = new EventFollower(join(workspace, ".corral/events.jsonl"));
}, 900_000);

afterAll(() => {
  events?.close();
  for (const id of workspaces) run(["herdr", "--session", "corral-test", "workspace", "close", id]);
  if (workspace) {
    rmSync(join(generationDir(workspace, "T-001", "R1", 1), ".."), { recursive: true, force: true });
    rmSync(workspace, { recursive: true, force: true });
  }
  assets?.cleanup();
});

test("a headless worker asks, survives host loss through explicit resume and completes without terminal input", async () => {
  if (!isolated) return;
  // Given a dispatched headless production worker whose kickoff turn asked a blocking question:
  const kickoff = events!.next((event) => event.kind === "agent_turn_ended" && event.task === "T-001");
  cli("dispatch", "T-001", "--profile", "pi", "--force");
  const pane: string = show("T-001").runs.at(-1).herdr.pane_id;
  workspaces.push(show("T-001").runs.at(-1).herdr.workspace_id);
  await kickoff;
  const first = show("T-001").runs.at(-1).agent_host.generations[0].bound;
  const [question] = JSON.parse(cli("question", "list", "--task", "T-001", "--json")).map((q: any) => q.question);
  expect([question.key, question.state, question.asked.session_id]).toEqual(["T-001/R1/Q1", "open", first.session_id]);

  // Then state names headless mode, the host runs no terminal UI and its pane shows only its log:
  expect([agentState().mode, agentState().generation]).toEqual(["headless", 1]);
  expect(cli("agent", "state", "T-001")).toContain("mode: headless");
  const live = await hostState(1);
  expect([live.mode, live.interactive, live.capabilities.enabled]).toEqual(["headless", false, ["headless", "todo"]]);
  const screen = ok(["herdr", "--session", "corral-test", "pane", "read", pane, "--source", "recent"]);
  expect(screen).toContain("headless: no terminal input");

  // When its host dies and the owner explicitly resumes the run:
  const exits = join(workspace, ".corral/state/launch-exits");
  mkdirSync(exits, { recursive: true });
  const exited = fileAppears(exits, "T-001-R1.exited");
  process.kill(first.pid, "SIGKILL");
  await exited;
  const lost = events!.next((event) => event.kind === "agent_lost" && event.task === "T-001");
  cli("sync");
  expect((await lost).detail?.reason).toBe("host_exited");
  const bound = events!.next((event) => event.kind === "agent_bound" && event.task === "T-001"
    && event.detail?.generation === 2);
  const resumed = JSON.parse(cli("agent", "resume", "T-001"));
  expect([resumed.fenced, resumed.generation, resumed.pane]).toEqual([1, 2, pane]);
  await bound;
  await within(hostState(2), 30_000, "generation 2 to bind");

  // Then generation 2 keeps the run's identity and its headless mode without a capability change:
  const host = show("T-001").runs.at(-1).agent_host;
  const second = host.generations[1];
  expect([second.bound.session_file, second.bound.session_id, second.pane])
    .toEqual([first.session_file, first.session_id, pane]);
  expect(second.capability_transition ?? null).toBeNull();
  expect([agentState().mode, agentState().generation]).toEqual(["headless", 2]);
  expect((await hostState(2)).mode).toBe("headless");

  // When the owner answers the question and sync delivers it (and the resume notice) by bridge,
  // and the worker saves the checkpoint a resumed run needs before it reports done:
  const identity = JSON.parse(cli("run", "progress", "T-001", "--json"));
  writeFileSync(steps, JSON.stringify([
    { toolCalls: [{ callId: "call_checkpoint", name: "corral_checkpoint", arguments: { checkpoint: {
      task: "T-001", run: "R1", spec_revision: identity.spec_revision, expected_revision: 0,
      progress_revision: identity.progress?.revision ?? 0, head: identity.head, base: identity.base, tree: identity.tree,
      criterion_ids: [], evidence_refs: [], next_action: "Report completion", decisions: [], waits: [], resources: [],
    } } }] },
    { toolCalls: [{ callId: "call_done", name: "corral_done", arguments: {
      summary: "Used the inbox adapter.\nVerified: headless cross-lane fixture\nNot verified: none",
    } }] },
  ]));
  const answered = JSON.parse(cli("question", "answer", question.key, "--rev", String(question.revision),
    "--text", "Use the inbox adapter.", "--option", "inbox", "--owner", "--json"));
  expect(answered.delivery).toBe("queued");
  const done = events!.next((event) => event.kind === "done" && event.task === "T-001", 120_000);
  cli("sync");
  await done;
  cli("sync");

  // Then the answer was observed from the bound transcript, and done committed from generation 2:
  const [container, seq] = String(answered.message_id).split("#");
  const inbox = join(workspace, ".corral/inbox", container ?? "");
  const receipt = JSON.parse(readFileSync(join(inbox, `${String(seq).padStart(6, "0")}.receipt.observed.json`), "utf8"));
  expect(receipt.via).toBe("pi_native");
  const envelopes = readdirSync(join(inbox, "envelopes")).filter((name) => name.endsWith(".json"))
    .map((name) => JSON.parse(readFileSync(join(inbox, "envelopes", name), "utf8")));
  const carried = envelopes.find((envelope) => existsSync(envelope.payload_path)
    && readFileSync(envelope.payload_path, "utf8").includes("Use the inbox adapter."));
  expect([carried?.adapter, carried?.binding.transcript]).toEqual(["pi_native", first.session_file]);
  const finished = show("T-001").runs.at(-1);
  expect(finished.outcome).toBe("done");
  const report = finished.agent_host.receipts.find((r: any) => r.op === "done");
  expect([report.state, report.identity.generation]).toEqual(["committed", 2]);
  const turns = finished.agent_host.turns.filter((turn: any) => turn.generation === 2);
  expect(turns.some((turn: any) => turn.origin.kind === "bridge" && turn.class === "completed")).toBeTrue();
  expect(finished.agent_host.turns.every((turn: any) => turn.origin.kind !== "owner_terminal")).toBeTrue();
}, 600_000);
