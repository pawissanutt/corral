// Question-answer cross-lane fixture (T-489): the real corral binary dispatches a pi-native run onto
// the package's production worker role and real delivery module, with the provider scripted from a
// steps file, in the private plugin-free herdr server of scripts/test-herdr.sh. The kickoff turn asks
// a real question through corral_ask; the owner then types /new into the pane, which the bound host
// refuses, and answers the question. corral must observe that answer from the transcript hello bound.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { type TestClient, connect } from "../../test/support/bridge-client.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "../../test/support/fixture-install.ts";
import { ownerNewIsRefused } from "../../test/support/replacement.ts";
import { within } from "../../test/support/wait.ts";
import {
  EventFollower, corralBinary, generationDir, ok, privateHerdr, privateRoot, run, sha256,
} from "../../test/support/workspace.ts";
import { ENTRY_TYPE } from "./index.ts";

const isolated = privateHerdr();
let corral = "";
let workspace = "";
let assets: FixtureInstall | undefined;
let events: EventFollower | undefined;
const workspaces: string[] = [];

const cli = (...args: string[]) => ok([corral, "--root", workspace, ...args], { cwd: workspace });
const show = (task: string): any => JSON.parse(cli("show", task, "--json"));
const herdr = (...args: string[]): any => JSON.parse(ok(["herdr", "--session", "corral-test", ...args])).result;
const ASK = { toolCalls: [{ callId: "call_ask", name: "corral_ask", arguments: {
  kind: "clarification", text: "Which adapter should the reply use?", blocking: true, options: ["inbox", "pane"],
} }] };
let steps = "";

/** The host's own record that herdr accepted a report of `state` after event `after`. */
async function untilReported(client: TestClient, cap: string, state: string, after: number): Promise<number> {
  for (let seq = after; ;) {
    const page = (await client.request({ version: 1, id: `qx-${seq}-${Math.random().toString(36).slice(2, 8)}`,
      ctl_cap: cap, generation: 1, command: "events_since", args: { seq } })).result as { events: any[]; next: number };
    const event = page.events.find((candidate) => candidate.kind === "herdr_reported" && candidate.state === state);
    if (event) return event.seq;
    seq = page.next;
  }
}

beforeAll(() => {
  if (!isolated) {
    console.log("question cross-lane: skipped (needs the private corral-test server of scripts/test-herdr.sh)");
    return;
  }
  corral = corralBinary();
  workspace = mkdtempSync(join(privateRoot() ?? "", "pi-question-"));
  const app = join(workspace, "app");
  mkdirSync(app);
  for (const argv of [["init", "-q", "-b", "main"], ["config", "user.name", "corral test"],
    ["config", "user.email", "corral@test.invalid"]]) ok(["git", "-C", app, ...argv]);
  writeFileSync(join(app, "README.md"), "# app\n");
  ok(["git", "-C", app, "add", "README.md"]);
  ok(["git", "-C", app, "commit", "-q", "-m", "initial commit"]);
  cli("init");
  // The kickoff turn asks one blocking question; every later provider request gets the default reply.
  steps = join(workspace, "steps.json");
  writeFileSync(steps, JSON.stringify([ASK]));
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
`);
  cli("new", "Pi question answered after a refused /new", "-p", "app", "--ready");
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

test("a question answered after the owner's refused /new is observed from the originally bound transcript", async () => {
  if (!isolated) return;
  // Given a dispatched production worker whose kickoff turn asked a real question:
  const kickoff = events!.next((event) => event.kind === "agent_turn_ended" && event.task === "T-001");
  cli("dispatch", "T-001", "--profile", "pi", "--force");
  const runRecord = show("T-001").runs.at(-1);
  workspaces.push(runRecord.herdr.workspace_id);
  await kickoff;
  const binding = show("T-001").runs.at(-1).agent_host.generations[0].bound;
  const listed = JSON.parse(cli("question", "list", "--task", "T-001", "--json"));
  expect(listed).toHaveLength(1);
  const question = listed[0].question;
  expect([question.key, question.state, question.asked.session_id]).toEqual(["T-001/R1/Q1", "open", binding.session_id]);
  // And herdr shows the bound worker as corral-pi blocked on it, after its turn settled (T-605):
  const pane: string = runRecord.herdr.pane_id;
  const blocked = herdr("agent", "wait", pane, "--until", "blocked", "--timeout", "30000").agent;
  expect([blocked.agent, blocked.agent_status, blocked.tokens?.corral_task, blocked.tokens?.corral_session])
    .toEqual(["corral-pi", "blocked", "T-001", binding.session_id]);

  // When the owner's /new in the pane is refused and the owner then answers the question:
  const refused = await ownerNewIsRefused(runRecord.herdr.pane_id, generationDir(workspace, "T-001", "R1", 1), 1);
  expect(refused.session_id).toBe(binding.session_id);
  const answered = JSON.parse(cli("question", "answer", question.key, "--rev", String(question.revision),
    "--text", "Use the inbox adapter.", "--option", "inbox", "--owner", "--json"));
  expect(answered.delivery).toBe("queued");
  const [container, seq] = String(answered.message_id).split("#");
  const inbox = join(workspace, ".corral/inbox", container ?? "");
  const receiptFile = join(inbox, `${String(seq).padStart(6, "0")}.receipt.observed.json`);
  expect(existsSync(receiptFile)).toBeFalse();
  // The first sync submits the answer to the host, which records it before Pi sees it; the second
  // observes that record.
  const answerTurn = events!.next((event) => event.kind === "agent_turn_ended" && event.task === "T-001"
    && event.detail?.turn === 2, 90_000, "the answer's agent_turn_ended");
  cli("sync");
  cli("sync");
  await answerTurn;
  // And the delivered answer settled the question in herdr: the host is no longer blocked.
  expect(["idle", "done"]).toContain(herdr("agent", "wait", pane, "--timeout", "30000").agent.agent_status);

  // Then corral observed that answer member from the transcript hello bound, in the bound session:
  expect(existsSync(receiptFile)).toBeTrue();
  const receipt = JSON.parse(readFileSync(receiptFile, "utf8"));
  expect(receipt.via).toBe("pi_native");
  const envelope = readdirSync(join(inbox, "envelopes")).filter((name) => name.endsWith(".json"))
    .map((name) => JSON.parse(readFileSync(join(inbox, "envelopes", name), "utf8")))
    .find((candidate) => readFileSync(candidate.payload_path, "utf8").includes("Use the inbox adapter."));
  expect(envelope.adapter).toBe("pi_native");
  expect(envelope.binding.transcript).toBe(binding.session_file);
  const session = readFileSync(binding.session_file, "utf8").trim().split("\n").map((line) => JSON.parse(line));
  const written = session.find((entry) => entry.id === receipt.detail.entry_id);
  expect(written?.customType).toBe(ENTRY_TYPE);
  expect(written?.data.session_id).toBe(binding.session_id);
  expect(written?.data.text_sha256).toBe(envelope.payload_sha256);
  expect(sha256(readFileSync(envelope.payload_path, "utf8"))).toBe(envelope.payload_sha256);
  const after = JSON.parse(cli("question", "show", question.key, "--json"));
  expect(after.answer?.message_id ?? after.question?.answer?.message_id).toBe(answered.message_id);
}, 600_000);

test("an owner's cancellation of a blocking question releases the host's herdr blocked state without a turn", async () => {
  if (!isolated) return;
  // Given a dispatched production worker whose kickoff turn asked a blocking question:
  writeFileSync(steps, JSON.stringify([ASK]));
  cli("new", "Pi question cancelled by the owner", "-p", "app", "--ready");
  const kickoff = events!.next((event) => event.kind === "agent_turn_ended" && event.task === "T-002");
  cli("dispatch", "T-002", "--profile", "pi", "--force");
  const runRecord = show("T-002").runs.at(-1);
  workspaces.push(runRecord.herdr.workspace_id);
  await kickoff;
  const pane: string = runRecord.herdr.pane_id;
  expect(herdr("agent", "wait", pane, "--until", "blocked", "--timeout", "30000").agent.agent_status).toBe("blocked");
  const dir = generationDir(workspace, "T-002", "R1", 1);
  const cap = readFileSync(join(dir, "ctl.cap"), "utf8");
  const client = await connect(join(dir, "host.sock"));
  const after = (await client.request({ version: 1, id: "qx-last", ctl_cap: cap, generation: 1,
    command: "events_since", args: { seq: 0 } })).result?.last as number;

  // When the owner cancels it, with no message to the host and no new turn:
  cli("question", "cancel", "T-002/R1/Q1", "--rev", "1", "--reason", "Not needed any more.", "--owner");

  // Then corral told the host over its bridge, and herdr no longer shows it blocked:
  await within(untilReported(client, cap, "idle", after), 30_000, "the idle report after the cancellation");
  client.close();
  expect(["idle", "done"]).toContain(herdr("pane", "get", pane).pane.agent_status);
  const shown = JSON.parse(cli("question", "show", "T-002/R1/Q1", "--json"));
  expect(shown.question?.state ?? shown.state).toBe("cancelled");
}, 600_000);
