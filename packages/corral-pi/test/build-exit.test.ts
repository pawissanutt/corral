import { describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { engineRun, gitRun, checked, json, type Run } from "../src/build/corral-cli.ts";
import { createExit, type ExitProgressRecord } from "../src/build/exit.ts";
import { createJournal } from "../src/build/record.ts";
import { summarize } from "../src/build/git.ts";
import type { BuildGateResult } from "../src/build/record-types.ts";
import { verifyBuild } from "../src/build/gate.ts";
import { prepareWorktree } from "../src/build/worktree.ts";
import { within } from "./support/wait.ts";
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
      : argv[0] === "diff" ? argv.includes("--numstat") ? "4\t2\tsrc/parser.ts\n-\t-\timage.png\n" : argv.includes("--name-only") ? "" : "patch"
      : argv[0] === "status" ? (dirty ? " M src/parser.ts\n" : "") : "";
    return { code: 0, stdout, stderr: "" };
  };
}
function gateResult(): BuildGateResult {
  return { attempt: "G1", head, result: "pass", commands: ["check"], started: "2026-01-01T00:00:00Z", ended: "2026-01-01T00:01:00Z", log: "/logs/G1.log" };
}
/** Stateful refs, with a real verification journal and disposable gate directory. */
function fixture(settings: { policy?: ExitPolicy; conflict?: boolean; dirtyBase?: boolean; onStaleOffer?: () => Promise<void> } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "build-exit-"));
  const start = context(settings.policy);
  const ctx = { ...start, dir, session: { ...start.session, workspaceRoot: dir, worktree: dir } };
  mkdirSync(join(dir, ".corral-next"));
  const setGate = (command: string, env = "") => writeFileSync(join(dir, ".corral-next", "corral.toml"),
    '[projects.app]\nbase="main"\ngate=[' + JSON.stringify(command) + ']\n' + env);
  setGate("true");
  const repo = { candidate: head, tip: base, dirty: false, ancestor: !settings.conflict };
  const calls: string[][] = [];
  const ok = (stdout: string) => ({ code: 0, stdout, stderr: "" });
  const underlying = git();
  const run: Run = async (argv, options) => {
    calls.push([...argv]);
    if (argv.includes("--absolute-git-dir")) return ok(dir);
    if (argv.includes("--git-common-dir")) return ok(dir);
    if (argv.includes("--show-prefix")) return ok("");
    if (argv[0] === "rev-parse" && !argv.includes("--abbrev-ref")) return ok(argv.at(-1) === "HEAD" && options.cwd !== "/primary" ? repo.candidate : repo.tip);
    if (argv[0] === "symbolic-ref") return ok(options.cwd === "/primary" ? "refs/heads/main" : "refs/heads/build/parser");
    if (argv[0] === "merge-base") return argv.includes("--is-ancestor") ? { code: repo.ancestor ? 0 : 1, stdout: "", stderr: "" } : ok(repo.ancestor ? repo.tip : base);
    if (argv.includes("rebase") && settings.conflict) {
      if (argv.includes("--abort")) return ok("");
      mkdirSync(join(dir, "rebase-merge"));
      return { code: 1, stdout: "", stderr: "conflict" };
    }
    if (argv.includes("--diff-filter=U")) return ok("src/parser.ts\n");
    if (argv.includes("--name-only") && settings.dirtyBase) return ok("src/parser.ts\n");
    if (argv[0] === "worktree") return ok("worktree /primary\0HEAD " + repo.tip + (settings.dirtyBase ? "\0branch refs/heads/main\0\0" : "\0branch refs/heads/other\0\0"));
    if (argv[0] === "status" && options.cwd !== "/primary") return ok(repo.dirty ? " M src/parser.ts\n" : "");
    if (argv[0] === "update-ref") {
      if (argv.at(-1) !== repo.tip) return { code: 1, stdout: "", stderr: "old tip changed" };
      repo.tip = argv.at(-2)!;
    }
    return underlying(argv, options);
  };
  const journal = createJournal({ context: ctx, git: run, fleet: () => fleet });
  const control = createExit({ context: ctx, git: run, journal, fleet: () => fleet, requestFinish: settings.onStaleOffer, save() {}, changed() {} });
  const offer = async () => {
    const candidate = await control.prepare();
    await verifyBuild(ctx, run, journal);
    control.open(candidate ?? null);
    return control.options().find(option => option.choice === "land")?.authorization;
  };
  return { ctx, journal, control, run, offer, repo, setGate, calls, close() { control.close(); rmSync(dir, { recursive: true, force: true }); } };
}
function child(id = "parser", kind: ChildSnapshot["kind"] = "slice"): ChildSnapshot {
  return { id, kind, call: "call", title: "Parser", brief: "Build parser", owns: ["src/parser.ts"], after: [], profile: "default", model: "provider/model",
    state: "done", createdAt: 1000, startedAt: 1000, endedAt: 4000,
    edits: [{ path: "docs/usage.md", outside: true, at: 2000 }], usage: { input: 10, output: 20, cost: 0.25, turns: 1 },
    steers: [], pulse: [], history: "d", messages: [], report: "done" };
}

describe("build exits", () => {
  test("preparation rebases first; only the owner advances the base to the exact gated commit", async () => {
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
      const records: ExitProgressRecord[] = [];
      const control = createExit({ context: ctx, git: run, journal, fleet: () => fleet,
        save: record => records.push(record), changed() {} });
      const prepared = await control.prepare();
      expect(prepared?.base).toBe(baseTip);
      expect(prepared?.head).not.toBe(before);
      const verified = await verifyBuild(ctx, run, journal);
      expect(verified).toMatchObject({ result: "pass", attempt: "G1", head: prepared!.head, commands });
      expect(readFileSync(verified.log!, "utf8")).toContain("worktree verified\n");
      control.open(prepared!);
      const option = control.options().find(option => option.choice === "land")!;
      const result = await control.choose("land", { authorization: option.authorization });
      expect(result.phase).toBe("landed");
      const landed = (await git(repo, "rev-parse", "release")).trim();
      expect(landed).not.toBe(before);
      expect((await git(repo, "rev-parse", "release^")).trim()).toBe(baseTip);
      expect(readFileSync(join(repo, "code.txt"), "utf8")).toBe("verified\n");
      expect(result.record).toMatchObject({ landed, baseBranch: "release", gateAttempt: "G1" });
      expect(journal.record().verification).toHaveLength(1);
    } finally { rmSync(dir, { recursive: true, force: true }); }
  });
  test("a failed gate cannot authorize or repair a candidate, even with a review override", async () => {
    const f = fixture({ policy: "review" });
    try {
      f.setGate("exit 1");
      const authorization = await f.offer();
      expect(f.journal.latestGate()?.result).toBe("fail");
      expect(f.control.options().find(o => o.choice === "land")?.unavailable).toBeDefined();
      await f.control.choose("land", { authorization, overrideReason: "Accept review risk" });
      expect(f.calls.some(args => ["merge", "update-ref"].includes(args[0]!))).toBeFalse();
      expect(f.repo.tip).toBe(base);
      expect((await f.control.choose("iterate")).phase).toBe("none");
    } finally { f.close(); }
  });

  for (const change of ["candidate", "base", "dirty", "commands", "env", "old offer"]) test(`stale ${change} cannot fast-forward or run a new gate`, async () => {
    const f = fixture();
    try {
      const authorization = await f.offer();
      expect(authorization).toBeDefined();
      if (change === "candidate") f.repo.candidate = "d".repeat(40);
      if (change === "base") f.repo.tip = "c".repeat(40);
      if (change === "dirty") f.repo.dirty = true;
      if (change === "commands") f.setGate("false");
      if (change === "env") f.setGate("true", 'env = { BUILD_FLAG = "changed" }');
      if (change === "old offer") {
        await f.control.choose("iterate");
        await f.offer();
      }
      const tip = f.repo.tip, count = f.journal.record().verification.length;
      const at = f.calls.length;
      const result = await f.control.choose("land", { authorization });
      expect(result.phase).not.toBe("landed");
      expect(f.journal.record().verification).toHaveLength(count);
      expect(f.calls.slice(at).some(args => ["rebase", "merge", "update-ref"].includes(args[0]!))).toBeFalse();
      expect(f.repo.tip).toBe(tip);
    } finally { f.close(); }
  });

  test("a moved base prepares a replacement offer but never carries the old consent", async () => {
    const again = Promise.withResolvers<void>();
    let refreshes = 0;
    const f = fixture({ onStaleOffer: async () => {
      refreshes++;
      const candidate = await f.control.prepare();
      f.control.open(candidate ?? null);
      again.resolve();
    } });
    try {
      const authorization = await f.offer();
      f.repo.tip = "c".repeat(40);
      expect((await f.control.choose("land", { authorization })).phase).not.toBe("landed");
      await within(again.promise, 3000, "replacement offer");
      const next = f.control.options().find(option => option.choice === "land")?.authorization;
      expect(next).toBeDefined(); expect(next).not.toBe(authorization);
      expect((await f.control.choose("land", { authorization })).phase).not.toBe("landed");
      expect(f.repo.tip).toBe("c".repeat(40));
      expect(refreshes).toBe(1);
      expect((await f.control.choose("land", { authorization: next })).phase).toBe("landed");
    } finally { f.close(); }
  });
  test("an approval of the same head against a different base is not approval of the offered diff", async () => {
    const f = fixture({ policy: "review" });
    try {
      const approve = (tip: string) => f.journal.review({ ...child("review-1", "review"), verdict: {
        verdict: "approve", summary: "Reviewed", findings: [], head, at: 1000,
      } }, head, tip);
      approve("c".repeat(40));
      await f.offer();
      expect(f.control.options().find(o => o.choice === "land")?.needsReason).toBeDefined();
      approve(base);
      expect(f.control.options().find(o => o.choice === "land")?.needsReason).toBeUndefined();
    } finally { f.close(); }
  });
  test("review override is explicit and recorded for the offered, gated head", async () => {
    const f = fixture({ policy: "review" });
    try {
      let authorization = await f.offer();
      expect(f.control.options().find(o => o.choice === "land")?.needsReason).toBeDefined();
      expect((await f.control.choose("land", { authorization })).phase).not.toBe("landed");
      authorization = await f.offer();
      expect((await f.control.choose("land", { authorization, overrideReason: "Owner accepts the risk" })).phase).toBe("landed");
      expect(f.journal.record().outcome.override_reason).toBe("Owner accepts the risk");
    } finally { f.close(); }
  });

  test("preparation aborts a conflicted rebase and leaves a useful path without moving main", async () => {
    const f = fixture({ conflict: true });
    try {
      f.repo.tip = "c".repeat(40);
      expect(await f.control.prepare()).toBeUndefined();
      expect(f.control.preparationDetail()).toContain("src/parser.ts");
      expect(f.calls.filter(args => args[0] === "rebase" && args[1] === "--abort")).toHaveLength(1);
      expect(f.repo.tip).toBe("c".repeat(40));
      expect(f.control.options().find(o => o.choice === "land")?.unavailable).toBeDefined();
    } finally { f.close(); }
  });
  test("overlapping changes in the owner's checkout block preparation without touching that checkout", async () => {
    const f = fixture({ dirtyBase: true });
    try {
      expect(await f.control.prepare()).toBeUndefined();
      expect(f.control.preparationDetail()).toContain("/primary");
      expect(f.calls.some(args => ["merge", "update-ref", "stash", "reset"].includes(args[0]!))).toBeFalse();
    } finally { f.close(); }
  });
  test("a later failed receipt invalidates a previously offered pass", async () => {
    const f = fixture();
    try {
      const authorization = await f.offer();
      expect(authorization).toBeDefined();
      const previous = f.journal.latestGate()!;
      f.journal.gate({ ...previous, attempt: "G2", result: "fail", failed_command: "true" });
      expect(f.journal.passedGate(head, previous.input_fingerprint!)).toBeUndefined();
      expect((await f.control.choose("land", { authorization })).phase).not.toBe("landed");
      expect(f.repo.tip).toBe(base);
    } finally { f.close(); }
  });
  test("Keep retains its outcome in the private session journal", async () => {
    const f = fixture();
    try {
      const state = await f.control.choose("keep");
      expect(state.phase).toBe("kept");
      expect(f.calls.some(args => ["rebase", "merge", "update-ref"].includes(args[0]!))).toBeFalse();
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

test("bare preparation respects the primary checkout; Land never rebases or runs a gate", async () => {
  for (const variant of ["ready", "switched", "dirty", "ignored obstruction", "sparse"] as const) {
    const dir = mkdtempSync(join(tmpdir(), "bare-exit-"));
    const repo = join(dir, "primary"), worktree = join(dir, "build");
    mkdirSync(repo);
    const run = gitRun();
    const git = (cwd: string, ...args: string[]) => checked(run, args, cwd);
    const writes: string[][] = [], phases: string[] = [];
    try {
      await git(repo, "init", "-b", "main");
      await git(repo, "config", "user.name", "Build Fixture");
      await git(repo, "config", "user.email", "build@example.test");
      writeFileSync(join(repo, "tracked"), "base");
      writeFileSync(join(repo, ".gitignore"), "ignored\n");
      await git(repo, "add", "tracked", ".gitignore");
      await git(repo, "commit", "-m", "base");
      const base = (await git(repo, "rev-parse", "HEAD")).trim();
      await git(repo, "worktree", "add", "-b", "build/parser", worktree);
      writeFileSync(join(worktree, "ignored"), "candidate");
      await git(worktree, "add", "-f", "ignored");
      await git(worktree, "commit", "-m", "candidate");
      const head = (await git(worktree, "rev-parse", "HEAD")).trim();
      if (variant === "switched") await git(repo, "switch", "-c", "other");
      if (variant === "dirty") writeFileSync(join(repo, "tracked"), "local work");
      if (variant === "ignored obstruction") writeFileSync(join(repo, "ignored"), "local ignored work");
      if (variant === "sparse") await git(repo, "sparse-checkout", "init", "--cone");
      const original = context("review");
      const ctx: BuildContext = { ...original, dir: join(dir, "session"), session: { ...original.session,
        repoRoot: repo, worktree, base, workspaceRoot: undefined, project: undefined, bare: true, repository: true, baseBranch: "main" } };
      const gitRun: Run = (argv, options) => {
        if (["rebase", "merge"].includes(argv[0]!)) writes.push([...argv]);
        return run(argv, options);
      };
      const journal = createJournal({ context: ctx, git: gitRun, fleet: () => fleet });
      const exit = createExit({ context: ctx, git: gitRun, journal, fleet: () => fleet,
        save() {}, changed() { phases.push(exit.state().phase); } });
      try {
        const candidate = await exit.prepare();
        if (variant !== "ready") {
          expect(candidate).toBeUndefined();
          expect(exit.preparationDetail()).toMatch({ switched: /main/, dirty: /changes/,
            "ignored obstruction": /untracked or ignored obstruction/, sparse: /sparse checkout/ }[variant]);
          if (variant === "ignored obstruction") expect(readFileSync(join(repo, "ignored"), "utf8")).toBe("local ignored work");
          expect((await git(repo, "rev-parse", "refs/heads/main")).trim()).toBe(base);
        } else {
          exit.open(candidate!);
          const authorization = exit.options().find(option => option.choice === "land")?.authorization;
          const result = await exit.choose("land", { authorization });
          expect(result.phase).toBe("landed");
          expect((await git(repo, "rev-parse", "refs/heads/main")).trim()).toBe(head);
          expect(result.record).toMatchObject({ landed: head, baseBranch: "main" });
          expect(result.record?.gateAttempt).toBeUndefined();
        }
        expect(phases).not.toContain("verifying");
        expect(writes).toEqual([]);
      } finally { exit.close(); }
    } finally { rmSync(dir, { recursive: true, force: true }); }
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

test("request titles persist, fall back for empty input, and preserve an explicit title", () => {
  const dir = mkdtempSync(join(tmpdir(), "build-title-"));
  const initial = context();
  const ctx = { ...initial, dir };
  try {
    const journal = createJournal({ context: ctx, git: git(), fleet: () => fleet });
    expect(journal.record().title).toBe("Build session");
    const titlePath = join(dir, "title.json");
    mkdirSync(titlePath);
    journal.noteOwnerInput("  \n  ");
    expect(journal.record().title).toBe("Build session");
    journal.noteOwnerInput("\n  Fix   the parser\nMore detail");
    expect(JSON.parse(readFileSync(join(dir, "journal.json"), "utf8")).title).toBe("Fix the parser");
    rmSync(titlePath, { recursive: true });
    journal.noteOwnerInput("Another request");
    expect(JSON.parse(readFileSync(titlePath, "utf8"))).toEqual({ title: "Fix the parser" });
    expect(createJournal({ context: ctx, git: git(), fleet: () => fleet }).record().title).toBe("Fix the parser");
    const unicode = createJournal({ context: { ...ctx, dir: join(dir, "unicode") }, git: git(), fleet: () => fleet });
    unicode.noteOwnerInput("a".repeat(118) + "😀rest");
    expect(unicode.record().title).toBe("a".repeat(118) + "😀…");
    expect(readFileSync(join(dir, "unicode", "journal.json"), "utf8")).not.toContain("\\ud83d");

    const named = { ...ctx, dir: join(dir, "named"), session: { ...ctx.session, title: "My chosen title" } };
    const namedJournal = createJournal({ context: named, git: git(), fleet: () => fleet });
    namedJournal.noteOwnerInput("A different request");
    namedJournal.finish("Generated finish title");
    expect(createJournal({ context: named, git: git(), fleet: () => fleet }).record().title).toBe("My chosen title");
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
