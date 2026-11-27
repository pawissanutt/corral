// Turn checkpoints and rewind with a fixture provider.
import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EventLog } from "../src/bridge/events.ts";
import { type BridgeServer, startBridge } from "../src/bridge/server.ts";
import { saveSession, sessionDir } from "../src/build/session-store.ts";
import { readBuildSettings } from "../src/build/settings.ts";
import type { BuildSessionRecord } from "../src/build/types.ts";
import { CHECKPOINT_ENTRY, type CheckpointRecord, Checkpoints } from "../src/capabilities/checkpoints.ts";
import { workspaceSettings } from "../src/host/config.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import type { CapabilitySet } from "../src/roles/types.ts";
import { buildRole } from "../src/roles/build/index.ts";
import { workerRole } from "../src/roles/worker/index.ts";
import { CorralClient } from "../src/tools/corral.ts";
import auth, { fixtureTransport } from "./fixtures/auth/index.ts";
import { fakeCodexTransport } from "./fixtures/auth/fake-transport.ts";
import { codeHost } from "./support/code-host.ts";
import { buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { writeFakeEngine } from "./support/fake-engine.ts";
import { connect, testHandlers } from "./support/bridge-client.ts";
import { within } from "./support/wait.ts";

const capabilities: CapabilitySet = { catalog: 3, digest: "c".repeat(64), preset: "full", enabled: ["checkpoints"] };

function git(cwd: string, ...args: string[]): string {
  return execFileSync("git", ["-C", cwd, ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
}

/** A linked worktree of a fresh repository, with the deterministic test identity. */
function worktree(work: string, branch = "corral/T-575"): string {
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
  git(repo, "worktree", "add", "-q", "-b", branch, cwd);
  return cwd;
}

type Entry = { type: string; id: string; parentId: string | null; timestamp: string; customType?: string;
  data?: CheckpointRecord; message?: { role: string; content: unknown } };

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
    const checkpointEntries = entries.filter((e) => e.type === "custom" && e.customType === CHECKPOINT_ENTRY);
    const checkpoints = checkpointEntries.map((e) => e.data as CheckpointRecord);
    const listedCheckpoints = checkpointEntries.map(({ data, timestamp }) => {
      expect(new Date(timestamp).toISOString()).toBe(timestamp);
      return { ...data, at: timestamp };
    });
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
    const listed = (await request("rewind", { list: true })).result?.checkpoints as
      Array<Record<string, unknown>> | undefined;
    expect(listed).toHaveLength(listedCheckpoints.length + 1);
    expect(listed?.slice(0, -1)).toMatchObject(listedCheckpoints);
    expect(listed?.at(-1)).toMatchObject({ phase: "start", turn: 3 });
    const isoStamp = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/;
    expect(listed?.at(-1)?.at).toMatch(isoStamp);
    const reply = await request("rewind", { turn: 2 });
    expect(reply.error).toBeUndefined();
    expect(provider.aborted).toBe(abortedBefore + 1);
    const rescue = String(reply.result?.rescue_ref);
    expect(rescue).toMatch(/^refs\/worktree\/corral\/rescue\//);
    expect(reply.result).toMatchObject({ turn: 2, head: firstHead, from_head: secondHead });
    expect(reply.result?.at).toMatch(isoStamp);
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
      checkpoints: listedCheckpoints.slice(0, 3), rewinds: [reply.result],
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

test("a build rewinds its own branch, retains its abandoned work and leaves the source checkout alone", async () => {
  const work = mkdtempSync(join(tmpdir(), "corral-build-rewind-"));
  let host: HostSession | undefined;
  try {
    const branch = "build/walk", cwd = worktree(work, branch), repoRoot = join(work, "repo");
    const base = git(cwd, "rev-parse", "HEAD");
    const identity = { store: "build", project: "", role: "build" as const };
    const at = new Date().toISOString();
    const record: BuildSessionRecord = { version: 1, id: "B-walk", name: "walk", created: at, updated: at,
      worktree: cwd, repoRoot, branch, base, here: false };
    const sdk = await import("@earendil-works/pi-coding-agent");
    const transport = fakeCodexTransport();
    const { capabilities, guidance } = workspaceSettings(repoRoot, "build");
    saveSession(work, record);
    host = await HostSession.create({ sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol",
      ui: "plain", network: guardedFetch(transport.send), capabilities, guidance, runtimeDir: join(work, "runtime"),
      build: { dir: sessionDir(work, record.id), corral: "", settings: readBuildSettings(record), session: record },
      modules: { auth, role: buildRole(PACKAGE_ROOT), assetRoot: PACKAGE_ROOT },
      client: new CorralClient({ corral: "", root: work, cwd, identity }), events: new EventLog(),
      sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")) });
    await host.session.bindExtensions({});
    await host.startLocal();
    transport.script([{ text: "Keep this" }, { text: "Discard this" }]);
    await host.session.prompt("First build prompt");
    writeFileSync(join(cwd, "tracked"), "at the second checkpoint\n");
    const obstruction = join(cwd, "local.cfg");
    writeFileSync(obstruction, "checkpoint version\n");
    await host.session.prompt("Second build prompt");
    rmSync(obstruction);
    writeFileSync(join(cwd, ".gitignore"), "ignored/\nlocal.cfg\n");
    writeFileSync(join(cwd, "tracked"), "after the checkpoint\n");
    git(cwd, "commit", "-qam", "post-checkpoint work");
    const later = git(cwd, "rev-parse", "HEAD");
    expect(host.state().checkpoints).toBeTrue();
    const listed = await host.rewind({ list: true });
    expect(listed.checkpoints).toMatchObject([
      { turn: 1, phase: "start" }, { turn: 1, phase: "end" },
      { turn: 2, phase: "start", head: base }, { turn: 2, phase: "end" },
    ]);
    expect((listed.checkpoints as CheckpointRecord[])[2]?.ref).toMatch(/^refs\/worktree\/corral\/checkpoints\/B-walk\//);
    git(cwd, "switch", "--detach", "-q");
    await expect(host.rewind({ turn: 2 })).rejects.toThrow("recorded branch");
    expect(git(cwd, "rev-parse", "HEAD")).toBe(later);
    git(cwd, "switch", "-q", branch);
    const lock = `${git(cwd, "rev-parse", "--path-format=absolute", "--git-path", "index")}.lock`;
    writeFileSync(lock, "another Git writer", { flag: "wx" });
    await expect(host.rewind({ turn: 2 })).rejects.toThrow("index.lock");
    transport.script([{ text: "The turn still runs" }]);
    await host.session.prompt("Turn while Git is locked");
    const blocked = (await host.rewind({ list: true })).checkpoints as CheckpointRecord[];
    expect(blocked.find(point => point.turn === 3 && point.phase === "start")?.error).toContain("index.lock");
    expect(readFileSync(lock, "utf8")).toBe("another Git writer");
    rmSync(lock);

    git(repoRoot, "switch", "-qc", "merge-topic");
    writeFileSync(join(repoRoot, "merge.txt"), "merge content\n");
    git(repoRoot, "add", "merge.txt");
    git(repoRoot, "commit", "-qm", "topic");
    git(repoRoot, "switch", "-q", "main");
    git(cwd, "merge", "--no-commit", "--no-ff", "merge-topic");
    const mergedIndex = git(cwd, "write-tree");
    const file = host.session.sessionFile!;
    const before = readFileSync(file, "utf8");
    await expect(host.rewind({ turn: 2 })).rejects.toThrow("in-progress Git operation");
    expect(git(cwd, "rev-parse", "MERGE_HEAD")).toBe(git(repoRoot, "rev-parse", "merge-topic"));
    expect(git(cwd, "rev-parse", "HEAD")).toBe(later);
    expect(git(cwd, "write-tree")).toBe(mergedIndex);
    expect(readFileSync(file, "utf8")).toBe(before);
    git(cwd, "merge", "--abort");

    writeFileSync(obstruction, "ignored owner data\n");
    await expect(host.rewind({ turn: 2 })).rejects.toThrow("ignored content obstructs");
    expect(readFileSync(obstruction, "utf8")).toBe("ignored owner data\n");
    expect(git(cwd, "rev-parse", "HEAD")).toBe(later);
    rmSync(obstruction);
    const result = await host.rewind({ turn: 2 });
    expect(result).toMatchObject({ turn: 2, head: base, from_head: later });
    expect(result.rescue_ref).toMatch(/^refs\/corral\/rescue\/B-walk\//);
    expect(git(repoRoot, "show", `${result.rescue_ref}:tracked`)).toBe("after the checkpoint");
    expect(readFileSync(obstruction, "utf8")).toBe("checkpoint version\n");
    expect(git(cwd, "symbolic-ref", "--short", "HEAD")).toBe(branch);
    expect(git(cwd, "rev-parse", "HEAD")).toBe(base);
    expect(readFileSync(join(cwd, "tracked"), "utf8")).toBe("at the second checkpoint\n");
    expect(readFileSync(join(repoRoot, "tracked"), "utf8")).toBe("initial\n");
    const retained = readFileSync(file, "utf8");
    expect(retained).toContain("First build prompt");
    expect(retained).not.toContain("Second build prompt");
  } finally {
    await host?.close();
    rmSync(work, { recursive: true, force: true });
  }
});

test("chat rewinds its saved conversation without touching a Git checkout", async () => {
  const settings = workspaceSettings(PACKAGE_ROOT, "assistant", undefined, {});
  const fx = await codeHost("assistant", { enabled: settings.capabilities.enabled.filter(name => name === "checkpoints") });
  try {
    git(fx.cwd, "init", "-qb", "main");
    git(fx.cwd, "add", ".");
    git(fx.cwd, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-qm", "initial");
    await fx.turn([{ text: "First reply" }], "First chat prompt");
    await fx.turn([{ text: "Second reply" }], "Second chat prompt");
    writeFileSync(join(fx.cwd, "one.txt"), "shared edit\n");
    git(fx.cwd, "add", "one.txt");
    const head = git(fx.cwd, "rev-parse", "HEAD"), index = git(fx.cwd, "write-tree");
    expect(fx.host.state().checkpoints).toBeTrue();
    const listed = await fx.host.rewind({ list: true });
    expect(listed.checkpoints).toMatchObject([1, 2].flatMap(turn => ["start", "end"].map(phase =>
      ({ id: `A-1-t${turn}-${phase}`, turn, phase, scope: "conversation" }))));
    const rewound = await fx.host.rewind({ turn: 2 });
    expect(typeof rewound.at).toBe("string");
    expect(rewound).toEqual({ turn: 2, checkpoint: "A-1-t2-start", at: rewound.at });
    expect((await fx.host.rewind({ list: true })).rewinds).toEqual([rewound]);
    expect(git(fx.cwd, "rev-parse", "HEAD")).toBe(head);
    expect(git(fx.cwd, "write-tree")).toBe(index);
    expect(git(fx.cwd, "for-each-ref", "refs/worktree")).toBe("");
    expect(readFileSync(join(fx.cwd, "one.txt"), "utf8")).toBe("shared edit\n");
    const retained = readFileSync(fx.host.session.sessionFile!, "utf8");
    expect(retained).toContain("First chat prompt");
    expect(retained).not.toContain("Second chat prompt");
  } finally { await fx.close(); }
});

test("build checkpoints and rescues refuse flags that hide worktree changes without modifying the index", () => {
  const work = mkdtempSync(join(tmpdir(), "corral-build-checkpoint-flags-"));
  try {
    const buildBranch = "build/flags", cwd = worktree(work, buildBranch);
    const file = join(cwd, "tracked");
    const index = git(cwd, "rev-parse", "--path-format=absolute", "--git-path", "index");
    const snapshots = new Checkpoints({ git: { cwd, buildBranch }, id: "B-flags", record: () => undefined });
    const checkpoint = snapshots.capture("start", 1);
    const { commit, head } = checkpoint;
    if (!commit || !head) throw new Error(checkpoint.error ?? "missing checkpoint");
    const refs = () => git(cwd, "for-each-ref", "refs/worktree/corral/checkpoints", "refs/corral/rescue/B-flags");
    const saved = refs();
    for (const flag of ["assume-unchanged", "skip-worktree"]) {
      git(cwd, "update-index", `--${flag}`, "--", "tracked");
      const originalIndex = readFileSync(index);
      writeFileSync(file, "not in HEAD\n");
      const entry = snapshots.capture("start", 2);
      expect(entry.commit).toBeUndefined();
      expect(entry.error).toContain(flag);
      expect(() => snapshots.restore({ ...checkpoint, commit, head })).toThrow(flag);
      expect(readFileSync(index)).toEqual(originalIndex);
      expect(readFileSync(file, "utf8")).toBe("not in HEAD\n");
      expect(git(cwd, "rev-parse", "HEAD")).toBe(head);
      expect(refs()).toBe(saved);
      git(cwd, "update-index", `--no-${flag}`, "--", "tracked");
      git(cwd, "restore", "--", "tracked");
    }
  } finally { rmSync(work, { recursive: true, force: true }); }
});

test("a failed snapshot is recorded with its error and never throws into the turn", () => {
  // An unborn repository has no HEAD even when TMPDIR sits inside another Git worktree.
  const work = mkdtempSync(join(tmpdir(), "corral-pi-checkpoints-"));
  try {
    git(work, "init", "-qb", "main");
    const recorded: CheckpointRecord[] = [];
    const checkpoints = new Checkpoints({ git: { cwd: work }, id: "R2", record: entry => recorded.push(entry) });
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

test("workspace settings enable checkpoints for workers with a run, chats and builds", () => {
  const work = mkdtempSync(join(tmpdir(), "corral-pi-checkpoint-settings-"));
  try {
    expect(workspaceSettings(work, "worker", "R1").capabilities.enabled).toContain("checkpoints");
    expect(workspaceSettings(work, "assistant").capabilities.enabled).toContain("checkpoints");
    expect(workspaceSettings(work, "build").capabilities.enabled).toContain("checkpoints");
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
    const entry = new Checkpoints({ git: { cwd }, id: "R1", record: () => undefined }).capture("start", 1, "l");
    expect(git(cwd, "show", `${entry.commit}:a.txt`)).toBe("two");
    expect(git(cwd, "status", "--porcelain")).toBe("M a.txt");
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});
