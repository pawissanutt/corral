// Recovery cross-lane fixture for upstream tools (T-252 recovery): the real corral binary dispatches
// a pi-native run onto the package's production worker role, with the fixture provider scripted from
// a steps file, in the private plugin-free herdr server of scripts/test-herdr.sh. The real Bun host
// is SIGKILLed inside an upstream edit, and its next generation inside a bash command whose child
// process keeps running. Both block on a FIFO, so each kill lands inside the tool by rendezvous,
// never by timing. Each resume reports its call outcome_unknown from the Pi session the host wrote,
// and nothing it started is reverted or signalled.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { type ChildProcess, spawn } from "node:child_process";
import { copyFileSync, cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { type TestClient, connect } from "../../test/support/bridge-client.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "../../test/support/fixture-install.ts";
import { within } from "../../test/support/wait.ts";
import {
  EventFollower, corralBinary, fileAppears, generationDir, ok, privateHerdr, privateRoot, run,
} from "../../test/support/workspace.ts";

const isolated = privateHerdr();
let corral = "";
let workspace = "";
let assets: FixtureInstall | undefined;
let events: EventFollower | undefined;
const workspaces: string[] = [];
const helpers: ChildProcess[] = [];
const started: number[] = [];

const cli = (...args: string[]) => ok([corral, "--root", workspace, ...args], { cwd: workspace });
const show = (task: string): any => JSON.parse(cli("show", task, "--json"));

/** A writer that opens `fifo` and holds it: `opened` resolves once a reader has opened it too. */
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

async function untilBound(client: TestClient, cap: string, generation: number): Promise<void> {
  for (;;) {
    const hello = await client.request({ version: 1, id: `rt-hello-${Math.random().toString(36).slice(2, 10)}`,
      ctl_cap: cap, generation, command: "hello", args: {} });
    if (hello.result?.bound === true) return;
  }
}

/** Kills generation `n`'s host while its pane stays, then lets sync record the loss. */
async function kill(n: number): Promise<void> {
  const host = show("T-001").runs.at(-1).agent_host.generations[n - 1];
  const exits = join(workspace, ".corral/state/launch-exits");
  mkdirSync(exits, { recursive: true });
  const exited = fileAppears(exits, "T-001-R1.exited");
  process.kill(host.bound.pid, "SIGKILL");
  await exited;
  const lost = events!.next((event) => event.kind === "agent_lost" && event.task === "T-001"
    && event.detail?.generation === n);
  cli("sync");
  expect((await lost).detail?.reason).toBe("host_exited");
}

/** Resumes generation `n` and waits for generation n+1 to bind; returns resume's outcome. */
async function resume(n: number): Promise<any> {
  const bound = events!.next((event) => event.kind === "agent_bound" && event.task === "T-001"
    && event.detail?.generation === n + 1);
  const outcome = JSON.parse(cli("agent", "resume", "T-001"));
  expect([outcome.fenced, outcome.generation]).toEqual([n, n + 1]);
  await bound;
  const dir = generationDir(workspace, "T-001", "R1", n + 1);
  const client = await connect(join(dir, "host.sock"));
  await within(untilBound(client, readFileSync(join(dir, "ctl.cap"), "utf8"), n + 1), 30_000, "the host to bind");
  client.close();
  return outcome;
}

let editFifo = "";
let bashFifo = "";
let childPid = "";

beforeAll(() => {
  if (!isolated) {
    console.log("recovery tools cross-lane: skipped (needs the private corral-test server of scripts/test-herdr.sh)");
    return;
  }
  corral = corralBinary();
  workspace = mkdtempSync(join(privateRoot() ?? "", "pi-recovery-tools-"));
  const app = join(workspace, "app");
  mkdirSync(app);
  for (const argv of [["init", "-q", "-b", "main"], ["config", "user.name", "corral test"],
    ["config", "user.email", "corral@test.invalid"]]) ok(["git", "-C", app, ...argv]);
  writeFileSync(join(app, "README.md"), "# app\n");
  ok(["git", "-C", app, "add", "README.md"]);
  ok(["git", "-C", app, "commit", "-q", "-m", "initial commit"]);
  cli("init");
  editFifo = join(workspace, "edit.fifo");
  bashFifo = join(workspace, "bash.fifo");
  childPid = join(workspace, "child.pid");
  for (const fifo of [editFifo, bashFifo]) ok(["mkfifo", fifo]);
  // Generation 1's kickoff edits through a FIFO; generation 2's notice turn runs the command.
  const steps = join(workspace, "steps.json");
  writeFileSync(steps, JSON.stringify([
    { toolCalls: [{ callId: "call_edit", name: "edit",
      arguments: { path: editFifo, edits: [{ oldText: "a", newText: "b" }] } }] },
    { toolCalls: [{ callId: "call_bash", name: "bash",
      arguments: { command: `sleep 600 </dev/null >/dev/null 2>&1 & echo $! > ${childPid}; cat ${bashFifo}` } }] },
  ]));
  // The package's own worker role and delivery module; the fixture auth module, wrapped in the temp
  // install so each provider request takes the next scripted step across host processes.
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
    return (input, init) => {
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
  cli("new", "Pi host killed inside upstream tools", "-p", "app", "--ready");
  events = new EventFollower(join(workspace, ".corral/events.jsonl"));
}, 900_000);

afterAll(() => {
  events?.close();
  for (const pid of started) {
    try { process.kill(pid, "SIGKILL"); } catch { /* already gone */ }
  }
  for (const helper of helpers) helper.kill("SIGKILL");
  for (const id of workspaces) run(["herdr", "--session", "corral-test", "workspace", "close", id]);
  if (workspace) {
    rmSync(join(generationDir(workspace, "T-001", "R1", 1), ".."), { recursive: true, force: true });
    rmSync(workspace, { recursive: true, force: true });
  }
  assets?.cleanup();
});

test("a host killed inside an upstream edit, then inside a command whose child keeps running, resumes reporting both unknown", async () => {
  if (!isolated) return;
  // Given a dispatched production worker whose kickoff turn is inside an upstream edit:
  const edit = holdFifo(editFifo);
  const first = events!.next((event) => event.kind === "agent_bound" && event.task === "T-001");
  cli("dispatch", "T-001", "--profile", "pi", "--force");
  workspaces.push(show("T-001").runs.at(-1).herdr.workspace_id);
  await first;
  await edit.opened;
  const session = show("T-001").runs.at(-1).agent_host.generations[0].bound.session_file;
  expect(readFileSync(session, "utf8")).toContain("call_edit");

  // When its host is killed there and the owner resumes the run:
  await kill(1);
  edit.close();
  const command = holdFifo(bashFifo);
  const afterEdit = await resume(1);

  // Then the edit, recorded by Pi without a result, is outcome_unknown with the worktree evidence:
  expect(afterEdit.calls.map((c: any) => [c.tool, c.class])).toEqual([["edit", "outcome_unknown"]]);
  expect(afterEdit.calls[0].detail).toContain("artifact");

  // When generation 2's notice turn runs a command that starts a child and blocks, and its host
  // is killed inside that command:
  cli("sync");
  await command.opened;
  const child = Number(readFileSync(childPid, "utf8").trim());
  started.push(child);
  expect(readFileSync(session, "utf8")).toContain("call_bash");
  await kill(2);
  const afterCommand = await resume(2);
  command.close();

  // Then the command is outcome_unknown saying its processes may still run, and its child does:
  expect(afterCommand.calls.map((c: any) => [c.tool, c.class])).toEqual([["bash", "outcome_unknown"]]);
  expect(afterCommand.calls[0].detail).toContain("processes it started may still run");
  expect(() => process.kill(child, 0)).not.toThrow();
  const host = show("T-001").runs.at(-1).agent_host;
  expect([host.binding.generation, host.binding.health]).toEqual([3, "healthy"]);
  expect(host.generations.map((g: any) => g.bound.session_file)).toEqual([session, session, session]);
}, 600_000);
