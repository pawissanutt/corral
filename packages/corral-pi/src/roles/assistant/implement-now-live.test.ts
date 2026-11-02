// Implement-now interaction (T-653): the production assistant role, auth and HostSession in a private
// live herdr pane, with the real corral binary as receipt client, shell and owner-action command. Only
// the provider is scripted, and its question and choices are read from the contract the host actually
// sent, so this proves the plumbing and its binding to the loaded contract, not real-model adherence.
import { expect, test } from "bun:test";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { zstdDecompressSync } from "node:zlib";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { EventLog } from "../../bridge/events.ts";
import { parseLaunchMessage } from "../../host/config.ts";
import { loadModules } from "../../host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../../host/restrict.ts";
import { HostSession } from "../../host/session.ts";
import { CorralClient, sha256Hex } from "../../tools/corral.ts";
import { fakeCodexTransport } from "../../../test/fixtures/auth/fake-transport.ts";
import { completionChoices, implementNowQuestion } from "../../../test/support/assistant-workflow.ts";
import { exportAssistantSession } from "./env.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "../../../test/support/fixture-install.ts";
import { corralBinary, ok, privateHerdr, privateRoot, probeBinary, run } from "../../../test/support/workspace.ts";
import { storeEvents as canonicalEvents } from "../../../test/support/canonical-store.ts";

function syntheticAccess(): string {
  const part = (value: object) => Buffer.from(JSON.stringify(value)).toString("base64url");
  return `${part({ alg: "none", typ: "JWT" })}.${part({ exp: Math.floor(Date.now() / 1000) + 3600,
    "https://api.openai.com/auth": { chatgpt_account_id: "synthetic" } })}.synthetic-signature`;
}

/** Every string in a provider request body, joined: the system contract the host sent is among them. */
function sentText(init: RequestInit | undefined): string {
  let bytes = Buffer.from(typeof init?.body === "string" ? init.body : (init?.body as Uint8Array | undefined) ?? []);
  if (new Headers(init?.headers).get("content-encoding") === "zstd") bytes = zstdDecompressSync(bytes);
  const out: string[] = [];
  const walk = (value: unknown): void => {
    if (typeof value === "string") out.push(value);
    else if (Array.isArray(value)) value.forEach(walk);
    else if (value && typeof value === "object") Object.values(value).forEach(walk);
  };
  walk(JSON.parse(bytes.toString("utf8")));
  return out.join("\n");
}

interface Fixture { session: string; bootstrap: string; launch: string; scratch: string[] }
interface StoreEvent { kind: string; task?: string; detail?: Record<string, unknown> }
type Step = { toolCalls: { callId: string; name: string; arguments: Record<string, unknown> }[] } | { text: string };

test("live implement-now: one ambiguous question, scratch work, a candidate-bound report, then one land-now answer", async () => {
  if (!privateHerdr()) {
    process.stdout.write("implement-now live: skipped (requires fresh scripts/test-herdr.sh private server)\n");
    return;
  }
  const originalEnv = { ...process.env };
  const originalFetch = globalThis.fetch;
  const workspace = mkdtempSync(join(privateRoot() ?? "", "assistant-implement-now-"));
  const assets = buildFixtureInstall({ parent: workspace, overlays: false });
  let workspaceId = "";
  let host: HostSession | undefined;
  try {
    cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
    expect(ok(["herdr", "--session", "corral-test", "plugin", "list"]).trim()).toBe("No plugins installed.");
    const corral = corralBinary();
    const app = join(workspace, "app");
    mkdirSync(app);
    for (const args of [["init", "-q", "-b", "main"], ["config", "user.name", "corral test"],
      ["config", "user.email", "corral@test.invalid"]]) ok(["git", "-C", app, ...args]);
    writeFileSync(join(app, "README.md"), "# app\n");
    writeFileSync(join(app, ".gitignore"), ".worktrees/\n");
    ok(["git", "-C", app, "add", "README.md", ".gitignore"]);
    ok(["git", "-C", app, "commit", "-q", "-m", "fixture"]);
    const created = JSON.parse(ok(["herdr", "--session", "corral-test", "workspace", "create", "--cwd", workspace, "--no-focus"]));
    workspaceId = created.result.workspace.workspace_id;
    const pane: string = created.result.root_pane.pane_id;
    const setup = ok([probeBinary(), "--exact", "agent::ops::assistant::cowork_live_tests::cowork_live_setup", "--ignored", "--nocapture"],
      { env: { ...process.env, CORRAL_COWORK_LIVE: JSON.stringify({ root: workspace, assets: assets.dir, pane }) } });
    const line = /COWORK_LIVE (\{.*\})/.exec(setup)?.[1];
    if (!line) throw new Error(`setup produced no result: ${setup.slice(-2000)}`);
    const fixture = JSON.parse(line) as Fixture;
    const bootstrap = JSON.parse(readFileSync(fixture.bootstrap, "utf8"));
    const launch = parseLaunchMessage(readFileSync(fixture.launch, "utf8"));
    const auth = join(workspace, "data/corral/corral-pi/auth");
    mkdirSync(auth, { recursive: true, mode: 0o700 });
    writeFileSync(join(auth, "auth.json"), JSON.stringify({ "openai-codex": { type: "oauth", access: syntheticAccess(),
      refresh: "synthetic-refresh", expires: Date.now() + 3_600_000, accountId: "synthetic" } }), { mode: 0o600 });
    process.env.XDG_DATA_HOME = join(workspace, "data");
    restrictEnvironment(process.env);
    const transport = fakeCodexTransport();
    let sent = "";
    const network = guardedFetch(async (input, init) => { sent = sentText(init); return transport.send(input, init); },
      undefined, "openai-codex", undefined,
      launch.capabilities?.web ? { settings: launch.capabilities.web, send: transport.send } : undefined,
      launch.capabilities?.mcp ? { origins: [], send: transport.send } : undefined);
    globalThis.fetch = network.fetch as typeof globalThis.fetch;
    const identity = { store: bootstrap.store as string, project: "", role: "assistant" as const,
      assistant: fixture.session, generation: bootstrap.generation as number };
    exportAssistantSession(process.env, identity);
    const client = new CorralClient({ corral, root: workspace, identity, hostCap: "e".repeat(64), sessionId: () => host?.sessionId() ?? "" });
    const sdk = await import("@earendil-works/pi-coding-agent");
    host = await HostSession.create({ sdk, identity, cwd: workspace, agentDir: join(workspace, "pi-agent"),
      model: bootstrap.model, modules: await loadModules(assets.dir, "assistant"), client,
      events: new EventLog(identity.generation), network, ui: "plain", todo: launch.todo === true,
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
    const turn = async (text: string, script: Step[]) => {
      transport.script(script as never);
      await live.session.prompt(text, { source: "interactive" });
      await live.drained();
    };
    const toolText = (callId: string): string => {
      const message = live.session.messages.find((m) => m.role === "toolResult" && m.toolCallId.split("|")[0] === callId);
      if (message?.role !== "toolResult") return "";
      return message.content.map((part) => (part.type === "text" ? part.text : "")).join("");
    };
    const said = (): string[] => live.session.messages.flatMap((m) => m.role === "assistant"
      ? [m.content.map((part) => (part.type === "text" ? part.text : "")).join("")] : []);
    const bash = (callId: string, command: string): Step => ({ toolCalls: [{ callId, name: "bash", arguments: { command } }] });
    const show = (id: string) => JSON.parse(ok([corral, "show", id, "--json"], { cwd: workspace }));
    const tasks = () => JSON.parse(ok([corral, "list", "--json"], { cwd: workspace })) as { id: string }[];
    const shown = () => JSON.parse(ok([corral, "assistant", "show", fixture.session, "--json"], { cwd: workspace }));
    const kinds = () => Object.fromEntries((shown().session.scratch as { slug: string; kind: string }[]).map((s) => [s.slug, s.kind]));
    const ownerActions = () => (shown().container.state.receipts as { op: string }[]).filter((r) => r.op === "owner_action");
    const storeEvents = (): StoreEvent[] => canonicalEvents(corral, workspace);
    const slug = "readme-usage";
    const scratch = join(app, ".worktrees", `${fixture.session}-${slug}`);
    const before = kinds();

    // Given a change request that neither says now nor queue, the assistant asks the contract's question once.
    await turn("the README should say how to run the app", [{ get text() { return implementNowQuestion(sent) ?? "NO_QUESTION"; } }]);
    const question = implementNowQuestion(sent);
    expect(question).toBeDefined();
    expect(said().at(-1)).toBe(question as string);
    // Then nothing changed before the owner answered: no task, scratch, owner action or worktree.
    expect(tasks()).toHaveLength(0);
    expect([kinds(), ownerActions().length, existsSync(scratch)]).toEqual([before, 0, false]);

    // When the owner says to implement it now, the work happens in a new scratch and ends in a report
    // built from the real candidate, diff and check, offering the contract's completion choices.
    const report = (): string => {
      const out = toolText("candidate");
      const sha = /\b[0-9a-f]{40}\b/.exec(out)?.[0] ?? "NO_SHA";
      const stat = out.split("\n").find((l) => l.includes("USAGE.md |"))?.trim() ?? "NO_STAT";
      const check = toolText("check").includes("CHECK_OK") ? "passed" : "failed";
      return [`Done in scratch ${slug}; candidate ${sha}.`, `Changed: ${stat}`, `Checks: test -s USAGE.md ${check}.`,
        "Risks: documentation only.", ...completionChoices(sent).map((choice, i) => `${i + 1}. ${choice}`)].join("\n");
    };
    await turn("implement it now", [
      bash("scratch", `${quote(corral)} --root ${quote(workspace)} assistant scratch new -p corral --slug ${slug}`),
      { toolCalls: [{ callId: "edit", name: "write", arguments: { path: join(scratch, "USAGE.md"), content: "# Usage\n\nRun `app --help`.\n" } }] },
      bash("candidate", `git -C ${quote(scratch)} add USAGE.md && git -C ${quote(scratch)} commit -q -m 'Add a usage section' `
        + `&& git -C ${quote(scratch)} rev-parse HEAD && git -C ${quote(scratch)} diff --stat main`),
      bash("check", `test -s ${quote(join(scratch, "USAGE.md"))} && echo CHECK_OK`),
      { get text() { return report(); } },
    ]);
    const candidate = ok(["git", "-C", scratch, "rev-parse", "HEAD"]).trim();
    const reported = said().at(-1) ?? "";
    const choices = completionChoices(sent);
    expect(choices).toHaveLength(4);
    for (const part of [candidate, "USAGE.md |", "passed", "Risks:", ...choices]) expect(reported).toContain(part);
    // Then still nothing is finished, and the question was not asked again.
    expect([tasks().length, kinds()[slug], ownerActions().length]).toEqual([0, "spike", 0]);
    expect(said().filter((text) => text.includes(question as string))).toHaveLength(1);

    // When the owner answers "land it now", one answer finishes exactly the reported candidate and approves it.
    await turn("land it now", [
      { toolCalls: [{ callId: "finish", name: "corral_owner_action", arguments: { kind: "finish", target: slug,
        args: ["--title", "Add a usage section", "--expected-head", candidate.slice(0, 12)], input: input(3) } }] },
      { toolCalls: [{ callId: "approve", name: "corral_owner_action", arguments: { kind: "approve", target: "T-001", input: input(3) } }] },
      { text: "Landed T-001." }]);
    const finished = JSON.parse(toolText("finish"));
    const approved = JSON.parse(toolText("approve"));
    expect([finished.result.outcome, approved.result.outcome, asked]).toEqual(["executed", "executed", []]);
    const task = show("T-001");
    expect([task.status, task.runs.length, task.runs[0].branch]).toEqual(["done", 1, `corral/${fixture.session}/${slug}`]);
    expect(ok(["git", "-C", app, "rev-parse", "main"]).trim()).toBe(candidate);
    for (const kind of ["cowork_finished", "approved"]) {
      const event = storeEvents().find((e) => e.kind === kind && e.task === "T-001");
      const authority = event?.detail?.owner_authority as { input?: string; owner_message?: string } | undefined;
      expect([authority?.input, authority?.owner_message]).toEqual([input(3), "land it now"]);
    }
    expect(storeEvents().find((e) => e.kind === "cowork_finished")?.detail?.candidate).toBe(candidate);
    expect(said().filter((text) => text.includes(question as string))).toHaveLength(1);

    const evidence = process.env.CORRAL_IMPLEMENT_NOW_EVIDENCE;
    if (evidence) {
      mkdirSync(evidence, { recursive: true });
      writeFileSync(join(evidence, "implement-now-live-summary.json"), JSON.stringify({ pane, assistant: fixture.session,
        provider: "deterministic Codex SSE transport; production host, auth, role and loaded contract",
        question, report: reported, choices, candidate, finish: finished, approve: approved,
        task: { status: task.status, runs: task.runs.length, branch: task.runs[0].branch } }, null, 2));
    }
    process.stdout.write(`PASS LIVE IMPLEMENT-NOW: pane ${pane}; asked once; report bound to ${candidate.slice(0, 12)}; T-001 landed on one answer\n`);
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
