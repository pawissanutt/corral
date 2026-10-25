// Recovery cross-lane fixture (T-252 recovery): the real corral binary dispatches a pi-native run
// onto a fixture installation in the private plugin-free herdr server of scripts/test-herdr.sh.
// `corral agent cancel` reaches the idle host and then a turn whose provider request was
// accepted, without fencing. The Bun host is SIGKILLed inside a held turn while the pane stays;
// sync records the loss and keeps the run, and `corral agent resume` launches generation 2 in
// the same pane on the same Pi session. Generation 2 receives the recovery notice ahead of a
// reply queued while the host was lost; Pi's session file keeps only the notice's placeholder,
// corral observes the delivery, and the silent generation-2 turn gets its nudge though the
// killed turn never ended.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { type TestClient, connect } from "../../test/support/bridge-client.ts";
import { type FixtureInstall, buildFixtureInstall } from "../../test/support/fixture-install.ts";
import { within } from "../../test/support/wait.ts";
import {
  EventFollower, corralBinary, fileAppears, generationDir, ok, privateHerdr, privateRoot, run, sha256,
} from "../../test/support/workspace.ts";

const isolated = privateHerdr();
let corral = "";
let workspace = "";
let assets: FixtureInstall | undefined;
let events: EventFollower | undefined;
const workspaces: string[] = [];

const cli = (...args: string[]) => ok([corral, "--root", workspace, ...args], { cwd: workspace });
const show = (task: string): any => JSON.parse(cli("show", task, "--json"));
const frame = (cap: string, command: string, args: Record<string, unknown>) =>
  ({ version: 1, id: `rc-${command}-${Math.random().toString(36).slice(2, 10)}`, ctl_cap: cap, generation: 1, command, args });

async function untilBound(client: TestClient, cap: string): Promise<void> {
  for (;;) {
    const hello = await client.request(frame(cap, "hello", {}));
    if (hello.result?.bound === true) return;
  }
}

/** The fixture provider holds a response it has accepted, so the host's turn stays open. */
async function untilHeld(client: TestClient, cap: string): Promise<void> {
  for (;;) {
    const state = await client.request(frame(cap, "get_state", {}));
    if ((state.result?.auth as { transport_held?: boolean } | undefined)?.transport_held === true) return;
  }
}

async function submit(client: TestClient, cap: string, text: string, key: string) {
  const reply = await client.request(frame(cap, "submit_input", {
    text, busy_policy: "follow_up", idempotency_key: key, origin: "cross-lane" }));
  return reply.result;
}

/** A turn of the host whose provider request is accepted and then held. */
async function heldTurn(client: TestClient, cap: string, n: number): Promise<void> {
  expect((await submit(client, cap, "/fixture-hold", `rc-hold-${n}`))?.disposition).toBe("handled");
  expect((await submit(client, cap, `Reply only HELD_${n}.`, `rc-held-${n}`))?.disposition).toBe("started");
  await within(untilHeld(client, cap), 30_000, "the fixture provider to hold its response");
}

const pad = (seq: number) => String(seq).padStart(6, "0");

beforeAll(() => {
  if (!isolated) {
    console.log("recovery cross-lane: skipped (needs the private corral-test server of scripts/test-herdr.sh)");
    return;
  }
  corral = corralBinary();
  workspace = mkdtempSync(join(privateRoot() ?? "", "pi-recovery-"));
  const app = join(workspace, "app");
  mkdirSync(app);
  for (const argv of [["init", "-q", "-b", "main"], ["config", "user.name", "corral test"],
    ["config", "user.email", "corral@test.invalid"]]) ok(["git", "-C", app, ...argv]);
  writeFileSync(join(app, "README.md"), "# app\n");
  ok(["git", "-C", app, "add", "README.md"]);
  ok(["git", "-C", app, "commit", "-q", "-m", "initial commit"]);
  cli("init");
  // The package's own delivery module, so corral observes generation 2 from its corral_delivery entry.
  assets = buildFixtureInstall({ parent: workspace, omit: ["delivery"] });
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
  cli("new", "Pi host recovery", "-p", "app", "--ready");
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

test("cancel never fences, and a host killed inside a turn resumes on its Pi session with the notice kept out of it", async () => {
  if (!isolated) return;
  // Given a dispatched run whose bound host has completed one turn, so Pi has written its session:
  const first = events!.next((event) => event.kind === "agent_bound" && event.task === "T-001");
  cli("dispatch", "T-001", "--profile", "pi", "--force");
  workspaces.push(show("T-001").runs.at(-1).herdr.workspace_id);
  await first;
  const dir = generationDir(workspace, "T-001", "R1", 1);
  const cap = readFileSync(join(dir, "ctl.cap"), "utf8");
  const client = await connect(join(dir, "host.sock"));
  // corral records agent_bound before the host's own hello call returns: read the host's state.
  await within(untilBound(client, cap), 30_000, "the host to report itself bound");
  const ended = events!.next((event) => event.kind === "agent_turn_ended" && event.task === "T-001");
  const started = await client.request(frame(cap, "submit_input", {
    text: "Reply only BEFORE.", busy_policy: "follow_up", idempotency_key: "rc-before", origin: "cross-lane" }));
  expect([started.result?.disposition, started.result?.reason]).toEqual(["started", undefined]);
  await ended;

  // When the owner cancels while the host is idle, then nothing is cancelled:
  const idle = JSON.parse(cli("agent", "cancel", "T-001"));
  expect([idle.cancelled, idle.reason]).toEqual([false, "idle"]);

  // When the owner cancels a turn whose provider request was accepted:
  const cancelEnded = events!.next((event) => event.kind === "agent_turn_ended" && event.task === "T-001"
    && event.detail?.generation === 1 && event.detail?.turn === 2);
  await heldTurn(client, cap, 2);
  const cancelled = JSON.parse(cli("agent", "cancel", "T-001"));
  expect([cancelled.cancelled, cancelled.turn]).toEqual([true, 2]);

  // Then the host ends that turn, classified cancelled, and its generation stays healthy and unfenced:
  await cancelEnded;
  let state = show("T-001").runs.at(-1).agent_host;
  const cancelledTurn = state.turns.find((t: any) => t.generation === 1 && t.seq === 2);
  expect(cancelledTurn?.class).toBe("cancelled");
  expect([state.binding.health, state.generations[0].fenced]).toEqual(["healthy", undefined]);

  // Given a third turn whose provider response is held, so the host is busy inside it:
  await heldTurn(client, cap, 3);
  client.close();
  const lostGeneration = show("T-001").runs.at(-1).agent_host.generations[0];

  // When the host process is killed while its pane stays, and sync runs:
  const exits = join(workspace, ".corral/state/launch-exits");
  mkdirSync(exits, { recursive: true });
  const exited = fileAppears(exits, "T-001-R1.exited");
  process.kill(lostGeneration.bound.pid, "SIGKILL");
  await exited;
  const lost = events!.next((event) => event.kind === "agent_lost" && event.task === "T-001");
  cli("sync");

  // Then the loss is recorded, and the run stays live in Needs me naming resume, retry and discard:
  expect((await lost).detail?.reason).toBe("host_exited");
  let task = show("T-001");
  expect(task.status).toBe("needs_me");
  expect(task.runs.at(-1).outcome).toBeUndefined();
  expect(task.runs.at(-1).blocked_excerpt).toMatch(/^Pi-native host lost: T-001\/R1 generation 1\./);

  // When the owner replies while the host is lost, then resumes the run:
  cli("reply", "T-001", "Queued while the host was lost.");
  const second = events!.next((event) => event.kind === "agent_bound" && event.task === "T-001"
    && event.detail?.generation === 2);
  const resumed = JSON.parse(cli("agent", "resume", "T-001"));
  expect([resumed.fenced, resumed.generation, resumed.pane]).toEqual([1, 2, lostGeneration.pane]);
  await second;

  // Then generation 2 runs in the same pane on the same Pi session, and the task is Running:
  task = show("T-001");
  const host = task.runs.at(-1).agent_host;
  const [fenced, current] = host.generations;
  expect(task.status).toBe("running");
  expect([host.binding.generation, host.binding.health]).toEqual([2, "healthy"]);
  expect(fenced.fenced.reason).toBe("resume");
  expect(current.pane).toBe(fenced.pane);
  expect(current.bound.session_id).toBe(fenced.bound.session_id);
  expect(current.bound.session_file).toBe(fenced.bound.session_file);
  expect(current.bound.pid).not.toBe(fenced.bound.pid);

  // And the next sync delivers the recovery notice ahead of the reply, to generation 2 of that session:
  const resumedEnded = events!.next((event) => event.kind === "agent_turn_ended" && event.task === "T-001"
    && event.detail?.generation === 2);
  cli("sync");
  const envelopes = join(workspace, ".corral/inbox/T-001_R1/envelopes");
  const latest = readdirSync(envelopes).filter((name) => name.endsWith(".payload")).sort().at(-1) ?? "";
  const payload = readFileSync(join(envelopes, latest), "utf8");
  const notice = payload.indexOf(" recovery_notice]");
  expect(notice).toBeGreaterThan(-1);
  expect(payload.indexOf("Queued while the host was lost.")).toBeGreaterThan(notice);
  const envelope = JSON.parse(readFileSync(join(envelopes, latest.replace(".payload", ".json")), "utf8"));
  const inbox = join(workspace, ".corral/inbox/T-001_R1");
  const [noticeSeq = -1] = envelope.members as number[];
  const admitted = JSON.parse(readFileSync(join(inbox, `${pad(noticeSeq)}.receipt.admitted.json`), "utf8"));
  expect([admitted.detail.generation, admitted.binding.session]).toEqual([2, fenced.bound.session_id]);

  // And once the turn it started ends, generation 2's delivery module has recorded that envelope,
  // while Pi's session file keeps the notice's placeholder and never its classifications:
  await resumedEnded;
  const raw = readFileSync(current.bound.session_file, "utf8");
  const entries = raw.trim().split("\n").map((line) => JSON.parse(line));
  const recorded = entries.filter((entry) => entry.customType === "corral_delivery" && entry.data?.generation === 2);
  expect(recorded.map((entry) => entry.data.text_sha256)).toEqual([sha256(payload)]);
  expect(raw).toContain(`#${noticeSeq} recovery_notice: given to this host as context only`);
  expect(raw.includes("corral resumed this host as generation 2")).toBeFalse();
  expect(raw).toContain("Queued while the host was lost.");

  // When sync runs again, then corral observes the notice from that entry, the killed turn of
  // generation 1 is still recorded open, and generation 2's silent turn gets its one nudge:
  cli("sync");
  expect(existsSync(join(inbox, `${pad(noticeSeq)}.receipt.observed.json`))).toBeTrue();
  state = show("T-001").runs.at(-1).agent_host;
  const killed = state.turns.find((t: any) => t.generation === 1 && t.seq === 3);
  expect([killed?.started === undefined, killed?.ended]).toEqual([false, undefined]);
  const nudges = readdirSync(inbox).filter((name) => /^\d{6}\.md$/.test(name))
    .map((name) => readFileSync(join(inbox, name), "utf8")).filter((text) => text.includes(" turn_nudge]"));
  expect(nudges).toHaveLength(1);
  expect(nudges[0]).toContain("generation: 2");
}, 600_000);
