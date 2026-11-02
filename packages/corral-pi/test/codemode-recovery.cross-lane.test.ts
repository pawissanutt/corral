// Code Mode recovery cross-lane (T-683): the real corral binary dispatches a pi-native run onto the
// package's production worker role (Code Mode on in its standard snapshot), in the private plugin-free
// herdr server of scripts/test-herdr.sh. The kickoff turn runs one cell that commits corral_ask through
// the real envelope with its parent identity, then blocks in a nested edit on a FIFO, where the real
// Bun host is SIGKILLed by rendezvous, never by timing. Resume reports the codemode call and the
// unsettled edit child under its child key from the session journal the host wrote; the ask's stored
// receipt keeps its parent, and nothing is replayed.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { type ChildProcess, spawn } from "node:child_process";
import { copyFileSync, cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import { toolRequestId } from "../src/tools/corral.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "./support/fixture-install.ts";
import { within } from "./support/wait.ts";
import { corralBinary, fileAppears, generationDir, ok, privateHerdr, privateRoot, run } from "./support/workspace.ts";

import { StoreEventFollower as EventFollower, launchExitDir } from "./support/canonical-store.ts";

const isolated = privateHerdr();
let corral = "";
let workspace = "";
let assets: FixtureInstall | undefined;
let events: EventFollower | undefined;
let editFifo = "";
let providerFifo = "";
const workspaces: string[] = [];
const helpers: ChildProcess[] = [];

const cli = (...args: string[]) => ok([corral, "--root", workspace, ...args], { cwd: workspace });
const show = (task: string): any => JSON.parse(cli("show", task, "--json"));

function holdFifo(fifo: string): { opened: Promise<void>; close(): void } {
  const child = spawn("bash", ["-c", 'exec 3>"$1"; echo opened; read -r _', "fifo-writer", fifo],
    { stdio: ["pipe", "pipe", "inherit"] });
  helpers.push(child);
  const opened = new Promise<void>((resolve) => {
    child.stdout?.on("data", (chunk) => {
      if (String(chunk).includes("opened")) resolve();
    });
  });
  return { opened: within(opened, 120_000, `a reader to open ${fifo}`), close: () => child.stdin?.end() };
}

beforeAll(async () => {
  if (!isolated) {
    console.log("codemode recovery cross-lane: skipped (needs the private corral-test server of scripts/test-herdr.sh)");
    return;
  }
  corral = corralBinary();
  workspace = mkdtempSync(join(privateRoot() ?? "", "pi-codemode-recovery-"));
  const app = join(workspace, "app");
  mkdirSync(app);
  for (const argv of [["init", "-q", "-b", "main"], ["config", "user.name", "corral test"],
    ["config", "user.email", "corral@test.invalid"]]) ok(["git", "-C", app, ...argv]);
  writeFileSync(join(app, "README.md"), "# app\n");
  ok(["git", "-C", app, "add", "README.md"]);
  ok(["git", "-C", app, "commit", "-q", "-m", "initial commit"]);
  cli("init");
  editFifo = join(workspace, "edit.fifo");
  providerFifo = join(workspace, "provider.fifo");
  for (const fifo of [editFifo, providerFifo]) ok(["mkfifo", fifo]);
  const code = `
    const asked = await tools.corral_ask({ kind: "clarification", text: "Which input?" });
    print(asked.state);
    await tools.edit({ path: ${JSON.stringify(editFifo)}, edits: [{ oldText: "a", newText: "b" }] });
  `;
  const steps = join(workspace, "steps.json");
  writeFileSync(steps, JSON.stringify([{ toolCalls: [{ callId: "call_code", name: "codemode", arguments: { code } }] }]));
  assets = buildFixtureInstall({ parent: workspace, omit: ["role", "delivery"] });
  cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
  const auth = join(assets.dir, "src/auth");
  copyFileSync(join(auth, "index.ts"), join(auth, "fixture-auth.ts"));
  writeFileSync(join(auth, "index.ts"), `import { readFileSync, writeFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import auth, { fixtureTransport } from "./fixture-auth.ts";
const GATE = ${JSON.stringify(providerFifo)};
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
      await readFile(GATE);
      transport.script([step ?? { text: "Recovered" }]);
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
  cli("new", "Pi host killed inside a Code Mode cell", "-p", "app", "--ready");
  events = await EventFollower.open(workspace);
}, 900_000);

afterAll(() => {
  events?.close();
  for (const helper of helpers) helper.kill("SIGKILL");
  for (const id of workspaces) run(["herdr", "--session", "corral-test", "workspace", "close", id]);
  if (workspace) {
    rmSync(join(generationDir(workspace, "T-001", "R1", 1), ".."), { recursive: true, force: true });
    rmSync(workspace, { recursive: true, force: true });
  }
  assets?.cleanup();
});

test("a host killed inside a cell's nested edit resumes reporting the unsettled child from its journal", async () => {
  if (!isolated) return;
  // Given a dispatched production worker whose kickoff cell commits a question, then edits a FIFO:
  const edit = holdFifo(editFifo);
  const bound = events!.next((event) => event.kind === "agent_bound" && event.task === "T-001");
  cli("dispatch", "T-001", "--profile", "pi", "--force");
  workspaces.push(show("T-001").runs.at(-1).herdr.workspace_id);
  await bound;
  const generation = show("T-001").runs.at(-1).agent_host.generations[0];
  expect(generation.capabilities.features.codemode?.enabled).toBe(true);
  await within(writeFile(providerFifo, "go"), 90_000, "the provider rendezvous");
  await edit.opened;

  // When its host is killed inside the nested edit and the owner resumes the run:
  const exits = launchExitDir(workspace);
  mkdirSync(exits, { recursive: true });
  const exited = fileAppears(exits, "T-001-R1.exited");
  process.kill(generation.bound.pid, "SIGKILL");
  await exited;
  const lost = events!.next((event) => event.kind === "agent_lost" && event.task === "T-001");
  cli("sync");
  await lost;
  edit.close();
  const outcome = JSON.parse(cli("agent", "resume", "T-001"));

  // Then the dangling codemode call and the unsettled edit child are outcome_unknown, the settled
  // ask is not reported again, and nothing ran twice:
  const session = generation.bound.session_id as string;
  const calls = outcome.calls as { tool: string; class: string; tool_call_id: string }[];
  expect(calls.map((call) => [call.tool, call.class])).toEqual([["codemode", "outcome_unknown"], ["edit", "outcome_unknown"]]);
  const codemodeId = calls[0]?.tool_call_id ?? "";
  expect(codemodeId.startsWith("call_code")).toBe(true);
  expect(calls[1]?.tool_call_id).toBe(`${codemodeId}/cell-1.2`);
  const host = show("T-001").runs.at(-1).agent_host;
  const asks = host.receipts.filter((receipt: any) => receipt.op === "ask");
  expect(asks).toHaveLength(1);
  expect(asks[0].state).toBe("committed");
  expect(asks[0].identity.parent).toEqual({ tool_call_id: codemodeId, cell: "cell-1", call: "cell-1.1" });
  expect(asks[0].request_id).toBe(toolRequestId(session, `${codemodeId}/cell-1.1`));
  const journal = readFileSync(generation.bound.session_file, "utf8").trim().split("\n").map((line) => JSON.parse(line))
    .filter((entry) => entry.type === "custom" && entry.customType === "corral_code_call")
    .map((entry) => [entry.data.phase, entry.data.tool, entry.data.parent.call]);
  expect(journal).toEqual([["started", "corral_ask", "cell-1.1"], ["settled", "corral_ask", "cell-1.1"],
    ["started", "edit", "cell-1.2"]]);
  expect([host.binding.generation, outcome.fenced]).toEqual([2, 1]);
}, 600_000);
