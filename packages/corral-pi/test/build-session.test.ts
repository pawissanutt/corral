import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { validateToolArguments } from "@earendil-works/pi-ai";
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ModelRuntime } from "@earendil-works/pi-coding-agent";
import { createBuildControls, type BuildControls } from "../src/build/host-controls.ts";
import { loadSession, saveSession, sessionDir } from "../src/build/session-store.ts";
import { gitRun, type Run } from "../src/build/corral-cli.ts";
import { prepareWorktree } from "../src/build/worktree.ts";
import type { BuildContext, ChildSpec, ExitRecord } from "../src/build/types.ts";
import type { ChildHandle, ChildHooks, ChildRunner } from "../src/extensions/fleet/child.ts";
import { boardPart } from "../src/host/remote/board.ts";
import { RemoteBuild } from "../src/host/remote/build.ts";
import { cardDetail } from "../src/host/remote/cards.ts";
import { RemoteDialogs } from "../src/host/remote/dialogs.ts";
import type { BuildEntry } from "../src/ui/build/build-entries.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { connect, type TestClient } from "./support/bridge-client.ts";
import { launch } from "./support/pi-process.ts";
import { within } from "./support/wait.ts";

const BUILD_TOOLS = ["task", "agent_message", "agent_roster", "agent_wait", "agent_cancel", "build_verify", "build_review", "build_resolve", "build_finish", "board_plan"];

class FakeRunner implements ChildRunner {
  readonly children = new Map<string, { finish(): void; signal: AbortSignal; hooks: ChildHooks; spec: ChildSpec }>();
  readonly started = Promise.withResolvers<string>();
  run(spec: ChildSpec, _model: unknown, hooks: ChildHooks, signal: AbortSignal): ChildHandle {
    const done = Promise.withResolvers<{ report: string }>();
    const finish = () => done.resolve({ report: spec.id });
    this.children.set(spec.id, { finish, signal, hooks, spec });
    this.started.resolve(spec.id);
    signal.addEventListener("abort", finish, { once: true });
    return { done: done.promise, async steer() {}, async setModel() { return "applied"; }, usage: () => ({ input: 0, output: 0, turns: 0 }) };
  }
}

function fixture(root: string, exit?: ExitRecord, workspace = false, bare = false, engine?: Run, binding?: { repo: string; checkout: ReturnType<typeof prepareWorktree> }) {
  const now = new Date().toISOString();
  const context: BuildContext = { dir: sessionDir(root, "B-test"), corral: "/unused/corral",
    settings: { maxParallel: 2, exitPolicy: binding ? "review" : "none", profiles: [] },
    session: { version: 1, id: "B-test", name: "test", created: now, updated: now, repoRoot: root, worktree: root,
      here: true, branch: "build/test", base: workspace ? "base" : null, ...(workspace ? { workspaceRoot: root, project: "app" } : {}),
      ...(bare ? { bare: true, repository: false } : {}), ...(exit ? { exit } : {}), ...(binding ? { repoRoot: binding.repo, here: false, ...binding.checkout } : {}) } };
  saveSession(root, context.session);
  if (workspace) {
    mkdirSync(join(root, ".corral-next"));
    writeFileSync(join(root, ".corral-next", "corral.toml"), `[projects.app]\nbase = "main"\ngate = ["printf 'local check'"]\n`);
  }
  const runner = new FakeRunner();
  const calls: string[][] = [];
  const run: Run = async (argv, options) => {
    calls.push([...argv]);
    if (engine) return engine(argv, options);
    throw new Error(`Unexpected engine command: ${argv.join(" ")}`);
  };
  const git: Run = binding ? gitRun() : async argv => ({ code: 0, stdout: argv[0] === "rev-parse" ? argv.includes("--abbrev-ref") ? "build/test\n" : "abc\n" : "", stderr: "" });
  let changed = 0;
  const controls = createBuildControls({ context, runner, run, git, sdk: {} as typeof Sdk,
    models: () => ({ getModels: () => [] }) as unknown as ModelRuntime, approved: () => ["approved"],
    parent: { provider: "approved", model: "parent", effort: "high" }, agentDir: join(root, "agent"), localTools: () => [],
    deliverToParent() {}, changed: () => { changed++; } });
  return { context, controls, runner, calls, changed: () => changed };
}

test("build controls expose owner proposals, track the live target, and cancel children on close", async () => {
  const root = mkdtempSync(join(tmpdir(), "build-controls-"));
  const { controls, runner, context, calls, changed } = fixture(root);
  try {
    expect(controls.tools.map(tool => tool.name)).toEqual(BUILD_TOOLS);
    expect(controls.ui.target()).toBe("parent");
    const first = controls.fleet.start("one", [{ id: "first", title: "First", brief: "Work", owns: [] }]);
    controls.ui.setTarget("first");
    expect(controls.panelState()).toMatchObject({ session: context.session, settings: context.settings, target: "first", fleet: { children: [{ id: "first", state: "running" }] }, exit: { phase: "none" }, board: [] });
    runner.children.get("first")!.finish();
    await first;
    await controls.fleet.wait(["first"]);
    expect(controls.ui.target()).toBe("parent");
    controls.ui.setTarget("missing");
    expect(controls.panelState().target).toBe("parent");
    controls.noteOwnerInput("Build the first slice");
    await controls.tools.find(tool => tool.name === "build_finish")!.execute("finish", { title: "Build the first slice" }, undefined, undefined, {} as never);
    expect(controls.ui.record()).toMatchObject({ title: "Build the first slice", intent: [{ text: "Build the first slice" }] });
    const unavailable = await controls.tools.find(tool => tool.name === "build_verify")!.execute("verify", {}, undefined, undefined, {} as never);
    expect(unavailable.isError).toBeTrue();
    expect(controls.panelState().exit.phase).toBe("pending");
    expect(calls).toEqual([]);
    saveSession(root, { ...context.session, sessionFile: join(context.dir, "parent.jsonl") });
    await controls.exit.choose("keep");
    expect(loadSession(root, context.session.id)).toMatchObject({ sessionFile: join(context.dir, "parent.jsonl"), exit: { choice: "keep" } });
    const running = controls.fleet.start("two", [{ id: "second", title: "Second", brief: "Work", owns: [] }]);
    controls.ui.setTarget("second");
    await controls.close();
    await running;
    expect(runner.children.get("second")!.signal.aborted).toBeTrue();
    expect(controls.panelState()).toMatchObject({ target: "parent", fleet: { children: [{ state: "done" }, { state: "cancelled" }] }, exit: { phase: "kept" } });
    expect(changed()).toBeGreaterThan(0);
  } finally { await controls.close(); rmSync(root, { recursive: true, force: true }); }
});

test("bare controls retain fleet and in-session review but expose no gate or board and never publish", async () => {
  const root = mkdtempSync(join(tmpdir(), "bare-controls-"));
  const { controls, calls } = fixture(root, undefined, false, true);
  try {
    expect(controls.tools.map(tool => tool.name)).toEqual(BUILD_TOOLS.filter(name => !["build_verify", "board_plan"].includes(name)));
    expect(controls.board).toBeUndefined();
    expect(controls.ui.board).toBeUndefined();
    expect(controls.panelState().session.bare).toBeTrue();
    expect(controls.panelState().board).toEqual([]);
    await controls.tools.find(tool => tool.name === "build_finish")!.execute("finish", { title: "Local changes" }, undefined, undefined, {} as never);
    await controls.exit.choose("keep");
    await controls.close();
    expect(calls).toEqual([]);
  } finally { await controls.close(); rmSync(root, { recursive: true, force: true }); }
});

test("resumed build controls restore saved exits with no persisted children", async () => {
  for (const choice of ["keep", "land"] as const) {
    const root = mkdtempSync(join(tmpdir(), "build-resume-"));
    const record: ExitRecord = { choice, at: new Date().toISOString(), ...(choice === "land" ? { landed: "abc", gateAttempt: "G1", baseBranch: "main" } : {}) };
    const { controls } = fixture(root, record);
    try {
      expect(controls.panelState()).toMatchObject({ target: "parent", fleet: { children: [] }, exit: { phase: choice === "keep" ? "kept" : "landed", record } });
    } finally { await controls.close(); rmSync(root, { recursive: true, force: true }); }
  }
});

test("Finish prepares against current main, gates and reviews once before offering direct Land", async () => {
  const root = mkdtempSync(join(tmpdir(), "build-finish-")), repo = join(root, "repo");
  mkdirSync(repo);
  const git = (...args: string[]) => execFileSync("git", ["-C", repo, ...args], { encoding: "utf8" }).trim();
  git("init", "-q", "-b", "main"); git("config", "user.name", "Test"); git("config", "user.email", "test@example.org");
  git("commit", "-qm", "start", "--allow-empty");
  const checkout = prepareWorktree(repo, "candidate", false);
  writeFileSync(join(checkout.worktree, "result"), "candidate");
  git("-C", checkout.worktree, "add", "result"); git("-C", checkout.worktree, "commit", "-qm", "work");
  git("commit", "-qm", "advanced", "--allow-empty");
  const target = git("rev-parse", "main");
  const { controls, runner } = fixture(root, undefined, true, false, undefined, { repo, checkout });
  try {
    const finish = controls.tools.find(tool => tool.name === "build_finish")!;
    const completing = finish.execute("finish", { title: "Build result" }, undefined, undefined, {} as never);
    const id = await within(runner.started.promise, 5000, "prepared reviewer");
    const reviewer = runner.children.get(id)!;
    const candidate = git("-C", checkout.worktree, "rev-parse", "HEAD");
    expect(git("rev-parse", candidate + "^")).toBe(target);
    expect(reviewer.spec.brief).toContain(target + ".." + candidate);
    reviewer.hooks.reviewVerdict({ verdict: "approve", summary: "Ready", findings: [], head: candidate });
    reviewer.finish();
    expect((await completing).isError).not.toBe(true);
    expect(git("rev-parse", "main")).toBe(target);
    const option = controls.exit.options().find(row => row.choice === "land")!;
    expect(option.authorization).toBeDefined();
    expect((await controls.exit.choose("land", { authorization: option.authorization })).phase).toBe("landed");
    expect(git("rev-parse", "main")).toBe(candidate);
    expect(controls.ui.record().verification).toHaveLength(1);
    expect(controls.ui.record().reviews).toHaveLength(1);
  } finally { await controls.close(); rmSync(root, { recursive: true, force: true }); }
});

test("a completed review survives a neutral base move but the replay needs its own gate and owner choice", async () => {
  const root = mkdtempSync(join(tmpdir(), "build-review-moved-")), repo = join(root, "repo");
  mkdirSync(join(repo, "docs"), { recursive: true });
  const git = (...args: string[]) => execFileSync("git", ["-C", repo, ...args], { encoding: "utf8" }).trim();
  git("init", "-q", "-b", "main"); git("config", "user.name", "Test"); git("config", "user.email", "test@example.org");
  const notes = "docs/release-notes.md";
  writeFileSync(join(repo, notes), "Previous release\n");
  git("add", notes); git("commit", "-qm", "start");
  const base = git("rev-parse", "main");
  const checkout = prepareWorktree(repo, "candidate", false);
  writeFileSync(join(checkout.worktree, "result"), "candidate");
  git("-C", checkout.worktree, "add", "result"); git("-C", checkout.worktree, "commit", "-qm", "work");
  const head = git("-C", checkout.worktree, "rev-parse", "HEAD");
  const { controls, context, runner } = fixture(root, undefined, true, false, undefined, { repo, checkout });
  writeFileSync(join(root, ".corral-next", "pi.toml"), `[build.review_neutral_docs]\napp = ["${notes}"]\n`);
  try {
    const finish = controls.tools.find(tool => tool.name === "build_finish")!;
    const first = finish.execute("first", { title: "Build result" }, undefined, undefined, {} as never);
    const id = await within(runner.started.promise, 5000, "reviewer on original pair");
    writeFileSync(join(repo, notes), "Latest release\n");
    git("add", notes); git("commit", "-qm", "main advanced");
    const moved = git("rev-parse", "main");
    const reviewer = runner.children.get(id)!;
    reviewer.hooks.reviewVerdict({ verdict: "approve", summary: "Ready", findings: [], head });
    reviewer.finish();
    expect((await within(first, 5000, "stale Finish")).details).toMatchObject({ candidate: null });
    expect(controls.exit.options().find(row => row.choice === "land")?.authorization).toBeUndefined();
    const saved = JSON.parse(readFileSync(join(context.dir, "journal.json"), "utf8"));
    expect(saved.reviews).toMatchObject([{ base, head, verdict: "approve" }]);
    expect(saved.verification).toMatchObject([{ head, result: "pass" }]);

    const second = finish.execute("second", { title: "Build result" }, undefined, undefined, {} as never);
    expect((await within(second, 5000, "Finish with carried approval")).isError).not.toBe(true);
    const replayed = git("-C", checkout.worktree, "rev-parse", "HEAD");
    expect(replayed).not.toBe(head);
    expect(git("rev-parse", replayed + "^")).toBe(moved);
    expect([...runner.children.keys()]).toEqual([id]);
    expect(controls.ui.record().reviews).toMatchObject([{ base, head, evidence: { replay: { base: moved, head: replayed } } }]);
    expect(controls.ui.record().verification).toMatchObject([{ head, result: "pass" }, { head: replayed, result: "pass" }]);
    expect(git("rev-parse", "main")).toBe(moved);
    const land = controls.exit.options().find(row => row.choice === "land")!;
    expect(land.authorization).toBeDefined();
    expect(land.needsReason).toBeUndefined();
    expect((await controls.exit.choose("land", { authorization: land.authorization })).phase).toBe("landed");
    expect(git("rev-parse", "main")).toBe(replayed);
  } finally { await controls.close(); rmSync(root, { recursive: true, force: true }); }
}, 12000);

test.skipIf(process.platform !== "linux")("moving main during preparation stops the gate group and records no receipt", async () => {
  const root = mkdtempSync(join(tmpdir(), "build-prep-moved-")), repo = join(root, "repo");
  mkdirSync(repo);
  const git = (...args: string[]) => execFileSync("git", ["-C", repo, ...args], { encoding: "utf8" }).trim();
  git("init", "-q", "-b", "main"); git("config", "user.name", "Test"); git("config", "user.email", "test@example.org");
  git("commit", "-qm", "start", "--allow-empty");
  const checkout = prepareWorktree(repo, "moved", false);
  git("-C", checkout.worktree, "commit", "-qm", "work", "--allow-empty");
  const { controls } = fixture(root, undefined, true, false, undefined, { repo, checkout });
  const pidfile = join(root, "gate.pid");
  const gate = "sh -c 'trap \"\" TERM; exec sleep 30' & echo $! > " + JSON.stringify(pidfile) + "; wait";
  writeFileSync(join(root, ".corral-next/corral.toml"), '[projects.app]\nbase="main"\ngate=[' + JSON.stringify(gate) + ']\n');
  try {
    const verifying = controls.tools.find(tool => tool.name === "build_verify")!.execute("verify", {}, undefined, undefined, {} as never);
    const deadline = performance.now() + 3000;
    while (!existsSync(pidfile) && performance.now() < deadline) await new Promise(resolve => setTimeout(resolve, 10));
    expect(existsSync(pidfile)).toBeTrue();
    expect(controls.exit.state().phase).toBe("verifying");
    expect(controls.exit.state().land?.gate?.command).toBe(gate);
    git("commit", "-qm", "main advanced", "--allow-empty");
    const result = await within(verifying, 6000, "gate stopped on stale base");
    expect(result.isError).toBeTrue();
    expect(controls.ui.record().verification).toHaveLength(0);
    const pid = Number(readFileSync(pidfile, "utf8"));
    const gone = performance.now() + 2000;
    while (existsSync("/proc/" + pid) && performance.now() < gone) await new Promise(resolve => setTimeout(resolve, 10));
    expect(() => process.kill(pid, 0)).toThrow();
  } finally { await controls.close(); rmSync(root, { recursive: true, force: true }); }
}, 12000);

function terminalDecisions(controls: BuildControls) {
  const entries: BuildEntry<unknown>[] = [];
  const appended = Promise.withResolvers<void>();
  const build = new RemoteBuild(controls, {
    fleet: () => controls.ui.fleet, children: () => [], ready: () => true, headless: () => false, working: () => false,
    changed() {}, mark() {}, error: appended.reject,
    async post(entry) { entries.push(entry); appended.resolve(); },
  });
  const dialogs = new RemoteDialogs(() => {}, { entries: () => build.entries(), absent: () => "missing", shared: () => true });
  return { dialogs, entries, appended: appended.promise };
}

test("owner-confirmed board_plan keeps agent metadata from the previews to CLI creation and posts once", async () => {
  const root = mkdtempSync(join(tmpdir(), "terminal-board-"));
  const metadata = { size: "s", difficulty: "hard", tags: ["bug", "BUG", "--API, old", "Needs  Design", "Needs\u00a0Design"] };
  const label = 'tags: ["bug","BUG","--API, old","Needs  Design","Needs\\u00a0Design"]';
  const { controls, calls } = fixture(root, undefined, true, false, async (argv, { input }) => {
    expect(argv[3]).toBe("board-plan");
    expect(JSON.parse(input!)).toMatchObject({ title: "Plan", project: "app", tasks: [{ key: "slice", project: "app", kind: "ui", ...metadata }] });
    if (argv[4] === "check") return { code: 0, stderr: "", stdout: "null" };
    expect(argv[4]).toBe("file");
    expect(argv).not.toContain("--start");
    return { code: 0, stderr: "", stdout: JSON.stringify({ tasks: [{ key: "slice", project: "app", id: "T-002", status: "backlog", depends_on: [] }] }) };
  });
  const { dialogs, entries, appended } = terminalDecisions(controls);
  try {
    const board = controls.board!;
    const tool = controls.tools.find(tool => tool.name === "board_plan")!;
    const args = validateToolArguments(tool, { type: "toolCall", id: "plan", name: tool.name, arguments: {
      title: "Plan", why: "", tasks: [{ key: "slice", title: "Slice", body: "", depends: [], kind: "ui", ...metadata, tags: [...metadata.tags, "bug"] }],
    } });
    expect((await tool.execute("plan", args, undefined, undefined, {} as never)).isError).not.toBe(true);
    expect(await board.preview(board.proposed()!)).toMatchObject({ tasks: [{ id: "slice", ...metadata }] });
    expect(boardPart(board).proposed?.tasks[0]).toMatchObject(metadata);
    const card = cardDetail(dialogs.entries().find(entry => entry.card.kind === "board_plan")!.card);
    expect(card.body).toContain('kind: "ui"');
    expect(card.body).toContain(`"slice": size: s · difficulty: hard · ${label}`);
    expect(card.body_more).toBeFalse();
    expect(calls.map(argv => argv.slice(3))).toEqual([["board-plan", "check"]]);

    const dock = dialogs.watch(card => card.kind === "board_plan", () => {})!;
    expect(await dock.answer([{ options: ["file"] }])).toBe("started");
    await within(appended, 2000, "board filing conversation entry");
    await expect(dialogs.resolve(dock.id, [{ options: ["file_start"] }], false, { source: "remote" }))
      .rejects.toMatchObject({ code: "invalid_card_answer" });
    expect(calls.map(argv => argv.slice(3))).toEqual([
      ["board-plan", "check"], ["board-plan", "file", "--id", calls[1]?.[6]],
    ]);
    expect(calls[1]?.[6]).toMatch(/^[\da-f-]{36}$/);
    expect(boardPart(board).tasks[0]).toMatchObject({ id: "T-002", ...metadata });
    expect(entries).toHaveLength(1);
    expect(entries).toMatchObject([{ customType: "corral_build_board",
      details: { filed: [{ id: "T-002", title: "Slice", depends: [], ...metadata }], started: false } }]);
    expect(entries[0]!.content).toContain(label);
  } finally { dialogs.close(); await controls.close(); rmSync(root, { recursive: true, force: true }); }
});

test("board_plan rejects silent kind omission even with a ui tag", async () => {
  const root = mkdtempSync(join(tmpdir(), "board-kind-"));
  const { controls, calls } = fixture(root, undefined, true);
  try {
    const tool = controls.tools.find(tool => tool.name === "board_plan")!;
    const plan = { title: "Plan", why: "", tasks: [{ key: "ui", title: "Make session Details summary-first", body: "", depends: [], tags: ["ui"] }] };
    const result = await tool.execute("plan", plan, undefined, undefined, {} as never);
    expect(result.isError).toBeTrue();
    expect(JSON.stringify(result.content)).toContain("explicit routing kind");
    expect(controls.board!.proposed()).toBeUndefined();
    expect(calls).toHaveLength(0);
  } finally { await controls.close(); rmSync(root, { recursive: true, force: true }); }
});

test("Iterate answered remotely posts its terminal conversation outcome once", async () => {
  const root = mkdtempSync(join(tmpdir(), "remote-iterate-"));
  const { controls } = fixture(root);
  const { dialogs, entries, appended } = terminalDecisions(controls);
  try {
    controls.exit.open();
    const exit = dialogs.entries().find(entry => entry.card.kind === "exit")!.card.id;
    expect(await dialogs.resolve(exit, [{ options: ["iterate"] }], false, { source: "remote" })).toBe("started");
    await within(appended, 2000, "Iterate conversation entry");
    await expect(dialogs.resolve(exit, [{ options: ["keep"] }], false, { source: "remote" }))
      .rejects.toMatchObject({ code: "invalid_card_answer" });
    expect(controls.exit.state().phase).toBe("none");
    expect(entries).toHaveLength(1);
    expect(entries).toMatchObject([{ customType: "corral_build_exit", details: { choice: "iterate", phase: "none" } }]);
  } finally { dialogs.close(); await controls.close(); rmSync(root, { recursive: true, force: true }); }
});

test("build evidence tools record the configured gate and retain the reviewer's exact verdict and resolutions", async () => {
  const root = mkdtempSync(join(tmpdir(), "build-evidence-"));
  const { controls, runner, calls } = fixture(root, undefined, true);
  try {
    controls.noteOwnerInput("Build a parser\nHandle empty input");
    const verify = await controls.tools.find(tool => tool.name === "build_verify")!.execute("verify", {}, undefined, undefined, {} as never);
    expect(verify.details).toMatchObject({ gate: { attempt: "G1", result: "pass" } });
    expect(calls).toEqual([]);
    const reviewing = controls.tools.find(tool => tool.name === "build_review")!.execute("review", { focus: "Empty input" }, undefined, undefined, {} as never);
    const id = await runner.started.promise;
    const reviewer = runner.children.get(id)!;
    expect(reviewer.spec).toMatchObject({ id: "review-1", kind: "review", profile: "default", owns: [] });
    expect(reviewer.spec.brief).toContain("Build a parser\nHandle empty input");
    expect(reviewer.spec.brief).toContain("base..abc");
    expect(reviewer.spec.brief).toContain("G1");
    reviewer.hooks.reviewVerdict({ verdict: "changes", summary: "Empty input crashes", head: "abc",
      findings: [{ severity: "major", path: "src/parser.ts", line: 3, text: "Check the empty input before indexing." }] });
    reviewer.finish();
    const result = await reviewing;
    expect(result.details).toMatchObject({ round: 1, review: { verdict: "changes", summary: "Empty input crashes", head: "abc",
      findings: [{ id: "F1", severity: "major", path: "src/parser.ts", line: 3, text: "Check the empty input before indexing." }] } });
    const resolve = controls.tools.find(tool => tool.name === "build_resolve")!;
    expect((await resolve.execute("decline", { round: 1, finding: "F1", state: "declined" }, undefined, undefined, {} as never)).isError).toBeTrue();
    await resolve.execute("fixed", { round: 1, finding: "F1", state: "fixed", commit: "abc" }, undefined, undefined, {} as never);
    expect(controls.ui.record().reviews[0]).toMatchObject({ verdict: "changes", findings: [{ resolution: { state: "fixed", commit: "abc" } }] });
    expect(controls.exit.state().phase).toBe("none");
  } finally { await controls.close(); rmSync(root, { recursive: true, force: true }); }
});

test("the real build entry binds and advertises all build tools without creating .corral", async () => {
  const work = mkdtempSync(join(tmpdir(), "build-entry-"));
  const assets = buildFixtureInstall({ omit: ["role"], parent: work });
  const repo = join(work, "repo");
  mkdirSync(repo);
  cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
  const git = (...args: string[]) => execFileSync("git", ["-C", repo, ...args], { encoding: "utf8" });
  git("init", "--initial-branch=main");
  git("-c", "user.name=Build Test", "-c", "user.email=build@example.test", "commit", "--allow-empty", "-m", "base");
  const data = join(work, "data");
  const socket = join(work, "build.sock");
  const child = launch(assets.dir, ["build", "--here", "--model", "openai-codex/gpt-6.1-sol", "--data-dir", data, "--socket", socket,
    "--corral", "/unused/corral"], repo,
    { PATH: process.env.PATH, HOME: work, TMPDIR: work, XDG_DATA_HOME: data });
  let client: TestClient | undefined;
  try {
    await child.output(/session socket ready/);
    const connection = client = await connect(socket);
    const call = async (command: string, args: Record<string, unknown> = {}) => {
      const reply = await connection.request({ version: 2, id: command, command, args });
      if (!reply.ok || !reply.result) throw new Error(`${command}: ${JSON.stringify(reply.error)}`);
      return reply.result;
    };
    const hello = await call("hello");
    expect(hello).toMatchObject({ protocol: 2, role: "build", pid: child.child.pid });
    const id = String(hello.session_id);
    expect(id).toMatch(/^B-[a-z0-9]+$/);
    const state = await call("get_state");
    expect(state.bound).toBeTrue();
    const context = await call("context");
    for (const tool of BUILD_TOOLS) expect(context.tools).toContain(tool);

    expect(await call("pause")).toMatchObject({ paused: true });
    expect((await call("submit_input", { origin: "auto", text: "Reply only BUILD_BOUND." })).disposition).toBe("queued");
    expect(await call("resume")).toEqual({ paused: false });
    await child.output(/turn 1 ended/);
    const record = JSON.parse(readFileSync(join(sessionDir(data, id), "session.json"), "utf8"));
    expect(record).toMatchObject({ id, role: "build", sessionFile: hello.session_file, paused: false, held_input: [] });
    expect(readFileSync(record.sessionFile, "utf8")).toContain("Reply only BUILD_BOUND.");
    expect(await call("shutdown")).toEqual({ shutting_down: true });
    expect(await within(child.exit, 10_000, "build shutdown")).toBe(0);
    const directories = [repo];
    while (directories.length) {
      const dir = directories.pop()!;
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        expect(entry.name).not.toBe(".corral");
        if (entry.isDirectory()) directories.push(join(dir, entry.name));
      }
    }
  } finally {
    client?.close();
    await child.stop();
    assets.cleanup(); rmSync(work, { recursive: true, force: true });
  }
}, 20_000);

test("resumed task calls cannot overwrite a prior child's recorded history by reusing its id", async () => {
  const root = mkdtempSync(join(tmpdir(), "build-history-"));
  const first = fixture(root);
  try {
    await first.controls.fleet.start("old", [{ id: "parser", title: "Parser", brief: "Build parser", owns: [] }]);
    first.runner.children.get("parser")!.finish();
    await first.controls.fleet.wait(["parser"]);
    await first.controls.close();
    const resumed = fixture(root);
    try {
      const result = await resumed.controls.tools.find(tool => tool.name === "task")!.execute("new", { agents: [
        { id: "parser", title: "Replacement", brief: "Different work", owns: [] },
      ] }, undefined, undefined, {} as never);
      expect(result.isError).toBeTrue();
      expect(resumed.controls.ui.record().slices.map(child => child.title)).toEqual(["Parser"]);
      expect(resumed.controls.fleet.snapshot().children).toEqual([]);
    } finally { await resumed.controls.close(); }
  } finally { await first.controls.close(); rmSync(root, { recursive: true, force: true }); }
});
