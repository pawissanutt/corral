// Hook interruption cross-lane fixture (T-572): the real corral binary dispatches a pi-native run
// onto the production worker role with full-preset hooks, in the private plugin-free herdr server
// of scripts/test-herdr.sh, with the fixture provider scripted from a steps file. Each mutating
// hook (pre-tool around a read, post-tool around a grep, turn-end) blocks on a FIFO, so the host
// is SIGKILLed inside that hook by rendezvous, never by timing. Each real resume reports the
// affected call or the unfinished hook outcome_unknown from the durable session records, and no
// hook script is launched again.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { type ChildProcess, spawn } from "node:child_process";
import { copyFileSync, cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import { type TestClient, connect } from "./support/bridge-client.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "./support/fixture-install.ts";
import { within } from "./support/wait.ts";
import {
  EventFollower, corralBinary, fileAppears, generationDir, ok, privateHerdr, privateRoot, run,
} from "./support/workspace.ts";

const isolated = privateHerdr();
let corral = "";
let workspace = "";
let hooksDir = "";
let providerFifo = "";
let assets: FixtureInstall | undefined;
let events: EventFollower | undefined;
let sessionTail: ChildProcess | undefined;
const workspaces: string[] = [];
const helpers: ChildProcess[] = [];

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
  return { opened: within(opened, 120_000, `a hook to open ${fifo}`), close: () => child.stdin?.end() };
}

async function untilBound(client: TestClient, cap: string, generation: number): Promise<void> {
  for (;;) {
    const hello = await client.request({ version: 1, id: `hk-hello-${Math.random().toString(36).slice(2, 10)}`,
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

type Entry = { type: string; customType?: string; data?: Record<string, unknown> };

/** Follows the bound session from its start; `recorded` resolves once a hook record is durable. */
function followSession(session: string): (invocation: string, state: string) => Promise<void> {
  let carry = "";
  const seen = new Set<string>();
  const waiters = new Map<string, () => void>();
  sessionTail = spawn("tail", ["-n", "+1", "-F", session], { stdio: ["ignore", "pipe", "ignore"] });
  sessionTail.stdout!.on("data", (chunk) => {
    carry += chunk.toString();
    for (let end = carry.indexOf("\n"); end !== -1; end = carry.indexOf("\n")) {
      const entry = JSON.parse(carry.slice(0, end)) as Entry;
      carry = carry.slice(end + 1);
      if (entry.type !== "custom" || entry.customType !== "corral_hook") continue;
      const key = `${entry.data?.invocation} ${entry.data?.state}`;
      seen.add(key);
      waiters.get(key)?.();
      waiters.delete(key);
    }
  });
  return (invocation, state) => {
    const key = `${invocation} ${state}`;
    if (seen.has(key)) return Promise.resolve();
    return within(new Promise<void>((resolve) => waiters.set(key, resolve)), 90_000, `the ${key} hook record`);
  };
}

/** Arms `phase`'s hook to block, then releases one provider step and waits inside the hook. */
async function interruptInside(phase: string, invocation: string, recorded: ReturnType<typeof followSession>) {
  writeFileSync(join(hooksDir, `block-${phase}`), "");
  const barrier = holdFifo(join(hooksDir, `${phase}.fifo`));
  const started = recorded(invocation, "started");
  cli("sync");
  await within(writeFile(providerFifo, "go"), 90_000, `the ${phase} provider rendezvous`);
  await barrier.opened;
  await started;
  return barrier;
}

beforeAll(() => {
  if (!isolated) {
    console.log("hooks recovery cross-lane: skipped (needs the private corral-test server of scripts/test-herdr.sh)");
    return;
  }
  corral = corralBinary();
  workspace = mkdtempSync(join(privateRoot() ?? "", "pi-hooks-recovery-"));
  const app = join(workspace, "app");
  mkdirSync(app);
  for (const argv of [["init", "-q", "-b", "main"], ["config", "user.name", "corral test"],
    ["config", "user.email", "corral@test.invalid"]]) ok(["git", "-C", app, ...argv]);
  writeFileSync(join(app, "README.md"), "# app\nneedle\n");
  ok(["git", "-C", app, "add", "README.md"]);
  ok(["git", "-C", app, "commit", "-q", "-m", "initial commit"]);
  cli("init");
  hooksDir = join(workspace, "hooks");
  mkdirSync(hooksDir);
  providerFifo = join(workspace, "provider.fifo");
  for (const fifo of [providerFifo, ...["pre", "post", "end"].map((phase) => join(hooksDir, `${phase}.fifo`))]) {
    ok(["mkfifo", fifo]);
  }
  // Each launch is logged, mutates the worktree, then blocks only while the test arms it.
  const hook = join(hooksDir, "hook.sh");
  writeFileSync(hook, `dir=$1; phase=$2
printf '%s %s\\n' "$CORRAL_HOOK_INVOCATION" "$phase" >> "$dir/launches.log"
printf '%s\\n' "$phase" >> hook-mutations.txt
if [ -e "$dir/block-$phase" ]; then cat "$dir/$phase.fifo" > /dev/null; fi
`);
  // Generation 1's kickoff reads, generation 2's notice turn greps, generation 3's just ends.
  const steps = join(workspace, "steps.json");
  writeFileSync(steps, JSON.stringify([
    { toolCalls: [{ callId: "call_read", name: "read", arguments: { path: "README.md" } }] },
    { toolCalls: [{ callId: "call_grep", name: "grep", arguments: { pattern: "needle", path: "." } }] },
    { text: "Done for now" },
  ]));
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
      if (step) {
        await readFile(GATE);
        transport.script([step]);
      }
      return transport.send(input, init);
    };
  },
};
`);
  const argv = (phase: string) => JSON.stringify(["bash", hook, hooksDir, phase]);
  const config = join(workspace, ".corral/corral.toml");
  writeFileSync(config, `${readFileSync(config, "utf8")}
[profiles.pi]
harness = "pi-native"
model = "gpt-6.1-sol"
vendor = "openai"
effort = "high"

[agent]
assets_dir = ${JSON.stringify(assets.dir)}

[agent.capabilities.worker.hooks]
enabled = true
timeout_ms = 600000

[[agent.capabilities.worker.hooks.pre_tool]]
name = "pre"
argv = ${argv("pre")}
tools = ["read"]

[[agent.capabilities.worker.hooks.post_tool]]
name = "post"
argv = ${argv("post")}
tools = ["grep"]

[[agent.capabilities.worker.hooks.turn_end]]
name = "end"
argv = ${argv("end")}
`);
  cli("new", "Pi host killed inside its hooks", "-p", "app", "--ready");
  events = new EventFollower(join(workspace, ".corral/events.jsonl"));
}, 900_000);

afterAll(() => {
  events?.close();
  sessionTail?.kill();
  for (const helper of helpers) helper.kill("SIGKILL");
  for (const id of workspaces) run(["herdr", "--session", "corral-test", "workspace", "close", id]);
  if (workspace) {
    rmSync(join(generationDir(workspace, "T-001", "R1", 1), ".."), { recursive: true, force: true });
    rmSync(workspace, { recursive: true, force: true });
  }
  assets?.cleanup();
});

test("a host killed inside each mutating hook resumes reporting it unknown and never relaunches the script", async () => {
  if (!isolated) return;
  // Given a dispatched production worker with pre-tool, post-tool and turn-end hooks:
  const first = events!.next((event) => event.kind === "agent_bound" && event.task === "T-001");
  cli("dispatch", "T-001", "--profile", "pi", "--force");
  workspaces.push(show("T-001").runs.at(-1).herdr.workspace_id);
  await first;
  const generation = show("T-001").runs.at(-1).agent_host.generations[0];
  expect(generation.capabilities.features.hooks.enabled).toBeTrue();
  const session = generation.bound.session_file;
  const recorded = followSession(session);

  // When the host is killed inside the pre-tool hook around its kickoff read and resumed:
  const pre = await interruptInside("pre", "hk-1-1", recorded);
  await kill(1);
  pre.close();
  const afterPre = await resume(1);
  // Then the read, a no-effect tool by itself, is unknown and names the hook invocation.
  expect(afterPre.calls.map((c: any) => [c.tool, c.class])).toEqual([["read", "outcome_unknown"]]);
  expect(afterPre.calls[0].detail).toContain("hk-1-1 (pre_tool pre, started)");

  // When generation 2's notice turn greps and its host is killed inside the post-tool hook:
  const post = await interruptInside("post", "hk-2-1", recorded);
  await kill(2);
  post.close();
  const afterPost = await resume(2);
  // Then the grep, which ran but whose result was never recorded, is unknown with that hook.
  expect(afterPost.calls.map((c: any) => [c.tool, c.class])).toEqual([["grep", "outcome_unknown"]]);
  expect(afterPost.calls[0].detail).toContain("hk-2-1 (post_tool post, started)");

  // When generation 3's notice turn ends and its host is killed inside the turn-end hook:
  const end = await interruptInside("end", "hk-3-1", recorded);
  rmSync(join(hooksDir, "block-end"));
  await kill(3);
  end.close();
  const afterEnd = await resume(3);
  // Then the unfinished turn-end hook itself is reported unknown under its invocation.
  expect(afterEnd.calls.map((c: any) => [c.tool, c.class, c.tool_call_id]))
    .toEqual([["hook:turn_end:end", "outcome_unknown", "hk-3-1"]]);
  expect(afterEnd.calls[0].detail).toContain("does not replay hook scripts");

  // And each interrupted invocation launched once: no resume started its script again, and the
  // records stayed durable in the one bound session with no completion for a killed host.
  const launches = readFileSync(join(hooksDir, "launches.log"), "utf8").trim().split("\n");
  expect(launches.filter((line) => /^hk-[123]-/.test(line))).toEqual(["hk-1-1 pre", "hk-2-1 post", "hk-3-1 end"]);
  expect(launches.filter((line) => / (pre|post)$/.test(line))).toEqual(["hk-1-1 pre", "hk-2-1 post"]);
  const worktree = show("T-001").runs.at(-1).worktree;
  expect(readFileSync(join(worktree, "hook-mutations.txt"), "utf8").split("\n").slice(0, 3)).toEqual(["pre", "post", "end"]);
  const states = new Map<string, string[]>();
  for (const line of readFileSync(session, "utf8").trim().split("\n")) {
    const entry = JSON.parse(line) as Entry;
    if (entry.type !== "custom" || entry.customType !== "corral_hook") continue;
    const id = String(entry.data?.invocation);
    states.set(id, [...states.get(id) ?? [], String(entry.data?.state)]);
  }
  for (const id of ["hk-1-1", "hk-2-1", "hk-3-1"]) expect([id, states.get(id)]).toEqual([id, ["prepared", "started"]]);
  const host = show("T-001").runs.at(-1).agent_host;
  expect(host.generations.map((g: any) => g.bound.session_file)).toEqual([session, session, session, session]);
}, 600_000);
