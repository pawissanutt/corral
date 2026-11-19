// Worker turn checkpoints (T-575). At each turn's start, before its prompt is recorded or any
// request is sent, and again when the turn settles, the host commits every non-ignored worktree
// file over HEAD through a private index (the worktree's own index is untouched), points a
// per-worktree ref at it and appends one durable `corral_checkpoint` session entry. A failed
// snapshot records its error and the turn continues.
import { copyFileSync, existsSync, mkdtempSync, rmSync, statSync, utimesSync } from "node:fs";
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
  readonly cwd: string;
  readonly run: string;
  /** Appends one durable record to the bound session. */
  record(entry: CheckpointRecord): void;
}

export class Checkpoints {
  constructor(private readonly host: CheckpointHost) {}

  private git(args: readonly string[], env: Record<string, string> = {}): string {
    return execFileSync("git", ["-C", this.host.cwd, ...args], {
      env: { ...process.env, ...env }, stdio: ["ignore", "pipe", "pipe"], timeout: GIT_TIMEOUT_MS, encoding: "utf8",
    }).trim();
  }

  private withIndex<T>(use: (env: Record<string, string>) => T): T {
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

  /** Takes one checkpoint synchronously, so nothing of the turn runs before it is recorded. */
  capture(phase: CheckpointPhase, turn: number, leaf?: string | null): CheckpointRecord {
    const key = `t${turn}-${phase}`;
    const id = `${this.host.run}-${key}`;
    const base = { id, phase, turn };
    let entry: CheckpointRecord;
    try {
      const head = this.git(["rev-parse", "--verify", "HEAD"]);
      const commit = this.withIndex(env => this.snapshot(`corral checkpoint ${id}`, head, env));
      // A rewound turn number can be reused without dropping an earlier snapshot.
      const ref = `refs/worktree/corral/checkpoints/${this.host.run}/${key}-${commit}`;
      this.git(["update-ref", ref, commit]);
      entry = { ...base, commit, head, ref, ...(phase === "start" && leaf ? { leaf } : {}) };
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error);
      entry = { ...base, error: reason.slice(0, ERROR_MAX_CHARS) };
    }
    this.host.record(entry);
    return entry;
  }

  restore(point: RewindPoint): Rescue {
    const head = this.git(["rev-parse", "--verify", "HEAD"]);
    const at = new Date().toISOString().replace(/[:.]/g, "-");
    const ref = `refs/worktree/corral/rescue/${at}-${crypto.randomUUID()}`;
    return this.withIndex(env => {
      // An index-only version may not exist in the worktree or HEAD. Keep it reachable too,
      // as the second parent, without creating an extra commit for an unchanged index.
      const staged = this.git(["write-tree"], env);
      const original = this.git(["rev-parse", "HEAD^{tree}"]);
      const indexCommit = staged === original ? undefined
        : this.git(["commit-tree", staged, "-p", head, "-m", "corral rewind: staged index"], IDENTITY);
      const commit = this.snapshot(`corral rewind to turn ${point.turn}`, head, env, indexCommit);
      this.git(["update-ref", ref, commit]);
      try {
        // The private index describes the rescued worktree. Two-tree checkout removes paths
        // absent at the checkpoint without overwriting uncaptured (e.g. ignored) files.
        this.git(["read-tree", "-u", "-m", commit, point.commit], env);
        this.git(["reset", "--mixed", "--no-refresh", point.head]);
      } catch (error) {
        throw new Error(`rewind could not restore the checkpoint; saved work is at ${ref}: ${String(error)}`);
      }
      return { ref, head, commit };
    });
  }
}
