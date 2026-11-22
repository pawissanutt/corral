import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { constants } from "node:fs";
import { lstat, mkdtemp, open, realpath, rename, rm, unlink, type FileHandle } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { createInterface } from "node:readline";
import { checked, type Run } from "./corral-cli.ts";

const TIMEOUT = 120_000;
const SHA = /^(?:[a-f0-9]{40}|[a-f0-9]{64})$/;
const LOCAL = { GIT_NO_LAZY_FETCH: "1", GIT_NO_REPLACE_OBJECTS: "1", GIT_TERMINAL_PROMPT: "0" };
const SAFE_READ = constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK;
const gitError = (error: unknown) => error instanceof Error ? error.message : String(error);
const missing = (error: unknown) => (error as NodeJS.ErrnoException).code === "ENOENT";

/** Caller must retain the worktree recovery detail instead of turning this into a retryable stale pin. */
export class PartialFastForward extends Error {}

export function checkLandingEnvironment(): void {
  const overrides = ["GIT_DIR", "GIT_COMMON_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_NAMESPACE", "GIT_CONFIG"];
  if (overrides.some(key => process.env[key] !== undefined)) throw new Error("Unset Git repository or index overrides before landing.");
}

/** Check known incompatibilities before offering Land and again under the checkout locks. */
export async function checkCheckoutSupport(git: Run, checkout: string): Promise<void> {
  checkLandingEnvironment();
  const boolConfig = async (key: string) => {
    const result = await git(["config", "--type=bool", "--get", key], { cwd: checkout });
    if (result.code === 1 || result.code === 0 && result.stdout.trim() === "false") return false;
    if (result.code === 0 && result.stdout.trim() === "true") return true;
    throw new Error(`Cannot determine ${key} for the landing checkout.`);
  };
  if (await boolConfig("core.sparseCheckout")) throw new Error("Landing into a sparse checkout is not supported. Use a full checkout.");
  if (await boolConfig("extensions.worktreeConfig")) {
    const configPath = (await checked(git, ["rev-parse", "--path-format=absolute", "--git-path", "config.worktree"], checkout)).trimEnd();
    const present = await lstat(configPath).catch(error => { if (!missing(error)) throw error; });
    if (present) throw new Error("Landing into a checkout with worktree-specific Git config is not supported.");
  }
  const conditional = await git(["config", "--name-only", "--get-regexp", "^includeif\\.(gitdir(/i)?|onbranch):"], { cwd: checkout });
  if (conditional.code === 0) throw new Error("Landing into a checkout with Git configuration conditional on the branch or Git directory is not supported.");
  if (conditional.code !== 1) throw new Error("Cannot check the landing checkout for conditional Git configuration.");
}

/** A live transaction: EOF without commit aborts. Never replay or guess the result of a failed commit. */
async function withPreparedRef<T>(cwd: string, operation: string, action: (commit: () => Promise<void>, assertHeld: () => void) => Promise<T>): Promise<T> {
  const child = spawn("git", ["update-ref", "-m", "corral: land prepared candidate", "--stdin"],
    { cwd, env: { ...process.env, ...LOCAL }, shell: false, stdio: ["pipe", "pipe", "pipe"] });
  let stderr = "", failure = "", ended = false;
  const done = new Promise<void>(resolve => {
    child.on("error", error => { failure = error.message; });
    child.on("close", (code, signal) => { failure ||= signal ? `terminated by ${signal}` : code ? `exit ${code}` : ""; ended = true; resolve(); });
  });
  child.stderr.setEncoding("utf8").on("data", chunk => { stderr = (stderr + chunk).slice(0, 16384); });
  child.stdin.on("error", error => { failure ||= error.message; });
  const lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
  const replies = lines[Symbol.asyncIterator]();
  const timeout = () => { failure ||= "transaction timed out"; child.kill("SIGKILL"); };
  let timer = setTimeout(timeout, TIMEOUT);
  const expect = async (text: string) => {
    const reply = await replies.next();
    if (reply.done) await done;
    if (reply.value !== `${text}: ok`) throw new Error(`Git ref transaction ${text} failed: ${stderr.trim() || failure || reply.value || "missing acknowledgement"}`);
  };
  try {
    child.stdin.write(`start\n${operation}\nprepare\n`);
    await expect("start");
    await expect("prepare");
    clearTimeout(timer); // Never time out a held lock while the checkout operation is still running.
    const assertHeld = () => {
      if (ended || failure) throw new Error(`Git ref transaction ended before commit: ${stderr.trim() || failure || "no result"}`);
    };
    return await action(async () => {
      assertHeld();
      timer = setTimeout(timeout, TIMEOUT);
      child.stdin.end("commit\n");
      await expect("commit");
      await done;
      if (failure) throw new Error(`Git ref transaction commit result is uncertain: ${stderr.trim() || failure}`);
    }, assertHeld);
  } finally {
    clearTimeout(timer);
    if (!ended && !child.stdin.writableEnded) child.stdin.end("abort\n");
    timer = setTimeout(timeout, TIMEOUT);
    await done;
    clearTimeout(timer);
    lines.close();
  }
}

async function scratchGit(cwd: string, args: string[], extra: NodeJS.ProcessEnv = {}, digestOutput = false): Promise<string> {
  // Only this invocation sees the private HEAD/index. Never change the process environment.
  return new Promise<string>((resolve, reject) => {
    const child = spawn("git", args, { cwd, env: { ...process.env, ...LOCAL, ...extra }, shell: false, stdio: ["ignore", "pipe", "pipe"] });
    let text = "", timedOut = false;
    const digest = digestOutput ? createHash("sha256") : undefined;
    const collect = (chunk: Buffer) => { text = (text + chunk.toString("utf8")).slice(0, 16384); };
    child.stdout.on("data", (chunk: Buffer) => { if (digest) digest.update(chunk); else collect(chunk); });
    child.stderr.on("data", collect);
    const timer = setTimeout(() => { timedOut = true; child.kill("SIGKILL"); }, TIMEOUT);
    child.once("error", reject);
    child.once("close", (code, signal) => {
      clearTimeout(timer);
      if (code === 0 && !timedOut) resolve(digest?.digest("hex") ?? "");
      else reject(new Error(digest ? "Cannot read the private checkout configuration." : text.trim() || (timedOut ? "Git checkout timed out" : `Git checkout failed (${signal ?? code})`)));
    });
  });
}

async function copyIndex(from: FileHandle, to: FileHandle): Promise<void> {
  const stat = await from.stat();
  if (!stat.isFile()) throw new Error("The checkout index is not a regular file.");
  const chunk = Buffer.allocUnsafe(256 * 1024);
  for (let offset = 0; offset < stat.size;) {
    const { bytesRead } = await from.read(chunk, 0, Math.min(chunk.length, stat.size - offset), offset);
    if (!bytesRead) throw new Error("The checkout index changed while it was being copied.");
    for (let written = 0; written < bytesRead;) {
      const { bytesWritten } = await to.write(chunk, written, bytesRead - written, offset + written);
      if (!bytesWritten) throw new Error("Could not write the checkout index.");
      written += bytesWritten;
    }
    offset += bytesRead;
  }
}

async function unchangedLock(lock: FileHandle, path: string): Promise<boolean> {
  const actual = await lstat(path).catch(error => { if (!missing(error)) throw error; });
  const own = await lock.stat();
  return actual?.ino === own.ino && actual.dev === own.dev;
}

/**
 * An exact-old CAS on the explicitly named local ref. In an attached checkout, lock its
 * index AND symbolic HEAD before touching files (switch writes the index before it writes
 * HEAD). Use a second transaction from the build checkout for the named ref so that
 * Git 2.43's implicit HEAD update does not conflict with the destination's HEAD verify.
 *
 * Git has no single commit spanning working files, the index, and refs. An interruption
 * after checkout starts may leave a partially applied result; never reset user work.
 */
export async function fastForward(git: Run, request: {
  source: string; checkout?: string; ref: string; base: string; head: string;
  beforeWrite: () => void | Promise<void>;
}): Promise<void> {
  const { source, checkout, ref, base, head, beforeWrite } = request;
  if (!SHA.test(base) || !SHA.test(head) || base.length !== head.length || base === head || !ref.startsWith("refs/heads/")) {
    throw new Error("Invalid pinned fast-forward.");
  }
  checkLandingEnvironment();
  await checked(git, ["check-ref-format", ref], source);
  // Protect the update-ref stdin protocol even if a runner is substituted in tests.
  if (/[\s\0]/.test(ref)) throw new Error("Invalid pinned branch name.");
  const operation = `option no-deref\nupdate ${ref} ${head} ${base}`;
  const guardedUpdate = (action: (commit: () => Promise<void>, assertHeld: () => void) => Promise<void>) => withPreparedRef(source, operation, async (commit, assertHeld) => {
    const symbolic = await git(["symbolic-ref", "--quiet", "--no-recurse", ref], { cwd: source });
    if (symbolic.code !== 1) throw new Error(`The landing target ${ref} is not a direct local branch.`);
    await action(commit, assertHeld);
  });
  if (!checkout) {
    // A ref-only CAS protects the named branch and its old SHA; it cannot prevent a
    // newly created worktree from starting a checkout concurrently.
    await beforeWrite();
    await checked(git, ["update-ref", "--no-deref", ref, head, base], source);
    return;
  }

  // Git's own paths handle linked worktrees and separate Git directories.
  const locate = async (args: string[], cwd: string) => realpath((await checked(git, ["rev-parse", ...args], cwd)).trimEnd());
  const destination = await locate(["--show-toplevel"], checkout);
  const gitDir = await locate(["--absolute-git-dir"], checkout);
  const commonArgs = ["--path-format=absolute", "--git-common-dir"];
  const common = await locate(commonArgs, checkout);
  if (destination !== await realpath(checkout) || common !== await locate(commonArgs, source) || gitDir === await locate(["--absolute-git-dir"], source)) {
    throw new Error("The landing checkout is no longer a separate checkout of the build repository.");
  }
  // With no custom index override, Git uses $GIT_DIR/index. Never follow a symlink
  // to the original index, nor overwrite or break an existing index lock.
  const index = join(gitDir, "index"), lockPath = `${index}.lock`;
  const lock = await open(lockPath, "wx", 0o666).catch(error => {
    throw new Error(`Cannot lock the index of ${checkout}: ${gitError(error)}`);
  });
  let temp: string | undefined, installed = false, checkoutStarted = false;
  try {
    await withPreparedRef(checkout, `option no-deref\nverify HEAD ${base}`, async (_neverCommitHead, assertHeadLocked) => {
      const branch = (await checked(git, ["symbolic-ref", "--quiet", "--no-recurse", "HEAD"], checkout)).trimEnd();
      if (branch !== ref || await locate(["--absolute-git-dir"], checkout) !== gitDir) throw new Error(`The checkout at ${checkout} switched branches before landing.`);
      await guardedUpdate(async (commit, assertRefLocked) => {
        await checkCheckoutSupport(git, checkout);
        temp = await mkdtemp(join(tmpdir(), "corral-fast-forward-"));
        const privateDir = join(temp, "git"), privateIndex = join(temp, "index");
        const original = await open(index, SAFE_READ);
        try {
          const stat = await original.stat();
          if (!stat.isFile()) throw new Error("The checkout index is not a regular file.");
          await lock.chmod(stat.mode & 0o777);
          const shadow = await open(privateIndex, "wx", 0o600);
          try { await copyIndex(original, shadow); } finally { await shadow.close(); }
        } finally { await original.close(); }
        const split = (await checked(git, ["rev-parse", "--path-format=absolute", "--shared-index-path"], checkout)).trimEnd();
        if (split) {
          if (dirname(split) !== gitDir || !/^sharedindex\.(?:[a-f\d]{40}|[a-f\d]{64})$/.test(basename(split))) {
            throw new Error("Cannot safely locate the split index for the landing checkout.");
          }
          const shared = await open(split, SAFE_READ);
          try {
            const copy = await open(join(temp, basename(split)), "wx", 0o600);
            try { await copyIndex(shared, copy); } finally { await copy.close(); }
          } finally { await shared.close(); }
        }
        // We must use porcelain's --no-overwrite-ignore: Git 2.43 read-tree -m -u
        // can silently overwrite ignored files, including with an alternate index.
        // Store porcelain bookkeeping privately; objects and project config come from
        // the original common dir. Disable scratch hooks, autostash, and maintenance.
        const format = (await checked(git, ["rev-parse", "--show-object-format"], checkout)).trim();
        if (format !== "sha1" && format !== "sha256") throw new Error("Unsupported object format for landing.");
        await scratchGit(temp, ["init", "--quiet", "--bare", "--template=", `--object-format=${format}`, privateDir]);
        const env = { GIT_DIR: privateDir, GIT_COMMON_DIR: common, GIT_WORK_TREE: destination, GIT_INDEX_FILE: privateIndex };
        const config = ["-c", "core.hooksPath=/dev/null", "-c", "core.splitIndex=false", "-c", "core.fsmonitor=false", "-c", "maintenance.auto=false",
          "-c", "gc.auto=0", "-c", "rerere.enabled=false", "-c", "submodule.recurse=false", "-c", "merge.autoStash=false"];
        await scratchGit(temp, [...config, "update-ref", "--no-deref", "HEAD", base], env);
        // Do not bypass worktree config or conditional Git includes (e.g. filters
        // configured for the real Git directory or target branch). Fail closed if
        // the private detached Git context cannot reproduce the checkout config.
        const configArgs = ["config", "--null", "--list"];
        const actualConfig = await git(configArgs, { cwd: checkout, stdoutDigest: "sha256" });
        if (actualConfig.code || actualConfig.stdout !== await scratchGit(destination, configArgs, env, true)) {
          throw new Error("Landing cannot reproduce the checkout's worktree-specific or conditional Git configuration.");
        }
        await beforeWrite();
        assertHeadLocked();
        assertRefLocked();
        if (!await unchangedLock(lock, lockPath)) throw new Error("The checkout index lock was removed or replaced before checkout.");
        checkoutStarted = true;
        await scratchGit(destination, [...config, "merge", "--ff-only", "--no-autostash", "--no-overwrite-ignore", "--no-edit", "--no-stat", head], env);
        // Keep index.lock until after ref commit, preventing a concurrent Git checkout
        // from installing its own index/worktree while the expected base is still old.
        const result = await open(privateIndex, SAFE_READ);
        try { await copyIndex(result, lock); } finally { await result.close(); }
        await lock.sync();
        if (!await unchangedLock(lock, lockPath)) throw new Error("The checkout index lock was removed or replaced during landing.");
        assertHeadLocked();
        await commit();
        if (!await unchangedLock(lock, lockPath)) throw new Error("The checkout index lock was removed or replaced while committing the ref.");
        await rename(lockPath, index);
        installed = true;
      });
    });
  } catch (error) {
    if (checkoutStarted) throw new PartialFastForward(`${gitError(error)}\nInspect ${checkout}'s branch, index and worktree before retrying. Landing may have been partially applied; no rollback was attempted.`);
    throw error;
  } finally {
    try {
      if (!installed && await unchangedLock(lock, lockPath)) await unlink(lockPath);
    } finally {
      await lock.close();
      if (temp) await rm(temp, { recursive: true, force: true });
    }
  }
}
