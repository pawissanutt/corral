// Integrated C11: production assistant role/auth/HostSession, private live herdr pane,
// actual corral CLI readers. Only the provider network boundary is deterministic/offline.
import { expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { zstdDecompressSync } from "node:zlib";
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

test("live private-pane C11: focused production assistant consumes brief and pages every harness without authority", async () => {
  if (!privateHerdr()) {
    process.stdout.write("C11 live: skipped (requires fresh scripts/test-herdr.sh private server)\n");
    return;
  }
  const originalEnv = { ...process.env };
  const originalFetch = globalThis.fetch;
  const workspace = mkdtempSync(join(privateRoot() ?? "", "assistant-c11-"));
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
    writeFileSync(join(app, "README.md"), "# C11\n");
    ok(["git", "-C", app, "add", "README.md"]);
    ok(["git", "-C", app, "commit", "-q", "-m", "fixture"]);
    const created = JSON.parse(ok(["herdr", "--session", "corral-test", "workspace", "create", "--cwd", workspace, "--no-focus"]));
    workspaceId = created.result.workspace.workspace_id;
    const pane: string = created.result.root_pane.pane_id;
    const setup = ok([probe, "--exact", "agent::ops::assistant::c11_tests::focused_assistant_c11_setup", "--ignored", "--nocapture"],
      { env: { ...process.env, CORRAL_ASSISTANT_C11: JSON.stringify({ root: workspace, assets: assets.dir, pane }) } });
    const line = /ASSISTANT_C11 (\{.*\})/.exec(setup)?.[1];
    if (!line) throw new Error(`C11 setup produced no result: ${setup.slice(-2000)}`);
    const fixture = JSON.parse(line) as { task: string; session: string; bootstrap: string; launch: string; task_path: string; missing: string };
    const bootstrap = JSON.parse(readFileSync(fixture.bootstrap, "utf8"));
    const launch = parseLaunchMessage(readFileSync(fixture.launch, "utf8"));
    expect([bootstrap.role, bootstrap.assistant, bootstrap.task]).toEqual(["assistant", fixture.session, undefined]);
    const brief = readFileSync(bootstrap.prompt_file, "utf8");
    expect(Buffer.byteLength(brief)).toBeLessThanOrEqual(24 * 1024);
    expect(brief).toContain("C11_BRIEF_SENTINEL");
    expect(brief).toContain("[truncated");
    const before = readFileSync(fixture.task_path, "utf8");
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
      else throw new Error("C11 unexpected provider request body");
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
      model: bootstrap.model, modules: await loadModules(assets.dir, "assistant"), client, events, network,
      promptFile: bootstrap.prompt_file, todo: launch.todo === true,
      ...(launch.capabilities ? { capabilities: launch.capabilities } : {}),
      ...(launch.guidance ? { guidance: launch.guidance } : {}),
      root: workspace, runtimeDir: dirname(fixture.bootstrap), exportDir: join(bootstrap.session_dir, "exports"),
      sessionManager: sdk.SessionManager.create(workspace, bootstrap.session_dir) });
    await host.hello(bootstrap.bind_nonce, pane, sha256Hex("d".repeat(64)));
    await host.session.bindExtensions({});
    const quote = (s: string) => `'${s.replaceAll("'", "'\\''")}'`;
    const cli = `${quote(corral)} --root ${quote(workspace)}`;
    const commands = [
      `${cli} run transcript ${fixture.task} R1 --lines 1-200`,
      `${cli} run transcript ${fixture.task} R2 --lines 201-250`,
      `${cli} run transcript ${fixture.task} R3 --lines 1-200`,
      `${cli} run transcript ${fixture.task} R3 --review 1 --lines 1-200`,
      `${cli} run transcript ${fixture.task} R4 --lines 1-200`,
      `${cli} approve ${fixture.task}`,
    ];
    transport.script([
      { toolCalls: [{ callId: "read-brief", name: "read", arguments: { path: bootstrap.prompt_file } }] },
      ...commands.map((command, n) => ({ toolCalls: [{ callId: `c11-${n}`, name: "bash", arguments: { command } }] })),
      { text: "C11_COMPLETE" },
    ]);
    // Interactive input enters the real production session. Tool calls execute through its
    // loaded assistant extensions, not a stand-alone invocation of the CLI helpers.
    await host.session.prompt("Read the focused brief and the worker/review transcripts; report the missing transcript diagnostic and whether task authority changed.", { source: "interactive" });
    await host.drained();
    expect(requests.length).toBeGreaterThanOrEqual(8);
    expect(requests[0]).toContain("C11_BRIEF_SENTINEL");
    expect(requests[0]).toContain("not authority over this task");
    const last = JSON.parse(requests[requests.length - 1] ?? "null");
    const toolOutputs = (last.input as { type?: string; output?: string }[]).filter(item => item.type === "function_call_output").map(item => item.output ?? "");
    expect(toolOutputs).toHaveLength(7);
    for (const page of toolOutputs.slice(1, 5)) expect(Buffer.byteLength(page)).toBeLessThanOrEqual(24 * 1024);
    expect((toolOutputs[2] ?? "").trim().split("\n")).toHaveLength(50);
    expect(toolOutputs[1]).toContain("resume with --lines");
    const output = toolOutputs.join("\n");
    for (const marker of ["OMO_C11", "CLAUDE_C11", "PI_C11", "REVIEW_C11", "transcript_unavailable", fixture.missing, "assistant_refused"]) {
      expect(output).toContain(marker);
    }
    expect(readFileSync(fixture.task_path, "utf8")).toBe(before);
    const record = JSON.parse(ok([corral, "assistant", "show", fixture.session, "--json"], { cwd: workspace }));
    expect(JSON.stringify(record)).toContain(fixture.task);
    const evidence = process.env.CORRAL_C11_EVIDENCE;
    if (evidence) {
      mkdirSync(evidence, { recursive: true });
      writeFileSync(join(evidence, "c11-provider-input.json"), JSON.stringify(JSON.parse(requests[0] ?? "null"), null, 2));
      writeFileSync(join(evidence, "c11-tool-results.json"), JSON.stringify(toolOutputs, null, 2));
      writeFileSync(join(evidence, "c11-summary.json"), JSON.stringify({ pane, workspaceId, task: fixture.task, assistant: fixture.session,
        pluginInventory: plugins.trim(), briefBytes: Buffer.byteLength(brief), pageBytes: toolOutputs.slice(1, 5).map(page => Buffer.byteLength(page)), provider: "deterministic Codex SSE transport; production host/auth/role", requests: requests.length,
        taskUnchanged: true, taskBoundToHost: false, commands, sourceRecords: 250, markers: ["OMO_C11", "CLAUDE_C11", "PI_C11", "REVIEW_C11"],
        missing: fixture.missing }, null, 2));
    }
    process.stdout.write(`PASS LIVE C11: private pane ${pane}; production focused assistant ${fixture.session}; brief ${Buffer.byteLength(brief)} bytes consumed by provider; OmO/Claude/Pi/reviewer CLI tools; missing searched path; approve refused; task unchanged\n`);
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
