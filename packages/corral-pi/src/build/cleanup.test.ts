import { expect, test } from "bun:test";
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EventLog } from "../bridge/events.ts";
import { startBridge } from "../bridge/server.ts";
import { inspectBuildCleanup, pruneBuilds } from "./cleanup.ts";
import { parseBuildFlags, parsePruneFlags } from "./launch.ts";
import { createSession, loadSession, saveSession, sessionDir } from "./session-store.ts";
import { prepareWorktree } from "./worktree.ts";
import type { BuildSessionRecord } from "./types.ts";

function git(cwd: string, ...args: string[]): string {
  return execFileSync("git", ["-C", cwd, ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
}

interface Fixture {
  repo: string;
  data: string;
  build(name: string): BuildSessionRecord;
  branchExists(name: string): boolean;
}

async function fixture(run: (f: Fixture) => void | Promise<void>): Promise<void> {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "pi-prune-")));
  try { await run(setup(root)); }
  finally { rmSync(root, { recursive: true, force: true }); }
}

function setup(root: string): Fixture {
  const repo = join(root, "repo"), data = join(root, "data");
  mkdirSync(repo);
  git(repo, "init", "--initial-branch=main");
  git(repo, "config", "user.name", "Build Test");
  git(repo, "config", "user.email", "build@example.test");
  writeFileSync(join(repo, ".git", "info", "exclude"), "target/\n.worktrees/\n");
  git(repo, "commit", "--allow-empty", "-m", "base");
  const workspace = join(root, ".corral-next");
  mkdirSync(workspace);
  writeFileSync(join(workspace, "corral.toml"), '[defaults]\nbase = "main"\n[projects.demo]\npath = "repo"\n');
  const build = (name: string) => {
    const { launch } = parseBuildFlags(["--data-dir", data, "--name", name], root, {}, repo);
    const record = createSession(launch, prepareWorktree(repo, name, false));
    saveSession(data, record);
    return record;
  };
  return { repo, data, build,
    branchExists: name => spawnSync("git", ["-C", repo, "show-ref", "--verify", "--quiet", `refs/heads/${name}`]).status === 0 };
}

test("a clean build has nothing to keep only if its patches are on the landing branch", async () => {
  await fixture(async ({ repo, data, build, branchExists }) => {
    const record = build("equivalent");
    expect(inspectBuildCleanup(record)).toEqual({ worktree: true, branches: [record.branch!] });
    writeFileSync(join(record.worktree, "new"), "different patch\n");
    expect("reason" in inspectBuildCleanup(record)).toBeTrue();
    git(record.worktree, "add", "new");
    git(record.worktree, "commit", "-m", "build patch");
    const original = git(record.worktree, "rev-parse", "HEAD");
    expect("reason" in inspectBuildCleanup(record)).toBeTrue();
    git(repo, "commit", "--allow-empty", "-m", "advance main");
    git(repo, "branch", "unrelated");
    git(repo, "checkout", "-b", "rebased", original);
    git(repo, "rebase", "main");
    const rebased = git(repo, "rev-parse", "HEAD");
    expect(rebased).not.toBe(original);
    git(repo, "checkout", "main");
    git(repo, "merge", "--ff-only", "rebased");
    git(repo, "checkout", "unrelated");
    expect(inspectBuildCleanup(record)).toEqual({ worktree: true, branches: [record.branch!] });
    await pruneBuilds(data, false, () => {});
    expect(existsSync(record.worktree)).toBeFalse();
    // Git branch -d requires ancestry even where cherry recognizes the patch.
    expect(branchExists(record.branch!)).toBeTrue();
    expect(loadSession(data, record.id)).toEqual(record);
  });
});

test("prune dry-run changes nothing and prune retains conversations, dirty and running sessions", async () => {
  await fixture(async ({ repo, data, build, branchExists }) => {
    const idle = build("idle"), chatted = build("chatted"), dirty = build("dirty"), running = build("running"), gone = build("gone");
    const conversations = join(sessionDir(data, chatted.id), "parent.jsonl");
    writeFileSync(conversations, `${JSON.stringify({ type: "session" })}\n${JSON.stringify({ type: "message", message: { role: "user", content: "keep this" } })}\n`);
    saveSession(data, { ...chatted, sessionFile: conversations });
    const output = (worktree: string) => { mkdirSync(join(worktree, "target")); writeFileSync(join(worktree, "target", "output"), "output\n"); };
    output(idle.worktree);
    output(dirty.worktree);
    writeFileSync(join(dirty.worktree, "untracked"), "keep\n");
    git(repo, "worktree", "remove", gone.worktree);
    git(chatted.worktree, "switch", "-c", "landing/chatted");
    const socket = join(data, "build", "active.sock");
    const bridge = await startBridge({ path: socket, sessionId: running.id, events: new EventLog() });
    saveSession(data, { ...running, socket });
    try {
      const snapshot = () => [git(repo, "worktree", "list", "--porcelain"), git(repo, "show-ref"),
        ...[idle, chatted, dirty, running, gone].map(record => readFileSync(join(sessionDir(data, record.id), "session.json"), "utf8"))];
      const before = snapshot();
      const dry: string[] = [], actual: string[] = [];
      await pruneBuilds(data, true, line => dry.push(line));
      expect(dry).toHaveLength(6);
      expect(snapshot()).toEqual(before);
      expect(existsSync(join(idle.worktree, "target", "output"))).toBeTrue();
      await pruneBuilds(data, false, line => actual.push(line));
      expect(actual).toHaveLength(6);
      for (const record of [idle, chatted, gone]) {
        expect(existsSync(record.worktree)).toBeFalse();
        expect(branchExists(record.branch!)).toBeFalse();
      }
      expect(branchExists("landing/chatted")).toBeFalse();
      for (const record of [idle, gone]) expect(existsSync(sessionDir(data, record.id))).toBeFalse();
      expect(loadSession(data, chatted.id)).toMatchObject({ cleaned: true, sessionFile: conversations });
      expect(existsSync(conversations)).toBeTrue();
      for (const record of [dirty, running]) {
        expect(existsSync(record.worktree)).toBeTrue();
        expect(branchExists(record.branch!)).toBeTrue();
        expect(existsSync(sessionDir(data, record.id))).toBeTrue();
      }
      expect(existsSync(join(dirty.worktree, "target", "output"))).toBeTrue();
    } finally { await bridge.close(); }
  });
});

test("prune accepts its own flags but not launch flags", () => {
  expect(parsePruneFlags(["--prune", "--dry-run", "--data-dir", "records"], {}, "/workspace"))
    .toEqual({ dataRoot: "/workspace/records", dryRun: true });
  expect(() => parsePruneFlags(["--prune", "--resume", "B-1"])).toThrow();
  expect(() => parsePruneFlags(["--prune", "--here"])).toThrow();
  expect(() => parsePruneFlags(["--dry-run"])).toThrow();
});
