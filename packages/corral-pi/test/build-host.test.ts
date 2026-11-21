import { afterEach, expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { fstatSync, mkdirSync, mkdtempSync, openSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { authDir } from "../src/auth/paths.ts";
import { parseBuildFlags } from "../src/build/launch.ts";
import { createSession, listSessions, loadSession, saveSession, sessionDir } from "../src/build/session-store.ts";
import { prepareWorktree } from "../src/build/worktree.ts";
import type { BuildLaunch } from "../src/build/types.ts";
import { buildHostConfig, validateBuildResume } from "../src/host/build.ts";
import { connect } from "./support/bridge-client.ts";
import { scriptedFixtureInstall } from "./support/fixture-install.ts";
import { launch } from "./support/pi-process.ts";
import { within } from "./support/wait.ts";

const temporary: string[] = [];
function directory(): string {
  const dir = mkdtempSync(join(tmpdir(), "corral-build-host-"));
  temporary.push(dir);
  return dir;
}
afterEach(() => { for (const dir of temporary.splice(0)) rmSync(dir, { recursive: true, force: true }); });

test("build flags retain paths, AWS settings and launch data", () => {
  const dir = directory();
  const { launch, local } = parseBuildFlags(["--data-dir", "data", "--here", "--name", "A Build", "--headless",
    "--provider", "amazon-bedrock", "--model", "amazon-bedrock/model", "--aws-profile", "sandbox", "--aws-region", "us-west-2"], dir,
    { CORRAL_NEXT_ROOT: dir }, dir);
  expect(launch).toMatchObject({ dataRoot: join(dir, "data"), repoRoot: dir, workspaceRoot: dir, here: true,
    settings: { exitPolicy: "review", maxParallel: 3 }, aws: { profile: "sandbox", region: "us-west-2" } });
  expect(local.headless).toBeTrue();
  expect(buildHostConfig(launch, createSession(launch, { worktree: dir, branch: null, base: null }), dir, local).aws).toEqual(launch.aws);
});

test("a build under a legacy installation leaves its saved logins reachable", () => {
  const data = directory(), dir = directory();
  const env = { XDG_DATA_HOME: data };
  mkdirSync(join(data, "corral", "pi-native", "auth"), { recursive: true });
  const { launch } = parseBuildFlags(["--name", "A Build"], dir, env, dir);
  saveSession(launch.dataRoot, createSession(launch, { worktree: dir, branch: null, base: null }));
  expect(authDir(env)).toBe(join(data, "corral", "pi-native", "auth"));
});

const fixtureLaunch = (dir: string) => parseBuildFlags(["--name", "A Build"], dir, {}, dir).launch;

test("build session records round trip privately and list newest first, skipping broken records", () => {
  const dir = directory();
  const launch = fixtureLaunch(dir);
  const first = createSession(launch, { worktree: dir, branch: "build/a", base: "abc" });
  expect(first.id).toMatch(/^B-[a-z0-9]+$/);
  expect(first.name).toBe("a-build");
  expect(first.title).toBe("A Build");
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
  const launch = { ...fixtureLaunch(repo), dataRoot: root };
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

test("a headless build takes --id and records its socket", async () => {
  const dir = directory(), data = join(dir, "data"), id = "B-daemon123";
  execFileSync("git", ["init", "--initial-branch=main", dir]);
  execFileSync("git", ["-C", dir, "-c", "user.name=Build Test", "-c", "user.email=build@example.test",
    "commit", "--allow-empty", "-m", "base"]);
  mkdirSync(sessionDir(data, id), { recursive: true, mode: 0o700 });
  const assets = scriptedFixtureInstall(dir, [{ text: "ready" }]);
  const env = { PATH: process.env.PATH, HOME: dir, TMPDIR: dir, CORRAL_NEXT_ROOT: dir };
  const run = async (socket: string, args: string[], transcript?: string): Promise<string> => {
    const child = launch(assets, ["build", "--headless", "--data-dir", data, "--socket", socket, "--corral", "/unused/corral", ...args], dir, env);
    try {
      await child.output(/session socket ready/);
      const connection = await connect(socket);
      try {
        const call = async (command: string, args: Record<string, unknown> = {}) => {
          const reply = await connection.request({ version: 2, id: command, command, args });
          if (!reply.ok || !reply.result) throw new Error(`${command}: ${JSON.stringify(reply.error)}`);
          return reply.result;
        };
        const hello = await call("hello");
        expect(hello).toMatchObject({ role: "build", session_id: id, pid: child.child.pid });
        expect((await call("get_state")).mode).toBe("headless");
        const record = loadSession(data, id);
        expect(record).toMatchObject({ id, socket, sessionFile: hello.session_file, pid: child.child.pid, workspaceRoot: dir });
        if (transcript === undefined) {
          expect((await call("submit_input", { text: "Begin", origin: "owner", input_id: "build-first" })).disposition).toBe("started");
          await child.output(/turn 1 ended/);
        } else {
          expect(record.sessionFile).toBe(transcript);
          expect((await call("get_state")).turn).toMatchObject({ current: 1, open: false });
        }
        expect(await call("shutdown")).toEqual({ shutting_down: true });
        expect(await within(child.exit, 10_000, "build shutdown")).toBe(0);
        return record.sessionFile!;
      } finally { connection.close(); }
    } finally { await child.stop(); }
  };
  const transcript = await run(join(dir, "start.sock"), ["--here", "--id", id]);
  await run(join(dir, "resume.sock"), ["--resume", id], transcript);
}, 20_000);
