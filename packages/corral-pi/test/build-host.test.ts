import { afterEach, expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { fstatSync, mkdirSync, mkdtempSync, openSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { authDir } from "../src/auth/paths.ts";
import { bindBuildResume, parseBuildFlags } from "../src/build/launch.ts";
import { createSession, listSessions, loadSession, saveSession, sessionDir } from "../src/build/session-store.ts";
import { prepareWorktree } from "../src/build/worktree.ts";
import type { BuildLaunch } from "../src/build/types.ts";
import { buildHostConfig, validateBuildResume } from "../src/host/build.ts";
import { resolveStartupModel } from "../src/host/startup-model.ts";
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

test("build flags retain paths, AWS settings and launch data", async () => {
  const dir = directory();
  const { launch, local } = parseBuildFlags(["--data-dir", "data", "--here", "--name", "A Build", "--headless",
    "--provider", "amazon-bedrock", "--model", "amazon-bedrock/model", "--aws-profile", "sandbox", "--aws-region", "us-west-2"], dir,
    { CORRAL_NEXT_ROOT: dir }, dir);
  expect(launch).toMatchObject({ dataRoot: join(dir, "data"), repoRoot: dir, workspaceRoot: dir, here: true,
    settings: { exitPolicy: "review", maxParallel: 32, profiles: [], delegation: { mode: "standard", routes: [] } },
    aws: { profile: "sandbox", region: "us-west-2" } });
  expect(local.headless).toBeTrue();
  const choice = (await resolveStartupModel({ ...launch, role: "build", headless: true }))!;
  const record = createSession({ ...launch, ...choice }, { worktree: dir, branch: null, base: null });
  expect(buildHostConfig(launch, record, dir, local).aws).toEqual(launch.aws);
});

test("build infers the configured project through workspace and project aliases", () => {
  const dir = directory(), ws = join(dir, "space"), repo = join(ws, "source"), alias = join(dir, "alias");
  mkdirSync(join(ws, ".corral-next"), { recursive: true });
  mkdirSync(repo);
  execFileSync("git", ["init", "--quiet", repo]);
  symlinkSync(ws, alias, "dir");
  symlinkSync(repo, join(ws, "project-link"), "dir");
  writeFileSync(join(ws, ".corral-next", "corral.toml"), '[projects.demo]\npath = "project-link"\n');
  expect(parseBuildFlags([], "/assets", { CORRAL_NEXT_ROOT: alias }, repo).launch.project).toBe("demo");
});

function writePiConfig(root: string, source: string): void {
  mkdirSync(join(root, ".corral-next"), { recursive: true });
  writeFileSync(join(root, ".corral-next", "pi.toml"), source);
}

test("standalone Build reads the Git root, or its start directory outside Git", () => {
  const root = directory(), subdir = join(root, "nested");
  mkdirSync(subdir);
  execFileSync("git", ["init", "--quiet", root]);
  writePiConfig(root, '[build]\nmax_parallel = 4\n[build.delegation]\nmode = "proactive"\n');
  writePiConfig(subdir, '[build.delegation]\nmode = "wrong"\n');
  const nongit = directory();
  writePiConfig(nongit, '[build]\nmax_parallel = 4\n[build.delegation]\nmode = "proactive"\n');
  for (const [cwd, repoRoot] of [[subdir, root], [nongit, nongit]] as const) {
    expect(parseBuildFlags([], "/assets", {}, cwd).launch).toMatchObject({ bare: true, repoRoot,
      settings: { exitPolicy: "none", maxParallel: 4, profiles: [], delegation: { mode: "proactive", routes: [] } } });
  }
});

test("Build route settings retain order and reject invalid fields and profile references", () => {
  const root = directory();
  const profile = '[build.profiles.fern]\nmodel = "openai-codex/example"\n';
  const route = '[[build.delegation.routes]]\nmatch = "Trace calls crossing two package boundaries"\nkind = "explore"\nprofile = "fern"\n';
  const second = '[[build.delegation.routes]]\nmatch = "Implement an independent migration"\nkind = "slice"\nprofile = "default"\nstrategy = "prefer_child"\n';
  const parse = () => parseBuildFlags([], "/assets", { CORRAL_NEXT_ROOT: root }, root).launch.settings;
  writePiConfig(root, profile + route + second);
  expect(parse().delegation).toEqual({ mode: "standard", routes: [
    { match: "Trace calls crossing two package boundaries", kind: "explore", profile: "fern", strategy: "when_delegating" },
    { match: "Implement an independent migration", kind: "slice", profile: "default", strategy: "prefer_child" },
  ] });
  for (const [source, field] of [
    ['[build.delegation]\nmode = "automatic"', "mode"],
    ['[build]\ndelegation = true', "delegation"],
    ['[build.delegation]\nroutes = "invalid"', "routes"],
    [profile + route.replace('kind = "explore"', 'kind = "review"'), "kind"],
    [profile + route.replace('match = "Trace calls crossing two package boundaries"', 'match = "  "'), "match"],
    [profile + route.replace('profile = "fern"\n', ''), "profile"],
    [route, "unknown Build child profile"],
    [profile + route + 'strategy = "always"', "strategy"],
    [profile + route + 'stratgey = "prefer_child"', "stratgey"],
    ['[build]\nmax_parallel = 33', "max_parallel"],
  ] as const) {
    writePiConfig(root, source);
    expect(parse).toThrow(field);
  }
});

test("resume reads the current settings at the recorded workspace or standalone root", () => {
  const caller = directory(), workspace = directory(), repo = directory();
  writePiConfig(caller, '[build.delegation]\nmode = "invalid caller setting"\n');
  writePiConfig(repo, '[build]\nmax_parallel = 2\n');
  for (const workspaceRoot of [workspace, undefined]) {
    const start = parseBuildFlags([], "/assets", { CORRAL_NEXT_ROOT: workspaceRoot }, repo).launch;
    const record = createSession({ ...start, ...fixtureModel }, { worktree: repo, branch: null, base: null });
    const { launch } = parseBuildFlags(["--resume", record.id], "/assets", { CORRAL_NEXT_ROOT: caller }, caller);
    writePiConfig(workspaceRoot ?? repo, '[build]\nmax_parallel = 7\n[build.delegation]\nmode = "proactive"\n');
    const bound = bindBuildResume(launch, record);
    expect(bound).toMatchObject({ repoRoot: repo, settings: { maxParallel: 7, delegation: { mode: "proactive", routes: [] } } });
    expect(bound.workspaceRoot).toBe(workspaceRoot);
    expect(bound.corral).toBe(workspaceRoot ? "corral-next" : undefined);
    writePiConfig(workspaceRoot ?? repo, '[[build.delegation.routes]]\nmatch = "Trace a caller"\nkind = "explore"\nprofile = "missing"\n');
    expect(() => bindBuildResume(launch, record)).toThrow("unknown Build child profile");
  }
});

test("a build under a legacy installation leaves its saved logins reachable", () => {
  const data = directory(), dir = directory();
  const env = { XDG_DATA_HOME: data };
  mkdirSync(join(data, "corral", "pi-native", "auth"), { recursive: true });
  const { launch } = parseBuildFlags(["--name", "A Build"], dir, env, dir);
  saveSession(launch.dataRoot, createSession({ ...launch, ...fixtureModel }, { worktree: dir, branch: null, base: null }));
  expect(authDir(env)).toBe(join(data, "corral", "pi-native", "auth"));
});

const fixtureModel = { provider: "openai-codex", model: "gpt-6.1-sol" } as const;
const fixtureLaunch = (dir: string): BuildLaunch => ({ ...parseBuildFlags(["--name", "A Build"], dir, {}, dir).launch, ...fixtureModel });

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
  const other = directory();
  writePiConfig(other, '[build.delegation]\nmode = "invalid caller setting"\n');
  const startModel = "openai-codex/gpt-6.2-sol";
  const run = async (socket: string, args: string[], transcript?: string): Promise<string> => {
    const cwd = transcript ? other : dir;
    const env = { PATH: process.env.PATH, HOME: dir, TMPDIR: dir, CORRAL_NEXT_ROOT: cwd };
    const child = launch(assets, ["build", "--headless", "--data-dir", data, "--socket", socket, "--corral", "/unused/corral", ...args], cwd, env);
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
        expect(await call("get_state")).toMatchObject({ mode: "headless", model: startModel });
        const record = loadSession(data, id);
        expect(record).toMatchObject({ id, socket, sessionFile: hello.session_file, pid: child.child.pid, workspaceRoot: dir });
        if (transcript === undefined) {
          expect((await call("submit_input", { text: "Fix the parser\nHandle empty input", origin: "owner", input_id: "build-first" })).disposition).toBe("started");
          await child.output(/turn 1 ended/);
          expect(JSON.parse(readFileSync(join(sessionDir(data, id), "journal.json"), "utf8")).title).toBe("Fix the parser");
          expect(JSON.parse(readFileSync(join(sessionDir(data, id), "title.json"), "utf8"))).toEqual({ title: "Fix the parser" });
        } else {
          expect(record.sessionFile).toBe(transcript);
          expect((await call("get_state")).turn).toMatchObject({ current: 1, open: false });
          expect(JSON.parse(readFileSync(join(sessionDir(data, id), "journal.json"), "utf8")).title).toBe("Fix the parser");
        }
        expect(await call("shutdown")).toEqual({ shutting_down: true });
        expect(await within(child.exit, 10_000, "build shutdown")).toBe(0);
        return record.sessionFile!;
      } finally { connection.close(); }
    } finally { await child.stop(); }
  };
  const transcript = await run(join(dir, "start.sock"), ["--here", "--id", id, "--model", startModel]);
  // A pre-route record resumes its conversation, not the workspace's new default.
  saveSession(data, { ...loadSession(data, id), route: undefined });
  writePiConfig(dir, '[startup]\nmodel = "anthropic/no-such-model"\n');
  await run(join(dir, "resume.sock"), ["--resume", id], transcript);
}, 20_000);
