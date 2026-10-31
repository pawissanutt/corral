import { expect, test } from "bun:test";
import { cpSync, existsSync, readFileSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, unlinkSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { homedir, tmpdir } from "node:os";
import type { ExtensionUIContext, ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { RoleContext } from "../types.ts";
import { CorralClient, sha256Hex, CorralCallError, toolRequestId } from "../../tools/corral.ts";
import { PermissionBroker } from "./permissions.ts";
import { EventLog } from "../../bridge/events.ts";
import { parseLaunchMessage } from "../../host/config.ts";
import { HostSession } from "../../host/session.ts";
import { loadModules } from "../../host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../../host/restrict.ts";
import { exportAssistantSession } from "./env.ts";
import { fakeCodexTransport } from "../../../test/fixtures/auth/fake-transport.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "../../../test/support/fixture-install.ts";
import { corralBinary, ok, privateHerdr, privateRoot, probeBinary, run } from "../../../test/support/workspace.ts";
import { classifyWrite, scopeCandidates, scopeLimit, writeSettings, type PathContext } from "./paths.ts";

function fixture() {
  const base = mkdtempSync(join(tmpdir(), "write-permissions-"));
  const home = join(base, "home/u"), workspace = join(base, "ws");
  for (const dir of [home, workspace]) mkdirSync(dir, { recursive: true });
  const paths: PathContext = { workspace, home, session: "A-1", temp: base, protectedDirs: [join(base, "config")], enabled: true };
  const calls: { id: string; payload: Record<string, unknown> }[] = [];
  const abort = new AbortController();
  let turn = 1, granted = false, fail = false;
  let afterDecide = () => {};
  const context: RoleContext = { cwd: workspace, model: "test", identity: { store: "s", project: "", role: "assistant", assistant: "A-1", generation: 1 },
    capabilities: { catalog: 9, digest: "d".repeat(64), preset: "standard", enabled: ["write_permissions"] },
    writePermission: { async onBound() {}, settings: { allow: [], deny: [] }, sessionId: () => "pi-session", turn: () => ({ id: turn, signal: abort.signal }), emit() {},
      async call(id, payload) {
        calls.push({ id, payload });
        if (payload.action === "decide") {
          afterDecide();
          if (fail) { fail = false; throw new CorralCallError("call_failed", "lost response"); }
          if (payload.choice === "hour" || payload.choice === "session") granted = true;
          return { version: 1, request_id: id, op: "write_permission", state: "committed", result: { authorized: payload.choice !== "deny", input: "A-1/g1/i1" } };
        }
        return { version: 1, request_id: id, op: "write_permission", state: "committed", result: payload.action === "list"
          ? { grants: granted ? [{ id: "G-1", scope: { shown: home }, lifetime: "session" }] : [] } : granted ? { grant: "G-1" } : {} };
      } },
  };
  const dialogs: { title: string; choices: string[] }[] = [];
  const answers: (string | undefined)[] = [];
  const notices: string[] = [];
  let onSelect = () => {};
  const ui = { hasUI: true, ui: { notify: (text: string) => notices.push(text), async select(title: string, choices: string[], options: { timeout: number; signal: AbortSignal }) {
    expect(options.timeout).toBe(300_000); expect(options.signal).toBe(abort.signal);
    dialogs.push({ title, choices }); onSelect(); return answers.shift();
  } } } as unknown as Pick<ExtensionContext, "hasUI" | "ui">;
  const broker = new PermissionBroker(context, paths);
  return { base, home, workspace, paths, calls, answers, dialogs, ui, broker, context, abort, notices,
    setTurn: () => { turn++; }, grant: (value: boolean) => { granted = value; }, lose: () => { fail = true; },
    select: (fn: () => void) => { onSelect = fn; }, decide: (fn: () => void) => { afterDecide = fn; },
    write: (path: string, toolCall = "same") => broker.authorize("write", { path, content: "hello\n" }, toolCall, ui),
    close: () => rmSync(base, { force: true, recursive: true }) };
}

test("private production host: global skill, external edit, audit and second-shell revoke actually prompt", async () => {
  if (!privateHerdr()) { console.log("write permissions live: skipped (requires scripts/test-herdr.sh private server)"); return; }
  const env = { ...process.env }, fetch = globalThis.fetch;
  const workspace = mkdtempSync(join(privateRoot()!, "write-permissions-live-"));
  const assets = buildFixtureInstall({ parent: workspace, overlays: false });
  const skill = join(homedir(), ".agents/skills", `permission-${Date.now()}`);
  let host: HostSession | undefined, workspaceId = "";
  try {
    expect(ok(["herdr", "--session", "corral-test", "plugin", "list"]).trim()).toBe("No plugins installed.");
    cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
    const app = join(workspace, "app"); mkdirSync(app);
    for (const args of [["init", "-q", "-b", "main"], ["config", "user.name", "test"], ["config", "user.email", "test@example.invalid"]]) ok(["git", "-C", app, ...args]);
    writeFileSync(join(app, "README.md"), "before\n");
    ok(["git", "-C", app, "add", "README.md"]); ok(["git", "-C", app, "commit", "-qm", "fixture"]);
    const created = JSON.parse(ok(["herdr", "--session", "corral-test", "workspace", "create", "--cwd", workspace, "--no-focus"]));
    workspaceId = created.result.workspace.workspace_id;
    const pane = created.result.root_pane.pane_id;
    const setup = ok([probeBinary(), "--exact", "agent::ops::assistant::cowork_live_tests::cowork_live_setup", "--ignored", "--nocapture"],
      { env: { ...process.env, CORRAL_COWORK_LIVE: JSON.stringify({ root: workspace, assets: assets.dir, pane }) } });
    const fixture = JSON.parse(/COWORK_LIVE (\{.*\})/.exec(setup)![1]!);
    const bootstrap = JSON.parse(readFileSync(fixture.bootstrap, "utf8"));
    const launch = parseLaunchMessage(readFileSync(fixture.launch, "utf8"));
    expect(launch.capabilities?.enabled).toContain("write_permissions");
    const corral = corralBinary();
    const auth = join(workspace, "data/corral/pi-native/auth"); mkdirSync(auth, { recursive: true, mode: 0o700 });
    const part = (value: object) => Buffer.from(JSON.stringify(value)).toString("base64url");
    const access = `${part({ alg: "none", typ: "JWT" })}.${part({ exp: Math.floor(Date.now() / 1000) + 3600, "https://api.openai.com/auth": { chatgpt_account_id: "synthetic" } })}.synthetic-signature`;
    writeFileSync(join(auth, "auth.json"), JSON.stringify({ "openai-codex": { type: "oauth", access, refresh: "synthetic", expires: Date.now() + 3600000, accountId: "synthetic" } }), { mode: 0o600 });
    process.env.XDG_DATA_HOME = join(workspace, "data"); restrictEnvironment(process.env);
    const transport = fakeCodexTransport();
    const network = guardedFetch(transport.send, undefined, "openai-codex", undefined,
      launch.capabilities?.web ? { settings: launch.capabilities.web, send: transport.send } : undefined,
      launch.capabilities?.mcp ? { origins: [], send: transport.send } : undefined);
    globalThis.fetch = network.fetch as typeof globalThis.fetch;
    const identity = { store: bootstrap.store as string, project: "", role: "assistant" as const, assistant: fixture.session as string, generation: bootstrap.generation as number };
    exportAssistantSession(process.env, identity);
    const client = new CorralClient({ corral, root: workspace, identity, hostCap: "e".repeat(64), sessionId: () => host?.sessionId() ?? "" });
    const actions: string[] = [], originalCall = client.call.bind(client);
    client.call = (op, call) => { if (op === "write_permission") actions.push(String(call.payload.action)); return originalCall(op, call); };
    const sdk = await import("@earendil-works/pi-coding-agent");
    host = await HostSession.create({ sdk, identity, cwd: workspace, agentDir: join(workspace, "pi-agent"), model: bootstrap.model,
      modules: await loadModules(assets.dir, "assistant"), client, events: new EventLog(identity.generation), network, ui: "plain",
      capabilities: launch.capabilities, guidance: launch.guidance, root: workspace, runtimeDir: dirname(fixture.bootstrap),
      exportDir: join(bootstrap.session_dir, "exports"), sessionManager: sdk.SessionManager.create(workspace, bootstrap.session_dir) });
    await host.hello(bootstrap.bind_nonce, pane, sha256Hex("d".repeat(64)));
    const answers: string[] = [], dialogs: string[] = [];
    await host.session.bindExtensions({ uiContext: { notify() {}, select: async (title: string, choices: string[]) => {
      if (title.includes(join(skill, "SKILL.md"))) expect(existsSync(skill)).toBeFalse();
      dialogs.push(title); const answer = answers.shift(); expect(choices).toContain(answer!); return answer;
    } } as unknown as ExtensionUIContext });
    expect(actions).toContain("list");
    const turn = async (name: string, args: Record<string, unknown>) => {
      transport.script([{ toolCalls: [{ callId: `call-${dialogs.length}-${actions.length}`, name, arguments: args }] }, { text: "done" }] as never);
      await host!.session.prompt("perform the requested fixture write", { source: "interactive" }); await host!.drained();
    };
    answers.push("Allow for this session", app);
    await turn("edit", { path: join(app, "README.md"), edits: [{ oldText: "before", newText: "after" }] });
    expect(dialogs).toHaveLength(2); expect(readFileSync(join(app, "README.md"), "utf8")).toBe("after\n");
    const count = actions.length;
    await turn("write", { path: join(app, ".git/config"), content: "never" });
    await turn("write", { path: join(process.env.XDG_CONFIG_HOME!, "corral/config.toml"), content: "never" });
    expect(actions).toHaveLength(count); expect(dialogs).toHaveLength(2);
    console.log("WRITE_PERMISSIONS_EXTERNAL_OK: in-place external edit and fixed refusals through real host/op");
    const before = actions.length;
    expect(existsSync(skill)).toBeFalse();
    answers.push("Allow for 1 hour", skill);
    await turn("write", { path: join(skill, "SKILL.md"), content: "# Example\n" });
    expect(dialogs).toHaveLength(4); expect(dialogs[2]).toContain("new file");
    expect(actions.slice(before)).toEqual(["check", "decide"]);
    expect(readFileSync(join(skill, "SKILL.md"), "utf8")).toBe("# Example\n");
    await turn("write", { path: join(skill, "scripts/run.sh"), content: "echo hi\n" });
    expect(dialogs).toHaveLength(4);
    const otherSkill = `${skill}-other/SKILL.md`;
    answers.push("Deny");
    await turn("write", { path: otherSkill, content: "never" });
    expect(dialogs).toHaveLength(5); expect(existsSync(otherSkill)).toBeFalse();
    const shown = JSON.parse(ok([corral, "assistant", "show", fixture.session, "--json"], { cwd: workspace }));
    const inputs = shown.container.owner_inputs ?? shown.container.state.owner_inputs;
    const record = inputs.find((i: { source: string; permission?: { choice: string } }) => i.source === "permission" && i.permission?.choice === "hour");
    expect(record.permission.tool).toBe("write"); expect(record.permission.choice).toBe("hour");
    expect(record.permission.scope.shown).toBe(skill); expect(record.permission.expires_at).toBeDefined();
    const denial = inputs.find((i: { permission?: { choice: string } }) => i.permission?.choice === "deny");
    expect(denial.permission.scope).toBeUndefined(); expect(denial.permission.grant).toBeUndefined();
    const event = readFileSync(join(workspace, ".corral/events.jsonl"), "utf8").trim().split("\n").map((line) => JSON.parse(line)).find((e) => e.kind === "assistant_write_permission" && e.detail.decision.choice === "hour");
    expect(event.detail.decision).toEqual(record.permission);
    // A separate owner's shell revokes the grant; no fabricated host-memory revocation.
    const ownerEnv = { ...process.env }; delete ownerEnv.CORRAL_ASSISTANT; delete ownerEnv.CORRAL_ASSISTANT_GENERATION;
    ok([corral, "assistant", "permissions", "revoke", fixture.session, record.permission.grant], { cwd: workspace, env: ownerEnv });
    answers.push("Deny"); await turn("write", { path: join(skill, "after-revoke"), content: "never" });
    expect(dialogs).toHaveLength(6); expect(existsSync(join(skill, "after-revoke"))).toBeFalse();
    console.log("WRITE_PERMISSIONS_LIVE_OK: real production tool guard/dialog callbacks, check/decide, skill and external edit, structured journal/event, second-shell revoke, fixed refusals; private plugin-free server");
  } finally {
    await host?.close();
    for (const key of Object.keys(process.env)) if (!(key in env)) delete process.env[key]; Object.assign(process.env, env); globalThis.fetch = fetch;
    if (workspaceId) run(["herdr", "--session", "corral-test", "workspace", "close", workspaceId]);
    assets.cleanup(); rmSync(workspace, { recursive: true, force: true }); rmSync(skill, { recursive: true, force: true });
  }
}, 240_000);

// Same scope-limit cases as the Rust suite, with an injected HOME.
test("scope limit, normalized settings, broad deny and role-independent syntax", () => {
  const home = "/home/u";
  for (const path of ["/", "/home", "/etc", home, `${home}/`, `${home}/.`]) {
    expect(scopeLimit(path, home)).toBeFalse();
  }
  for (const entry of ["/", "~", "~/.", "~//", "/home", "/etc", home, `${home}/`, "relative/dir", "~/a/../b", "~/skills/*"]) {
    expect(() => writeSettings({ allow: [entry] }, home)).toThrow();
  }
  expect(writeSettings({ deny: ["/", "~"] }, home).deny).toEqual(["/", "~"]);
  const cases: [string, boolean][] = [["/", false], ["/etc", false], ["/home", false],
    ["/home/u", false], ["/home/u/", false], ["/data/users", false], ["/data/users/u", false],
    ["/home/u/proj", true], ["/data/users/u/proj", true], ["/opt/lib", true], ["relative/dir", false], ["/srv/../home", false]];
  for (const [path, allowed] of cases) expect(scopeLimit(path, home, "/data/users/u")).toBe(allowed);
  expect(scopeLimit("/home/u/proj", home)).toBeTrue();
  expect(scopeLimit("/opt/lib", home)).toBeTrue();
  expect(scopeLimit("/opt/home", "/opt/home/user")).toBeFalse();
});

test("fixed rules beat default roots and settings; temp excludes HOME and workspace", () => {
  const f = fixture();
  try {
    const scratch = join(f.workspace, ".worktrees/A-1-work");
    mkdirSync(join(f.workspace, ".corral/assistants"), { recursive: true });
    mkdirSync(scratch, { recursive: true });
    writeFileSync(join(f.workspace, ".corral/assistants/A-1.md"), `---\nscratch:\n- path: ${scratch}\n---\n`);
    symlinkSync(join(f.workspace, ".corral"), join(scratch, "link"));
    const artifact = join(f.workspace, ".corral/artifacts/assistant/A-1");
    const refused = [join(scratch, ".git"), join(scratch, "sub/.git/config"), join(scratch, ".corral/x"),
      join(f.base, "repo/.git/config"), join(f.workspace, ".corral/corral.toml"), join(f.workspace, ".corral/tasks/T-1.md"),
      join(f.workspace, ".corral/artifacts/assistant/A-2/x"), join(f.workspace, ".worktrees/T-9/x"),
      join(f.workspace, ".worktrees/A-2-x/y"), join(f.base, "config/corral.toml"), join(scratch, "link/tasks/x"), join(artifact, ".git/x")];
    for (const path of refused) expect(classifyWrite(path, f.paths).rule).toBe("fixed_rule");
    for (const path of [join(scratch, "src/a.rs"), join(artifact, "notes.md"), join(f.base, "note.txt")]) expect(classifyWrite(path, f.paths).rule).toBe("default");
    for (const path of [join(f.home, "x"), join(f.workspace, "x")]) expect(classifyWrite(path, f.paths).decision).toBe("ask");
    expect(classifyWrite(join(f.home, "x"), { ...f.paths, enabled: false }).rule).toBe("capability_off");
    expect(classifyWrite(join(f.home, "x"), { ...f.paths, settings: { allow: [join(f.home, "notes")], deny: ["/"] } }).rule).toBe("configured_deny");
  } finally { f.close(); }
});

test("global skill asks exactly once before decide, grants checked on every write, revoke asks again", async () => {
  const f = fixture();
  try {
    const directory = join(f.home, ".agents/skills/example");
    f.answers.push("Allow for 1 hour", directory);
    expect(await f.write(join(directory, "SKILL.md"))).toBeUndefined();
    expect(f.calls.map((c) => c.payload.action)).toEqual(["check", "decide"]);
    const decision = f.calls[1]!.payload;
    expect(decision.operation).toEqual({ kind: "new_file" });
    expect(decision.scope).toEqual({ shown: directory, canonical: directory });
    expect(f.dialogs[0]!.title).toContain("write " + join(directory, "SKILL.md"));
    expect(f.dialogs[0]!.title).toContain("new file\nhello");
    expect(await f.write(join(directory, "scripts/run.sh"))).toBeUndefined();
    expect(f.dialogs).toHaveLength(2);
    f.grant(false); f.answers.push("Deny");
    expect((await f.write(join(f.home, ".agents/skills/other/SKILL.md")))?.reason).toContain("owner_denied");
    const count = f.calls.length;
    expect((await f.write(join(f.home, ".agents/skills/other/SKILL.md")))?.reason).toContain("owner_denied");
    expect(f.calls).toHaveLength(count);
    f.setTurn(); f.answers.push("Allow once");
    expect(await f.write(join(f.home, ".agents/skills/other/SKILL.md"))).toBeUndefined();
  } finally { f.close(); }
});

test("external project offers distant Git root; session grant and resumed notice", async () => {
  const f = fixture();
  try {
    const root = join(f.home, "proj"); mkdirSync(join(root, ".git"), { recursive: true });
    expect(scopeCandidates(join(root, "src/deep/module/lib.rs"), f.home).map((c) => c.shown)).toContain(root);
    expect(scopeCandidates(join(root, "a"), f.home).map((c) => c.shown)).toEqual([root]);
    f.answers.push("Allow for this session", root);
    expect(await f.broker.authorize("edit", { path: join(root, "src/deep/module/lib.rs"), edits: [{ oldText: "a", newText: "b" }] }, "edit", f.ui)).toBeUndefined();
    await new PermissionBroker(f.context, f.paths).notice(f.ui);
    expect(f.notices[0]).toContain("active write permissions: G-1");
    expect((await f.write(join(root, ".git/config")))?.reason).toContain("fixed_rule");
  } finally { f.close(); }
});

test("once repeats and concurrent nested calls have distinct invocation IDs; resend only within invocation", async () => {
  const f = fixture();
  try {
    const path = join(f.home, "file"); f.answers.push("Allow once", "Allow once", "Allow once");
    f.lose(); expect(await f.write(path)).toBeUndefined();
    expect(f.calls[1]!.id).toBe(f.calls[2]!.id);
    await Promise.all([f.write(path), f.write(path)]);
    const decisions = f.calls.filter((c) => c.payload.action === "decide");
    expect(new Set(decisions.map((c) => c.id)).size).toBe(3);
    for (const call of decisions) {
      const invocation = call.payload.invocation as { generation: number; id: string };
      expect(invocation.id).toMatch(/^[a-f0-9]{32}$/);
      expect(call.id).toBe(toolRequestId("pi-session", `wp:1:${invocation.id}`));
    }
    expect(f.dialogs).toHaveLength(3);
    f.answers.push("Allow once");
    const resumed = new PermissionBroker({ ...f.context, identity: { ...f.context.identity, generation: 2 } }, f.paths);
    expect(await resumed.authorize("write", { path, content: "hello\n" }, "same", f.ui)).toBeUndefined();
    expect(f.calls.at(-1)!.id).not.toBe(decisions.at(-1)!.id);
  } finally { f.close(); }
});

test("cancel, absent UI, abort after decide, no candidates and default roots never authorize accidentally", async () => {
  const f = fixture();
  try {
    const path = join(f.home, "notes.txt");
    expect((await f.write(path))?.reason).toContain("no_answer");
    expect(f.dialogs[0]!.choices).toEqual(["Allow once", "Deny"]);
    expect(f.calls.every((c) => c.payload.action === "check")).toBeTrue();
    expect((await f.broker.authorize("write", { path }, "x", { ...f.ui, hasUI: false }))?.reason).toContain("no_ui");
    const count = f.calls.length;
    expect(await f.write(join(f.base, "note.txt"))).toBeUndefined(); expect(f.calls).toHaveLength(count);
    f.answers.push("Allow once"); f.decide(() => f.abort.abort());
    expect((await f.write(path))?.reason).toContain("aborted");
    expect(f.calls.at(-1)!.payload.choice).toBe("once");
  } finally { f.close(); }
});

test("a symlinked external target shows its real destination before consent", async () => {
  const f = fixture();
  try {
    const real = join(f.home, "real"), link = join(f.home, "link");
    mkdirSync(real); symlinkSync(real, link);
    f.answers.push("Allow once");
    expect(await f.write(join(link, "new.txt"))).toBeUndefined();
    expect(f.dialogs[0]!.title).toContain(`Resolved destination: ${join(real, "new.txt")}`);
    expect(f.calls[0]!.payload.target).toEqual({ shown: join(link, "new.txt"), canonical: join(real, "new.txt") });
  } finally { f.close(); }
});

test("symlink allow re-resolution, target swaps before decide and after authorization", async () => {
  const f = fixture();
  try {
    const real = join(f.home, "real"), link = join(f.home, "link"); mkdirSync(real); symlinkSync(real, link);
    const config = { ...f.paths, settings: { allow: [link], deny: [] } };
    expect(classifyWrite(join(link, "x"), config).rule).toBe("config");
    unlinkSync(link); symlinkSync("/", link);
    expect(classifyWrite(join(link, "etc/x"), config).decision).toBe("ask");
    expect(classifyWrite(join(link, "etc/x"), config).ignored).toEqual([link]);
    const target = join(f.home, "target");
    f.answers.push("Allow once"); f.select(() => { symlinkSync(join(f.workspace, ".git/config"), target); });
    expect((await f.write(target))?.reason).toContain("target_changed");
    expect(f.calls.some((c) => c.payload.action === "decide")).toBeFalse();
    unlinkSync(target); f.select(() => {}); f.answers.push("Allow once");
    f.decide(() => { symlinkSync(join(f.workspace, ".git/config"), target); });
    expect((await f.write(target))?.reason).toContain("target_changed");
    expect(f.calls.at(-1)!.payload.action).toBe("decide");
  } finally { f.close(); }
});
