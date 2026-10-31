// A real dispatched interactive worker reports completion through corral_done in private herdr.
import { expect, test } from "bun:test";
import { copyFileSync, cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { connect } from "./support/bridge-client.ts";
import { within } from "./support/wait.ts";
import { EventFollower, corralBinary, generationDir, ok, privateHerdr, privateRoot, run } from "./support/workspace.ts";

test("completed interactive worker exits without a stack trace", async () => {
  if (!privateHerdr()) { console.log("skipped: requires private herdr"); return; }
  const workspace = mkdtempSync(join(privateRoot()!, "pi-finished-"));
  const app = join(workspace, "app"); mkdirSync(app);
  let workspaceId = "";
  const assets = buildFixtureInstall({ parent: workspace, omit: ["role", "delivery"] });
  let events: EventFollower | undefined;
  try {
    for (const args of [["init", "-q", "-b", "main"], ["config", "user.name", "test"], ["config", "user.email", "test@example.invalid"]]) ok(["git", "-C", app, ...args]);
    writeFileSync(join(app, "README.md"), "fixture\n");
    ok(["git", "-C", app, "add", "README.md"]); ok(["git", "-C", app, "commit", "-qm", "fixture"]);
    const corral = corralBinary();
    const cli = (...args: string[]) => ok([corral, "--root", workspace, ...args], { cwd: workspace });
    cli("init");
    cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
    const auth = join(assets.dir, "src/auth");
    copyFileSync(join(auth, "index.ts"), join(auth, "fixture-auth.ts"));
    const steps = join(workspace, "steps.json");
    writeFileSync(steps, JSON.stringify([{ text: "Ready to complete the fixture." }]));
    writeFileSync(join(auth, "index.ts"), `import { readFileSync, writeFileSync } from 'node:fs';
import auth, { fixtureTransport } from './fixture-auth.ts';
export default { ...auth, transport() { const transport = fixtureTransport(); return async (input, init) => {
  const steps = JSON.parse(readFileSync(${JSON.stringify(steps)}, 'utf8')); const step = steps.shift();
  writeFileSync(${JSON.stringify(steps)}, JSON.stringify(steps)); if (step) transport.script([step]);
  return transport.send(input, init);
}; } };`);
    const config = join(workspace, ".corral/corral.toml");
    writeFileSync(config, `${readFileSync(config, "utf8")}\n[profiles.pi]\nharness = "pi-native"\nmodel = "gpt-6.1-sol"\nvendor = "openai"\n[agent]\nassets_dir = ${JSON.stringify(assets.dir)}\n`);
    cli("new", "Finish fixture worker", "-p", "app", "--ready");
    events = new EventFollower(join(workspace, ".corral/events.jsonl"));
    const kickoff = events.next(e => e.kind === "agent_turn_ended" && e.task === "T-001", 90_000, "kickoff");
    cli("dispatch", "T-001", "--profile", "pi", "--force");
    const show = () => JSON.parse(cli("show", "T-001", "--json"));
    const launched = show().runs.at(-1);
    workspaceId = launched.herdr.workspace_id;
    const pane = launched.herdr.pane_id;
    await kickoff;
    const identity = JSON.parse(cli("run", "progress", "T-001", "--json"));
    writeFileSync(steps, JSON.stringify([
      { toolCalls: [{ callId: "checkpoint", name: "corral_checkpoint", arguments: { checkpoint: {
        task: "T-001", run: "R1", spec_revision: identity.spec_revision, expected_revision: 0,
        progress_revision: 0, head: identity.head, base: identity.base, tree: identity.tree,
        criterion_ids: [], evidence_refs: [], next_action: "Finish", decisions: [], waits: [], resources: [],
      } } }] },
      { toolCalls: [{ callId: "finish", name: "corral_done", arguments: {
        summary: "Completed fixture task.\nVerified: deterministic worker\nNot verified: none",
      } }] }, { text: "Worker completed." },
    ]));
    const dir = generationDir(workspace, "T-001", "R1", 1);
    const client = await connect(join(dir, "host.sock"));
    const cap = readFileSync(join(dir, "ctl.cap"), "utf8");
    const done = events.next(e => e.kind === "done" && e.task === "T-001", 60_000, "worker done");
    await client.request({ version: 1, id: "finish", ctl_cap: cap, generation: 1, command: "submit_input", args: {
      text: "Complete the task.", busy_policy: "follow_up", idempotency_key: "finish", origin: "test",
    } });
    try { await done; } catch (error) {
      console.log("task:", JSON.stringify(show()));
      console.log("pane:", ok(["herdr", "--session", "corral-test", "pane", "read", pane, "--source", "recent", "--lines", "400"]));
      throw error;
    }
    const finished = show().runs.at(-1);
    expect(finished.outcome).toBe("done");
    expect(finished.agent_host.receipts.some((r: any) => r.op === "done" && r.state === "committed")).toBeTrue();
    // A committed done does not itself issue bridge shutdown; request it after completion.
    const closed = client.closed();
    await client.request({ version: 1, id: "shutdown", ctl_cap: cap, generation: 1, command: "shutdown", args: {} });
    await within(closed, 30_000, "completed worker shutdown");
    const exits = join(workspace, ".corral/state/launch-exits");
    await within((async () => { const { fileAppears } = await import("./support/workspace.ts"); await fileAppears(exits, "T-001-R1.exited"); })(), 30_000, "worker process exit");
    const exitCode = readFileSync(join(exits, "T-001-R1.exited"), "utf8").trim();
    const screen = ok(["herdr", "--session", "corral-test", "pane", "read", pane, "--source", "recent", "--lines", "400"]);
    console.log(`completed task outcome=${finished.outcome}; committed corral_done; post-completion bridge shutdown; exit=${exitCode}; worker pane:\n${screen}`);
    expect(exitCode).toBe("0");
    expect(screen).not.toMatch(/setRawMode failed|corral-pi host stopped|\n\s+at /);
  } finally {
    events?.close();
    if (workspaceId) run(["herdr", "--session", "corral-test", "workspace", "close", workspaceId]);
    assets.cleanup(); rmSync(workspace, { recursive: true, force: true });
  }
}, 180_000);
