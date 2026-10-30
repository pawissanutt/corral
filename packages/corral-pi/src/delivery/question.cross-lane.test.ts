// Question-answer cross-lane fixture (T-489): the real corral binary dispatches a pi-native run onto
// the package's production worker role and real delivery module, with the provider scripted from a
// steps file, in the private plugin-free herdr server of scripts/test-herdr.sh. The kickoff turn asks
// a real question through corral_ask; the owner then types /new into the pane, which the bound host
// refuses, and answers the question. corral must observe that answer from the transcript hello bound.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "../../test/support/fixture-install.ts";
import { ownerNewIsRefused } from "../../test/support/replacement.ts";
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
  const steps = join(workspace, "steps.json");
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
  cli("sync");
  cli("sync");

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
