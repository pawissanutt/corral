// Worker checkpoints and rewind through the socket, on a linked worktree and a fixture provider.
import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../src/bridge/events.ts";
import { type BridgeServer, startBridge } from "../src/bridge/server.ts";
import { CHECKPOINT_ENTRY, type CheckpointRecord, Checkpoints } from "../src/capabilities/checkpoints.ts";
import { workspaceSettings } from "../src/host/config.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import type { CapabilitySet } from "../src/roles/types.ts";
import { workerRole } from "../src/roles/worker/index.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeEngine } from "./support/fake-engine.ts";
import { connect, testHandlers } from "./support/bridge-client.ts";
import { within } from "./support/wait.ts";

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

test("rewind restores the start of a turn and continues the retained conversation", async () => {
  // Given the production worker with checkpoints, bound in a linked worktree with staged work.
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-pi-checkpoints-host-"));
  const originalFetch = globalThis.fetch;
  let host: HostSession | undefined;
  let bridge: BridgeServer | undefined;
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
    const identity = { store: "st-demo", project: "demo", role: "worker" as const, task: "T-575", run: "R1" };
    const events = new EventLog();
    const client = new CorralClient({ corral: writeFakeEngine(work).path, root: work, cwd, identity });
    host = await HostSession.create({
      sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", capabilities, client, events, network,
      guidance: { projectText: false, settings: {} },
      modules: { ...modules, role: workerRole(PACKAGE_ROOT) },
      sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
    });
    await host.session.bindExtensions({});
    await host.startLocal();
    const live = host;
    const turn = async (key: string, command: string) => {
      fixtureTransport().script([
        { toolCalls: [{ callId: `${key}-bash`, name: "bash", arguments: { command } }] },
        { text: "Finished" },
      ]);
      let off = () => {};
      const ended = new Promise<void>((resolve) => {
        off = events.on((event) => { if (event.kind === "turn_ended") resolve(); });
      });
      try {
        expect((await live.submitLocal({ text: `${key}-turn-prompt`, busyPolicy: "prompt", origin: "owner" })).disposition).toBe("started");
        await ended;
        await live.drained();
      } finally {
        off();
      }
    };

    // When two turns each change the worktree, one of them in an ignored directory too.
    await turn("first", "echo one > a.txt && git add a.txt && git commit -qm first -- a.txt && echo dirty > kept.txt && mkdir -p ignored && echo x > ignored/cache");
    const firstHead = git(cwd, "rev-parse", "HEAD");
    await turn("second", "echo two > a.txt && rm tracked && git commit -qm second -- a.txt tracked");
    const secondHead = git(cwd, "rev-parse", "HEAD");

    // Then each turn has a start and an end checkpoint, in order, with refs on their commits.
    const file = live.session.sessionFile ?? "";
    const entries = readFileSync(file, "utf8").trim().split("\n").map((line) => JSON.parse(line) as Entry);
    const checkpoints = entries.filter((e) => e.type === "custom" && e.customType === CHECKPOINT_ENTRY)
      .map((e) => e.data as CheckpointRecord);
    expect(checkpoints.map(c => [c.id, c.phase])).toEqual([
      ["R1-t1-start", "start"], ["R1-t1-end", "end"], ["R1-t2-start", "start"], ["R1-t2-end", "end"],
    ]);
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
    for (const [id, text, leaf] of [[start1.id, "first-turn-prompt", start1.leaf], [start2.id, "second-turn-prompt", start2.leaf]] as const) {
      const at = startEntry(id);
      expect(at).toBeGreaterThan(0);
      expect(at < index(text)).toBeTrue();
      expect(entries[at]?.parentId).toBe(leaf ?? "");
    }
    expect(end1.leaf).toBeUndefined();

    writeFileSync(join(cwd, "hand.txt"), "owner edit\n");
    writeFileSync(join(cwd, "staged.txt"), "unstaged worktree version\n");
    const provider = fixtureTransport();
    provider.holdNext();
    const held = provider.whenHeld();
    expect((await live.submitLocal({ text: "discard this open turn", busyPolicy: "prompt", origin: "owner" })).disposition).toBe("started");
    await within(held, 30_000, "the active turn waiting for a model reply");
    const abortedBefore = provider.aborted;
    const liveCursor = live.liveEvents.since(0, Infinity).last;
    bridge = await startBridge({
      path: join(work, "sock", "host.sock"), sessionId: "T-575/R1", events,
      handlers: testHandlers({ rewind: input => live.rewind(input) }), liveEvents: live.liveEvents,
    });
    const socket = await connect(bridge.path);
    const request = (command: string, args: Record<string, unknown>) => socket.request({ version: 2, id: "request", command, args });
    expect((await request("rewind", { list: true })).result?.checkpoints).toMatchObject([
      ...checkpoints, { phase: "start", turn: 3 },
    ]);
    const reply = await request("rewind", { turn: 2 });
    expect(reply.error).toBeUndefined();
    expect(provider.aborted).toBe(abortedBefore + 1);
    const rescue = String(reply.result?.rescue_ref);
    expect(rescue).toMatch(/^refs\/worktree\/corral\/rescue\//);
    expect(reply.result).toMatchObject({ turn: 2, head: firstHead, from_head: secondHead });
    expect(git(cwd, "symbolic-ref", "HEAD")).toBe("refs/heads/corral/T-575");
    expect(git(cwd, "rev-parse", "HEAD")).toBe(firstHead);
    expect(readFileSync(join(cwd, "tracked"), "utf8")).toBe("initial\n");
    expect(readFileSync(join(cwd, "a.txt"), "utf8")).toBe("one\n");
    expect(readFileSync(join(cwd, "kept.txt"), "utf8")).toBe("dirty\n");
    expect(readFileSync(join(cwd, "ignored/cache"), "utf8")).toBe("x\n");
    expect(existsSync(join(cwd, "hand.txt"))).toBeFalse();
    expect(git(cwd, "rev-parse", `${rescue}^1`)).toBe(secondHead);
    expect(git(cwd, "show", `${rescue}:a.txt`)).toBe("two");
    expect(git(cwd, "show", `${rescue}:hand.txt`)).toBe("owner edit");
    expect(git(cwd, "show", `${rescue}:staged.txt`)).toBe("unstaged worktree version");
    expect(git(cwd, "show", `${rescue}^2:staged.txt`)).toBe("staged");
    expect((await request("rewind", { list: true })).result).toEqual({
      checkpoints: checkpoints.slice(0, 3), rewinds: [reply.result],
    });
    expect((await request("live_since", { seq: liveCursor })).result?.gap).toMatchObject({ reconcile: true });
    socket.close();

    const cut = readFileSync(file, "utf8");
    expect(cut).toContain("first-turn-prompt");
    expect(cut).not.toContain("second-turn-prompt");
    expect(cut).not.toContain("discard this open turn");
    expect(JSON.parse(cut.trimEnd().split("\n").at(-1) ?? "").data).toEqual(start2);
    const prompts = () => live.session.messages.filter(m => m.role === "user").map(m => m.content);
    const before = prompts();
    expect(JSON.stringify(before)).toContain("first-turn-prompt");
    expect(JSON.stringify(before)).not.toContain("second-turn-prompt");
    await turn("followup", "echo continued > a.txt");
    expect(prompts().slice(0, -1)).toEqual(before);
    expect(JSON.stringify(prompts().at(-1))).toContain("followup-turn-prompt");
    expect(readFileSync(join(cwd, "a.txt"), "utf8")).toBe("continued\n");
  } finally {
    await bridge?.close();
    host?.session.dispose();
    globalThis.fetch = originalFetch;
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
// The event and drain barriers, not Bun's wall-clock timeout, govern fixture cleanup.
}, 0);

test("a failed snapshot is recorded with its error and never throws into the turn", () => {
  // An unborn repository has no HEAD even when TMPDIR sits inside another Git worktree.
  const work = mkdtempSync(join(tmpdir(), "corral-pi-checkpoints-"));
  try {
    git(work, "init", "-qb", "main");
    const recorded: CheckpointRecord[] = [];
    const checkpoints = new Checkpoints({ cwd: work, run: "R2", record: entry => recorded.push(entry) });
    // When a turn starts, then the entry names the failure and carries no commit or cursor.
    const entry = checkpoints.capture("start", 4, "leaf-1");
    expect(entry.id).toBe("R2-t4-start");
    expect(typeof entry.error).toBe("string");
    expect(entry.commit).toBeUndefined();
    expect(recorded).toEqual([entry]);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});

test("workspace settings enable checkpoints only for workers with a run", () => {
  const work = mkdtempSync(join(tmpdir(), "corral-pi-checkpoint-settings-"));
  try {
    expect(workspaceSettings(work, "worker", "R1").capabilities.enabled).toContain("checkpoints");
    expect(workspaceSettings(work, "worker").capabilities.enabled).not.toContain("checkpoints");
    expect(workspaceSettings(work, "reviewer", "R1").capabilities.enabled).not.toContain("checkpoints");
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
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
    const entry = new Checkpoints({ cwd, run: "R1", record: () => undefined }).capture("start", 1, "l");
    expect(git(cwd, "show", `${entry.commit}:a.txt`)).toBe("two");
    expect(git(cwd, "status", "--porcelain")).toBe("M a.txt");
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});
