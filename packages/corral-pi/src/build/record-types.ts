/** A local run of one project's configured gate on the Build worktree. */
export interface BuildGateResult {
  /** `G1`, `G2`, ...: unique within the session. */
  readonly attempt: string;
  /** The commit the gate ran on. */
  readonly head: string;
  /** Digest of ordered commands and configured project env. Legacy receipts lack this and cannot be reused. */
  readonly input_fingerprint?: string;
  /** `skipped` when the project configures no gate commands. */
  readonly result: "pass" | "fail" | "skipped";
  readonly failed_command?: string;
  readonly commands: readonly string[];
  /** RFC 3339. */
  readonly started: string;
  readonly ended: string;
  /** Absolute path of the combined command log; absent when skipped. */
  readonly log?: string;
}

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

export interface ReviewEvidence {
  /** Null for an advisory review that did not prepare a landing candidate. */
  readonly scope: string | null;
  readonly context: string;
  /** Host-certified replay of the original pair, not a separate reviewer verdict. */
  readonly replay?: { readonly base: string; readonly head: string };
}

export interface RecordReview {
  /** 1-based. */
  readonly round: number;
  /** The review child's id. */
  readonly reviewer: string;
  readonly model: string;
  /** Integration tip used for the reviewed diff. Older reviews did not record it. */
  readonly base?: string;
  /** The commit reviewed. */
  readonly head: string;
  readonly evidence?: ReviewEvidence;
  readonly verdict: "approve" | "changes";
  readonly at: string;
  readonly summary?: string;
  readonly findings: readonly RecordFinding[];
}

export interface RecordMessage {
  readonly at: string;
  /** A child id, `parent` or `owner` in this session. */
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
 * The locally tracked record of one in-session build.
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
  readonly verification: readonly Omit<BuildGateResult, "commands">[];
  readonly reviews: readonly RecordReview[];
  readonly messages: readonly RecordMessage[];
  readonly decisions: readonly RecordDecision[];
  readonly usage: { readonly parent: RecordUsage; readonly children: RecordUsage; readonly reviewers: RecordUsage; readonly total: RecordUsage };
  readonly timeline: readonly { readonly at: string; readonly phase: "started" | "children" | "verify" | "review" | "land" | "exit"; readonly detail?: string }[];
  readonly transcripts: { readonly parent?: string; readonly children: Readonly<Record<string, string>> };
}
