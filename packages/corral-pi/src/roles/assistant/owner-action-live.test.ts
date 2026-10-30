// Integrated owner authority (T-544 C3 to C6): the production assistant role, auth and
// HostSession in a private live herdr pane, with the real corral binary as receipt client and as
// every owner action's command. Only the provider network boundary is deterministic and offline.
import { expect, test } from "bun:test";
import { spawn } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { createInterface } from "node:readline";
import { zstdDecompressSync } from "node:zlib";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { EventLog } from "../../bridge/events.ts";
import { parseLaunchMessage } from "../../host/config.ts";
import { loadModules } from "../../host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../../host/restrict.ts";
import { HostSession } from "../../host/session.ts";
import { CorralClient, sha256Hex, toolRequestId } from "../../tools/corral.ts";
import { fakeCodexTransport } from "../../../test/fixtures/auth/fake-transport.ts";
import { exportAssistantSession } from "./env.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "../../../test/support/fixture-install.ts";
import { nextEvent, within } from "../../../test/support/wait.ts";
import { corralBinary, ok, privateHerdr, privateRoot, probeBinary, run } from "../../../test/support/workspace.ts";

function syntheticAccess(): string {
  const part = (value: object) => Buffer.from(JSON.stringify(value)).toString("base64url");
  return `${part({ alg: "none", typ: "JWT" })}.${part({ exp: Math.floor(Date.now() / 1000) + 3600,
    "https://api.openai.com/auth": { chatgpt_account_id: "synthetic" } })}.synthetic-signature`;
}

interface Fixture { land: string; held: string; dispatch: string; drop: string; session: string; bootstrap: string; launch: string }
interface StoreEvent { kind: string; task?: string; actor?: string; via?: string; detail?: Record<string, unknown> }
type Step = { toolCalls: { callId: string; name: string; arguments: Record<string, unknown> }[] } | { text: string };

test("live private-pane owner authority: landing, synthetic inputs, do it, confirm y/N, routing and a held lock", async () => {
  if (!privateHerdr()) {
    process.stdout.write("owner authority live: skipped (requires fresh scripts/test-herdr.sh private server)\n");
    return;
  }
  const originalEnv = { ...process.env };
  const originalFetch = globalThis.fetch;
  const workspace = mkdtempSync(join(privateRoot() ?? "", "assistant-owner-"));
  const assets = buildFixtureInstall({ parent: workspace, overlays: false });
  let workspaceId = "";
  let host: HostSession | undefined;
  let holder: ReturnType<typeof spawn> | undefined;
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
    writeFileSync(join(app, "README.md"), "# owner authority\n");
    ok(["git", "-C", app, "add", "README.md"]);
    ok(["git", "-C", app, "commit", "-q", "-m", "fixture"]);
    const created = JSON.parse(ok(["herdr", "--session", "corral-test", "workspace", "create", "--cwd", workspace, "--no-focus"]));
    workspaceId = created.result.workspace.workspace_id;
    const pane: string = created.result.root_pane.pane_id;
    const setup = ok([probe, "--exact", "agent::ops::assistant::owner_live_tests::owner_authority_live_setup", "--ignored", "--nocapture"],
      { env: { ...process.env, CORRAL_OWNER_LIVE: JSON.stringify({ root: workspace, assets: assets.dir, pane }) } });
    const line = /OWNER_LIVE (\{.*\})/.exec(setup)?.[1];
    if (!line) throw new Error(`owner setup produced no result: ${setup.slice(-2000)}`);
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
    const requests: string[] = [];
    const network = guardedFetch(async (input, init) => {
      const body = init?.body;
      let bytes: Buffer;
      if (typeof body === "string") bytes = Buffer.from(body);
      else if (body instanceof Uint8Array) bytes = Buffer.from(body);
      else throw new Error("unexpected provider request body");
      if (new Headers(init?.headers).get("content-encoding") === "zstd") bytes = zstdDecompressSync(bytes, { maxOutputLength: 4 * 1024 * 1024 });
      requests.push(bytes.toString("utf8"));
      return transport.send(input, init);
    }, undefined, "openai-codex", undefined,
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
    const answers: boolean[] = [];
    const asked: string[] = [];
    const ui = { confirm: async (_title: string, message: string) => { asked.push(message); return answers.shift() ?? false; } };
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
    const act = (callId: string, args: Record<string, unknown>): Step =>
      ({ toolCalls: [{ callId, name: "corral_owner_action", arguments: args }] });
    const result = (callId: string): { state: string; result: Record<string, unknown> } => {
      const message = live.session.messages.find((m) => m.role === "toolResult" && m.toolCallId.split("|")[0] === callId);
      if (message?.role !== "toolResult") throw new Error(`no result for ${callId}`);
      return JSON.parse(message.content.map((part) => (part.type === "text" ? part.text : "")).join(""));
    };
    const toolCallId = (callId: string): string => {
      const message = live.session.messages.find((m) => m.role === "toolResult" && m.toolCallId.split("|")[0] === callId);
      if (message?.role !== "toolResult") throw new Error(`no result for ${callId}`);
      return message.toolCallId;
    };
    const status = (id: string) => JSON.parse(ok([corral, "show", id, "--json"], { cwd: workspace })).status as string;
    const storeEvents = (): StoreEvent[] => readFileSync(join(workspace, ".corral/events.jsonl"), "utf8").trim().split("\n")
      .map((l) => JSON.parse(l) as StoreEvent);
    const journal = () => (JSON.parse(readFileSync(join(workspace, `.corral/state/agent-assistant/${fixture.session}.json`), "utf8"))
      .owner_inputs ?? []) as { id: string; source: string; text: string; answers?: string }[];
    const toml = () => readFileSync(join(workspace, ".corral/corral.toml"), "utf8");

    // C3: the owner's own words land T-001 through the normal approve path, with no question.
    const landWords = `${fixture.land} looks good, please land it`;
    await turn(landWords, [act("land", { kind: "approve", target: fixture.land, input: input(1) }), { text: "Landed." }]);
    const landed = result("land");
    process.stdout.write(`OWNER_LIVE_LAND ${JSON.stringify(landed).slice(0, 1500)}\n`);
    expect([landed.state, landed.result.outcome]).toEqual(["committed", "executed"]);
    expect(status(fixture.land)).toBe("done");
    const approved = storeEvents().filter((e) => e.kind === "approved" && e.task === fixture.land);
    expect(approved).toHaveLength(1);
    const authority = approved[0]?.detail?.owner_authority as Record<string, unknown>;
    expect([approved[0]?.actor, approved[0]?.via]).toEqual(["owner", `assistant:${fixture.session}`]);
    expect([authority.session, authority.generation, authority.input, authority.owner_message])
      .toEqual([fixture.session, identity.generation, input(1), landWords]);
    expect(ok(["git", "-C", app, "log", "--format=%s", "-1"]).trim()).toBe("Land the owner's change");
    const landRequest = toolRequestId(live.sessionId(), toolCallId("land"));
    const replay = await client.call("owner_action", { requestId: landRequest,
      payload: { kind: "approve", target: fixture.land, input: input(1) } });
    expect([replay.state, replay.replayed]).toEqual(["committed", true]);
    expect(storeEvents().filter((e) => e.kind === "approved" && e.task === fixture.land)).toHaveLength(1);

    // C4: a corral wake is not journaled, and wake, PM outcome and recovery IDs never authorize.
    const journaled = journal().length;
    transport.script([
      act("wake", { kind: "approve", target: fixture.held, input: `qwake:${fixture.held}:Q1:1` }),
      act("pmi", { kind: "approve", target: fixture.held, input: "pmi:PMI-9:2" }),
      act("recovery", { kind: "approve", target: fixture.held, input: `recovery_notice:g${identity.generation}` }),
      { text: "None of those were the owner." },
    ] as never);
    const wakeEnded = nextEvent(events, (event) => event.kind === "turn_ended");
    expect((await live.submit({ text: `[corral] ${fixture.land} landed`, busyPolicy: "prompt", idempotencyKey: "wake-1",
      origin: "corral-inbox" }, "rq-wake-1")).disposition).toBe("started");
    await wakeEnded;
    await live.drained();
    for (const call of ["wake", "pmi", "recovery"]) {
      expect([result(call).state, result(call).result.code]).toEqual(["rejected", "not_owner_input"]);
    }
    expect(journal()).toHaveLength(journaled);
    expect(status(fixture.held)).toBe("review");

    // C5: "do it" right after the assistant proposed the exact command dispatches, unconfirmed.
    await turn(`What should happen to ${fixture.dispatch}?`, [{ text: `I propose: corral dispatch ${fixture.dispatch} --profile owner-fake` }]);
    await turn("do it", [act("dispatch", { kind: "dispatch", target: fixture.dispatch, args: ["--profile", "owner-fake"],
      input: input(3) }), { text: "Dispatched." }]);
    expect([result("dispatch").state, result("dispatch").result.outcome]).toEqual(["committed", "executed"]);
    expect(status(fixture.dispatch)).toBe("running");
    expect(asked).toEqual([]);

    // C5: dropping T-003 and the config change wait for the owner: N changes nothing, y once.
    answers.push(false);
    await turn(`drop task ${fixture.drop}`, [act("drop-no", { kind: "discard", target: fixture.drop, input: input(4) }), { text: "Kept." }]);
    expect(result("drop-no").result.outcome).toBe("declined");
    expect(status(fixture.drop)).toBe("backlog");
    answers.push(true);
    await turn(`yes, drop ${fixture.drop}`, [act("drop-yes", { kind: "discard", target: fixture.drop, input: input(6) }), { text: "Dropped." }]);
    expect(result("drop-yes").result.outcome).toBe("executed");
    expect(status(fixture.drop)).toBe("dropped");
    const tomlBefore = toml();
    answers.push(false);
    await turn("set limits.running to 3", [act("config-no", { kind: "config_set", target: "limits.running", args: ["3"],
      input: input(8) }), { text: "Unchanged." }]);
    expect(result("config-no").result.outcome).toBe("declined");
    expect(toml()).toBe(tomlBefore);
    answers.push(true);
    await turn("please do set limits.running to 3", [act("config-yes", { kind: "config_set", target: "limits.running", args: ["3"],
      input: input(10) }), { text: "Set." }]);
    expect(result("config-yes").result.outcome).toBe("executed");
    expect(ok([corral, "config", "get", "limits.running"], { cwd: workspace }).trim()).toBe("3");
    expect(asked).toEqual([`corral discard ${fixture.drop}`, `corral discard ${fixture.drop}`,
      "corral config set limits.running 3", "corral config set limits.running 3"]);
    const answered = journal().filter((e) => e.source === "confirmation");
    expect(answered.map((e) => e.text)).toEqual(["n", "y", "n", "y"]);
    const dropRequest = toolRequestId(live.sessionId(), toolCallId("drop-yes"));
    expect(answered[1]?.answers).toBe(dropRequest);
    const dropReplay = await client.call("owner_action", { requestId: dropRequest,
      payload: { kind: "discard", target: fixture.drop, input: input(6) } });
    expect([dropReplay.state, dropReplay.replayed]).toEqual(["committed", true]);
    expect(asked).toHaveLength(4);

    // C6: an unclear request becomes a quoted PM inbox item, and the owner is told how to follow it.
    const unclear = "land whatever is ready";
    const post = `${cli} pm inbox post --kind routing --subject ${quote("Land whatever is ready")} --quote ${quote(unclear)} --input ${input(12)}`;
    await turn(unclear, [{ toolCalls: [{ callId: "route", name: "bash", arguments: { command: post } }] },
      { text: "routed to the PM as PMI-1; follow with corral pm inbox show PMI-1" }]);
    const item = JSON.parse(ok([corral, "pm", "inbox", "show", "PMI-1", "--json"], { cwd: workspace }));
    expect(JSON.stringify(item)).toContain(unclear);
    expect(JSON.stringify(item)).toContain(`assistant:${fixture.session}`);
    expect(JSON.stringify(item)).toContain(input(12));
    const reply = live.session.messages.filter((m) => m.role === "assistant").at(-1);
    expect(JSON.stringify(reply)).toContain("corral pm inbox show PMI-1");

    // C6: approving T-004 while autopilot holds its approval lock refuses, named, once.
    const lock = join(workspace, ".corral/lock", `autopilot-${fixture.held}.lock`);
    mkdirSync(dirname(lock), { recursive: true });
    holder = spawn("flock", ["-x", lock, "-c", "echo HELD; read _"], { stdio: ["pipe", "pipe", "ignore"] });
    const held = holder;
    await within(new Promise<void>((resolve) => {
      createInterface({ input: held.stdout! }).on("line", (l) => { if (l === "HELD") resolve(); });
    }), 30_000, "the autopilot lock to be held");
    await turn(`approve ${fixture.held}`, [act("held", { kind: "approve", target: fixture.held, input: input(13) }),
      { text: `autopilot holds autopilot-${fixture.held}; reported, not retried.` }]);
    expect([result("held").state, result("held").result.code, result("held").result.holder])
      .toEqual(["rejected", "conflict_in_flight", `autopilot-${fixture.held}`]);
    expect(live.session.messages.filter((m) => m.role === "toolResult" && m.toolCallId.startsWith("held"))).toHaveLength(1);
    expect(status(fixture.held)).toBe("review");
    held.stdin?.end();

    const evidence = process.env.CORRAL_OWNER_EVIDENCE;
    if (evidence) {
      mkdirSync(evidence, { recursive: true });
      writeFileSync(join(evidence, "owner-live-summary.json"), JSON.stringify({ pane, workspaceId, assistant: fixture.session,
        pluginInventory: plugins.trim(), provider: "deterministic Codex SSE transport; production host/auth/role", requests: requests.length,
        c3: { task: fixture.land, approved: approved[0], replay: { state: replay.state, replayed: replay.replayed } },
        c4: ["wake", "pmi", "recovery"].map((call) => result(call)), journal: journal(),
        c5: { dispatch: result("dispatch"), dropNo: result("drop-no"), dropYes: result("drop-yes"), configNo: result("config-no"),
          configYes: result("config-yes"), asked, dropReplay: { state: dropReplay.state, replayed: dropReplay.replayed } },
        c6: { item, reply: JSON.stringify(reply).slice(0, 400), held: result("held") } }, null, 2));
    }
    process.stdout.write(`PASS LIVE OWNER AUTHORITY: private pane ${pane}; assistant ${fixture.session}; ${fixture.land} landed and replayed once; synthetic inputs refused; do-it dispatch; confirm N/y for discard and config; PMI-1 routed; autopilot lock reported once\n`);
  } finally {
    holder?.kill();
    await host?.close();
    globalThis.fetch = originalFetch;
    for (const key of Object.keys(process.env)) if (!(key in originalEnv)) delete process.env[key];
    Object.assign(process.env, originalEnv);
    if (workspaceId) run(["herdr", "--session", "corral-test", "workspace", "close", workspaceId]);
    assets.cleanup();
    rmSync(workspace, { recursive: true, force: true });
  }
}, 900_000);
