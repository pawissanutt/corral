// Wire contracts between the corral-pi host and `corral build gate|land|record|list|show`.
// Snake case exactly as the JSON on the wire: the host writes these shapes, the corral binary
// stores and prints them, and the app reads the stored record. Types only.

/** stdout of `corral build gate --json`. Exit status 0 for pass, fail and skipped; non-zero only for a refusal. */
export interface GateReceipt {
  /** `G1`, `G2`, ...: unique within the session. */
  readonly attempt: string;
  /** `quick` or `full`. */
  readonly tier: string;
  /** The commit the gate ran on. */
  readonly head: string;
  /** `skipped` when the project configures no gate commands for the tier. */
  readonly result: "pass" | "fail" | "skipped";
  readonly failed_command?: string;
  readonly commands: readonly string[];
  /** RFC 3339. */
  readonly started: string;
  readonly ended: string;
  /** Absolute path of the combined command log; absent when skipped. */
  readonly log?: string;
  /** Absolute path of this receipt's JSON file. */
  readonly receipt: string;
}

/** stdout of `corral build land --json`. */
export interface LandResult {
  /** The commit the base branch now points at. */
  readonly landed: string;
  readonly base_branch: string;
  /** The base branch's commit before landing. */
  readonly previous: string;
}

/** stderr of a refused `corral build land` starts with one of these codes, then `: detail`. */
export type LandRefusal = "landing_stale" | "gate_missing" | "gate_stale" | "gate_failed" | "main_dirty" | "main_off_base" | "worktree_dirty" | "head_mismatch";

export interface RecordUsage {
  readonly input: number;
  readonly output: number;
  /** API-equivalent USD; absent when any contributing model has no price. */
  readonly cost?: number;
}

export interface RecordSlice {
  readonly id: string;
  readonly title: string;
  readonly kind: "slice" | "explore" | "review";
  readonly owns: readonly string[];
  /** `<provider>/<id>`. */
  readonly model: string;
  readonly effort?: string;
  readonly state: "queued" | "running" | "needs" | "done" | "failed" | "cancelled";
  readonly started?: string;
  readonly ended?: string;
  readonly usage: RecordUsage & { readonly turns: number };
  /** Repository-relative paths edited outside `owns`. */
  readonly outside: readonly string[];
  /** The child's final report, capped at 8,192 characters. */
  readonly report?: string;
}

export interface RecordFinding {
  /** `F1`, `F2`, ...: unique within the review round. */
  readonly id: string;
  readonly severity: "blocker" | "major" | "minor";
  /** Repository-relative. */
  readonly path?: string;
  readonly line?: number;
  readonly text: string;
  readonly resolution?: { readonly state: "fixed" | "declined" | "open"; readonly commit?: string; readonly reason?: string };
}

export interface RecordReview {
  /** 1-based. */
  readonly round: number;
  /** The review child's id. */
  readonly reviewer: string;
  readonly model: string;
  /** The commit reviewed. */
  readonly head: string;
  readonly verdict: "approve" | "changes";
  readonly at: string;
  readonly summary?: string;
  readonly findings: readonly RecordFinding[];
}

export interface RecordMessage {
  readonly at: string;
  /** A child id, `parent`, `owner`, or a cross-session recipient key such as `T-144_R2` or `pm`. */
  readonly from: string;
  /** A child id, `parent`, `all`, or a cross-session recipient key. */
  readonly to: string;
  /** Capped at 2,048 characters in the record. */
  readonly text: string;
  readonly delivery: "delivered" | "queued" | "undeliverable" | "answered_from_report";
}

export interface RecordDecision {
  readonly at: string;
  readonly kind: "question" | "exit" | "override";
  readonly text: string;
  readonly answer?: string;
}

/**
 * The tracked record of one in-session build: `corral build record --file <json>` upserts it by `id`.
 * Caps: `intent` 32 entries of 4,096 characters; `commits` 200; `messages` the newest 500.
 */
export interface BuildRecord {
  readonly version: 1;
  /** `B-<base36>` for a build session; `A-<n>-<slug>` for an assistant scratch finished in-session. Matches `^[AB]-[a-z0-9][a-z0-9-]{0,80}$`. */
  readonly id: string;
  readonly kind: "build" | "assistant";
  readonly project?: string;
  /** Model-written, at most 120 characters; never a generated slug. */
  readonly title: string;
  /** The owner's prompts verbatim, oldest first. */
  readonly intent: readonly { readonly at: string; readonly text: string }[];
  readonly created: string;
  readonly updated: string;
  readonly repo_root: string;
  readonly worktree: string;
  readonly branch: string | null;
  readonly base: string | null;
  readonly outcome: {
    readonly state: "open" | "landed" | "kept" | "abandoned";
    readonly at?: string;
    /** The session branch head at the outcome. */
    readonly head?: string;
    /** The base branch commit after landing. */
    readonly landed?: string;
    readonly base_branch?: string;
    /** Why the owner landed over open review findings. */
    readonly override_reason?: string;
  };
  readonly diff?: { readonly commits: number; readonly files: number; readonly added: number; readonly removed: number };
  readonly commits: readonly { readonly sha: string; readonly subject: string; readonly slice?: string }[];
  readonly slices: readonly RecordSlice[];
  readonly verification: readonly Omit<GateReceipt, "commands" | "receipt">[];
  readonly reviews: readonly RecordReview[];
  readonly messages: readonly RecordMessage[];
  readonly decisions: readonly RecordDecision[];
  readonly usage: { readonly parent: RecordUsage; readonly children: RecordUsage; readonly reviewers: RecordUsage; readonly total: RecordUsage };
  readonly timeline: readonly { readonly at: string; readonly phase: "started" | "children" | "verify" | "review" | "land" | "exit"; readonly detail?: string }[];
  readonly transcripts: { readonly parent?: string; readonly children: Readonly<Record<string, string>> };
}

/** stdout of `corral build record --json`. */
export interface RecordPublished {
  readonly id: string;
  /** Starts at 1 and grows by one per accepted upsert. */
  readonly revision: number;
}
