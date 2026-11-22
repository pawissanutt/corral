import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { checked, gitRun, type Run } from "../src/build/corral-cli.ts";
import { fastForward } from "../src/build/fast-forward.ts";

async function fixture(linkedTarget = false) {
  const root = mkdtempSync(join(tmpdir(), "corral-fast-forward-test-"));
  const repo = join(root, "repo"), linked = join(root, "linked checkout");
  const checkout = linkedTarget ? linked : repo, source = linkedTarget ? repo : linked;
  const run = gitRun();
  const git = (cwd: string, ...args: string[]) => checked(run, args, cwd);
  const sha = async (cwd: string, rev = "HEAD") => (await git(cwd, "rev-parse", rev)).trim();
  const state = (cwd: string, ...args: string[]) => run(args, { cwd });
  const close = () => rmSync(root, { recursive: true, force: true });
  mkdirSync(repo);
  try {
    await git(repo, "init", "-b", "main");
    await git(repo, "config", "user.email", "test@example.org");
    await git(repo, "config", "user.name", "Fixture");
    await git(repo, "config", "core.hooksPath", "/dev/null");
    writeFileSync(join(repo, "staged"), "staged base\n");
    writeFileSync(join(repo, "unstaged"), "unstaged base\n");
    writeFileSync(join(repo, ".gitignore"), "ignored.txt\n");
    await git(repo, "add", ".");
    await git(repo, "commit", "-m", "base");
    const base = await sha(repo);
    await git(repo, "switch", "-c", "other");
    writeFileSync(join(repo, "other.txt"), "on other branch\n");
    await git(repo, "add", "other.txt");
    await git(repo, "commit", "-m", "other branch");
    const other = await sha(repo);
    await git(repo, "branch", "same", base);
    if (linkedTarget) {
      await git(repo, "worktree", "add", linked, "main");
      await git(repo, "switch", "-c", "build", base);
    } else {
      await git(repo, "switch", "main");
      await git(repo, "worktree", "add", "-b", "build", linked);
    }
    await git(source, "commit", "--allow-empty", "-m", "intermediate candidate");
    const intermediate = await sha(source);
    writeFileSync(join(source, "incoming.txt"), "pinned candidate\n");
    writeFileSync(join(source, "ignored.txt"), "candidate ignored content\n");
    await git(source, "add", "-f", "incoming.txt", "ignored.txt");
    await git(source, "commit", "-m", "candidate");
    const head = await sha(source);
    await git(source, "commit", "--allow-empty", "-m", "candidate advanced again");
    const later = await sha(source);
    const index = (await git(checkout, "rev-parse", "--path-format=absolute", "--git-path", "index")).trimEnd();
    const land = (beforeWrite: () => void | Promise<void> = () => {}, runner: Run = run) => fastForward(runner, { source, checkout, ref: "refs/heads/main", base, head, beforeWrite });
    return { root, checkout, source, run, git, sha, state, close, base, head, other, intermediate, later, land, index };
  } catch (error) { close(); throw error; }
}

for (const linkedTarget of [false, true]) test(`exact attached fast-forward preserves disjoint user work (${linkedTarget ? "linked" : "primary"} target)`, async () => {
  const f = await fixture(linkedTarget);
  try {
    writeFileSync(join(f.checkout, "staged"), "staged local\n");
    await f.git(f.checkout, "add", "staged");
    writeFileSync(join(f.checkout, "unstaged"), "unstaged local\n");
    writeFileSync(join(f.checkout, "untracked"), "untracked local\n");
    if (linkedTarget) await f.git(f.checkout, "update-index", "--split-index");
    const status = await f.git(f.checkout, "status", "--porcelain");
    await f.land();
    expect(await f.sha(f.checkout)).toBe(f.head);
    expect(await f.sha(f.source)).toBe(f.later);
    expect(await f.git(f.checkout, "status", "--porcelain")).toBe(status);
    expect(readFileSync(join(f.checkout, "incoming.txt"), "utf8")).toBe("pinned candidate\n");
    expect(readFileSync(join(f.checkout, "untracked"), "utf8")).toBe("untracked local\n");
    expect(await f.git(f.checkout, "show", ":staged")).toBe("staged local\n");
    expect(readFileSync(join(f.checkout, "unstaged"), "utf8")).toBe("unstaged local\n");
    expect(existsSync(`${f.index}.lock`)).toBeFalse();
  } finally { f.close(); }
});

for (const movement of ["intermediate", "later", "same-SHA branch switch"] as const) test(`movement at last unlocked call: ${movement}`, async () => {
  const f = await fixture();
  try {
    let raced = false;
    const runner: Run = async (argv, options) => {
      const result = await f.run(argv, options);
      // This is the final lookup before landing starts taking locks. A fresh ref
      // lookup earlier in exit preparation cannot prevent this race.
      if (!raced && options.cwd === f.source && argv.includes("--absolute-git-dir")) {
        raced = true;
        if (movement === "same-SHA branch switch") await f.git(f.checkout, "switch", "same");
        else await f.git(f.checkout, "update-ref", "refs/heads/main", f[movement], f.base);
      }
      return result;
    };
    await expect(f.land(undefined, runner)).rejects.toThrow();
    expect(raced).toBeTrue();
    expect(await f.sha(f.checkout, "refs/heads/main")).toBe(movement === "same-SHA branch switch" ? f.base : f[movement]);
    expect(await f.sha(f.checkout, "refs/heads/same")).toBe(f.base);
    expect((await f.git(f.checkout, "symbolic-ref", "HEAD")).trim()).toBe(movement === "same-SHA branch switch" ? "refs/heads/same" : "refs/heads/main");
    expect(existsSync(join(f.checkout, "incoming.txt"))).toBeFalse();
    expect(existsSync(`${f.index}.lock`)).toBeFalse();
  } finally { f.close(); }
});

test("Git cannot change HEAD, index or base while landing is between preparation and checkout", async () => {
  const f = await fixture();
  try {
    let observed = false;
    await f.land(async () => {
      expect(await f.sha(f.checkout)).toBe(f.base);
      expect((await f.state(f.checkout, "switch", "other")).code).not.toBe(0);
      expect(existsSync(join(f.checkout, "other.txt"))).toBeFalse();
      expect((await f.state(f.checkout, "symbolic-ref", "HEAD", "refs/heads/same")).code).not.toBe(0);
      expect((await f.state(f.source, "update-ref", "refs/heads/main", f.intermediate, f.base)).code).not.toBe(0);
      observed = true;
    });
    expect(observed).toBeTrue();
    expect(await f.sha(f.checkout, "refs/heads/main")).toBe(f.head);
    expect(await f.sha(f.checkout, "refs/heads/other")).toBe(f.other);
    expect((await f.git(f.checkout, "symbolic-ref", "HEAD")).trim()).toBe("refs/heads/main");
    expect(await f.git(f.checkout, "status", "--porcelain")).toBe("");
  } finally { f.close(); }
});

for (const path of ["incoming.txt", "ignored.txt"]) test(`late ${path} obstruction is not overwritten or landed`, async () => {
  const f = await fixture();
  try {
    await expect(f.land(() => writeFileSync(join(f.checkout, path), "private local data\n"))).rejects.toThrow(/would be overwritten/);
    expect(readFileSync(join(f.checkout, path), "utf8")).toBe("private local data\n");
    expect(await f.sha(f.checkout)).toBe(f.base);
    expect(existsSync(`${f.index}.lock`)).toBeFalse();
  } finally { f.close(); }
});

test("an existing index lock is never broken", async () => {
  const f = await fixture();
  try {
    writeFileSync(`${f.index}.lock`, "someone else owns this", { flag: "wx" });
    await expect(f.land()).rejects.toThrow(/Cannot lock the index/);
    expect(readFileSync(`${f.index}.lock`, "utf8")).toBe("someone else owns this");
    expect(await f.sha(f.checkout)).toBe(f.base);
  } finally { f.close(); }
});

test("replacing the index lock before checkout refuses without deleting someone else's lock", async () => {
  const f = await fixture();
  try {
    await expect(f.land(() => {
      renameSync(`${f.index}.lock`, `${f.index}.original-lock`);
      writeFileSync(`${f.index}.lock`, "replacement lock", { flag: "wx" });
    })).rejects.toThrow(/before checkout/);
    expect(await f.sha(f.checkout)).toBe(f.base);
    expect(readFileSync(`${f.index}.lock`, "utf8")).toBe("replacement lock");
    expect(existsSync(join(f.checkout, "incoming.txt"))).toBeFalse();
  } finally { f.close(); }
});

for (const phase of ["prepared", "committed"]) test(`ref hook ${phase} failure requires no destructive rollback`, async () => {
  const f = await fixture();
  try {
    const hooks = join(f.root, "hooks");
    mkdirSync(hooks);
    const script = `#!${process.execPath}
import { readFileSync, renameSync, writeFileSync } from 'node:fs';
if (process.argv[2] === ${JSON.stringify(phase)} && readFileSync(0, 'utf8').includes('refs/heads/main')) {
  ${phase === "prepared" ? "process.exit(1);" : `const lock = ${JSON.stringify(`${f.index}.lock`)};
  renameSync(lock, lock + '.original');
  writeFileSync(lock, 'replacement', { flag: 'wx' });`}
}`;
    writeFileSync(join(hooks, "reference-transaction"), script, { mode: 0o755 });
    await f.git(f.checkout, "config", "core.hooksPath", hooks);
    if (phase === "prepared") {
      await expect(f.land()).rejects.toThrow(/prepare failed/);
      expect(await f.sha(f.checkout)).toBe(f.base);
      expect(existsSync(join(f.checkout, "incoming.txt"))).toBeFalse();
      expect(existsSync(`${f.index}.lock`)).toBeFalse();
    } else {
      await expect(f.land()).rejects.toThrow(/may have been partially applied/);
      expect(await f.sha(f.checkout)).toBe(f.head);
      expect(readFileSync(`${f.index}.lock`, "utf8")).toBe("replacement");
      expect(readFileSync(join(f.checkout, "incoming.txt"), "utf8")).toBe("pinned candidate\n");
    }
  } finally { f.close(); }
});

test("unchecked branch uses exact-old CAS at the mutation itself", async () => {
  const f = await fixture();
  try {
    await f.git(f.checkout, "switch", "same");
    let raced = false;
    const runner: Run = async (argv, options) => {
      if (argv[0] === "update-ref") {
        raced = true;
        await f.git(f.source, "update-ref", "refs/heads/main", f.intermediate, f.base);
      }
      return f.run(argv, options);
    };
    await expect(fastForward(runner, { source: f.source, ref: "refs/heads/main", base: f.base, head: f.head, beforeWrite() {} })).rejects.toThrow();
    expect(raced).toBeTrue();
    expect(await f.sha(f.checkout, "refs/heads/main")).toBe(f.intermediate);
    expect(await f.sha(f.checkout)).toBe(f.base);
    expect(existsSync(join(f.checkout, "incoming.txt"))).toBeFalse();
  } finally { f.close(); }
});
