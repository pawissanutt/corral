// Cross-lane fixture: the real corral binary dispatches a pi-native run whose [agent] assets_dir
// is a fixture installation, in the private plugin-free herdr server of scripts/test-herdr.sh.
// The pane runs `corral agent launch`, the Bun host binds generation 1 with hello and serves the
// bridge; a fresh corral process (the corral-ops probe) reconnects through ctl.cap alone.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync, existsSync } from "node:fs";
import { isAbsolute, join } from "node:path";

import { type FixtureInstall, buildFixtureInstall } from "./support/fixture-install.ts";
import { connect } from "./support/bridge-client.ts";
import {
  EventFollower, fileAppears, generationDir, ok, privateHerdr, privateRoot, probeBinary, corralBinary, run, secretLeaks, sha256,
} from "./support/workspace.ts";

const isolated = privateHerdr();
const herdrRoot = privateRoot() ?? "";
let corral = "";
let probe = "";
let workspace = "";
let good: FixtureInstall | undefined;
let badBun: FixtureInstall | undefined;
let events: EventFollower | undefined;
const workspaces: string[] = [];

const cli = (...args: string[]) => ok([corral, "--root", workspace, ...args], { cwd: workspace });
const show = (task: string): any => JSON.parse(cli("show", task, "--json"));
const frame = (cap: string | undefined, command: string, args: Record<string, unknown> = {}) =>
  ({ version: 1, id: `xl-${command}-${Math.random().toString(36).slice(2, 10)}`, ctl_cap: cap, generation: 1, command, args });

function probeOnce(extra: Record<string, unknown> = {}): Record<string, unknown> {
  const result = run([probe, "--exact", "agent::bridge::tests::fresh_process_probe", "--ignored", "--nocapture",
    "--test-threads", "1"], { env: { ...process.env, CORRAL_BRIDGE_PROBE: JSON.stringify({
    root: workspace, task: "T-001", run: "R1", generation: 1, ...extra }) } });
  const line = /BRIDGE_PROBE (\{.*\})$/m.exec(result.stdout)?.[1];
  if (!line) throw new Error(`the probe printed no result: ${result.stdout}\n${result.stderr}`);
  return JSON.parse(line) as Record<string, unknown>;
}

function configure(assets: string): void {
  const path = join(workspace, ".corral/corral.toml");
  const config = readFileSync(path, "utf8").replace(/\n\[agent\]\nassets_dir = .*\n/, "\n");
  writeFileSync(path, `${config}\n[agent]\nassets_dir = ${JSON.stringify(assets)}\n`);
}

beforeAll(() => {
  if (!isolated) {
    console.log("cross-lane: skipped (needs the private corral-test server of scripts/test-herdr.sh)");
    return;
  }
  corral = corralBinary();
  probe = probeBinary();
  workspace = mkdtempSync(join(herdrRoot, "pi-cross-"));
  mkdirSync(join(workspace, "app"));
  for (const argv of [["init", "-q", "-b", "main"], ["config", "user.name", "corral test"],
    ["config", "user.email", "corral@test.invalid"]]) ok(["git", "-C", join(workspace, "app"), ...argv]);
  writeFileSync(join(workspace, "app/README.md"), "# app\n");
  ok(["git", "-C", join(workspace, "app"), "add", "README.md"]);
  ok(["git", "-C", join(workspace, "app"), "commit", "-q", "-m", "initial commit"]);
  cli("init");
  writeFileSync(join(workspace, ".corral/corral.toml"), `${readFileSync(join(workspace, ".corral/corral.toml"), "utf8")}
[profiles.pi]
harness = "pi-native"
model = "gpt-6.1-sol"
vendor = "openai"
effort = "high"
`);
  good = buildFixtureInstall({ parent: workspace });
  // Hello binds before InteractiveMode.init (the documented host order). Observe the
  // actual attached event in this disposable host, rather than racing it with get_state.
  // Only the fixture writes this readiness signal; production startup stays unchanged.
  const main = join(good.dir, "src/host/main.ts");
  const source = readFileSync(main, "utf8");
  const attach = '  session.markAttached("interactive");';
  expect(source.split(attach)).toHaveLength(2);
  writeFileSync(main, source.replace(attach, `${attach}\n  (await import("node:fs")).writeFileSync(${JSON.stringify(join(workspace, "interactive-ready"))}, "attached");`));
  badBun = buildFixtureInstall({ parent: workspace });
  const compat = JSON.parse(readFileSync(join(badBun.dir, "compat.json"), "utf8"));
  writeFileSync(join(badBun.dir, "compat.json"), `${JSON.stringify({ ...compat, bun: { range: "0.0.1", tested: ["0.0.1"] } })}\n`);
  configure(good.dir);
  cli("new", "Pi host bridge", "-p", "app", "--ready");
  events = new EventFollower(join(workspace, ".corral/events.jsonl"));
}, 900_000);

afterAll(() => {
  events?.close();
  for (const id of workspaces) run(["herdr", "--session", "corral-test", "workspace", "close", id]);
  if (workspace) {
    for (const task of ["T-001", "T-002"]) rmSync(join(generationDir(workspace, task, "R1", 1), ".."), { recursive: true, force: true });
    rmSync(workspace, { recursive: true, force: true });
  }
});

test("dispatch launches the host in the run pane and hello binds generation 1 without leaking a secret", async () => {
  if (!isolated) return;
  const bound = events!.next((event) => event.kind === "agent_bound" && event.task === "T-001");
  cli("dispatch", "T-001", "--profile", "pi", "--force");
  const dispatched = show("T-001").runs.at(-1);
  workspaces.push(dispatched.herdr.workspace_id);
  expect(dispatched.harness).toBe("pi-native");
  await bound;
  const runRecord = show("T-001").runs.at(-1);
  const host = runRecord.agent_host;
  expect(host.binding.health).toBe("healthy");
  expect(host.binding.generation).toBe(1);
  const generation = host.generations[0];
  expect(generation.n).toBe(1);
  expect(generation.pane).toBe(runRecord.herdr.pane_id);
  const dir = generationDir(workspace, "T-001", "R1", 1);
  expect(statSync(dir).mode & 0o777).toBe(0o700);
  expect(existsSync(join(dir, "bootstrap.json"))).toBeFalse();
  expect(statSync(join(dir, "ctl.cap")).mode & 0o777).toBe(0o600);
  expect(statSync(join(dir, "host.sock")).mode & 0o777).toBe(0o600);
  const ctlCap = readFileSync(join(dir, "ctl.cap"), "utf8");
  expect(sha256(ctlCap)).toBe(generation.bound.ctl_cap_sha256);
  const pid = generation.bound.pid as number;
  const argv = readFileSync(`/proc/${pid}/cmdline`, "utf8").split("\0").filter(Boolean);
  expect(argv.slice(1, 4)).toEqual(["--no-env-file", join(good!.dir, "src/host/main.ts"), "--launch-fd"]);
  const tty = readFileSync(`/proc/${pid}/stat`, "utf8").split(") ")[1]?.split(" ")[4];
  expect(tty).not.toBe("0");
  const leaks = secretLeaks(
    [join(workspace, ".corral"), join(herdrRoot, "state")],
    {
      cmdline: argv.join(" "), environ: readFileSync(`/proc/${pid}/environ`, "utf8"),
      server_log: readFileSync(join(herdrRoot, "server.log"), "utf8"),
    },
    { plain: [ctlCap], digests: [generation.bind_nonce_sha256, generation.bound.host_cap_sha256] },
  );
  expect(leaks).toEqual([]);
}, 300_000);

test("a fresh corral process reaches the host through ctl.cap alone; a wrong or missing ctl_cap is refused", async () => {
  if (!isolated) return;
  // A bounded fs.watch barrier, not a sleep, a retried assertion or repeated probes.
  await fileAppears(workspace, "interactive-ready", 4_000);
  const first = probeOnce() as { ok: boolean; hello: Record<string, unknown>; state: Record<string, unknown> };
  expect(first.ok).toBeTrue();
  expect(first.hello.bound).toBeTrue();
  expect(first.hello.role).toBe("worker");
  expect(first.state.generation).toBe(1);
  expect(first.state.interactive).toBeTrue();
  expect([...(first.state.active_tools as string[])].sort()).toEqual([
    "browser_navigate", "browser_screenshot", "codemode", "corral_receipt_lookup", "explore", "mcp_activate", "mcp_connect",
    "mcp_read_resource", "mcp_resources",
    "read", "tool_search", "web_fetch", "web_search",
  ]);
  const dir = generationDir(workspace, "T-001", "R1", 1);
  for (const cap of [undefined, "f".repeat(64)]) {
    const client = await connect(join(dir, "host.sock"));
    const reply = await client.request(frame(cap, "get_state"));
    expect(reply.error?.code).toBe("auth_failed");
    await client.closed();
  }
  cli("sync");
  const again = probeOnce() as { ok: boolean; state: Record<string, unknown> };
  expect(again.ok).toBeTrue();
  expect(again.state.bound).toBeTrue();
}, 300_000);

test("bridge input through the pane host is truthful and its turn reaches corral as receipts", async () => {
  if (!isolated) return;
  const dir = generationDir(workspace, "T-001", "R1", 1);
  const cap = readFileSync(join(dir, "ctl.cap"), "utf8");
  const client = await connect(join(dir, "host.sock"));
  const handled = await client.request(frame(cap, "submit_input", {
    text: "/fixture-handled", busy_policy: "prompt", idempotency_key: "xl-handled", origin: "cross-lane" }));
  expect(handled.result?.disposition).toBe("handled");
  const ended = events!.next((event) => event.kind === "agent_turn_ended" && event.task === "T-001");
  const started = await client.request(frame(cap, "submit_input", {
    text: "Reply only CROSSLANE.", busy_policy: "follow_up", idempotency_key: "xl-start", origin: "cross-lane" }));
  expect(started.result).toEqual({ request_id: started.id, idempotency_key: "xl-start", disposition: "started", turn: 1 });
  await ended.catch(async (error: Error) => {
    const state = (await client.request(frame(cap, "get_state"))).result;
    throw new Error(`${error.message}; host turn ${JSON.stringify(state?.turn)} corral_calls ${JSON.stringify(state?.corral_calls)}`);
  });
  const page = (await client.request(frame(cap, "events_since", { seq: 0 }))).result as {
    events: { seq: number; generation: number; kind: string }[];
  };
  expect(page.events.every((event) => event.generation === 1)).toBeTrue();
  const seqs = page.events.map((event) => event.seq);
  expect(seqs).toEqual([...seqs].sort((a, b) => a - b));
  expect(page.events.map((event) => event.kind)).toContain("turn_ended");
  const cancel = await client.request(frame(cap, "cancel", {}));
  expect(cancel.result).toEqual({ request_id: cancel.id, cancelled: false, reason: "idle" });
  const turns = show("T-001").runs.at(-1).agent_host.turns as { seq: number; origin: { kind: string; idempotency_key?: string }; ended?: string }[];
  expect(turns[0]?.origin).toEqual({ kind: "bridge", idempotency_key: "xl-start" });
  expect(turns[0]?.ended).toBeDefined();
  client.close();
}, 300_000);

test("a busy host's event backlog pages through the Rust reader, which keeps its connection for the next request", async () => {
  if (!isolated) return;
  const dir = generationDir(workspace, "T-001", "R1", 1);
  const cap = readFileSync(join(dir, "ctl.cap"), "utf8");
  const client = await connect(join(dir, "host.sock"));
  for (let n = 0; n < 256; n++) {
    const reply = await client.request(frame(cap, "submit_input", {
      text: "/fixture-handled", busy_policy: "prompt", origin: "o".repeat(64),
      idempotency_key: `${String(n).padStart(4, "0")}-${"k".repeat(122)}`,
    }));
    expect(reply.result?.disposition).toBe("handled");
  }
  const state = await client.request(frame(cap, "get_state"));
  expect(state.ok).toBeTrue();
  expect(state.wireBytes).toBeLessThanOrEqual(65_536);
  expect((state.result?.delivery as { omitted?: string }).omitted).toBe("state_too_large");
  const backlog = probeOnce({ events: true }) as { ok: boolean; pages: number; seqs: number[]; last: number; after: number };
  expect(backlog.ok).toBeTrue();
  expect(backlog.pages).toBeGreaterThan(1);
  expect(backlog.seqs.length).toBeGreaterThan(256);
  expect(backlog.seqs).toEqual(Array.from({ length: backlog.seqs.length }, (_, index) => index + 1));
  expect(backlog.last).toBe(backlog.seqs.at(-1));
  expect(backlog.after).toBe(1);
  const shut = await client.request(frame(cap, "shutdown"));
  expect(shut.result?.shutting_down).toBeTrue();
  await client.closed();
}, 300_000);

test("a launch whose Bun is outside the GO range is refused before exec, fails the run and frees its slot", async () => {
  if (!isolated) return;
  configure(badBun!.dir);
  cli("new", "Pi bad bun", "-p", "app", "--ready");
  cli("dispatch", "T-002", "--profile", "pi", "--force");
  workspaces.push(show("T-002").runs.at(-1).herdr.workspace_id);
  const exits = join(workspace, ".corral/state/launch-exits");
  await fileAppears(exits, "T-002-R1.exited");
  expect(readFileSync(join(exits, "T-002-R1.exited"), "utf8").trim()).not.toBe("0");
  const dir = generationDir(workspace, "T-002", "R1", 1);
  expect(existsSync(join(dir, "bootstrap.json"))).toBeFalse();
  expect(existsSync(join(dir, "ctl.cap"))).toBeFalse();
  cli("sync");
  const task = show("T-002");
  const attempt = task.runs.at(-1);
  expect(attempt.outcome).toBe("failed");
  expect(attempt.ended).toBeDefined();
  expect(attempt.agent_host.generations[0].bound).toBeUndefined();
  const log = (task.artifacts as { title?: string; path: string }[]).find((artifact) => artifact.title === "Agent launch failure");
  expect(log).toBeDefined();
  expect(readFileSync(isAbsolute(log!.path) ? log!.path : join(workspace, ".corral", log!.path), "utf8"))
    .toContain("bun_version_unsupported");
}, 300_000);
