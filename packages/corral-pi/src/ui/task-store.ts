// The bound task's cached, validated read of `corral show --json`. The task record is canonical; this store only holds
// the last good read, its failure and its freshness. It is the only reader: rendering never does I/O.
import { z } from "zod";
import type { HostIdentity } from "../roles/types.ts";
import { readState, type ReadState } from "./read-state.ts";

const Text = z.string().max(65_536);
const TaskSchema = z.object({
  id: Text, title: Text, status: Text,
  status_since: Text.optional(), kind: Text.nullable().optional(), size: Text.nullable().optional(),
  difficulty: Text.nullable().optional(), priority: Text.nullable().optional(), project: Text.optional(),
  body: Text.nullable().optional(),
  criteria_progress: z.array(z.object({
    criterion: z.object({ id: Text, required: z.boolean(), scenario: Text, expected: Text }),
    steps: z.array(z.unknown()).optional(),
    evidence: z.array(z.unknown()).optional(),
  })).max(100).optional(),
  runs: z.array(z.object({
    id: Text, agent: Text.nullable().optional(), outcome: Text.nullable().optional(),
    agent_state: Text.nullable().optional(), started: Text.nullable().optional(), ended: Text.nullable().optional(),
    summary: Text.nullable().optional(),
  })).max(200).default([]),
  comments: z.array(z.object({ at: Text, author: Text, text: Text })).max(500).default([]),
});
export type TaskRecord = z.infer<typeof TaskSchema>;

/** One cached, validated read. Rendering never does I/O. */
export class TaskStore {
  current: TaskRecord | undefined;
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
      const data = TaskSchema.parse(await this.load());
      if (data.id !== this.identity.task) throw new Error("Task identity mismatch");
      if (this.disposed || version !== this.version) return;
      this.current = data; this.error = undefined; this.failedAt = undefined; this.stale = false; this.checkedAt = Date.now();
    } catch (error) {
      if (this.disposed || version !== this.version) return;
      this.error = error instanceof Error ? error.message : String(error); this.failedAt = Date.now(); this.stale = true;
    } finally { this.refreshing = false; if (!this.disposed) this.changed(); }
  }
  dispose(): void { this.disposed = true; this.version++; }
}
