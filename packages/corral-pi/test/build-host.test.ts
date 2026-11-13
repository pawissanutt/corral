import { afterEach, expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { fstatSync, mkdirSync, mkdtempSync, openSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { LocalControlPlane, LocalRefusal } from "../src/build/control-plane.ts";
import { readBuildLaunch } from "../src/build/launch.ts";
import { createSession, listSessions, loadSession, saveSession, sessionDir } from "../src/build/session-store.ts";
import { prepareWorktree } from "../src/build/worktree.ts";
import type { BuildLaunch } from "../src/build/types.ts";
import { buildHostConfig, validateBuildResume } from "../src/host/build.ts";
import { parseLaunchMessage } from "../src/host/config.ts";

const temporary: string[] = [];
function directory(): string {
  const dir = mkdtempSync(join(tmpdir(), "corral-build-host-"));
  temporary.push(dir);
  return dir;
}
afterEach(() => { for (const dir of temporary.splice(0)) rmSync(dir, { recursive: true, force: true }); });
const identity = { store: "build", project: "", role: "build", generation: 1 } as const;
class NoSpawnPlane extends LocalControlPlane {
  readonly spawnAttempts: string[] = [];
  protected override spawnCall(op: string): never {
    this.spawnAttempts.push(op);
    throw new Error("A local call must not spawn");
  }
}

test("the local control plane handles lifecycle, accounting, lookup and replay without spawning", async () => {
  const dir = directory();
  const client = new NoSpawnPlane({ dir, root: dir, identity, sessionId: () => "parent",
    handlers: { todo: () => { throw new LocalRefusal("ledger_conflict", "stale revision"); } } });
  const hello = await client.call("hello", { requestId: "hello", payload: {} });
  expect(hello.state).toBe("committed");
  const input = await client.call("event", { requestId: "input", payload: { kind: "owner_input", text: "PRIVATE OWNER TEXT" } });
  const ownerInput = input.result;
  expect(ownerInput !== null && typeof ownerInput === "object" && "input" in ownerInput && typeof ownerInput.input === "string").toBeTrue();
  const replay = await client.call("event", { requestId: "input", payload: { kind: "owner_input", text: "other" } });
  expect(replay).toEqual({ ...input, replayed: true });
  expect((await client.call("event", { requestId: "state", payload: { kind: "budget", action: "state", request: "state" } })).result)
    .toMatchObject({ admitted: true, exhausted: false, limits: { max_tokens: null, max_usd_nanos: null } });
  expect((await client.call("event", { requestId: "reserve", payload: { kind: "budget", action: "reserve", request: "r", tokens: 200, usd_nanos: 300 } })).result)
    .toMatchObject({ admitted: true, exhausted: false, reserved: { tokens: 200, usd_nanos: 300 } });
  expect((await client.call("event", { requestId: "settle", payload: { kind: "budget", action: "settle", request: "r", tokens: 10, usd_nanos: 20 } })).result)
    .toMatchObject({ admitted: true, exhausted: false, used: { tokens: 10, usd_nanos: 20 }, reserved: { tokens: 0, usd_nanos: 0 } });
  expect((await client.call("event", { requestId: "settle-again", payload: { kind: "budget", action: "settle", request: "r", tokens: 10, usd_nanos: 20 } })).result)
    .toMatchObject({ used: { tokens: 10, usd_nanos: 20 } });
  expect((await client.call("event", { requestId: "aux", payload: { kind: "auxiliary", action: "prepare", request: "child-1" } })).result)
    .toEqual({ session_id: "child-1", session_file: join(dir, "auxiliary", "child-1.jsonl") });
  expect((await client.call("artifact", { requestId: "unsupported", payload: {} }))).toMatchObject({ state: "rejected", result: { code: "unsupported_in_build" } });
  expect((await client.call("todo", { requestId: "refusal", payload: {} }))).toMatchObject({ state: "rejected", result: { code: "ledger_conflict", reason: "stale revision" } });
  expect(await client.call("receipt_lookup", { requestId: "lookup", payload: { request_id: "input" } }))
    .toMatchObject({ ...input, lookup: { by: "lookup", request_id: "input", found: true } });
  expect(await client.call("receipt_lookup", { requestId: "missing", payload: { request_id: "absent" } }))
    .toMatchObject({ state: "committed", lookup: { found: false }, result: { found: false } });
  await client.call("job", { requestId: "job", payload: { action: "prepare" } });
  await client.call("job_monitor", { requestId: "monitor", payload: {} });
  expect(await client.attentionSnapshot()).toMatchObject({ version: 1, workspace: dir, items: [] });
  expect(await client.projectScopes()).toEqual({ workspace: dir, projects: {}, session: null });
  expect(await client.todoSnapshot()).toEqual({ container: { state: { todo: null, todo_requests: null } } });
  await expect(client.taskSnapshot()).rejects.toThrow("task_unbound");
  await expect(client.inspectionSnapshot()).rejects.toThrow("inspection_unbound");
  await expect(client.history({} as Parameters<typeof client.history>[0])).rejects.toThrow("unavailable in a build session");
  await expect(client.artifactViewer("x", "show")).rejects.toThrow("unavailable in a build session");
  expect(client.spawnAttempts).toEqual([]);
  const journal = readFileSync(join(dir, "journal.jsonl"), "utf8");
  expect(journal).not.toContain("PRIVATE OWNER TEXT");
  expect(journal.trim().split("\n")).toHaveLength(14);
});

test("coded ledger errors and unexpected local faults are rejected receipts", async () => {
  const dir = directory();
  const client = new LocalControlPlane({ dir, root: dir, identity, sessionId: () => "s", handlers: {
    todo: call => {
      if (call.payload.coded) throw Object.assign(new Error("invalid todo"), { code: "bad_request" });
      throw new Error("local failure");
    },
  } });
  expect(await client.call("todo", { requestId: "coded", payload: { coded: true } }))
    .toMatchObject({ state: "rejected", result: { code: "bad_request", reason: "invalid todo" } });
  expect(await client.call("todo", { requestId: "fault", payload: {} }))
    .toMatchObject({ state: "rejected", result: { code: "local_fault", reason: "local failure" } });
});

test("concurrent duplicate requests execute their handler once", async () => {
  const dir = directory();
  let executions = 0;
  const client = new LocalControlPlane({ dir, root: dir, identity, sessionId: () => "s", handlers: {
    todo: async () => { executions++; await Promise.resolve(); return { revision: executions }; },
  } });
  const [first, replay] = await Promise.all([client.call("todo", { requestId: "same", payload: {} }), client.call("todo", { requestId: "same", payload: {} })]);
  expect(executions).toBe(1);
  expect(replay).toEqual({ ...first, replayed: true });
});

function wire(root: string): Record<string, unknown> {
  return { version: 1, assets: root, data_root: root, corral: join(root, "corral"), repo_root: root, workspace_root: root,
    project: "demo", here: false, name: "A Build", provider: "openai-codex", model: "gpt-5.4", effort: "high",
    capabilities: { catalog: 10, digest: "a".repeat(64), preset: "default", enabled: ["todo"] },
    guidance: { settings: {}, project_text: false },
    settings: { exit_policy: "review", max_parallel: 3, profiles: [{ name: "fast", model: "openai-codex/gpt-5.4", effort: "low", description: "Fast model" }] } };
}
function readWire(dir: string, value: unknown): BuildLaunch {
  const file = join(dir, "launch.json");
  writeFileSync(file, JSON.stringify(value));
  const fd = openSync(file, "r");
  try { return readBuildLaunch(fd); }
  finally { expect(() => fstatSync(fd)).toThrow(); }
}

test("build launches validate paths, exit policy, profile models and rich snapshots", () => {
  const dir = directory();
  const valid = wire(dir);
  expect(readWire(dir, valid)).toMatchObject({ dataRoot: dir, repoRoot: dir, workspaceRoot: dir, provider: "openai-codex",
    guidance: { settings: {}, projectText: false }, capabilities: { enabled: ["todo"] }, settings: { exitPolicy: "review", maxParallel: 3 } });
  expect(() => readWire(dir, { ...valid, repo_root: "relative" })).toThrow("absolute path");
  expect(() => readWire(dir, { ...valid, settings: { exit_policy: "always", max_parallel: 3, profiles: [] } })).toThrow("exit_policy");
  expect(() => readWire(dir, { ...valid, settings: { exit_policy: "review", max_parallel: 3, profiles: [{ name: "bad", model: "unqualified" }] } })).toThrow("<provider>/<id>");
  expect(() => readWire(dir, { ...valid, settings: { exit_policy: "review", max_parallel: 9, profiles: [] } })).toThrow("max_parallel");
  expect(() => readWire(dir, { ...valid, provider: "unknown" })).toThrow("provider");
  expect(() => readWire(dir, { ...valid, capabilities: { catalog: 10, enabled: ["unknown"] } })).toThrow("not implemented");
  expect(() => readWire(dir, { ...valid, guidance: { settings: { hooks: false }, project_text: false } })).toThrow("settings");
  expect(() => readWire(dir, { ...valid, name: "x".repeat(65536) })).toThrow("bound");
});

test("Bedrock build launches preserve the ordinary launch AWS binding through host configuration", () => {
  const dir = directory();
  const aws = { profile: "dev.team+build@company", region: "us-west-2" };
  const message = { ...wire(dir), provider: "amazon-bedrock", model: "anthropic.claude-sonnet-4", aws };
  const launch = readWire(dir, message);
  const ordinary = {
    ...message, role: "worker", store: "st-demo", project: "demo", generation: 1, pane: "p",
    secrets: { host_cap: "h".repeat(64), ctl_cap: "c".repeat(64), bind_nonce: "n".repeat(64) },
    cwd: dir, root: dir, runtime_dir: dir, session_dir: dir,
  };
  const record = createSession(launch, { worktree: dir, branch: null, base: null });
  expect(launch.aws).toEqual(aws);
  expect(buildHostConfig(launch, record, dir).aws).toEqual(parseLaunchMessage(JSON.stringify(ordinary)).aws);
  expect(readWire(dir, { ...message, aws: null }).aws).toBeUndefined();
  expect(readWire(dir, { ...message, aws: undefined }).aws).toBeUndefined();
  for (const invalid of [[], "dev", {}, { profile: "", region: "us-west-2" }, { profile: "dev team", region: "us-west-2" },
    { profile: "x".repeat(65), region: "us-west-2" }, { profile: "dev", region: "US-WEST-2" }, { profile: "dev", region: "unknown-region" }]) {
    expect(() => readWire(dir, { ...message, aws: invalid })).toThrow();
    expect(() => parseLaunchMessage(JSON.stringify({ ...ordinary, aws: invalid }))).toThrow();
  }
});

test("build session records round trip privately and list newest first, skipping broken records", () => {
  const dir = directory();
  const launch = readWire(dir, wire(dir));
  const first = createSession(launch, { worktree: dir, branch: "build/a", base: "abc" });
  expect(first.id).toMatch(/^B-[a-z0-9]+$/);
  expect(first.name).toBe("a-build");
  const old = { ...first, updated: "2026-01-01T00:00:00.000Z" };
  const fresh = { ...createSession({ ...launch, name: undefined }, { worktree: dir, branch: null, base: null }), id: `${first.id}a`, updated: "2026-02-01T00:00:00.000Z" };
  saveSession(dir, old);
  saveSession(dir, fresh);
  expect(loadSession(dir, first.id)).toEqual(old);
  expect(statSync(sessionDir(dir, first.id)).mode & 0o777).toBe(0o700);
  expect(statSync(join(sessionDir(dir, first.id), "session.json")).mode & 0o777).toBe(0o600);
  mkdirSync(sessionDir(dir, "B-broken"));
  writeFileSync(join(sessionDir(dir, "B-broken"), "session.json"), "invalid");
  expect(listSessions(dir).map(record => record.id)).toEqual([fresh.id, old.id]);
  expect(() => sessionDir(dir, "../escape")).toThrow("Invalid");
});

test("worktree preparation creates one branch, refuses collisions and leaves here mode untouched", () => {
  const dir = directory();
  const git = (...args: string[]) => execFileSync("git", ["-C", dir, ...args], { encoding: "utf8" }).trim();
  git("init", "--initial-branch=main");
  git("-c", "user.name=Build Test", "-c", "user.email=build@example.test", "commit", "--allow-empty", "-m", "base");
  const base = git("rev-parse", "HEAD");
  const created = prepareWorktree(dir, "example", false);
  expect(created).toEqual({ worktree: join(dir, ".worktrees", "build-example"), branch: "build/example", base });
  expect(git("rev-parse", "build/example")).toBe(base);
  expect(() => prepareWorktree(dir, "example", false)).toThrow("already exists");
  git("branch", "build/occupied");
  expect(() => prepareWorktree(dir, "occupied", false)).toThrow("branch already exists");
  const before = [git("worktree", "list", "--porcelain"), git("show-ref"), git("status", "--porcelain")];
  expect(prepareWorktree(dir, "unused", true)).toEqual({ worktree: dir, branch: "main", base });
  expect([git("worktree", "list", "--porcelain"), git("show-ref"), git("status", "--porcelain")]).toEqual(before);
  git("checkout", "--detach", "HEAD");
  expect(prepareWorktree(dir, "unused", true).branch).toBeNull();
});

test("resume refuses a switched branch or a replaced repository without rewriting the record", () => {
  const root = directory();
  const repo = join(root, "repo");
  const other = join(root, "other");
  const git = (cwd: string, ...args: string[]) => execFileSync("git", ["-C", cwd, ...args], { encoding: "utf8" }).trim();
  for (const path of [repo, other]) {
    mkdirSync(path);
    git(path, "init", "--initial-branch=main");
    git(path, "-c", "user.name=Build Test", "-c", "user.email=build@example.test", "commit", "--allow-empty", "-m", "base");
  }
  const launch = readWire(root, { ...wire(repo), data_root: root });
  const record = createSession(launch, prepareWorktree(repo, "resume", false));
  saveSession(root, record);
  const file = join(sessionDir(root, record.id), "session.json");
  const saved = readFileSync(file, "utf8");
  expect(validateBuildResume(record)).toBeUndefined();
  git(record.worktree, "checkout", "-b", "unexpected");
  expect(() => validateBuildResume(record)).toThrow("branch differs");
  git(record.worktree, "checkout", "build/resume");
  expect(validateBuildResume({ ...record, repoRoot: record.worktree })).toBeUndefined();
  expect(validateBuildResume({ ...record, here: true, worktree: repo, branch: "old-branch" })).toBeUndefined();
  git(repo, "worktree", "remove", "--force", record.worktree);
  symlinkSync(other, record.worktree, "dir");
  try {
    validateBuildResume(record);
    throw new Error("Expected resume refusal");
  } catch (error) {
    expect(error instanceof Error && "code" in error && error.code).toBe("build_resume_refused");
    expect(error instanceof Error && error.message).toContain(record.worktree);
    expect(error instanceof Error && error.message).toContain("Git common directory differs");
  }
  rmSync(record.worktree);
  mkdirSync(record.worktree);
  git(record.worktree, "init", "--initial-branch=main");
  expect(() => validateBuildResume(record)).toThrow("Git common directory differs");
  expect(readFileSync(file, "utf8")).toBe(saved);
});
