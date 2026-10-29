// Turn checkpoints in the real worker host (T-575): each turn's start snapshot is taken before
// its prompt is recorded and names the entry the prompt goes under, the end snapshot follows the
// turn's edits, the worktree's own index is untouched, and a rewind's cursor reopens the session
// on that entry without the abandoned turn while the file keeps it.
import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../src/bridge/events.ts";
import { CHECKPOINT_ENTRY, type CheckpointRecord, Checkpoints } from "../src/capabilities/checkpoints.ts";
import { LaunchMessageInvalid, parseLaunchMessage } from "../src/host/config.ts";
import { openSession } from "../src/host/lifecycle.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import type { CapabilitySet } from "../src/roles/types.ts";
import { workerRole } from "../src/roles/worker/index.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";

const capabilities: CapabilitySet = { catalog: 3, digest: "c".repeat(64), preset: "full", enabled: ["checkpoints"] };

function git(cwd: string, ...args: string[]): string {
  return execFileSync("git", ["-C", cwd, ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
}

/** A linked worktree of a fresh repository, with the deterministic test identity. */
function worktree(work: string): string {
  const repo = join(work, "repo");
  mkdirSync(repo);
  git(repo, "init", "-qb", "main");
  git(repo, "config", "user.name", "t");
  git(repo, "config", "user.email", "t@example.invalid");
  writeFileSync(join(repo, "tracked"), "initial\n");
  writeFileSync(join(repo, ".gitignore"), "ignored/\n");
  git(repo, "add", ".");
  git(repo, "commit", "-qm", "initial");
  const cwd = join(work, "wt");
  git(repo, "worktree", "add", "-q", "-b", "corral/T-575", cwd);
  return cwd;
}

type Entry = { type: string; id: string; parentId: string | null; customType?: string; data?: CheckpointRecord;
  message?: { role: string; content: unknown } };

test("the worker host checkpoints each turn's start and end with the entry before its prompt", async () => {
  // Given the production worker with checkpoints, bound in a linked worktree with staged work.
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-pi-checkpoints-host-"));
  const originalFetch = globalThis.fetch;
  let host: HostSession | undefined;
  try {
    const cwd = worktree(work);
    writeFileSync(join(cwd, "staged.txt"), "staged\n");
    git(cwd, "add", "staged.txt");
    const staged = git(cwd, "diff", "--cached", "--name-only");
    restrictEnvironment(process.env);
    const sdk = await import("@earendil-works/pi-coding-agent");
    const modules = await loadModules(assets.dir, "worker");
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("fixture transport missing");
    const network = guardedFetch(transport);
    globalThis.fetch = network.fetch;
    const identity = { store: "st-demo", project: "demo", role: "worker" as const, generation: 1, task: "T-575", run: "R1" };
    const events = new EventLog(1);
    const corral = writeFakeCorral(work);
    const client = new CorralClient({
      corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "",
    });
    host = await HostSession.create({
      sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", capabilities, client, events, network,
      guidance: { projectText: false, settings: {} },
      modules: { ...modules, role: workerRole(PACKAGE_ROOT) },
      sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
    });
    await host.session.bindExtensions({});
    await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
    const live = host;
    const turn = async (key: string, command: string) => {
      fixtureTransport().script([
        { toolCalls: [{ callId: `${key}-bash`, name: "bash", arguments: { command } }] },
        { text: "Finished" },
      ]);
      const ended = nextEvent(events, (event) => event.kind === "turn_ended");
      expect((await live.submit({ text: `${key}-turn-prompt`, busyPolicy: "prompt", idempotencyKey: key, origin: "test" }, `rq-${key}`))
        .disposition).toBe("started");
      await ended;
      await live.drained();
    };

    // When two turns each change the worktree, one of them in an ignored directory too.
    await turn("first", "echo one > a.txt && mkdir -p ignored && echo x > ignored/cache");
    await turn("second", "echo two > a.txt && rm tracked");

    // Then each turn has a start and an end checkpoint, in order, with refs on their commits.
    const file = live.session.sessionFile ?? "";
    const entries = readFileSync(file, "utf8").trim().split("\n").map((line) => JSON.parse(line) as Entry);
    const checkpoints = entries.filter((e) => e.type === "custom" && e.customType === CHECKPOINT_ENTRY)
      .map((e) => e.data as CheckpointRecord);
    expect(checkpoints.map((c) => `${c.id}:${c.phase}`)).toEqual(["g1-t1:start", "g1-t1:end", "g1-t2:start", "g1-t2:end"]);
    for (const c of checkpoints) {
      expect(c.error).toBeUndefined();
      expect(git(cwd, "rev-parse", c.ref ?? "")).toBe(c.commit ?? "");
      expect(git(cwd, "rev-parse", `${c.commit}^`)).toBe(c.head ?? "");
    }
    const [start1, end1, start2, end2] = checkpoints as [CheckpointRecord, CheckpointRecord, CheckpointRecord, CheckpointRecord];
    // And the snapshots hold the worktree, staged and untracked files included, ignored ones not.
    expect(git(cwd, "show", `${start1.commit}:staged.txt`)).toBe("staged");
    expect(() => git(cwd, "show", `${start1.commit}:a.txt`)).toThrow();
    expect(git(cwd, "show", `${end1.commit}:a.txt`)).toBe("one");
    expect(() => git(cwd, "show", `${end1.commit}:ignored/cache`)).toThrow();
    expect(git(cwd, "show", `${start2.commit}:a.txt`)).toBe("one");
    expect(git(cwd, "show", `${end2.commit}:a.txt`)).toBe("two");
    expect(() => git(cwd, "show", `${end2.commit}:tracked`)).toThrow();
    // And the worktree's own index and the repository's shared refs are untouched.
    expect(git(cwd, "diff", "--cached", "--name-only")).toBe(staged);
    expect(git(join(work, "repo"), "for-each-ref", "refs/worktree")).toBe("");
    expect(git(cwd, "for-each-ref", "--format=%(refname)", "refs/worktree")).toContain("refs/worktree/corral/checkpoints/R1/");

    // And each start names the last entry before it, ahead of the turn's prompt.
    const index = (text: string) => entries.findIndex((e) => e.type === "message" && e.message?.role === "user" &&
      JSON.stringify(e.message.content).includes(text));
    const startEntry = (id: string) => entries.findIndex((e) => e.customType === CHECKPOINT_ENTRY && e.data?.id === id &&
      e.data.phase === "start");
    for (const [id, text, leaf] of [["g1-t1", "first-turn-prompt", start1.leaf], ["g1-t2", "second-turn-prompt", start2.leaf]] as const) {
      const at = startEntry(id);
      expect(at).toBeGreaterThan(0);
      expect(at < index(text)).toBeTrue();
      expect(entries[at]?.parentId).toBe(leaf ?? "");
    }
    expect(end1.leaf).toBeUndefined();

    // When the next generation opens the session at turn 2's cursor, as after a rewind:
    const before = readFileSync(file, "utf8");
    const rewound = openSession(sdk, cwd, join(work, "sessions"), 2, file, false, start2.leaf);
    // Then the conversation stops before turn 2's prompt, and the file still has every entry.
    expect(rewound.getLeafId()).toBe(start2.leaf ?? "");
    const prompts = JSON.stringify(rewound.buildSessionContext().messages.filter((m) => m.role === "user"));
    expect(prompts).toContain("first-turn-prompt");
    expect(prompts).not.toContain("second-turn-prompt");
    expect(readFileSync(file, "utf8")).toBe(before);
    expect(() => openSession(sdk, cwd, join(work, "sessions"), 2, file, false, "missing-entry")).toThrow();
  } finally {
    host?.session.dispose();
    globalThis.fetch = originalFetch;
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
});

test("a failed snapshot is recorded with its error and never throws into the turn", () => {
  // Given a cwd that is not a git worktree.
  const work = mkdtempSync(join(tmpdir(), "corral-pi-checkpoints-"));
  try {
    const recorded: CheckpointRecord[] = [];
    const checkpoints = new Checkpoints({ cwd: work, generation: 3, run: "R2", record: (entry) => recorded.push(entry) });
    // When a turn starts, then the entry names the failure and carries no commit or cursor.
    const entry = checkpoints.capture("start", 4, "leaf-1");
    expect(entry.id).toBe("g3-t4");
    expect(typeof entry.error).toBe("string");
    expect(entry.commit).toBeUndefined();
    expect(recorded).toEqual([entry]);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});

test("a launch carries a rewind cursor only with its resumed session, and checkpoints only for workers", () => {
  const message = {
    version: 1, store: "st-demo", project: "demo", role: "worker", task: "T-575", run: "R1", generation: 2,
    secrets: { host_cap: "h".repeat(64), ctl_cap: "c".repeat(64), bind_nonce: "n".repeat(64) },
    pane: "p1", cwd: "/work/wt", root: "/work", corral: "/bin/corral", assets: "/assets", runtime_dir: "/runtime",
    session_dir: "/sessions", model: "gpt-6.1-sol", resume_session: "/sessions/s.jsonl",
  };
  expect(parseLaunchMessage(JSON.stringify({ ...message, resume_leaf: "a1b2c3d4" })).resumeLeaf).toBe("a1b2c3d4");
  expect(parseLaunchMessage(JSON.stringify(message)).resumeLeaf).toBeUndefined();
  for (const invalid of [
    { ...message, resume_leaf: "a1", resume_session: undefined }, { ...message, resume_leaf: "bad leaf" },
    { ...message, role: "reviewer", review: "R1-review1", capabilities },
  ]) {
    expect(() => parseLaunchMessage(JSON.stringify(invalid))).toThrow(LaunchMessageInvalid);
  }
  expect(parseLaunchMessage(JSON.stringify({ ...message, capabilities })).capabilities?.enabled).toEqual(["checkpoints"]);
});

test("a checkpoint re-hashes a racily clean file as git does", () => {
  // Given a committed file whose index entry is as new as the index file itself, then rewritten
  // with the same size and mtime (one coarse timestamp tick), with ctime not trusted.
  const work = mkdtempSync(join(tmpdir(), "corral-pi-checkpoints-racy-"));
  try {
    const cwd = worktree(work);
    git(cwd, "config", "core.trustctime", "false");
    const tick = 1_790_000_000;
    writeFileSync(join(cwd, "a.txt"), "one\n");
    utimesSync(join(cwd, "a.txt"), tick, tick);
    git(cwd, "add", "a.txt");
    git(cwd, "commit", "-qm", "a");
    utimesSync(git(cwd, "rev-parse", "--path-format=absolute", "--git-path", "index"), tick, tick);
    writeFileSync(join(cwd, "a.txt"), "two\n");
    utimesSync(join(cwd, "a.txt"), tick, tick);
    // When the host checkpoints, then the snapshot has the new content, as `git status` sees it.
    const entry = new Checkpoints({ cwd, generation: 1, run: "R1", record: () => undefined }).capture("start", 1, "l");
    expect(git(cwd, "show", `${entry.commit}:a.txt`)).toBe("two");
    expect(git(cwd, "status", "--porcelain")).toBe("M a.txt");
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});
