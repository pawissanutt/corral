// Turn checkpoints live on the active transcript branch. An exclusively owned worktree also
// saves every non-ignored file over HEAD through a private index and a per-worktree ref. A
// failed Git snapshot records its error and the turn continues.
import { closeSync, copyFileSync, existsSync, fchmodSync, fstatSync, lstatSync, mkdtempSync, openSync,
  readFileSync, realpathSync, renameSync, rmSync, statSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";

export const CHECKPOINT_ENTRY = "corral_checkpoint";
const GIT_TIMEOUT_MS = 120_000;
const ERROR_MAX_CHARS = 512;
const IDENTITY = {
  GIT_AUTHOR_NAME: "corral checkpoint", GIT_AUTHOR_EMAIL: "corral@localhost",
  GIT_COMMITTER_NAME: "corral checkpoint", GIT_COMMITTER_EMAIL: "corral@localhost",
};

export type CheckpointPhase = "start" | "end";

export interface CheckpointRecord {
  readonly id: string;
  readonly phase: CheckpointPhase;
  readonly turn: number;
  /** Absent for Git-backed checkpoints (including existing worker transcripts). */
  readonly scope?: "conversation";
  readonly commit?: string;
  readonly head?: string;
  readonly ref?: string;
  readonly leaf?: string;
  readonly error?: string;
}

export interface RewindPoint extends CheckpointRecord {
  readonly commit: string;
  readonly head: string;
}

export interface Rescue {
  readonly ref: string;
  readonly head: string;
  readonly commit: string;
}

export interface CheckpointHost {
  /** Run id for workers, session id otherwise. */
  readonly id: string;
  readonly git?: { readonly cwd: string; readonly buildBranch?: string };
  /** Appends one durable record to the bound session. */
  record(entry: CheckpointRecord): void;
}

type IndexEnv = { GIT_INDEX_FILE: string };

export class Checkpoints {
  constructor(private readonly host: CheckpointHost) {}

  get conversationOnly(): boolean { return this.host.git === undefined; }

  private git(args: readonly string[], env: Record<string, string> = {}, raw = false): string {
    if (!this.host.git) throw new Error("this checkpoint has no Git worktree");
    const result = execFileSync("git", ["-C", this.host.git.cwd, ...args], {
      env: { ...process.env, ...env }, stdio: ["ignore", "pipe", "pipe"], timeout: GIT_TIMEOUT_MS, encoding: "utf8",
    });
    return raw ? result : result.trim();
  }

  private withIndex<T>(use: (env: IndexEnv) => T): T {
    const scratch = mkdtempSync(join(tmpdir(), "corral-checkpoint-"));
    try {
      const index = join(scratch, "index");
      const real = this.git(["rev-parse", "--path-format=absolute", "--git-path", "index"]);
      if (existsSync(real)) {
        // Git re-hashes an entry whose stat data is not older than its index file; the copy keeps
        // the original's mtime (truncated to ms, never later), or a same-tick rewrite reads clean.
        copyFileSync(real, index);
        const { atime, mtime } = statSync(real);
        utimesSync(index, atime, mtime);
      }
      return use({ GIT_INDEX_FILE: index });
    } finally {
      rmSync(scratch, { recursive: true, force: true });
    }
  }

  private snapshot(message: string, head: string, env: Record<string, string>, indexCommit?: string): string {
    this.git(["add", "-A"], env);
    const tree = this.git(["write-tree"], env);
    return this.git(["commit-tree", tree, "-p", head, ...(indexCommit ? ["-p", indexCommit] : []), "-m", message], IDENTITY);
  }

  private requireBuildBranch(): void {
    const git = this.host.git;
    if (!git?.buildBranch) return;
    if (["GIT_DIR", "GIT_COMMON_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_NAMESPACE", "GIT_CONFIG"]
      .some(key => process.env[key] !== undefined)) throw new Error("unset Git repository or index overrides before build rewind");
    if (realpathSync(this.git(["rev-parse", "--show-toplevel"])) !== realpathSync(git.cwd)) {
      throw new Error("build rewind needs its own worktree root");
    }
    const branch = git.buildBranch;
    if (this.git(["rev-parse", "--symbolic-full-name", "HEAD"]) !== `refs/heads/${branch}`) {
      throw new Error(`build rewind needs the recorded branch ${branch} checked out`);
    }
  }

  /** Takes one checkpoint synchronously, so nothing of the turn runs before it is recorded. */
  capture(phase: CheckpointPhase, turn: number, leaf?: string | null): CheckpointRecord {
    const key = `t${turn}-${phase}`;
    const id = `${this.host.id}-${key}`;
    const base = { id, phase, turn };
    const cursor = phase === "start" && leaf ? { leaf } : {};
    if (this.conversationOnly) {
      const entry = { ...base, ...cursor, scope: "conversation" as const };
      this.host.record(entry);
      return entry;
    }
    let entry: CheckpointRecord;
    try {
      const capture = (requireLock?: () => void) => {
        const head = this.git(["rev-parse", "--verify", "HEAD"]);
        const commit = this.withIndex(env => this.snapshot(`corral checkpoint ${id}`, head, env));
        if (requireLock) {
          requireLock();
          this.requireBuildHead(head);
        }
        // A rewound turn number can be reused without dropping an earlier snapshot.
        const ref = `refs/worktree/corral/checkpoints/${this.host.id}/${key}-${commit}`;
        this.git(["update-ref", ref, commit]);
        return { ...base, ...cursor, commit, head, ref };
      };
      entry = this.host.git?.buildBranch ? this.withBuildIndexLock(capture) : capture();
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error);
      entry = { ...base, error: reason.slice(0, ERROR_MAX_CHARS) };
    }
    this.host.record(entry);
    return entry;
  }

  restore(point: RewindPoint): Rescue {
    const branch = this.host.git?.buildBranch;
    if (branch) return this.restoreBuild(point, branch);
    return this.restoreTree(point, (commit, _head, env) => {
      this.git(["read-tree", "-u", "-m", commit, point.commit], env);
      this.git(["reset", "--mixed", "--no-refresh", point.head]);
    });
  }

  private restoreTree(point: RewindPoint, apply: (commit: string, head: string, env: IndexEnv) => void): Rescue {
    const head = this.git(["rev-parse", "--verify", "HEAD"]);
    const at = new Date().toISOString().replace(/[:.]/g, "-");
    // Build cleanup can remove even a clean rewound checkout. Keep recovery in the repo itself.
    const prefix = this.host.git?.buildBranch ? `refs/corral/rescue/${this.host.id}` : "refs/worktree/corral/rescue";
    const ref = `${prefix}/${at}-${crypto.randomUUID()}`;
    return this.withIndex(env => {
      // An index-only version may not exist in the worktree or HEAD. Keep it reachable too,
      // as the second parent, without creating an extra commit for an unchanged index.
      const staged = this.git(["write-tree"], env);
      const original = this.git(["rev-parse", "HEAD^{tree}"]);
      const indexCommit = staged === original ? undefined
        : this.git(["commit-tree", staged, "-p", head, "-m", "corral rewind: staged index"], IDENTITY);
      const commit = this.snapshot(`corral rewind to turn ${point.turn}`, head, env, indexCommit);
      if (this.host.git?.buildBranch) this.checkBuildObstructions(commit, point.commit, env);
      this.git(["update-ref", ref, commit]);
      try {
        apply(commit, head, env);
      } catch (error) {
        throw new Error(`rewind could not restore the checkpoint; saved work is at ${ref}: ${String(error)}`);
      }
      return { ref, head, commit };
    });
  }

  private requireBuildHead(head: string): void {
    this.requireBuildBranch();
    if (this.git(["rev-parse", "--verify", "HEAD"]) !== head) throw new Error("the build HEAD changed during rewind");
  }

  private restoreBuild(point: RewindPoint, branch: string): Rescue {
    return this.withBuildIndexLock((requireLock, install) => this.restoreTree(point, (commit, head, env) => {
      requireLock();
      this.requireBuildHead(head);
      this.git(["read-tree", "--no-recurse-submodules", "-u", "-m", commit, point.commit], env);
      this.git(["-c", "core.splitIndex=false", "read-tree", "--reset", point.head], env);
      install(env.GIT_INDEX_FILE, () => {
        this.requireBuildHead(head);
        // Pin the named branch and its old tip; never reset whichever branch may be on HEAD.
        this.git(["update-ref", "--no-deref", "-m", "corral rewind", `refs/heads/${branch}`, point.head, head]);
        this.requireBuildHead(point.head);
      });
    }));
  }

  private withBuildIndexLock<T>(use: (requireLock: () => void,
    install: (shadow: string, moveBranch: () => void) => void) => T): T {
    this.requireBuildBranch();
    const index = this.git(["rev-parse", "--path-format=absolute", "--git-path", "index"]);
    const lock = `${index}.lock`;
    // A concurrent Git switch needs the real index lock, even though we use a private index.
    const fd = openSync(lock, "wx", 0o666);
    const ownsLock = () => {
      const held = fstatSync(fd), path = lstatSync(lock, { throwIfNoEntry: false });
      return held.ino === path?.ino && held.dev === path.dev;
    };
    const requireLock = () => { if (!ownsLock()) throw new Error("the build index lock was removed or replaced"); };
    let installed = false;
    try {
      this.requireBuildBranch();
      const gitDir = this.git(["rev-parse", "--absolute-git-dir"]);
      if (["MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD", "sequencer", "rebase-apply", "rebase-merge"]
        .some(name => existsSync(join(gitDir, name)))) throw new Error("finish or abort the in-progress Git operation before rewinding the build");
      const stat = lstatSync(index, { throwIfNoEntry: false });
      if (stat && !stat.isFile()) throw new Error("build rewind needs a regular Git index");
      return use(requireLock, (shadow, moveBranch) => {
        if (stat) fchmodSync(fd, stat.mode & 0o777);
        writeFileSync(fd, readFileSync(shadow));
        requireLock();
        moveBranch();
        requireLock();
        renameSync(lock, index);
        installed = true;
      });
    } finally {
      try { if (!installed && ownsLock()) rmSync(lock, { force: true }); }
      finally { closeSync(fd); }
    }
  }

  private checkBuildObstructions(from: string, to: string, env: IndexEnv): void {
    const paths = (output: string) => {
      if (output.includes("\uFFFD")) throw new Error("cannot check an unrepresentable Git path for obstructions");
      return output.split("\0").filter(Boolean);
    };
    const incoming = paths(this.git(["diff", "--name-only", "-z", "--diff-filter=AMT", "--no-renames", from, to, "--"], env, true));
    if (!incoming.length) return;
    // read-tree -u can overwrite ignored files. Include them (no --exclude-standard) and
    // deliberately treat a collapsed uncaptured directory as an obstruction in either direction.
    const others = paths(this.git(["ls-files", "--others", "--directory", "--no-empty-directory", "-z"], env, true));
    if (!others.length) return;
    const ignoreCase = this.git(["config", "--default", "false", "--type=bool", "--get", "core.ignorecase"]) === "true";
    const key = (path: string) => ignoreCase ? path.normalize("NFD").toLowerCase() : path;
    const uncaptured = new Set(others.map(path => key(path.replace(/\/$/, ""))));
    const parents = new Set<string>();
    for (const path of uncaptured) for (let end = path.indexOf("/"); end !== -1; end = path.indexOf("/", end + 1)) {
      parents.add(path.slice(0, end));
    }
    for (const path of incoming) {
      const name = key(path);
      let obstructed = parents.has(name) || uncaptured.has(name);
      for (let end = name.indexOf("/"); !obstructed && end !== -1; end = name.indexOf("/", end + 1)) {
        obstructed = uncaptured.has(name.slice(0, end));
      }
      if (obstructed) throw new Error(`untracked or ignored content obstructs ${JSON.stringify(path)}; move it before rewinding`);
    }
  }
}
