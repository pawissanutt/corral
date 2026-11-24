// The bound task's cached, validated read of `corral show --json`: the task, its runs, the newest checks and its
// artifacts. The engine is canonical; this store only holds the last good read, its failure and its freshness. Task,
// Proof and the committed candidate in Changes read it; rendering never does I/O.
import { z } from "zod";
import type { HostIdentity } from "../roles/types.ts";
import { readState, type ReadState } from "./read-state.ts";

const Text = z.string().max(65_536);
const Optional = Text.nullable().optional();
const TaskSchema = z.object({
  id: Text, title: Text, body: Text, status: Text, priority: Text,
  project: Optional, kind: Optional, size: Optional, profile: Optional, base: Optional, parent: Optional,
  depends_on: z.array(Text).max(200).default([]),
  blocked: z.object({ reason: Text, detail: Optional }).nullable().optional(),
  round: z.number().int().nonnegative().optional(),
  created: Optional, updated: Optional,
});
const RunSchema = z.object({
  id: Text, role: Text, profile: Text, model: Optional, effort: Optional,
  worktree: Text, branch: Text, base_sha: Text,
  started: Text, ended: Optional, outcome: Optional, summary: Optional, head_sha: Optional,
});
const CheckSchema = z.object({
  kind: z.enum(["gate", "review"]), key: Text, result: z.enum(["pass", "fail"]),
  notes: Optional, log: Optional, by: Text, at: Text,
});
/** The engine's artifact kinds; never null. */
export const ARTIFACT_KINDS = ["reference", "report", "evidence", "diff", "summary", "gate", "log", "usage", "tool", "legacy"] as const;
export type ArtifactKind = (typeof ARTIFACT_KINDS)[number];
/** Reference -> Reference; report -> Deliverable; evidence -> Proof; diff, summary -> Submission; the rest -> Record. */
export function artifactGroup(kind: ArtifactKind): "Reference" | "Deliverable" | "Proof" | "Submission" | "Record" {
  switch (kind) {
    case "reference": return "Reference";
    case "report": return "Deliverable";
    case "evidence": return "Proof";
    case "diff": case "summary": return "Submission";
    default: return "Record";
  }
}
/** Shown by default: not a Submission or Record row, and not replaced. Lists and counts share this. */
export function artifactShown(a: { readonly kind: ArtifactKind; readonly replaced_by?: string | null | undefined }): boolean {
  const group = artifactGroup(a.kind);
  return group !== "Submission" && group !== "Record" && !a.replaced_by;
}
const ArtifactSchema = z.object({
  task: Text, id: z.number().int().positive(), sha256: Text, bytes: z.number().int().nonnegative(),
  kind: z.enum(ARTIFACT_KINDS), title: Text, at: Text, by: Text, replaced_by: Optional,
});
const ViewSchema = z.object({
  task: TaskSchema,
  runs: z.array(RunSchema).max(200).default([]),
  checks: z.array(CheckSchema).max(200).default([]),
  artifacts: z.array(ArtifactSchema).max(500).default([]),
});
export type TaskView = z.infer<typeof ViewSchema>;
export type TaskRecord = TaskView["task"];
export type TaskRun = TaskView["runs"][number];
export type TaskCheck = TaskView["checks"][number];
export type TaskArtifact = TaskView["artifacts"][number];

/** `T-42:3`, the reference `corral artifact show` reads. */
export function artifactRef(artifact: TaskArtifact): string {
  return `${artifact.task}:${artifact.id}`;
}

/** One cached, validated read. Rendering never does I/O. */
export class TaskStore {
  current: TaskView | undefined;
  error: string | undefined;
  /** When the last failed read failed; set with `error`, cleared by the next successful read. */
  failedAt: number | undefined;
  stale = true; refreshing = false; checkedAt: number | undefined;
  private active: Promise<void> | undefined; private version = 0; private disposed = false;
  constructor(private readonly identity: HostIdentity, private readonly load: (() => Promise<unknown>) | undefined, private readonly changed: () => void) {}
  get bound(): boolean { return !!this.identity.task && !!this.load; }
  get state(): ReadState {
    return readState({
      applicable: this.bound, reason: this.identity.task ? "no_reader" : "no_task", hasData: this.current !== undefined,
      stale: this.stale, refreshing: this.refreshing, checkedAt: this.checkedAt, error: this.error, failedAt: this.failedAt,
    });
  }
  /** This session's run in the last read; undefined while unread or when the record does not hold it. */
  get run(): TaskRun | undefined {
    const id = this.identity.run;
    return id === undefined ? undefined : this.current?.runs.find(run => run.id === id);
  }
  /** Marks the held read stale (the worktree changed); a read already in flight cannot clear it. */
  invalidate(): void {
    if (!this.bound || this.disposed) return;
    this.version++; this.stale = true; this.changed();
  }
  refresh(force = false): Promise<void> {
    if (this.disposed || !this.bound) return Promise.resolve();
    if (this.active) return this.active;
    if (!force && !this.stale && this.checkedAt && Date.now() - this.checkedAt < 5000) return Promise.resolve();
    const operation = this.read(); this.active = operation;
    void operation.finally(() => { if (this.active === operation) this.active = undefined; });
    return operation;
  }
  private async read(): Promise<void> {
    const version = this.version;
    this.refreshing = true; this.changed();
    try {
      if (!this.load) throw new Error("No task bound to this session");
      const data = ViewSchema.parse(await this.load());
      if (data.task.id !== this.identity.task) throw new Error("Task identity mismatch");
      if (this.disposed || version !== this.version) return;
      this.current = data; this.error = undefined; this.failedAt = undefined; this.stale = false; this.checkedAt = Date.now();
    } catch (error) {
      if (this.disposed || version !== this.version) return;
      this.error = error instanceof z.ZodError ? "the read returned data of an unexpected shape" : error instanceof Error ? error.message : String(error);
      this.failedAt = Date.now(); this.stale = true;
    } finally { this.refreshing = false; if (!this.disposed) this.changed(); }
  }
  dispose(): void { this.disposed = true; this.version++; }
}
