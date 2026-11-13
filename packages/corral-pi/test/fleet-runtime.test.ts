import { describe, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Type } from "typebox";
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ModelRuntime, ToolDefinition } from "@earendil-works/pi-coding-agent";
import type { BuildContext, ChildSpec } from "../src/build/types.ts";
import { FleetRuntime } from "../src/extensions/fleet/runtime.ts";
import { createSdkRunner, type ChildHandle, type ChildHooks, type ChildRunner } from "../src/extensions/fleet/child.ts";
import { resolveModel, resolveProfiles } from "../src/extensions/fleet/profiles.ts";

class FakeRunner implements ChildRunner {
  readonly order: string[] = [];
  readonly children = new Map<string, { hooks: ChildHooks; signal: AbortSignal; finish: (result: { report: string; failed?: string }) => void; steers: string[]; models: string[] }>();
  active = 0;
  peak = 0;
  finishOnAbort = true;
  run(spec: ChildSpec, _model: unknown, hooks: ChildHooks, signal: AbortSignal): ChildHandle {
    this.order.push(spec.id); this.active++; this.peak = Math.max(this.peak, this.active);
    const done = Promise.withResolvers<{ report: string; failed?: string }>();
    const steers: string[] = [];
    const models: string[] = [];
    let ended = false;
    const finish = (result: { report: string; failed?: string }) => {
      if (ended) return;
      ended = true; this.active--; done.resolve(result);
    };
    this.children.set(spec.id, { hooks, signal, finish, steers, models });
    signal.addEventListener("abort", () => { if (this.finishOnAbort) finish({ report: "cancelled", failed: "cancelled" }); }, { once: true });
    if (signal.aborted && this.finishOnAbort) finish({ report: "cancelled", failed: "cancelled" });
    return { done: done.promise, usage: () => ({ input: ended ? 100 : 10, output: ended ? 200 : 20, turns: 1 }),
      async steer(text) { steers.push(text); }, async setModel(provider, model) { models.push(`${provider}/${model}`); return "deferred"; } };
  }
  finish(id: string, failed?: string): void { this.children.get(id)!.finish({ report: `Report from ${id}`, failed }); }
}
const spec = (id: string, overrides: Partial<ChildSpec> = {}): ChildSpec => ({ id, title: id, brief: `Implement ${id}`, owns: ["src"], ...overrides });
function fixture(maxParallel = 2) {
  const runner = new FakeRunner();
  let now = 1000;
  let notifications = 0;
  const context: BuildContext = { dir: "/data/build/test", corral: "corral", settings: { maxParallel, exitPolicy: "none", profiles: [] },
    session: { version: 1, id: "B-test", name: "test", created: "", updated: "", repoRoot: "/repo", worktree: "/repo/.worktrees/build-test", branch: "build/test", base: null, here: false } };
  const runtime = new FleetRuntime({ sdk: {} as typeof Sdk, models: { getModels: () => [] } as unknown as ModelRuntime,
    approved: ["approved"], parent: { provider: "approved", model: "parent", effort: "high" }, context, agentDir: "/data/agent", localTools: [],
    runner, now: () => now, changed: () => { notifications++; } });
  return { runtime, runner, context, advance(ms: number) { now += ms; }, notifications: () => notifications };
}
async function flush(): Promise<void> { for (let i = 0; i < 8; i++) await Promise.resolve(); }

describe("FleetRuntime", () => {
  test("enforces the parallel cap and FIFO order across calls", async () => {
    const { runtime, runner } = fixture(2);
    const first = runtime.start("one", [spec("a"), spec("b"), spec("c")]);
    const second = runtime.start("two", [spec("d"), spec("e")]);
    expect(runner.order).toEqual(["a", "b"]);
    runner.finish("b"); await flush();
    expect(runner.order).toEqual(["a", "b", "c"]);
    runner.finish("a"); await flush();
    expect(runner.order).toEqual(["a", "b", "c", "d"]);
    runner.finish("c"); await flush(); runner.finish("d"); await flush(); runner.finish("e");
    await Promise.all([first, second]);
    expect(runner.peak).toBe(2);
    await runtime.close();
  });

  test("holds dependencies until done and cancels dependents on failure", async () => {
    const { runtime, runner } = fixture(3);
    const success = runtime.start("success", [spec("dependent", { after: ["root"] }), spec("root")]);
    expect(runner.order).toEqual(["root"]);
    runner.finish("root"); await flush();
    expect(runner.order).toEqual(["root", "dependent"]);
    runner.finish("dependent"); await success;
    const failure = runtime.start("failure", [spec("blocked", { after: ["broken"] }), spec("broken")]);
    runner.finish("broken", "provider failed");
    const children = await failure;
    expect(children[0]!.state).toBe("cancelled");
    expect(children[0]!.error).toContain("broken");
    expect(runner.order).not.toContain("blocked");
    await runtime.close();
  });

  test("cancels one child without stopping another and rejects finished steering", async () => {
    const { runtime, runner } = fixture();
    const work = runtime.start("call", [spec("a"), spec("b")]);
    expect(await runtime.steer("b", "Keep the API")).toBe(true);
    expect(runner.children.get("b")!.steers).toEqual(["Keep the API"]);
    expect(runtime.cancel("a")).toBe(true);
    expect(runner.children.get("a")!.signal.aborted).toBe(true);
    expect(runner.children.get("b")!.signal.aborted).toBe(false);
    expect(runtime.snapshot().children.find(child => child.id === "b")!.state).toBe("running");
    runner.finish("b"); await work;
    expect(await runtime.steer("b", "too late")).toBe(false);
    expect(runtime.cancel("b")).toBe(false);
    expect(runtime.snapshot().children[1]!.steers).toEqual(["Keep the API"]);
    await runtime.close();
  });

  test("normalizes edits, flags ownership boundaries and deduplicates paths", async () => {
    const { runtime, runner, context, advance } = fixture();
    const work = runtime.start("call", [spec("a", { owns: ["src/../src/owned"] }), spec("b", { owns: [] })]);
    const hooks = runner.children.get("a")!.hooks;
    hooks.onToolStart("edit", { path: "src/owned/a.ts" });
    hooks.onToolStart("write", { path: "src/owned-other/a.ts" });
    advance(10);
    hooks.onToolStart("write", { path: `${context.session.worktree}/src/owned/./a.ts` });
    runner.children.get("b")!.hooks.onToolStart("write", { path: "elsewhere.ts" });
    const snapshot = runtime.snapshot();
    expect(snapshot.children[0]!.owns).toEqual(["src/owned"]);
    expect(snapshot.children[0]!.edits).toEqual([{ path: "src/owned-other/a.ts", outside: true, at: 1000 }, { path: "src/owned/a.ts", outside: false, at: 1010 }]);
    expect(snapshot.children[1]!.edits[0]!.outside).toBe(false);
    expect(Object.isFrozen(snapshot.children[0]!.edits[0])).toBe(true);
    runner.finish("a"); runner.finish("b"); await work; await runtime.close();
  });

  test("asks the owner then resumes on answer", async () => {
    const { runtime, runner } = fixture();
    const work = runtime.start("call", [spec("a")]);
    const response = runner.children.get("a")!.hooks.ask({ question: "Which API?", options: [{ label: "one" }, { label: "two" }], askedAt: 1000 });
    expect(runtime.snapshot().children[0]!.state).toBe("needs");
    expect(runtime.snapshot().children[0]!.question?.question).toBe("Which API?");
    expect(runtime.answer("a", "two")).toBe(true);
    expect(await response).toBe("two");
    expect(runtime.snapshot().children[0]!.state).toBe("running");
    expect(runtime.snapshot().children[0]!.question).toBeUndefined();
    expect(runtime.answer("a", "again")).toBe(false);
    runner.finish("a"); await work; await runtime.close();
  });

  test("refuses invalid batches before any child starts or id is reserved", async () => {
    const { runtime, runner } = fixture();
    await expect(runtime.start("dup", [spec("a"), spec("a")])).rejects.toThrow("Duplicate");
    await expect(runtime.start("cycle", [spec("a", { after: ["b"] }), spec("b", { after: ["a"] })])).rejects.toThrow("cycle");
    await expect(runtime.start("unknown", [spec("a", { after: ["missing"] })])).rejects.toThrow("missing");
    expect(runner.order).toEqual([]);
    expect(runtime.snapshot().children).toEqual([]);
    const work = runtime.start("valid", [spec("a")]);
    await expect(runtime.start("reused", [spec("b"), spec("a")])).rejects.toThrow("Duplicate");
    expect(runner.order).toEqual(["a"]);
    runner.finish("a"); await work; await runtime.close();
  });

  test("refuses unapproved model providers and applies deferred models on a turn", async () => {
    const { runtime, runner } = fixture();
    const work = runtime.start("call", [spec("a")]);
    expect(() => runtime.setModel("a", "unapproved/model")).toThrow("not approved");
    expect(() => runtime.setModel("a", "missing-profile")).toThrow("Unknown child profile");
    runtime.setModel("a", "approved/other"); await flush();
    expect(runtime.snapshot().children[0]!.pendingModel).toBe("approved/other");
    runner.children.get("a")!.hooks.onTurn();
    expect(runtime.snapshot().children[0]!.model).toBe("approved/other");
    expect(runtime.snapshot().children[0]!.pendingModel).toBeUndefined();
    runner.finish("a"); await work; await runtime.close();
  });

  test("aborting a call cancels only its live children, including queued slices", async () => {
    const { runtime, runner } = fixture(2);
    const controller = new AbortController();
    const first = runtime.start("first", [spec("a"), spec("b"), spec("c")], controller.signal);
    const second = runtime.start("second", [spec("d")]);
    controller.abort(); await first; await flush();
    expect(runtime.snapshot().children.filter(child => child.call === "first").map(child => child.state)).toEqual(["cancelled", "cancelled", "cancelled"]);
    expect(runner.order).not.toContain("c");
    expect(runner.children.get("d")!.signal.aborted).toBe(false);
    runner.finish("d"); await second; await runtime.close();
  });

  test("caps pulse buckets and messages, preserves historical state and immutable copies", async () => {
    const { runtime, runner, advance, notifications } = fixture(1);
    const work = runtime.start("call", [spec("a"), spec("b")]);
    const hooks = runner.children.get("a")!.hooks;
    for (let i = 0; i < 10; i++) hooks.onToolStart("read", { path: "src/a" });
    expect(runtime.snapshot().children[0]!.pulse).toEqual([7]);
    advance(10000);
    const question = hooks.ask({ question: "Continue?", options: [{ label: "yes" }, { label: "no" }], askedAt: 11000 });
    expect(runtime.snapshot().children[0]!.history).toBe("rrn");
    runtime.answer("a", "yes"); await question;
    for (let i = 0; i < 100; i++) hooks.onEvent({ type: "tool", id: String(i), name: "read", phase: "completed", text: "x".repeat(5000) });
    const before = runtime.snapshot();
    expect(before.children[0]!.messages.length).toBeLessThanOrEqual(64);
    expect(before.children[0]!.messages.every(item => item.text.length <= 4096)).toBe(true);
    expect(before.children[0]!.messages.reduce((sum, item) => sum + item.text.length + (item.tool?.length ?? 0), 0)).toBeLessThanOrEqual(32768);
    expect(notifications()).toBeGreaterThan(100);
    advance(5000 * 500); hooks.onToolStart("read", {});
    expect(runtime.snapshot().children[0]!.pulse.length).toBe(120);
    expect(runtime.snapshot().children[0]!.history.length).toBe(240);
    expect(before.children[0]!.pulse).toEqual([7, 0, 0]);
    runner.children.get("a")!.finish({ report: "x".repeat(40000) }); await flush(); runner.finish("b");
    const children = await work;
    expect(children[0]!.report?.length).toBe(32768);
    await runtime.close();
  });
});

test("profile resolution adds the parent default but preserves an explicit default", () => {
  const settings = { exitPolicy: "none" as const, maxParallel: 2, profiles: [{ name: "fast", model: "approved/fast" }] };
  const profiles = resolveProfiles(settings, { provider: "approved", model: "parent", effort: "high" });
  expect(profiles.map(profile => profile.name)).toEqual(["default", "fast"]);
  expect(resolveModel(undefined, profiles, ["approved"])).toEqual({ profile: "default", provider: "approved", model: "parent", effort: "high" });
  expect(resolveModel("approved/family/model", profiles, ["approved"]).model).toBe("family/model");
  expect(resolveProfiles({ ...settings, profiles: [{ name: "default", model: "approved/chosen" }] }, { provider: "approved", model: "parent" })).toEqual([{ name: "default", model: "approved/chosen" }]);
});

type GuardEvent = { toolName: string; input: Record<string, unknown>; parentToolCallId?: string };
type Guard = (event: GuardEvent) => { block: boolean; reason: string } | undefined;

/** Capture the actual child extension guard without a provider or real SDK session. */
async function childGuard(): Promise<Guard> {
  const dir = mkdtempSync(join(tmpdir(), "fleet-guard-"));
  let guard: Guard | undefined;
  const { context, runtime } = fixture();
  const fakeSdk = {
    CURRENT_SESSION_VERSION: 3,
    SettingsManager: { inMemory: () => ({}) },
    SessionManager: { open: () => ({}) },
    async createAgentSessionServices(options: { resourceLoaderOptions: { extensionFactories: ((api: ExtensionAPI) => void)[] } }) {
      const api = { registerTool() {}, on(name: string, handler: unknown) { if (name === "tool_call") guard = handler as Guard; } };
      for (const extension of options.resourceLoaderOptions.extensionFactories) extension(api as unknown as ExtensionAPI);
      return {};
    },
    async createAgentSessionFromServices() {
      return { session: { messages: [], subscribe: () => () => {}, async bindExtensions() {}, async prompt() {},
        getLastAssistantText: () => "", getSessionStats: () => ({ tokens: { input: 0, output: 0 }, cost: 0 }),
        getContextUsage: () => undefined, dispose() {} } };
    },
  } as unknown as typeof Sdk;
  const localTools: ToolDefinition[] = ["job_start", "shell_command"].map(name => ({
    name, label: name, description: "Run a command", parameters: Type.Object({ command: Type.String() }),
    async execute() { throw new Error("Guard tests never execute tools"); },
  }));
  try {
    const runner = createSdkRunner({ sdk: fakeSdk, models: { getModel: () => ({}) } as unknown as ModelRuntime,
      context: { ...context, dir }, agentDir: dir, localTools,
      shellRefusal: command => command.includes(".corral") ? "Reserved .corral path" : undefined });
    const handle = runner.run(spec("guard"), { profile: "default", provider: "approved", model: "parent" },
      { onEvent() {}, onToolStart() {}, onTurn() {}, async ask() { return ""; } }, new AbortController().signal);
    const result = await handle.done;
    expect(result.failed).toBeUndefined();
    expect(guard).toBeDefined();
    return guard!;
  } finally { await runtime.close(); rmSync(dir, { recursive: true, force: true }); }
}

test("child shell checks cover background jobs and other command tools", async () => {
  const guard = await childGuard();
  for (const toolName of ["bash", "job_start", "shell_command"]) {
    expect(guard({ toolName, input: { command: "git reset --hard" } })?.block).toBe(true);
    expect(guard({ toolName, input: { command: "printf bad > .corral/state" } })?.reason).toBe("Reserved .corral path");
    expect(guard({ toolName, input: { command: "git status --short" } })).toBeUndefined();
  }
});

test("child Git guard allows only read-only commands and listing forms", async () => {
  const guard = await childGuard();
  const allowed = ["status --short", "diff -- src", "log -3", "show HEAD", "blame src/a.ts", "grep pattern", "ls-files", "rev-parse HEAD", "cat-file -p HEAD",
    "branch", "branch --list feature", "branch -av", "tag", "tag --list v*", "remote", "remote -v", "remote show -n origin", "stash list", "worktree list --porcelain",
    "config --get user.name", "config --list"];
  for (const command of allowed) expect(guard({ toolName: "bash", input: { command: `git ${command}` } })).toBeUndefined();
  const refused = ["clean -fd", "restore .", "apply patch", "add .", "update-ref refs/heads/main HEAD", "checkout -- src/a.ts", "stash", "stash pop",
    "commit", "push", "fetch", "branch feature", "branch -d feature", "branch --list --delete feature", "tag v1", "tag -d v1", "tag --list --delete v1",
    "remote add origin url", "remote update", "remote show origin", "worktree add ../other", "config user.name x", "config --get --unset user.name",
    "config --list --edit", "diff --output=out", "log --output out", "cat-file --filters HEAD:src/a", "grep --open-files-in-pager pattern"];
  for (const command of refused) expect(guard({ toolName: "bash", input: { command: `git ${command}` } })?.reason).toBe("Build children cannot run Git write commands");
});

test("child Git guard retains quoted global-option values and checks compound commands", async () => {
  const guard = await childGuard();
  for (const command of [
    'git -C "/work/my repo" reset --hard', "git -C '/work/my repo' reset --hard", "git -C /work/my\\ repo reset --hard",
    'git --work-tree="/work/my repo" --git-dir "/work/my repo/.git" reset --hard',
    'git -c "core.quotePath=false" --namespace="my space" reset --hard',
    "git status && git clean -fd", "git status || git reset --hard", "git status; git add .", "git status | git apply",
    "git status\ngit restore .", "(git reset --hard)", "echo $(git reset --hard)", 'echo "$(git reset --hard)"',
    'git -C "/work/my repo reset --hard', "git status && git 'unterminated",
  ]) expect(guard({ toolName: "bash", input: { command } })?.block).toBe(true);
  for (const command of ['git -C "/work/my repo" status', "git -C '/work/my repo' diff", "git -C /work/my\\ repo log",
    'git --git-dir="/work/my repo/.git" --work-tree "/work/my repo" status', 'git -c "core.quotePath=false" --namespace "my space" log',
    "git status && git diff; git log | cat", 'echo "$(git -C "/work/my repo" status)"', "git grep git", "git log --grep git"]) {
    expect(guard({ toolName: "bash", input: { command } })).toBeUndefined();
  }
});

test("cancelling a running child waits for its final report and usage without an owner error", async () => {
  const { runtime, runner } = fixture(2);
  runner.finishOnAbort = false;
  let returned = false;
  const call = runtime.start("cancelled-call", [spec("stopping")]).then(children => { returned = true; return children; });
  const other = runtime.start("other-call", [spec("other")]);
  try {
    expect(runtime.cancel("stopping")).toBe(true);
    expect(runner.children.get("stopping")!.signal.aborted).toBe(true);
    expect(runtime.snapshot().children[0]!.state).toBe("cancelled");
    expect(runtime.snapshot().children[0]!.error).toBeUndefined();
    await flush();
    expect(returned).toBe(false);
    expect(runner.children.get("other")!.signal.aborted).toBe(false);
    runner.children.get("stopping")!.finish({ report: "Final cleanup report", failed: "aborted" });
    const [stopped] = await call;
    expect(stopped!.state).toBe("cancelled");
    expect(stopped!.error).toBeUndefined();
    expect(stopped!.report).toBe("Final cleanup report");
    expect(stopped!.usage).toEqual({ input: 100, output: 200, turns: 1 });
  } finally { runner.finish("stopping"); runner.finish("other"); await Promise.all([call, other]); await runtime.close(); }
});
