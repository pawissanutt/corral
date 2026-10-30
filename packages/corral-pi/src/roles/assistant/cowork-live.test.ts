// Integrated co-work landing (T-544 C9 and C10): the production assistant role, auth and
// HostSession in a private live herdr pane, with the real corral binary as receipt client, as the
// assistant's shell commands and as every owner action's command. Only the provider network
// boundary is deterministic and offline.
import { expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { EventLog } from "../../bridge/events.ts";
import { parseLaunchMessage } from "../../host/config.ts";
import { loadModules } from "../../host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../../host/restrict.ts";
import { HostSession } from "../../host/session.ts";
import { CorralClient, sha256Hex } from "../../tools/corral.ts";
import { fakeCodexTransport } from "../../../test/fixtures/auth/fake-transport.ts";
import { exportAssistantSession } from "./env.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "../../../test/support/fixture-install.ts";
import { corralBinary, ok, privateHerdr, privateRoot, probeBinary, run } from "../../../test/support/workspace.ts";

function syntheticAccess(): string {
  const part = (value: object) => Buffer.from(JSON.stringify(value)).toString("base64url");
  return `${part({ alg: "none", typ: "JWT" })}.${part({ exp: Math.floor(Date.now() / 1000) + 3600,
    "https://api.openai.com/auth": { chatgpt_account_id: "synthetic" } })}.synthetic-signature`;
}

interface Fixture { session: string; bootstrap: string; launch: string; scratch: string[] }
interface StoreEvent { kind: string; task?: string; actor?: string; via?: string; detail?: Record<string, unknown> }
type Step = { toolCalls: { callId: string; name: string; arguments: Record<string, unknown> }[] } | { text: string };

test("live private-pane co-work: shell finish refused, unclear ship asks, promote, finish without review, approve lands", async () => {
  if (!privateHerdr()) {
    process.stdout.write("cowork live: skipped (requires fresh scripts/test-herdr.sh private server)\n");
    return;
  }
  const originalEnv = { ...process.env };
  const originalFetch = globalThis.fetch;
  const workspace = mkdtempSync(join(privateRoot() ?? "", "assistant-cowork-"));
  const assets = buildFixtureInstall({ parent: workspace, overlays: false });
  let workspaceId = "";
  let host: HostSession | undefined;
  try {
    cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
    const plugins = ok(["herdr", "--session", "corral-test", "plugin", "list"]);
    expect(plugins.trim()).toBe("No plugins installed.");
    const corral = corralBinary();
    const probe = probeBinary();
    const app = join(workspace, "app");
    mkdirSync(app);
    for (const args of [["init", "-q", "-b", "main"], ["config", "user.name", "corral test"],
      ["config", "user.email", "corral@test.invalid"]]) ok(["git", "-C", app, ...args]);
    writeFileSync(join(app, "README.md"), "# co-work\n");
    writeFileSync(join(app, ".gitignore"), ".worktrees/\n");
    ok(["git", "-C", app, "add", "README.md", ".gitignore"]);
    ok(["git", "-C", app, "commit", "-q", "-m", "fixture"]);
    const created = JSON.parse(ok(["herdr", "--session", "corral-test", "workspace", "create", "--cwd", workspace, "--no-focus"]));
    workspaceId = created.result.workspace.workspace_id;
    const pane: string = created.result.root_pane.pane_id;
    const setup = ok([probe, "--exact", "agent::ops::assistant::cowork_live_tests::cowork_live_setup", "--ignored", "--nocapture"],
      { env: { ...process.env, CORRAL_COWORK_LIVE: JSON.stringify({ root: workspace, assets: assets.dir, pane }) } });
    const line = /COWORK_LIVE (\{.*\})/.exec(setup)?.[1];
    if (!line) throw new Error(`co-work setup produced no result: ${setup.slice(-2000)}`);
    const fixture = JSON.parse(line) as Fixture;
    const bootstrap = JSON.parse(readFileSync(fixture.bootstrap, "utf8"));
    const launch = parseLaunchMessage(readFileSync(fixture.launch, "utf8"));
    const data = join(workspace, "data");
    const auth = join(data, "corral/pi-native/auth");
    mkdirSync(auth, { recursive: true, mode: 0o700 });
    writeFileSync(join(auth, "auth.json"), JSON.stringify({ "openai-codex": { type: "oauth", access: syntheticAccess(),
      refresh: "synthetic-refresh", expires: Date.now() + 3_600_000, accountId: "synthetic" } }), { mode: 0o600 });
    process.env.XDG_DATA_HOME = data;
    restrictEnvironment(process.env);
    const transport = fakeCodexTransport();
    const network = guardedFetch(async (input, init) => transport.send(input, init), undefined, "openai-codex", undefined,
      launch.capabilities?.web ? { settings: launch.capabilities.web, send: transport.send } : undefined,
      launch.capabilities?.mcp ? { origins: [], send: transport.send } : undefined);
    globalThis.fetch = network.fetch as typeof globalThis.fetch;
    const identity = { store: bootstrap.store as string, project: "", role: "assistant" as const,
      assistant: fixture.session, generation: bootstrap.generation as number };
    exportAssistantSession(process.env, identity);
    const client = new CorralClient({ corral, root: workspace, identity, hostCap: "e".repeat(64), sessionId: () => host?.sessionId() ?? "" });
    const events = new EventLog(identity.generation);
    const sdk = await import("@earendil-works/pi-coding-agent");
    host = await HostSession.create({ sdk, identity, cwd: workspace, agentDir: join(workspace, "pi-agent"),
      model: bootstrap.model, modules: await loadModules(assets.dir, "assistant"), client, events, network, ui: "plain",
      todo: launch.todo === true,
      ...(launch.capabilities ? { capabilities: launch.capabilities } : {}),
      ...(launch.guidance ? { guidance: launch.guidance } : {}),
      root: workspace, runtimeDir: dirname(fixture.bootstrap), exportDir: join(bootstrap.session_dir, "exports"),
      sessionManager: sdk.SessionManager.create(workspace, bootstrap.session_dir) });
    await host.hello(bootstrap.bind_nonce, pane, sha256Hex("d".repeat(64)));
    const asked: string[] = [];
    const ui = { confirm: async (_title: string, message: string) => { asked.push(message); return false; } };
    await host.session.bindExtensions({ uiContext: ui as unknown as ExtensionUIContext });
    const live = host;
    const input = (n: number) => `${fixture.session}/g${identity.generation}/i${n}`;
    const quote = (s: string) => `'${s.replaceAll("'", "'\\''")}'`;
    const cli = `${quote(corral)} --root ${quote(workspace)}`;
    const turn = async (text: string, script: Step[]) => {
      transport.script(script as never);
      await live.session.prompt(text, { source: "interactive" });
      await live.drained();
    };
    const toolText = (callId: string): string => {
      const message = live.session.messages.find((m) => m.role === "toolResult" && m.toolCallId.split("|")[0] === callId);
      if (message?.role !== "toolResult") throw new Error(`no result for ${callId}`);
      return message.content.map((part) => (part.type === "text" ? part.text : "")).join("");
    };
    const result = (callId: string): { state: string; result: Record<string, unknown> } => JSON.parse(toolText(callId));
    const bash = (callId: string, command: string): Step => ({ toolCalls: [{ callId, name: "bash", arguments: { command } }] });
    const show = (id: string) => JSON.parse(ok([corral, "show", id, "--json"], { cwd: workspace }));
    const tasks = () => JSON.parse(ok([corral, "list", "--json"], { cwd: workspace })) as { id: string }[];
    const record = () => JSON.parse(ok([corral, "assistant", "show", fixture.session, "--json"], { cwd: workspace })).session;
    const kinds = () => Object.fromEntries((record().scratch as { slug: string; kind: string }[]).map((s) => [s.slug, s.kind]));
    const storeEvents = (): StoreEvent[] => readFileSync(join(workspace, ".corral/events.jsonl"), "utf8").trim().split("\n")
      .map((l) => JSON.parse(l) as StoreEvent);

    // C10: the assistant's shell offers the unattended spike for landing; corral refuses it.
    await turn("offer spike-idea for landing", [
      bash("offer", `${cli} assistant scratch finish spike-idea --title 'Spike an idea'; echo EXIT=$?`),
      { text: "The shell cannot finish a scratch; finishing is your call." }]);
    const offered = toolText("offer");
    expect(offered).toContain("assistant_refused: corral assistant scratch finish needs the owner");
    expect(offered).toContain("EXIT=2");
    expect(tasks()).toHaveLength(0);

    // C10: "ship something like this" names no scratch, title or review choice: the assistant asks.
    const question = "Which scratch should I finish (cache-fix or spike-idea), under what title, and should it be reviewed?";
    await turn("ship something like this", [{ text: question }]);
    expect(JSON.stringify(live.session.messages.filter((m) => m.role === "assistant").at(-1))).toContain(question);
    expect(tasks()).toHaveLength(0);
    expect(kinds()).toEqual({ "cache-fix": "spike", "spike-idea": "spike" });

    // C10: promoting the spike files a Backlog task with its diff and leaves the scratch as it was.
    const spikeHead = ok(["git", "-C", fixture.scratch[1] ?? "", "rev-parse", "HEAD"]).trim();
    await turn("file spike-idea as a backlog task instead", [
      bash("promote", `${cli} assistant scratch promote spike-idea --title 'Spike an idea' --json`), { text: "Filed T-001." }]);
    const promoted = JSON.parse(toolText("promote").slice(toolText("promote").indexOf("{")));
    expect(promoted.task).toBe("T-001");
    const backlog = show("T-001");
    expect(backlog.status).toBe("backlog");
    expect(backlog.artifacts.some((a: { kind: string }) => a.kind === "diff")).toBe(true);
    expect(ok(["git", "-C", fixture.scratch[1] ?? "", "rev-parse", "HEAD"]).trim()).toBe(spikeHead);
    expect(kinds()["spike-idea"]).toBe("spike");

    // C9: the owner's clear instruction finishes cache-fix at once: task, gate, Review, reviewer none.
    const words = "finish this as a fix for the cache, no review";
    await turn(words, [{ toolCalls: [{ callId: "finish", name: "corral_owner_action", arguments: { kind: "finish",
      target: "cache-fix", args: ["--title", "Fix the cache"], input: input(4) } }] }, { text: "Finished as T-002; gate pass." }]);
    const finished = result("finish");
    process.stdout.write(`COWORK_LIVE_FINISH ${JSON.stringify(finished).slice(0, 1500)}\n`);
    expect([finished.state, finished.result.outcome]).toEqual(["committed", "executed"]);
    expect(asked).toEqual([]);
    const task = show("T-002");
    expect([task.status, task.origin, task.labels]).toEqual(["review", "assistant", ["cowork"]]);
    expect(task.cowork).toEqual({ session: fixture.session, scratch: "cache-fix", run: "R1", review: "none" });
    const runR1 = task.runs[0];
    expect([runR1.id, runR1.branch, runR1.worktree]).toEqual(["R1", `corral/${fixture.session}/cache-fix`, fixture.scratch[0]]);
    expect(runR1.gate.result).toBe("pass");
    expect(runR1.usage?.input ?? 0).toBeGreaterThan(0);
    expect(task.body).toContain(words);
    expect(kinds()["cache-fix"]).toBe("cowork");
    const finishedEvent = storeEvents().find((e) => e.kind === "cowork_finished" && e.task === "T-002");
    expect([finishedEvent?.actor, finishedEvent?.via]).toEqual(["owner", `assistant:${fixture.session}`]);

    // C9: "approve T-002" lands it through the normal approve path; the session lists the commits.
    await turn("approve T-002", [{ toolCalls: [{ callId: "approve", name: "corral_owner_action", arguments: {
      kind: "approve", target: "T-002", input: input(5) } }] }, { text: "Landed T-002." }]);
    const approved = result("approve");
    process.stdout.write(`COWORK_LIVE_APPROVE ${JSON.stringify(approved).slice(0, 1500)}\n`);
    expect([approved.state, approved.result.outcome]).toEqual(["committed", "executed"]);
    expect(show("T-002").status).toBe("done");
    const head = ok(["git", "-C", app, "rev-parse", "main"]).trim();
    expect(ok(["git", "-C", app, "log", "-1", "--format=%s", "main"]).trim()).toBe("Fix the cache");
    const landed = record().landed as { task: string; run: string; commits: string[] }[];
    expect(landed).toHaveLength(1);
    expect([landed[0]?.task, landed[0]?.run, landed[0]?.commits]).toEqual(["T-002", "R1", [head]]);
    const approvedEvent = storeEvents().find((e) => e.kind === "approved" && e.task === "T-002");
    expect([approvedEvent?.actor, approvedEvent?.via]).toEqual(["owner", `assistant:${fixture.session}`]);
    expect(Object.keys(kinds())).toEqual(["spike-idea"]);

    const evidence = process.env.CORRAL_COWORK_EVIDENCE;
    if (evidence) {
      mkdirSync(evidence, { recursive: true });
      writeFileSync(join(evidence, "cowork-live-summary.json"), JSON.stringify({ pane, workspaceId, assistant: fixture.session,
        pluginInventory: plugins.trim(), provider: "deterministic Codex SSE transport; production host/auth/role",
        c10: { shellFinish: offered, question, promoted, backlog: { status: backlog.status, artifacts: backlog.artifacts } },
        c9: { finish: finished, task: { status: task.status, origin: task.origin, labels: task.labels, cowork: task.cowork,
          run: { id: runR1.id, branch: runR1.branch, gate: runR1.gate.result, usage: runR1.usage } },
          finishedEvent, approve: approved, landed, head, approvedEvent } }, null, 2));
    }
    process.stdout.write(`PASS LIVE COWORK: private pane ${pane}; assistant ${fixture.session}; shell finish refused; unclear ship asked; spike promoted as T-001; cache-fix finished as T-002 (gate pass, review none) and landed at ${head}\n`);
  } finally {
    await host?.close();
    globalThis.fetch = originalFetch;
    for (const key of Object.keys(process.env)) if (!(key in originalEnv)) delete process.env[key];
    Object.assign(process.env, originalEnv);
    if (workspaceId) run(["herdr", "--session", "corral-test", "workspace", "close", workspaceId]);
    assets.cleanup();
    rmSync(workspace, { recursive: true, force: true });
  }
}, 900_000);
