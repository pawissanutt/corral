import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { gitRun, type Run } from "../src/build/corral-cli.ts";
import { createExit } from "../src/build/exit.ts";
import { gateInputFingerprint, verifyBuild } from "../src/build/gate.ts";
import { checkIndexVisibility } from "../src/build/index-visibility.ts";
import { createJournal } from "../src/build/record.ts";
import { createBuildEvidence } from "../src/build/tools.ts";
import type { BuildContext, ChildSnapshot, ChildSpec } from "../src/build/types.ts";
import { within } from "./support/wait.ts";

function fixture(tap?: (args: readonly string[], result: Awaited<ReturnType<Run>>) => Promise<void>, evidenceChanged = () => {}) {
  const root = mkdtempSync(join(tmpdir(), "build-gate-identity-"));
  const repo = join(root, "repo"), source = join(repo, "source.txt");
  mkdirSync(repo);
  const git = (...args: string[]) => execFileSync("git", ["-C", repo, ...args], { encoding: "utf8" }).trim();
  git("init", "-q", "-b", "main");
  git("config", "user.name", "Gate Test");
  git("config", "user.email", "gate@example.test");
  writeFileSync(source, "base\n");
  git("add", "source.txt");
  git("commit", "-qm", "Base");
  const base = git("rev-parse", "HEAD");
  git("switch", "-qc", "build/test");
  writeFileSync(source, "candidate\n");
  git("commit", "-qam", "Candidate");
  const head = git("rev-parse", "HEAD");
  const at = new Date().toISOString();
  const context: BuildContext = { corral: "unused", dir: join(root, "session"), settings: { exitPolicy: "gate", maxParallel: 2, profiles: [] },
    session: { version: 1, id: "B-gate", name: "test", created: at, updated: at, repoRoot: repo, worktree: repo, here: true,
      base, branch: "build/test", project: "app", workspaceRoot: root } };
  mkdirSync(join(root, ".corral-next"));
  const setGate = (commands: string[], mode = "configured-first") => writeFileSync(join(root, ".corral-next/corral.toml"),
    `[projects.app]\nbase = "main"\ngate = ${JSON.stringify(commands)}\nenv = { MODE = ${JSON.stringify(mode)} }\n`);
  const runGit = gitRun();
  const run: Run = async (args, options) => {
    const result = await runGit(args, options);
    await tap?.(args, result);
    return result;
  };
  let child: ChildSnapshot | undefined;
  const started = Promise.withResolvers<ChildSnapshot>();
  const completed = Promise.withResolvers<ChildSnapshot>();
  const snapshot = () => ({ children: child ? [child] : [], mail: [], maxParallel: 2 });
  const finishReview = () => {
    if (!child) return false;
    child = { ...child, state: "done", verdict: { verdict: "approve", head, at: Date.now(), summary: "Reviewed", findings: [] } };
    completed.resolve(child);
    return true;
  };
  const fleet = { snapshot, profiles: () => [],
    async start(call: string, specs: readonly ChildSpec[]) {
      const { id, title, brief } = specs[0]!;
      child = { id, title, brief, call, kind: "review", state: "running", createdAt: Date.now(), owns: [], after: [], profile: "default",
        model: "test/model", edits: [], steers: [], pulse: [], history: "r", messages: [], usage: { input: 0, output: 0, turns: 0 } };
      started.resolve(child);
      return [child];
    },
    async wait() { return [await completed.promise]; }, cancel: finishReview,
  };
  const journal = createJournal({ context, git: run, fleet: snapshot });
  const exit = createExit({ context, git: run, journal, fleet: snapshot, save() {}, changed() {} });
  const evidence = createBuildEvidence({ context, git: run, fleet, journal, exit, changed: evidenceChanged });
  const tool = (name: string) => evidence.tools.find(candidate => candidate.name === name)!.execute(name, {}, undefined, undefined, {} as never);
  return { root, source, git, run, context, head, base, dir: context.dir, setGate, journal, exit, evidence, tool, started: started.promise, finishReview,
    async close() { await evidence.close(); exit.close(); rmSync(root, { recursive: true, force: true }); } };
}

async function waitForFile(path: string): Promise<void> {
  const deadline = performance.now() + 4000;
  while (!existsSync(path)) {
    if (performance.now() >= deadline) throw new Error(`Gate did not create ${path}`);
    await new Promise(resolve => setTimeout(resolve, 10));
  }
}

const quote = (path: string) => `'${path.replaceAll("'", "'\\''")}'`;

test("the gate guard observes source edits alongside a live reviewer and rejects the run even after stash", async () => {
  const sampledDirty = Promise.withResolvers<void>();
  const continueCheck = Promise.withResolvers<void>();
  const f = fixture(async (argv, result) => {
    if (argv[0] === "--no-optional-locks" && argv[1] === "status" && result.stdout.includes("source.txt")) {
      sampledDirty.resolve();
      await continueCheck.promise;
    }
  });
  const ready = join(f.root, "ready"), sample = join(f.root, "sample"), observed = join(f.root, "observed"), release = join(f.root, "release");
  const wait = (path: string) => `while [ ! -f ${quote(path)} ]; do sleep 0.02; done`;
  const commands = [`touch ${quote(ready)}; ${wait(sample)}; cat source.txt > ${quote(observed)}; ${wait(release)}`];
  f.setGate(commands);
  try {
    const reviewing = f.tool("build_review");
    const verifying = f.tool("build_verify");
    await within(f.started, 4000, "read-only reviewer");
    await waitForFile(ready);
    writeFileSync(f.source, "edited during gate\n");
    writeFileSync(sample, "");
    await within(sampledDirty.promise, 4000, "dirty status sample with reviewer still running");
    await waitForFile(observed);
    expect(readFileSync(observed, "utf8")).toBe("edited during gate\n");
    f.git("stash", "push", "-qm", "during gate");
    expect(f.git("status", "--porcelain")).toBe("");
    writeFileSync(release, "");
    continueCheck.resolve();
    expect((await within(verifying, 4000, "gate invalidation")).isError).toBeTrue();
    f.finishReview();
    expect((await reviewing).isError).not.toBe(true);
    expect(f.journal.latestGate()).toBeUndefined();
    expect(f.journal.passedGate(f.head, gateInputFingerprint({ gate: commands, env: { MODE: "configured-first" } }))).toBeUndefined();
  } finally { continueCheck.resolve(); await f.close(); }
}, 12_000);

test("a gate leaving only a staged source change cannot record a pass", async () => {
  const f = fixture();
  try {
    f.setGate(["printf 'staged\\n' > source.txt && git add source.txt"]);
    expect((await f.tool("build_verify")).isError).toBeTrue();
    expect(f.git("diff", "--name-only")).toBe("");
    expect(f.git("diff", "--cached", "--name-only")).toBe("source.txt");
    expect(f.journal.latestGate()).toBeUndefined();
  } finally { await f.close(); }
});

for (const flag of ["assume-unchanged", "skip-worktree"]) test(`a dirty source hidden by ${flag} cannot earn a receipt`, async () => {
  const f = fixture();
  try {
    const ran = join(f.root, "ran");
    f.setGate([`touch ${quote(ran)}`]);
    f.git("update-index", `--${flag}`, "source.txt");
    writeFileSync(f.source, "hidden change\n");
    expect(f.git("--no-optional-locks", "status", "--porcelain")).toBe("");
    const index = join(f.context.session.worktree, ".git", "index");
    const before = readFileSync(index);
    await expect(verifyBuild(f.context, f.run, f.journal)).rejects.toThrow("assume-unchanged or skip-worktree entries");
    expect(readFileSync(index)).toEqual(before);
    expect(existsSync(ran)).toBeFalse();
    expect(f.journal.latestGate()).toBeUndefined();
  } finally { await f.close(); }
});

test("an initialized, locally inactive submodule cannot hide gate inputs from the receipt", async () => {
  const f = fixture();
  try {
    const cwd = f.context.session.worktree, dep = join(f.root, "dependency");
    execFileSync("git", ["clone", "--quiet", "--local", cwd, dep]);
    f.git("-c", "protocol.file.allow=always", "submodule", "add", "--quiet", dep, "dependency");
    f.git("commit", "-qm", "Pin dependency");
    f.git("config", "submodule.dependency.active", "false");
    f.git("config", "submodule.dependency.ignore", "all");
    await checkIndexVisibility(f.run, cwd);
    f.git("-C", "dependency", "update-index", "--assume-unchanged", "source.txt");
    writeFileSync(join(cwd, "dependency/source.txt"), "hidden\n");
    f.setGate(["grep -qx hidden dependency/source.txt"]);
    expect(f.git("status", "--porcelain", "--ignore-submodules=none")).toBe("");
    await expect(verifyBuild(f.context, f.run, f.journal)).rejects.toThrow("assume-unchanged or skip-worktree entries");
    expect(f.journal.latestGate()).toBeUndefined();
  } finally { await f.close(); }
});

test("a pass is not reused if a tracked edit is hidden after preparation", async () => {
  let onEvidenceChange: (() => void) | undefined;
  const f = fixture(undefined, () => {
    const run = onEvidenceChange;
    onEvidenceChange = undefined;
    run?.();
  });
  try {
    f.setGate(["true"]);
    expect((await f.tool("build_verify")).isError).not.toBe(true);
    const pass = f.journal.latestGate();
    expect(pass).toMatchObject({ attempt: "G1", result: "pass" });
    // The first evidence notification occurs after prepare's source checks but before receipt reuse.
    onEvidenceChange = () => {
      f.git("update-index", "--assume-unchanged", "source.txt");
      writeFileSync(f.source, "hidden change\n");
    };
    const verifying = await f.tool("build_verify");
    expect(f.git("--no-optional-locks", "status", "--porcelain")).toBe("");
    expect(verifying.isError).toBeTrue();
    expect(JSON.stringify(verifying.content)).toContain("assume-unchanged or skip-worktree entries");
    expect(JSON.stringify(verifying.content)).not.toContain("source.txt");
    expect(f.journal.latestGate()).toEqual(pass);
  } finally { await f.close(); }
});

test("the index audit streams a large NUL-delimited listing without a stdout cap", async () => {
  const f = fixture();
  try {
    const cwd = f.context.session.worktree, blob = f.git("rev-parse", "HEAD:source.txt");
    const prefix = "a".repeat(240);
    // A newline followed by a tag-like prefix is part of the tracked filename, not a new record.
    const input = Array.from({ length: 1100 }, (_, i) => `100644 ${blob}\t${prefix}/${i}\nS file`).join("\0") + "\0";
    execFileSync("git", ["-C", cwd, "update-index", "-z", "--index-info"], { input });
    await checkIndexVisibility(f.run, cwd);
    // source.txt sorts after the generated names, beyond the ordinary runner's 256 KiB cap.
    f.git("update-index", "--skip-worktree", "source.txt");
    await expect(checkIndexVisibility(f.run, cwd)).rejects.toThrow("assume-unchanged or skip-worktree entries");
  } finally { await f.close(); }
});

test("legacy gates and passes for a different configured env cannot authorize or be reused", async () => {
  const f = fixture();
  try {
    const commands = ['test "$MODE" = configured-first'];
    f.setGate(commands);
    const at = new Date().toISOString();
    f.journal.gate({ attempt: "G1", head: f.head, commands, started: at, ended: at, result: "pass" });
    const oldFingerprint = createHash("sha256").update(JSON.stringify(["corral-project-gate-v2", commands, [["MODE", "configured-first"]]])).digest("hex");
    f.journal.gate({ attempt: "G2", head: f.head, input_fingerprint: oldFingerprint, commands, started: at, ended: at, result: "pass" });
    const initial = await f.evidence.finish("finish");
    const authorization = f.exit.options().find(option => option.choice === "land")?.authorization;
    expect(initial.nextAction).toBeUndefined();
    expect(authorization).toBeDefined();
    const fingerprint = gateInputFingerprint({ gate: commands, env: { MODE: "configured-first" } });
    expect(f.journal.passedGate(f.head, fingerprint)).toMatchObject({ attempt: "G3", input_fingerprint: fingerprint });
    const newMode = "configured-second-private-fixture";
    f.setGate(commands, newMode);
    expect((await f.exit.choose("land", { authorization })).phase).not.toBe("landed");
    expect(f.git("rev-parse", "main")).toBe(f.base);
    expect((await f.tool("build_verify")).isError).toBeTrue();
    const changed = gateInputFingerprint({ gate: commands, env: { MODE: newMode } });
    expect(f.journal.latestGate()).toMatchObject({ attempt: "G4", input_fingerprint: changed, result: "fail" });
    expect(f.journal.passedGate(f.head, changed)).toBeUndefined();
    expect(readFileSync(join(f.dir, "journal.json"), "utf8")).not.toContain(newMode);
    const reviewing = f.tool("build_review");
    const reviewer = await within(f.started, 4000, "reviewing the changed gate");
    expect(reviewer.brief).toContain(changed);
    expect(reviewer.brief).not.toContain(newMode);
    f.finishReview();
    expect((await reviewing).isError).not.toBe(true);
  } finally { await f.close(); }
});

test("gate input fingerprints retain command order, canonicalize configured env, and exclude ambient credentials", () => {
  const input = { gate: ["first", "second"], env: { Z: "last", A: "first" } };
  const fingerprint = gateInputFingerprint(input);
  const key = "CORRAL_GATE_TEST_AMBIENT_TOKEN", previous = process.env[key];
  try {
    process.env[key] = "ambient-secret-fixture";
    expect(gateInputFingerprint({ gate: input.gate, env: { A: "first", Z: "last" } })).toBe(fingerprint);
    expect(gateInputFingerprint({ ...input, gate: [...input.gate].reverse() })).not.toBe(fingerprint);
  } finally { if (previous === undefined) delete process.env[key]; else process.env[key] = previous; }
});
