// T-618: actual CLI cancellation -> authenticated questions bridge -> live herdr readback.
// No sync or model input between cancellation and the readback; fixture provider only.
import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { connect } from "./support/bridge-client.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { EventFollower, corralBinary, fileAppears, generationDir, ok, privateHerdr, privateRoot, run } from "./support/workspace.ts";

test("CLI cancel releases a live worker's blocking question without another turn or inbox message", async () => {
  if (!privateHerdr()) {
    console.log("SKIP: needs scripts/test-herdr.sh private corral-test server");
    return;
  }
  const root = mkdtempSync(join(privateRoot()!, "pi-question-cancel-"));
  const assets = buildFixtureInstall({ parent: root, isolatePiState: true });
  const corral = corralBinary();
  const cli = (...args: string[]) => ok([corral, "--root", root, ...args], { cwd: root });
  const show = () => JSON.parse(cli("show", "T-001", "--json"));
  const herdr = (...args: string[]) => JSON.parse(ok(["herdr", "--session", "corral-test", ...args])).result;
  let workspace: string | undefined;
  let follower: EventFollower | undefined;
  let client: Awaited<ReturnType<typeof connect>> | undefined;
  let runtime: string | undefined;
  try {
    mkdirSync(join(root, "app"));
    for (const args of [["init", "-q", "-b", "main"], ["config", "user.name", "test"],
      ["config", "user.email", "test@example.invalid"]]) ok(["git", "-C", join(root, "app"), ...args]);
    writeFileSync(join(root, "app/README.md"), "# fixture\n");
    ok(["git", "-C", join(root, "app"), "add", "."]);
    ok(["git", "-C", join(root, "app"), "commit", "-qm", "fixture"]);
    cli("init");
    const config = join(root, ".corral/corral.toml");
    writeFileSync(config, readFileSync(config, "utf8") + `
[profiles.pi]
harness = "pi-native"
model = "gpt-6.1-sol"
vendor = "openai"
[agent]
assets_dir = ${JSON.stringify(assets.dir)}
`);
    // Keep the fixture role and its offline provider controls, adding only an actual ask tool.
    const role = join(assets.dir, "src/roles/fixture-role.ts");
    writeFileSync(role, readFileSync(role, "utf8")
      .replace('corralOps: ["hello", "event", "receipt_lookup"]', 'corralOps: ["hello", "event", "receipt_lookup", "ask"]')
      .replace('return [{\n        name: "corral_receipt_lookup",', `return [{
        name: "corral_ask", label: "Ask", description: "Ask a blocking question", op: "ask",
        parameters: Type.Object({ kind: Type.String(), text: Type.String(), blocking: Type.Boolean() }),
        payload: (params: Record<string, unknown>) => params,
      }, {\n        name: "corral_receipt_lookup",`)
      .replace('name, arguments: {}', 'name, arguments: { kind: "clarification", text: "Which input?", blocking: true }'));
    // A file notification is a startup barrier, not a probe loop.
    const main = join(assets.dir, "src/host/main.ts");
    writeFileSync(main, readFileSync(main, "utf8").replace('  session.markAttached("interactive");',
      `  session.markAttached("interactive");\n  (await import("node:fs")).writeFileSync(${JSON.stringify(join(root, "attached"))}, "ready");`));
    cli("new", "Cancellation readback", "-p", "app", "--ready");
    follower = new EventFollower(join(root, ".corral/events.jsonl"));
    const bound = follower.next((event) => event.kind === "agent_bound", 90_000, "worker hello");
    cli("dispatch", "T-001", "--profile", "pi", "--force");
    const record = show().runs.at(-1);
    workspace = record.herdr.workspace_id;
    const pane = record.herdr.pane_id;
    await bound;
    await fileAppears(root, "attached");
    runtime = generationDir(root, "T-001", "R1", 1);
    const cap = readFileSync(join(runtime, "ctl.cap"), "utf8");
    client = await connect(join(runtime, "host.sock"));
    let id = 0;
    const request = async (command: string, args: Record<string, unknown> = {}) => {
      const reply = await client!.request({ version: 1, id: `cancel-${++id}`, ctl_cap: cap, generation: 1, command, args });
      expect(reply.ok).toBeTrue();
      return reply.result as any;
    };
    const input = (text: string, key: string) => request("submit_input", {
      text, busy_policy: "follow_up", idempotency_key: key, origin: "cross-lane",
    });
    expect((await input("/fixture-call corral_ask", "script")).disposition).toBe("handled");
    const ended = follower.next((event) => event.kind === "agent_turn_ended", 90_000, "ask turn to end");
    expect((await input("Ask the question.", "ask")).disposition).toBe("started");
    await ended;
    // Herdr's event-driven wait is the report barrier: no projection polling or sleeps.
    herdr("agent", "wait", pane, "--until", "blocked", "--timeout", "30000");
    expect(herdr("pane", "get", pane).pane.agent_status).toBe("blocked");
    const before = await request("get_state");
    expect(before.turn.open).toBeFalse();
    const question = show().runs.at(-1).agent_host.questions[0];
    expect(question.blocking).toBeTrue();
    const cancelled = JSON.parse(cli("question", "cancel", question.key, "--rev", String(question.revision),
      "--owner", "--reason", "Input no longer needed", "--json"));
    expect(cancelled.state).toBe("cancelled");
    expect(cancelled.delivery).toBe("none");
    herdr("agent", "wait", pane, "--until", "idle", "--until", "done", "--timeout", "30000");
    const after = await request("get_state");
    const shown = herdr("pane", "get", pane).pane;
    expect(["idle", "done"]).toContain(shown.agent_status);
    expect(after.herdr.blocked_on).toEqual([]);
    expect(after.turn).toEqual(before.turn);
    expect(after.session.bound_id).toBe(before.session.bound_id);
    console.log(JSON.stringify({ cancellation: cancelled, before: { status: "blocked", turn: before.turn },
      after: { status: shown.agent_status, turn: after.turn, blocked_on: after.herdr.blocked_on }, model_notice: false }));
    await request("shutdown");
  } catch (error) {
    console.error("Cancellation fixture task:", cli("show", "T-001", "--json"));
    if (workspace) console.error("Cancellation fixture pane:", ok(["herdr", "--session", "corral-test", "pane", "read", show().runs.at(-1).herdr.pane_id, "--source", "recent", "--lines", "400"]));
    throw error;
  } finally {
    client?.close();
    follower?.close();
    if (workspace) run(["herdr", "--session", "corral-test", "workspace", "close", workspace]);
    if (runtime) rmSync(join(runtime, ".."), { recursive: true, force: true });
    assets.cleanup();
    rmSync(root, { recursive: true, force: true });
  }
}, 300_000);
