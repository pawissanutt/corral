import { describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { engineRun, gitRun, checked, json, type Run } from "../src/build/corral-cli.ts";
import { createExit, LAND_REPAIR_ROUNDS, type ExitProgressRecord } from "../src/build/exit.ts";
import { createJournal } from "../src/build/record.ts";
import { summarize } from "../src/build/git.ts";
import type { BuildGateResult } from "../src/build/record-types.ts";
import { verifyBuild } from "../src/build/gate.ts";
import { prepareWorktree } from "../src/build/worktree.ts";
import type { BuildContext, ChildSnapshot, ExitPolicy, ExitRecord, FleetSnapshot } from "../src/build/types.ts";

const head = "a".repeat(40), base = "b".repeat(40);
const fleet: FleetSnapshot = { children: [], mail: [], maxParallel: 3 };
function context(policy: ExitPolicy = "gate"): BuildContext {
  return { corral: "corral", dir: "/unused", settings: { exitPolicy: policy, maxParallel: 3, profiles: [] }, session: {
    version: 1, id: "B-one", name: "parser", created: "2026-01-01T00:00:00Z", updated: "2026-01-01T00:00:00Z",
    repoRoot: "/repo", worktree: "/repo/.worktrees/build-parser", workspaceRoot: "/workspace", project: "app",
    here: false, branch: "build/parser", base,
  } };
}
function git(dirty = false, commits = 2, calls: string[][] = [], branch: string | null = "build/parser"): Run {
  return async argv => {
    calls.push([...argv]);
    const stdout = argv[0] === "rev-parse" ? (argv.includes("--abbrev-ref") ? branch ?? "HEAD" : argv[1] === "HEAD" ? head : base)
      : argv[0] === "merge-base" ? base : argv[0] === "rev-list" ? String(commits)
      : argv[0] === "diff" ? argv.includes("--numstat") ? "4\t2\tsrc/parser.ts\n-\t-\timage.png\n" : "patch"
      : argv[0] === "status" ? (dirty ? " M src/parser.ts\n" : "") : "";
    return { code: 0, stdout, stderr: "" };
  };
}
function gateResult(): BuildGateResult {
  return { attempt: "G1", head, result: "pass", commands: ["check"], started: "2026-01-01T00:00:00Z", ended: "2026-01-01T00:01:00Z", log: "/logs/G1.log" };
}
type Repair = (prompt: string, f: { commit(): void; approve(): void }) => { aborted: boolean } | Promise<{ aborted: boolean }>;
/** `failGate` fails every gate; `failGates` fails only the first that many. */
function fixture(settings: { policy?: ExitPolicy; failGate?: boolean; failGates?: number; repair?: Repair; stale?: boolean; conflict?: boolean; patchChanged?: boolean } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "build-exit-"));
  const start = context(settings.policy);
  const ctx = { ...start, dir, session: { ...start.session, workspaceRoot: dir, worktree: dir } };
  mkdirSync(join(dir, ".corral-next"));
  const setGate = (fail: boolean) => writeFileSync(join(dir, ".corral-next", "corral.toml"),
    `[projects.app]\nbase = \"main\"\ngate = [\"${fail ? "echo failing-check; exit 1" : "true"}\"]\n`);
  setGate(settings.failGate === true);
  const events: string[] = [], records: ExitRecord[] = [], calls: string[][] = [];
  let gates = 0, rebased = false, current = head;
  const prompts: string[] = [];
  const underlying = git(false, 2, calls);
  const gitRun: Run = async (argv, options) => {
    if (argv[0] === "rev-parse" && argv[1] === "HEAD") return { code: 0, stdout: current, stderr: "" };
    if (argv[0] === "rebase") {
      events.push(argv.includes("--abort") ? "abort" : "rebase"); rebased = true;
      return { code: settings.conflict && !argv.includes("--abort") ? 1 : 0, stdout: "", stderr: "conflict" };
    }
    if (argv.includes("--diff-filter=U")) return { code: 0, stdout: "src/parser.ts\n", stderr: "" };
    if (argv.includes("--binary") && settings.patchChanged && rebased) return { code: 0, stdout: "changed patch", stderr: "" };
    if (argv.includes("--verify") && settings.stale && rebased) return { code: 0, stdout: "c".repeat(40), stderr: "" };
    if (argv[0] === "worktree") return { code: 0, stdout: "worktree /primary\0branch refs/heads/main\0\0", stderr: "" };
    if (argv[0] === "merge") events.push("land");
    return underlying(argv, options);
  };
  const journal = createJournal({ context: ctx, git: gitRun, fleet: () => fleet });
  const repair = settings.repair;
  const control = createExit({ context: ctx, git: gitRun, journal, fleet: () => fleet,
    save: record => {
      records.push(record);
      if ((record as ExitProgressRecord).progress?.phase !== "verifying") return;
      // The gate reads the project's commands after this save, so each attempt can be made to pass or fail.
      events.push("gate"); gates++;
      setGate(settings.failGate === true || gates <= (settings.failGates ?? 0));
    }, changed() {},
    ...(repair ? { repair: async (prompt: string) => {
      events.push("repair"); prompts.push(prompt);
      return repair(prompt, {
        commit() { current = String(prompts.length).repeat(40); },
        approve() { journal.review({ ...child(`review-${prompts.length}`, "review"), verdict: { verdict: "approve", summary: "Approved", findings: [], head: current, at: 1000 } }, current); },
      });
    } } : {}) });
  return { ctx, journal, control, events, records, calls, prompts, close() { control.close(); rmSync(dir, { recursive: true, force: true }); } };
}
function child(id = "parser", kind: ChildSnapshot["kind"] = "slice"): ChildSnapshot {
  return { id, kind, call: "call", title: "Parser", brief: "Build parser", owns: ["src/parser.ts"], after: [], profile: "default", model: "provider/model",
    state: "done", createdAt: 1000, startedAt: 1000, endedAt: 4000,
    edits: [{ path: "docs/usage.md", outside: true, at: 2000 }], usage: { input: 10, output: 20, cost: 0.25, turns: 1 },
    steers: [], pulse: [], history: "d", messages: [], report: "done" };
}

describe("build exits", () => {
  test("Verify executes the project's shell lines and Land rebases and fast-forwards its base", async () => {
    const dir = mkdtempSync(join(tmpdir(), "build-local-land-"));
    const repo = join(dir, "repo");
    const run = gitRun();
    const git = (cwd: string, ...args: string[]) => checked(run, args, cwd);
    try {
      mkdirSync(repo);
      mkdirSync(join(dir, ".corral-next"));
      await git(repo, "init", "-b", "release");
      await git(repo, "config", "user.name", "Build Fixture");
      await git(repo, "config", "user.email", "build@example.test");
      await git(repo, "commit", "--allow-empty", "-m", "base");
      const checkout = prepareWorktree(repo, "gate", false);
      writeFileSync(join(checkout.worktree, "code.txt"), "verified\n");
      await git(checkout.worktree, "add", "code.txt");
      await git(checkout.worktree, "commit", "-m", "build change");
      const before = (await git(checkout.worktree, "rev-parse", "HEAD")).trim();
      await git(repo, "commit", "--allow-empty", "-m", "base advanced");
      const baseTip = (await git(repo, "rev-parse", "HEAD")).trim();
      const initial = context();
      const ctx = { ...initial, dir: join(dir, "session"), session: { ...initial.session, repoRoot: repo, workspaceRoot: dir, ...checkout } };
      const commands = ["test \"$PWD\" = \"$BUILD_WORKTREE\" && printf 'worktree verified\\n'", "grep -x verified code.txt"];
      writeFileSync(join(dir, ".corral-next", "corral.toml"), `[projects.app]\npath = "repo"\nbase = "release"\ngate = ${JSON.stringify(commands)}\nenv = { BUILD_WORKTREE = ${JSON.stringify(checkout.worktree)} }\n`);
      const journal = createJournal({ context: ctx, git: run, fleet: () => fleet });
      const verified = await verifyBuild(ctx, run, journal);
      expect(verified).toMatchObject({ result: "pass", attempt: "G1", head: before, commands });
      expect(readFileSync(verified.log!, "utf8")).toContain("worktree verified\n");
      const records: ExitProgressRecord[] = [];
      const control = createExit({ context: ctx, git: run, journal, fleet: () => fleet,
        save: record => records.push(record), changed() {} });
      const result = await control.choose("land");
      expect(result.phase).toBe("landed");
      const landed = (await git(repo, "rev-parse", "release")).trim();
      expect(landed).not.toBe(before);
      expect((await git(repo, "rev-parse", "release^")).trim()).toBe(baseTip);
      expect(readFileSync(join(repo, "code.txt"), "utf8")).toBe("verified\n");
      expect(records.map(record => record.progress?.phase)).toEqual(["rebasing", "verifying", "landing", "landed"]);
      expect(result.record).toMatchObject({ landed, baseBranch: "release", gateAttempt: "G2" });
      expect(journal.record().verification).toMatchObject([{ head: before, result: "pass" }, { head: landed, result: "pass" }]);
    } finally { rmSync(dir, { recursive: true, force: true }); }
  });
  test("a failing full gate leaves the session open and never lands", async () => {
    const f = fixture({ failGate: true });
    try {
      const state = await f.control.choose("land");
      expect(state.phase).toBe("failed"); expect(state.detail).toContain("failing-check");
      expect(readFileSync(f.journal.latestGate()!.log!, "utf8")).toContain("failing-check");
      expect(f.events).not.toContain("land");
      expect(f.journal.record().outcome.state).toBe("open");
      expect((await f.control.choose("iterate")).phase).toBe("none");
    } finally { f.close(); }
  });
  test("a failing landing gate is handed to the session and Land resumes on its fix without the owner", async () => {
    const f = fixture({ failGates: 2, repair: (_prompt, session) => { session.commit(); return { aborted: false }; } });
    try {
      const state = await f.control.choose("land");
      expect(state.phase).toBe("landed");
      expect(f.events).toEqual(["rebase", "gate", "repair", "rebase", "gate", "repair", "rebase", "gate", "land"]);
      const [first, second] = f.journal.record().verification;
      expect(f.prompts[0]).toContain(`rebased head ${head}`); expect(f.prompts[0]).toContain("Failed command: echo failing-check; exit 1");
      expect(f.prompts[0]).toContain(first!.log!); expect(f.prompts[0]).toContain(`repair 1 of ${LAND_REPAIR_ROUNDS}`);
      expect(f.prompts[0]).not.toContain("build_review");
      expect(f.prompts[1]).toContain(second!.log!);
      expect(state.record).toMatchObject({ landed: "2".repeat(40), gateAttempt: "G3" });
      expect(f.records.map(record => (record as ExitProgressRecord).progress?.phase)).toContain("repairing");
      expect(f.journal.record().decisions.filter(decision => decision.kind === "exit")).toHaveLength(1);
    } finally { f.close(); }
  });
  test("automatic landing repair is bounded and stops without a new commit or when the turn is stopped", async () => {
    const endless = fixture({ failGate: true, repair: (_prompt, session) => { session.commit(); return { aborted: false }; } });
    try {
      const state = await endless.control.choose("land");
      const last = endless.journal.latestGate()!;
      expect(last.attempt).toBe(`G${LAND_REPAIR_ROUNDS + 1}`);
      expect(state.phase).toBe("failed"); expect(state.detail).toContain(last.log!);
      expect(endless.events.filter(event => event === "repair")).toHaveLength(LAND_REPAIR_ROUNDS);
      expect(endless.events).not.toContain("land");
    } finally { endless.close(); }
    for (const [repair, why] of [[() => ({ aborted: false }), "made no new commit"], [(_p, s) => { s.commit(); return { aborted: true }; }, "was stopped"],
      [() => { throw new Error("host_closing"); }, "host_closing"]] as const satisfies readonly (readonly [Repair, string])[]) {
      const f = fixture({ failGates: 1, repair });
      try {
        const state = await f.control.choose("land");
        expect(state.phase).toBe("failed"); expect(state.detail).toContain(f.journal.latestGate()!.log!); expect(state.detail).toContain(why);
        expect(f.events).toEqual(["rebase", "gate", "repair"]);
      } finally { f.close(); }
    }
  });
  test("under review policy a repaired head lands only with a fresh approving review or the owner's override", async () => {
    for (const [approve, override, landed] of [[false, false, false], [true, false, true], [false, true, true]] as const) {
      const f = fixture({ policy: "review", failGates: 1, repair: (_prompt, session) => { session.commit(); if (approve) session.approve(); return { aborted: false }; } });
      try {
        f.journal.review({ ...child("review-0", "review"), verdict: { verdict: "approve", summary: "Approved", findings: [], head, at: 1000 } }, head);
        const state = await f.control.choose("land", override ? { overrideReason: "Owner accepts the risk" } : undefined);
        expect(state.phase).toBe(landed ? "landed" : "failed");
        expect(f.prompts[0]!.includes("build_review")).toBe(!override);
        expect(f.journal.record().decisions.some(decision => decision.kind === "override" && decision.text.startsWith("Repair 1 commits"))).toBe(override);
        if (!landed) { expect(state.detail).toContain("no approving review covers the repaired head"); expect(f.events).not.toContain("land"); }
      } finally { f.close(); }
    }
  });
  test("a base move during verification leaves landing for the next owner choice", async () => {
    const f = fixture({ stale: true });
    try {
      const result = await f.control.choose("land");
      expect(result.phase).toBe("failed");
      expect(result.detail).toContain("base main moved");
      expect(f.events).toEqual(["rebase", "gate"]);
    } finally { f.close(); }
  });
  test("review policy requires an approving round or an owner's recorded override", async () => {
    const f = fixture({ policy: "review" });
    try {
      await f.control.summary();
      const option = f.control.options().find(option => option.choice === "land");
      expect(option?.unavailable).toBeUndefined(); expect(typeof option?.needsReason).toBe("string");
      expect((await f.control.choose("land")).phase).toBe("failed"); expect(f.events).not.toContain("rebase");
      expect((await f.control.choose("land", { overrideReason: "Owner accepts the risk" })).phase).toBe("landed");
      expect(f.journal.record()).toMatchObject({ outcome: { override_reason: "Owner accepts the risk" }, decisions: [
        { kind: "exit", text: "land" }, { kind: "exit", text: "land" }, { kind: "override", text: "Owner accepts the risk" },
      ] });
    } finally { f.close(); }
  });
  test("a rebase conflict aborts and names conflicted paths", async () => {
    const f = fixture({ conflict: true });
    try {
      const state = await f.control.choose("land");
      expect(state.phase).toBe("failed"); expect(state.detail).toContain("src/parser.ts");
      expect(f.events.filter(event => event !== "record")).toEqual(["rebase", "abort"]);
    } finally { f.close(); }
  });
  test("a changed rebased patch needs review again even after an approving round", async () => {
    const f = fixture({ policy: "review", patchChanged: true });
    try {
      f.journal.review({ ...child("review-1", "review"), verdict: { verdict: "approve", summary: "Approved", findings: [], head, at: 1000 } }, head);
      const state = await f.control.choose("land");
      expect(state.phase).toBe("failed"); expect(state.detail).toContain("build_review");
      expect(f.events).not.toContain("gate");
    } finally { f.close(); }
  });
  test("Keep retains its outcome in the private session journal", async () => {
    const f = fixture();
    try {
      const state = await f.control.choose("keep");
      expect(state.phase).toBe("kept");
      expect(f.events).toEqual([]);
      expect(f.journal.record()).toMatchObject({ outcome: { state: "kept", head }, decisions: [{ kind: "exit", text: "keep" }] });
      expect(statSync(join(f.ctx.dir, "journal.json")).mode & 0o777).toBe(0o600);
    } finally { f.close(); }
  });
  test("landing admission follows workspace, policy, clean commits, live children and the current branch", async () => {
    const dir = mkdtempSync(join(tmpdir(), "build-admission-"));
    try {
      mkdirSync(join(dir, ".corral-next"));
      writeFileSync(join(dir, ".corral-next", "corral.toml"), "[projects.app]\nbase = \"main\"\n");
      for (const variant of ["outside", "project", "none", "empty", "dirty", "live", "base", "detached"] as const) {
        const initial = context(variant === "none" ? "none" : "gate");
        const ctx = { ...initial, dir, session: { ...initial.session, here: true,
          workspaceRoot: variant === "outside" ? undefined : dir, ...(variant === "project" ? { project: undefined } : {}) } };
        const snapshot = variant === "live" ? { ...fleet, children: [{ ...child(), state: "running" as const }] } : fleet;
        const gitRun = git(variant === "dirty", variant === "empty" ? 0 : 2, [], variant === "base" ? "main" : variant === "detached" ? null : "build/parser");
        const journal = createJournal({ context: ctx, git: gitRun, fleet: () => snapshot });
        const control = createExit({ context: ctx, git: gitRun, journal, fleet: () => snapshot, save() {}, changed() {} });
        await control.summary(); expect(typeof control.options().find(option => option.choice === "land")?.unavailable).toBe("string"); control.close();
      }
    } finally { rmSync(dir, { recursive: true, force: true }); }
  });

  test("summary includes working-tree numstat and fleet ownership deviations", async () => {
    const calls: string[][] = [];
    const child: FleetSnapshot["children"][number] = {
      id: "child-1", kind: "slice", call: "call-1", title: "Parser", brief: "Build parser", owns: ["src/parser.ts"], after: [],
      profile: "default", model: "provider/model", state: "done", createdAt: 1000, startedAt: 1000, endedAt: 4000,
      edits: [{ path: "src/parser.ts", outside: false, at: 2000 }, { path: "docs/usage.md", outside: true, at: 3000 }],
      usage: { input: 10, output: 20, cost: 0.25, turns: 1 }, steers: [], pulse: [], history: "d", messages: [],
    };
    const summary = await summarize(git(true, 2, calls), context().session, { ...fleet, children: [child] });
    expect(summary).toMatchObject({ head, base, commits: 2, files: 2, added: 4, removed: 2, dirty: true });
    expect(calls.find(argv => argv[0] === "diff")).toEqual(["diff", "--numstat", base, "--"]);
    expect(summary.slices).toEqual([{ id: "child-1", state: "done", seconds: 3, cost: 0.25 }]);
    expect(summary.outside).toEqual([{ path: "docs/usage.md", by: ["child-1"] }]);
  });
});

test("bare Land only rebases and fast-forwards Git in the clean primary checkout on its starting branch", async () => {
  for (const [primaryBranch, tracked, succeeds] of [["main", "", true], ["other", "", false], ["main", " M tracked\n", false]] as const) {
    const dir = mkdtempSync(join(tmpdir(), "bare-exit-"));
    const original = context("review");
    const ctx: BuildContext = { ...original, dir, session: { ...original.session,
      workspaceRoot: undefined, project: undefined, bare: true, repository: true, baseBranch: "main" } };
    const writes: { argv: string[]; cwd: string }[] = [], phases: string[] = [];
    const fallback = git();
    const gitRun: Run = async (argv, options) => {
      if (argv[0] === "worktree") return { code: 0, stdout: "worktree /primary\0HEAD ignored\0branch refs/heads/main\0\0", stderr: "" };
      if (argv[0] === "symbolic-ref") {
        expect(options.cwd).toBe("/primary");
        return { code: 0, stdout: primaryBranch, stderr: "" };
      }
      if (argv[0] === "status" && options.cwd === "/primary") return { code: 0, stdout: tracked, stderr: "" };
      if (["rebase", "merge"].includes(argv[0]!)) writes.push({ argv: [...argv], cwd: options.cwd });
      return fallback(argv, options);
    };
    const journal = createJournal({ context: ctx, git: gitRun, fleet: () => fleet });
    const exit = createExit({ context: ctx, git: gitRun, journal, fleet: () => fleet,
      save() {}, changed() { phases.push(exit.state().phase); } });
    try {
      await exit.summary();
      expect(exit.options().find(option => option.choice === "land")?.needsReason).toBeUndefined();
      const result = await exit.choose("land");
      expect(result.phase).toBe(succeeds ? "landed" : "failed");
      if (succeeds) {
        expect(writes).toEqual([{ argv: ["rebase", "main"], cwd: ctx.session.worktree },
          { argv: ["merge", "--ff-only", head], cwd: "/primary" }]);
        expect(phases).toContain("rebasing"); expect(phases).toContain("landing");
        expect(phases).not.toContain("verifying");
        expect(result.record).toMatchObject({ landed: head, baseBranch: "main" });
        expect(result.record?.gateAttempt).toBeUndefined();
      } else {
        expect(writes).toEqual([]);
        expect(result.detail).toContain(primaryBranch === "other" ? "must be on main" : "tracked changes");
      }
    } finally { exit.close(); rmSync(dir, { recursive: true, force: true }); }
  }
});

test("bare sessions outside Git offer only Keep and Iterate", async () => {
  const dir = mkdtempSync(join(tmpdir(), "bare-no-git-"));
  const original = context();
  const ctx: BuildContext = { ...original, dir, session: { ...original.session,
    workspaceRoot: undefined, project: undefined, bare: true, repository: false, branch: null, base: null } };
  const refuse: Run = async () => { throw new Error("no command needed outside Git"); };
  const journal = createJournal({ context: ctx, git: refuse, fleet: () => fleet });
  const exit = createExit({ context: ctx, git: refuse, journal, fleet: () => fleet, save() {}, changed() {} });
  try {
    await exit.summary();
    expect(exit.options().map(option => option.choice)).toEqual(["keep", "iterate"]);
    expect((await exit.choose("keep")).phase).toBe("kept");
    expect((await exit.choose("iterate")).phase).toBe("none");
  } finally { exit.close(); rmSync(dir, { recursive: true, force: true }); }
});

test("JSON parse failures include corral's stderr", async () => {
  await expect(json(async () => ({ code: 0, stdout: "not JSON", stderr: "command diagnostic" }), ["show", "T-1", "--json"], "/workspace")).rejects.toThrow("command diagnostic");
});

test("build records retain intent, evidence, resolutions, slice attribution, messages and priced usage across resume", async () => {
  const dir = mkdtempSync(join(tmpdir(), "build-record-"));
  const sessionFile = join(dir, "parent.jsonl");
  const initial = context();
  const ctx = { ...initial, dir, session: { ...initial.session, sessionFile } };
  writeFileSync(sessionFile, [
    { type: "message", message: { role: "assistant", usage: { input: 3, cacheRead: 2, output: 4, cost: { total: 0.5 } } } },
    { type: "compaction", usage: { input: 1, output: 1, cost: { total: 0.25 } } },
  ].map(entry => JSON.stringify(entry)).join("\n") + "\n");
  const snapshot: FleetSnapshot = { ...fleet, children: [{ ...child(), steers: ["Use the strict parser"], steeredAt: [2000] }, child("review-1", "review")],
    mail: [{ seq: 1, at: 1000, from: "parser", to: "parent", text: "Ready", delivery: "delivered" }] };
  const underlying = git();
  const gitRun: Run = async (argv, options) => argv[0] === "log"
    ? { code: 0, stdout: `${head}\0Implement parser\0Slice: parser\n\0\n`, stderr: "" } : underlying(argv, options);
  try {
    const journal = createJournal({ context: ctx, git: gitRun, fleet: () => snapshot, now: () => 1000 });
    journal.noteOwnerInput("Build a parser\nwith clear diagnostics");
    expect(journal.record().title).toBe("Build a parser");
    journal.finish("Build a diagnostic parser");
    journal.gate(gateResult());
    journal.review({ ...snapshot.children[1]!, verdict: { verdict: "changes", summary: "One defect", head, at: 2000,
      findings: [{ severity: "major", path: "src/parser.ts", line: 3, text: "Empty input crashes" }] } }, head);
    expect(() => journal.resolve(1, "F1", { state: "declined" })).toThrow();
    journal.resolve(1, "F1", { state: "fixed", commit: head });
    await journal.refresh();
    const record = journal.record();
    expect(record).toMatchObject({ version: 1, title: "Build a diagnostic parser", intent: [{ text: "Build a parser\nwith clear diagnostics" }, { text: "Use the strict parser" }],
      commits: [{ sha: head, subject: "Implement parser", slice: "parser" }], diff: { commits: 2, files: 2, added: 4, removed: 2 },
      slices: [{ id: "parser", kind: "slice", model: "provider/model", outside: ["docs/usage.md"], report: "done" }, { kind: "review" }],
      verification: [{ attempt: "G1", result: "pass" }],
      reviews: [{ verdict: "changes", head, model: "provider/model", findings: [{ id: "F1", text: "Empty input crashes", resolution: { state: "fixed", commit: head } }] }],
      messages: [{ from: "parser", to: "parent", text: "Ready" }],
      usage: { parent: { input: 4, output: 5, cost: 0.75 }, children: { input: 10, output: 20, cost: 0.25 }, reviewers: { input: 10, output: 20, cost: 0.25 },
        total: { input: 24, output: 45, cost: 1.25 } } });
    expect("commands" in record.verification[0]!).toBeFalse();
    expect("receipt" in record.verification[0]!).toBeFalse();
    const resumed = createJournal({ context: ctx, git: gitRun, fleet: () => fleet });
    expect(resumed.record()).toEqual(record);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("record caps bound wire content and missing prices never produce partial total cost", async () => {
  const dir = mkdtempSync(join(tmpdir(), "build-record-caps-"));
  const ctx = { ...context(), dir };
  const snapshot: FleetSnapshot = { ...fleet, children: [{ ...child(), report: "r".repeat(8193), usage: { input: 1, output: 2, turns: 1 } }],
    mail: Array.from({ length: 501 }, (_, seq) => ({ seq, at: seq, from: "parent", to: "parser", text: `${seq}:` + "m".repeat(2049), delivery: "delivered" as const })) };
  const underlying = git();
  const gitRun: Run = async (argv, options) => argv[0] === "log" ? { code: 0, stdout: Array.from({ length: 201 }, (_, index) => `${index}\0Commit ${index}\0\0`).join("\n"), stderr: "" } : underlying(argv, options);
  try {
    const journal = createJournal({ context: ctx, git: gitRun, fleet: () => snapshot });
    for (let n = 0; n < 33; n++) journal.noteOwnerInput("i".repeat(4097));
    await journal.refresh();
    const record = journal.record();
    expect(record.title).toHaveLength(120);
    expect(record.intent).toHaveLength(32); expect(record.intent[0]!.text).toHaveLength(4096);
    expect(record.commits).toHaveLength(200);
    expect(record.slices[0]!.report).toHaveLength(8192);
    expect(record.messages).toHaveLength(500); expect(record.messages[0]!.text.startsWith("1:")).toBeTrue(); expect(record.messages[0]!.text).toHaveLength(2048);
    expect(record.usage.children.cost).toBeUndefined(); expect(record.usage.total.cost).toBeUndefined();
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("patch digest subprocesses do not reject patches larger than the text output cap", async () => {
  const size = 256 * 1024 + 1;
  const result = await engineRun(process.execPath)(["-e", `process.stdout.write("x".repeat(${size}))`], { cwd: process.cwd(), stdoutDigest: "sha256" });
  expect(result.code).toBe(0);
  expect(result.stdout).toBe(createHash("sha256").update("x".repeat(size)).digest("hex"));
});
