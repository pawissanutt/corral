// Live herdr readback (T-605) in the private plugin-free server of scripts/test-herdr.sh. For each of
// the six roles the real host entry point starts in a fresh pane of its own, from a launch message
// and a stand-in corral that commits hello. herdr's pane and agent readbacks must name corral-pi, the
// role, its task/run or assistant session and the bound session; a held turn reads working and its
// cancellation settles idle; a graceful shutdown over the bridge releases the pane and clears the
// metadata while the pane and its shell stay.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import type { HostIdentity, Role } from "../src/roles/types.ts";
import { type TestClient, connect } from "./support/bridge-client.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { type FixtureInstall, buildFixtureInstall } from "./support/fixture-install.ts";
import { within } from "./support/wait.ts";
import { fileAppears, ok, privateHerdr, privateRoot, run } from "./support/workspace.ts";

const isolated = privateHerdr();
let assets: FixtureInstall | undefined;
let root = "";
const workspaces: string[] = [];
const CTL = "c".repeat(48);

const herdr = (...args: string[]): any => JSON.parse(ok(["herdr", "--session", "corral-test", ...args])).result;
const frame = (command: string, args: Record<string, unknown> = {}) =>
  ({ version: 1, id: `hx-${command}-${Math.random().toString(36).slice(2, 10)}`, ctl_cap: CTL, generation: 1, command, args });

async function untilBound(client: TestClient): Promise<void> {
  for (;;) if ((await client.request(frame("hello"))).result?.bound === true) return;
}

/** The host's own record that herdr accepted a state report: reading back afterwards is exact. */
async function untilReported(client: TestClient, state: string, after: number): Promise<number> {
  for (let seq = after; ;) {
    const page = (await client.request(frame("events_since", { seq }))).result as { events: any[]; next: number };
    const event = page.events.find((candidate) => candidate.kind === "herdr_reported" && candidate.state === state);
    if (event) return event.seq;
    seq = page.next;
  }
}

const cases: readonly { identity: HostIdentity; display: string }[] = [
  { identity: { store: "st-0123456789abcdef", project: "app", role: "worker", task: "T-001", run: "R1", generation: 1 },
    display: "corral-pi worker T-001/R1 g1" },
  { identity: { store: "st-0123456789abcdef", project: "app", role: "reviewer", task: "T-001", run: "R1",
    review: "review1", callback: "gate-1:cb-1", generation: 1 }, display: "corral-pi reviewer T-001/R1 review1 g1" },
  { identity: { store: "st-0123456789abcdef", project: "app", role: "planner", task: "T-002", run: "P1", generation: 1 },
    display: "corral-pi planner T-002/P1 g1" },
  { identity: { store: "st-0123456789abcdef", project: "app", role: "plan_reviewer", task: "T-002", run: "P1",
    review: "PR1", generation: 1 }, display: "corral-pi plan_reviewer T-002/P1 PR1 g1" },
  { identity: { store: "st-0123456789abcdef", project: "app", role: "pm", generation: 1 }, display: "corral-pi pm app g1" },
  { identity: { store: "st-0123456789abcdef", project: "", role: "assistant", assistant: "A-1", generation: 1 },
    display: "corral-pi assistant A-1 g1" },
];

beforeAll(() => {
  if (!isolated) {
    console.log("herdr cross-lane: skipped (needs the private corral-test server of scripts/test-herdr.sh)");
    return;
  }
  root = mkdtempSync(join(privateRoot() ?? "", "pi-herdr-"));
  assets = buildFixtureInstall({ parent: root });
}, 120_000);

afterAll(() => {
  for (const id of workspaces) run(["herdr", "--session", "corral-test", "workspace", "close", id]);
  if (root) rmSync(root, { recursive: true, force: true });
  assets?.cleanup();
});

function launch(dir: string, pane: string, identity: HostIdentity): string {
  const corral = writeFakeCorral(dir);
  const runtime = join(dir, "runtime");
  mkdirSync(runtime, { mode: 0o700 });
  const planning: readonly Role[] = ["planner", "plan_reviewer"];
  const message = {
    version: 1, store: identity.store, project: identity.project, role: identity.role, task: identity.task,
    run: identity.run, review: identity.review, callback: identity.callback, assistant: identity.assistant,
    generation: identity.generation,
    secrets: { host_cap: "h".repeat(48), ctl_cap: CTL, bind_nonce: "n".repeat(48) },
    pane, cwd: dir, root: dir, corral: corral.path, assets: assets?.dir, runtime_dir: runtime,
    session_dir: join(dir, "sessions"), model: "gpt-6.1-sol", ui: "plain",
    ...(planning.includes(identity.role)
      ? { capabilities: { catalog: 1, digest: "0".repeat(64), preset: "default", enabled: ["role_enabled"] } } : {}),
  };
  const file = join(dir, "launch.json");
  writeFileSync(file, JSON.stringify(message), { mode: 0o600 });
  const entry = join(assets?.dir ?? "", "src/host/main.ts");
  ok(["herdr", "--session", "corral-test", "pane", "run", pane, `${process.execPath} --no-env-file ${entry} --launch-fd 3 3<${file}`]);
  return runtime;
}

for (const { identity, display } of cases) {
  test(`a bound ${identity.role} host is the herdr agent corral-pi with its identity, state and graceful release`, async () => {
    if (!isolated) return;
    // Given a bound host of this role started by its real entry point in a fresh pane:
    const dir = mkdtempSync(join(root, `${identity.role}-`));
    const created = herdr("workspace", "create", "--cwd", dir, "--label", `t605-${identity.role}`, "--no-focus");
    workspaces.push(created.workspace.workspace_id);
    const pane: string = created.root_pane.pane_id;
    const runtime = launch(dir, pane, identity);
    await fileAppears(runtime, "host.sock");
    const client = await connect(join(runtime, "host.sock"));
    await within(untilBound(client), 60_000, "the host to bind");
    const session = ((await client.request(frame("get_state"))).result?.session as { bound_id: string }).bound_id;

    // Then herdr's readbacks name corral-pi, this role's identity and the bound session, idle:
    let seq = await within(untilReported(client, "idle", 0), 30_000, "the idle report");
    const tokens: Record<string, string> = {
      corral_role: identity.role, corral_generation: "1", corral_session: session,
      ...(identity.project ? { corral_project: identity.project } : {}),
      ...(identity.task ? { corral_task: identity.task } : {}), ...(identity.run ? { corral_run: identity.run } : {}),
      ...(identity.review ? { corral_review: identity.review } : {}),
      ...(identity.assistant ? { corral_assistant: identity.assistant } : {}),
    };
    let shown = herdr("pane", "get", pane).pane;
    expect([shown.agent, shown.agent_status, shown.display_agent, shown.tokens]).toEqual(["corral-pi", "idle", display, tokens]);
    const agents = herdr("agent", "list").agents.filter((agent: any) => agent.pane_id === pane);
    expect(agents.map((agent: any) => [agent.agent, agent.display_agent, agent.tokens?.corral_session]))
      .toEqual([["corral-pi", display, session]]);

    // When a turn is held inside its accepted provider response:
    expect((await client.request(frame("submit_input", {
      text: "/fixture-hold", busy_policy: "follow_up", idempotency_key: "hx-hold", origin: "cross-lane" }))).result?.disposition)
      .toBe("handled");
    expect((await client.request(frame("submit_input", {
      text: "Reply only HELD.", busy_policy: "follow_up", idempotency_key: "hx-held", origin: "cross-lane" }))).result?.disposition)
      .toBe("started");

    // Then herdr reads working; and once the turn is cancelled, idle again:
    seq = await within(untilReported(client, "working", seq), 30_000, "the working report");
    expect(herdr("pane", "get", pane).pane.agent_status).toBe("working");
    expect((await client.request(frame("cancel"))).result?.cancelled).toBeTrue();
    seq = await within(untilReported(client, "idle", seq), 30_000, "the settled idle report");
    expect(["idle", "done"]).toContain(herdr("pane", "get", pane).pane.agent_status);

    // When corral reports an escalated question over the bridge, and later its cancellation:
    const question = { key: "T-001/R1/Q7", kind: "clarification", blocking: false };
    await client.request(frame("questions", { questions: [{ ...question, revision: 2, state: "escalated" }] }));
    seq = await within(untilReported(client, "blocked", seq), 30_000, "the blocked report");
    expect(herdr("pane", "get", pane).pane.agent_status).toBe("blocked");
    await client.request(frame("questions", { questions: [{ ...question, revision: 3, state: "cancelled" }] }));
    await within(untilReported(client, "idle", seq), 30_000, "the idle report after the cancellation");

    // Then herdr showed blocked for the escalation and no longer blocks after the cancellation:
    expect(["idle", "done"]).toContain(herdr("pane", "get", pane).pane.agent_status);

    // When corral asks the host to shut down over the bridge:
    const closed = client.closed();
    await client.request(frame("shutdown"));
    await within(closed, 30_000, "the host to close its bridge");

    // Then the host released the pane and cleared its metadata, and the pane itself stays:
    shown = herdr("pane", "get", pane).pane;
    expect(shown.pane_id).toBe(pane);
    expect(shown.agent === "corral-pi").toBeFalse();
    expect([shown.display_agent, shown.tokens]).toEqual([undefined, undefined]);
  }, 120_000);
}
