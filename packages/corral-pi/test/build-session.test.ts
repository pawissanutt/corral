import { expect, test } from "bun:test";
import { execFileSync, spawn } from "node:child_process";
import { closeSync, cpSync, mkdirSync, mkdtempSync, openSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ModelRuntime } from "@earendil-works/pi-coding-agent";
import { createBuildControls } from "../src/build/host-controls.ts";
import { loadSession, saveSession, sessionDir } from "../src/build/session-store.ts";
import type { Run } from "../src/build/corral-cli.ts";
import type { BuildContext, ChildSpec, ExitRecord } from "../src/build/types.ts";
import type { ChildHandle, ChildHooks, ChildRunner } from "../src/extensions/fleet/child.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
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

function fixture(root: string, exit?: ExitRecord, workspace = false, bare = false) {
  const now = new Date().toISOString();
  const context: BuildContext = { dir: sessionDir(root, "B-test"), corral: "/unused/corral",
    settings: { maxParallel: 2, exitPolicy: "none", profiles: [] },
    session: { version: 1, id: "B-test", name: "test", created: now, updated: now, repoRoot: root, worktree: root,
      here: true, branch: "build/test", base: workspace ? "base" : null, ...(workspace ? { workspaceRoot: root, project: "app" } : {}),
      ...(bare ? { bare: true, repository: false } : {}), ...(exit ? { exit } : {}) } };
  saveSession(root, context.session);
  const runner = new FakeRunner();
  const calls: string[][] = [];
  const run: Run = async (argv, options) => {
    calls.push([...argv]);
    if (argv[1] === "gate") {
      expect(options.timeoutMs).toBe(7_200_000);
      return { code: 0, stdout: JSON.stringify({ attempt: "G1", tier: argv[argv.indexOf("--tier") + 1], head: "abc", result: "pass",
        commands: ["check"], started: now, ended: now, receipt: "/logs/G1.json", log: "/logs/G1.log" }), stderr: "" };
    }
    return { code: 0, stdout: JSON.stringify({ id: "B-test", revision: 1 }), stderr: "" };
  };
  const git: Run = async argv => ({ code: 0, stdout: argv[0] === "rev-parse" ? argv.includes("--abbrev-ref") ? "build/test\n" : "abc\n" : "", stderr: "" });
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

test("build evidence tools run the quick gate and retain an independent reviewer's exact verdict and resolutions", async () => {
  const root = mkdtempSync(join(tmpdir(), "build-evidence-"));
  const { controls, runner, calls } = fixture(root, undefined, true);
  try {
    controls.noteOwnerInput("Build a parser\nHandle empty input");
    const verify = await controls.tools.find(tool => tool.name === "build_verify")!.execute("verify", {}, undefined, undefined, {} as never);
    expect(verify.details).toMatchObject({ receipt: { attempt: "G1", tier: "quick", result: "pass", log: "/logs/G1.log" } });
    expect(calls[0]).toEqual(["build", "gate", "--session", "B-test", "--worktree", root, "--project", "app", "--tier", "quick", "--base", "base", "--json"]);
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

// Observe the real hello and one provider request, then stop before InteractiveMode takes a terminal.
// No lifecycle, model-runtime, tool-registration or local control-plane implementation is replaced.
const ENTRY_OBSERVER = `
import { writeFileSync } from "node:fs";
import { HostSession } from "./src/host/session.ts";
import { buildCommand } from "./src/host/build.ts";
const hello = HostSession.prototype.hello;
HostSession.prototype.hello = async function (...args) {
  const receipt = await hello.apply(this, args);
  if (receipt.state !== "committed") throw new Error("build did not bind");
  await this.session.prompt("Reply only BUILD_BOUND.", { source: "extension" });
  await this.drained();
  writeFileSync(4, JSON.stringify({ bound: this.bound, tools: this.session.getActiveToolNames() }) + "\\n");
  await this.close();
  process.exit(0);
};
await buildCommand(["--launch-fd", "3"], import.meta.dirname);
`;

test("the real build entry binds and advertises all build tools without creating .corral", async () => {
  const work = mkdtempSync(join(tmpdir(), "build-entry-"));
  const assets = buildFixtureInstall({ omit: ["role"], isolatePiState: true });
  const repo = join(work, "repo");
  mkdirSync(repo);
  cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
  const git = (...args: string[]) => execFileSync("git", ["-C", repo, ...args], { encoding: "utf8" });
  git("init", "--initial-branch=main");
  git("-c", "user.name=Build Test", "-c", "user.email=build@example.test", "commit", "--allow-empty", "-m", "base");
  const launch = join(work, "launch.json");
  writeFileSync(launch, JSON.stringify({ version: 1, assets: assets.dir, data_root: join(work, "data"), corral: "/unused/corral",
    repo_root: repo, here: true, provider: "openai-codex", model: "gpt-6.1-sol",
    settings: { exit_policy: "none", max_parallel: 2, profiles: [] } }));
  writeFileSync(join(assets.dir, "observe-build.ts"), ENTRY_OBSERVER);
  const fd = openSync(launch, "r");
  const child = spawn(process.execPath, [join(assets.dir, "observe-build.ts")], {
    cwd: assets.dir, env: { ...process.env, PI_CODING_AGENT_DIR: join(work, "pi-state") },
    stdio: ["ignore", "pipe", "pipe", fd, "pipe"],
  });
  closeSync(fd);
  let output = "", errors = "", report = "";
  child.stdout?.on("data", (chunk: Buffer) => { output += chunk.toString(); });
  child.stderr?.on("data", (chunk: Buffer) => { errors += chunk.toString(); });
  child.stdio[4]?.on("data", (chunk: Buffer) => { report += chunk.toString(); });
  const exited = Promise.withResolvers<number | null>();
  child.once("error", exited.reject);
  child.once("close", exited.resolve);
  try {
    const code = await within(exited.promise, 60_000, "the real build entry");
    if (code !== 0) throw new Error(`build entry exited ${code}: ${errors}\n${output}`);
    const observed = JSON.parse(report) as { bound: boolean; tools: string[] };
    expect(observed.bound).toBeTrue();
    for (const tool of BUILD_TOOLS) expect(observed.tools).toContain(tool);
    const directories = [repo];
    while (directories.length) {
      const dir = directories.pop()!;
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        expect(entry.name).not.toBe(".corral");
        if (entry.isDirectory()) directories.push(join(dir, entry.name));
      }
    }
  } finally {
    if (child.exitCode === null) child.kill("SIGKILL");
    await exited.promise.catch(() => {});
    assets.cleanup(); rmSync(work, { recursive: true, force: true });
  }
}, 120_000);

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
