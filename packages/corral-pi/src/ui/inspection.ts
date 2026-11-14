import { z } from "zod";
import { UsagePanelSchema } from "./usage-model.ts";
import { readState, type ReadState } from "./read-state.ts";
import type { HostIdentity } from "../roles/types.ts";

const Text = z.string().max(16_384);
const Sha = z.string().regex(/^[a-f0-9]{40,64}$/);
const InspectionSchema = z.object({
  version: z.literal(1), task: Text, run: Text, root: Text, base: Sha, head: Sha, dirty: z.boolean(),
  usage: UsagePanelSchema.optional(),
  manifest_digest: Text.nullable(), audited: z.boolean(), manifest_failures: z.array(Text).max(100),
  task_status: Text,
  criteria: z.array(z.object({
    id: Text, required: z.boolean(), live: z.boolean(), scenario: Text, expected: Text,
    implemented: z.boolean(), failures: z.array(Text).max(100),
    observation: z.object({
      key: Text, authority: z.literal("worker_assertion"), status: z.enum(["pass", "fail", "not_run"]),
      observed: Text, action: Text, environment: Text, captured: Text, head: Sha,
      manifest_digest: Text, artifacts: z.array(Text).max(500),
    }).nullable(),
  })).max(100),
  gate: z.object({ result: z.enum(["pass", "fail", "running", "stale"]), tier: Text.nullable(), fresh: z.boolean(), log_passed: z.boolean(), head: Sha, attempt: Text.nullable() }).nullable(),
  review: z.object({ verdict: Text.nullable(), fresh: z.boolean(), head: Sha, reviewer: Text, notes: Text.nullable() }).nullable(),
  findings: z.array(z.object({ run: Text, attempt: Text, head: Sha, verdict: Text.nullable(), superseded: z.boolean(), notes: Text })).max(20),
  findings_omitted: z.number().int().nonnegative(),
  artifacts: z.array(z.object({ path: Text, title: Text, sha256: Text })).max(100),
  artifacts_omitted: z.number().int().nonnegative(), worker_summary: Text.nullable(),
});
export type Inspection = z.infer<typeof InspectionSchema>;

/** One cached, validated projection. Rendering never does I/O. No client-side freshness joins. */
export class InspectionStore {
  current: Inspection | undefined;
  error: string | undefined;
  stale = true; refreshing = false; checkedAt: number | undefined; failedAt: number | undefined;
  private active: Promise<void> | undefined; private version = 0; private disposed = false;
  constructor(private readonly identity: HostIdentity, private readonly load: (() => Promise<unknown>) | undefined, private readonly changed: () => void) {}
  get applicable(): boolean { return !!this.load && !!this.identity.task && !!this.identity.run; }
  get state(): ReadState {
    return readState({ applicable: this.applicable, reason: !this.load ? "no_reader" : "no_task_run",
      hasData: !!this.current, stale: this.stale, refreshing: this.refreshing,
      checkedAt: this.checkedAt, error: this.error, failedAt: this.failedAt });
  }
  invalidate(): void { if (!this.applicable || this.disposed) return; this.version++; this.stale = true; this.changed(); }
  refresh(force = false): Promise<void> {
    if (this.disposed || !this.applicable) return Promise.resolve();
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
      const data = InspectionSchema.parse(await this.load!());
      if (data.task !== this.identity.task || data.run !== this.identity.run) throw new Error("Inspection identity mismatch");
      if (this.disposed || version !== this.version) return;
      this.current = data; this.error = undefined; this.failedAt = undefined; this.stale = false; this.checkedAt = Date.now();
    } catch (error) {
      if (this.disposed || version !== this.version) return;
      this.error = error instanceof Error ? error.message : String(error); this.failedAt = Date.now(); this.stale = true;
    } finally { this.refreshing = false; if (!this.disposed) this.changed(); }
  }
  dispose(): void { this.disposed = true; this.version++; }
}
