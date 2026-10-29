// Worker turn checkpoints (T-575). At each turn's start, before its prompt is recorded or any
// request is sent, and again when the turn settles, the host commits every non-ignored worktree
// file over HEAD through a private index (the worktree's own index is untouched), points a
// per-worktree ref at it and appends one durable `corral_checkpoint` session entry. A start
// entry also names the session leaf the prompt goes under, which `corral agent rewind` restores
// as the next generation's cursor. A failed snapshot records its error and the turn continues.
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
  readonly generation: number;
  readonly turn: number;
  readonly commit?: string;
  readonly head?: string;
  readonly ref?: string;
  readonly leaf?: string;
  readonly error?: string;
}

export interface CheckpointHost {
  readonly cwd: string;
  readonly generation: number;
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

  private snapshot(message: string): { commit: string; head: string } {
    const head = this.git(["rev-parse", "--verify", "HEAD"]);
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
      this.git(["add", "-A"], { GIT_INDEX_FILE: index });
      const tree = this.git(["write-tree"], { GIT_INDEX_FILE: index });
      return { commit: this.git(["commit-tree", tree, "-p", head, "-m", message], IDENTITY), head };
    } finally {
      rmSync(scratch, { recursive: true, force: true });
    }
  }

  /** Takes one checkpoint synchronously, so nothing of the turn runs before it is recorded. */
  capture(phase: CheckpointPhase, turn: number, leaf?: string | null): CheckpointRecord {
    const id = `g${this.host.generation}-t${turn}`;
    const base = { id, phase, generation: this.host.generation, turn };
    let entry: CheckpointRecord;
    try {
      const { commit, head } = this.snapshot(`corral checkpoint ${id} ${phase}`);
      const ref = `refs/worktree/corral/checkpoints/${this.host.run}/${id}-${phase}`;
      this.git(["update-ref", ref, commit]);
      entry = { ...base, commit, head, ref, ...(phase === "start" && leaf ? { leaf } : {}) };
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error);
      entry = { ...base, error: reason.slice(0, ERROR_MAX_CHARS) };
    }
    this.host.record(entry);
    return entry;
  }
}
