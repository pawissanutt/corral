import { z } from "zod";
import { UsagePanelSchema } from "./usage-view.ts";
import type { HostIdentity } from "../roles/types.ts";
import { plainLabel, type PanelLine } from "./panels-view.ts";

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
  stale = true; refreshing = false; checkedAt: number | undefined;
  private active: Promise<void> | undefined; private version = 0; private disposed = false;
  constructor(private readonly identity: HostIdentity, private readonly load: (() => Promise<unknown>) | undefined, private readonly changed: () => void) {}
  invalidate(): void { this.version++; this.stale = true; this.changed(); }
  refresh(force = false): Promise<void> {
    if (this.disposed) return Promise.resolve();
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
      if (!this.load || !this.identity.task || !this.identity.run) throw new Error("No task/run inspection bound to this session");
      const data = InspectionSchema.parse(await this.load());
      if (data.task !== this.identity.task || data.run !== this.identity.run) throw new Error("Inspection identity mismatch");
      if (this.disposed || version !== this.version) return;
      this.current = data; this.error = undefined; this.stale = false; this.checkedAt = Date.now();
    } catch (error) {
      if (this.disposed || version !== this.version) return;
      this.error = error instanceof Error ? error.message : String(error); this.stale = true;
    } finally { this.refreshing = false; if (!this.disposed) this.changed(); }
  }
  dispose(): void { this.disposed = true; this.version++; }
}
export interface EvidenceLine extends PanelLine { readonly criterion?: number }
export function evidenceLines(store: InspectionStore, selected: number, width: number): EvidenceLine[] {
  const rows: EvidenceLine[] = [];
  const add = (text: string, heading = false, criterion?: number) => rows.push({ text: plainLabel(text), heading, criterion });
  add("Evidence · read-only canonical inspection", true);
  add("j/k select criterion · r reconcile · F7 changes · F9 conversation");
  if (store.stale || store.error) add("STALE / unavailable: " + (store.error ?? "canonical read required; retained rows are historical"));
  if (store.refreshing) add("Checking canonical records… retained results are not a new check");
  const data = store.current;
  if (!data) { add("Evidence unavailable · no acceptance inferred"); return rows; }
  add(`${data.task} / ${data.run} · task ${data.task_status} · read ${store.checkedAt ? new Date(store.checkedAt).toISOString() : "unavailable"}`);
  add(`Comparison base ${data.base}`); add(`Candidate HEAD ${data.head}`);
  add(data.dirty ? "Uncommitted edits excluded from this candidate; worktree dirty" : "Committed candidate · clean worktree at read");
  if (!data.manifest_digest) add("Legacy / no criterion manifest · acceptance unknown, not zero criteria passed");
  else add(`Manifest ${data.manifest_digest} · ${data.audited ? "audited" : "not audited"}`);
  for (const failure of data.manifest_failures) add(`Manifest: ${failure}`);
  const gate = data.gate, review = data.review;
  add(gate ? `Gate receipt: ${gate.result} · ${gate.fresh && !store.stale ? "fresh at read" : "STALE / retained"} · log ${gate.log_passed ? "passed" : "not passed"} · tier ${gate.tier ?? "legacy"} · ${gate.attempt ?? "legacy identity"} · ${gate.head}` : "Gate: missing receipt");
  add(review ? `Review (candidate-wide): ${review.verdict ?? "pending"} · ${review.fresh && !store.stale ? "fresh at read" : "STALE / legacy / retained"} · ${review.reviewer} · ${review.head}` : "Review: missing record");
  add("Implemented ≠ tested ≠ reviewed ≠ accepted. No per-criterion acceptance record exists.");
  add("Test observations below are worker_assertion, never machine PASS.");
  if (width >= 90) add("Criterion │ Implemented │ Tested assertion / canonical proof │ Reviewed │ Accepted", true);
  const reviewed = !review ? "missing" : store.stale ? "retained" : !review.fresh ? "stale" : review.verdict ?? "pending";
  data.criteria.forEach((c, index) => {
    const proof = store.stale ? "retained / not current" : c.failures.length || data.manifest_failures.length ? "invalid / " + [...data.manifest_failures, ...c.failures].join(", ") : c.observation ? "admitted at read*" : "missing";
    const implemented = c.implemented ? "done*" : "not claimed";
    if (width >= 90) add(`${index === selected ? ">" : " "} ${c.id} │ ${implemented} │ ${c.observation?.status ?? "missing"} / ${proof} │ ${reviewed}* │ not recorded`, false, index);
    else {
      add(`${index === selected ? ">" : " "} ${c.id} · implemented ${implemented}`, true, index);
      add(`Tested: ${c.observation?.status ?? "missing"} · ${proof}`, false, index);
      add(`Reviewed: ${reviewed} (candidate-wide) · Accepted: not recorded`, false, index);
    }
  });
  const c = data.criteria[selected];
  if (c) {
    add(`Inspect ${c.id} · ${c.required ? "required" : "optional"}${c.live ? " · live proof required" : ""}`, true);
    add(`Scenario: ${c.scenario}`); add(`Expected: ${c.expected}`);
    add("Implemented: self-reported canonical progress only; not an independent check");
    const obs = c.observation;
    if (!obs) add("Missing proof · no observation recorded");
    else {
      add(`worker_assertion ${obs.key} · ${obs.status} · captured ${obs.captured}`);
      add(`Observation HEAD ${obs.head} · manifest ${obs.manifest_digest}`);
      add(`Action: ${obs.action} · environment: ${obs.environment}`); add(`Observed: ${obs.observed}`);
      for (const path of obs.artifacts) {
        const artifact = data.artifacts.find(a => a.path === path);
        add(`Artifact ${path} · ${artifact ? artifact.title + " · registered SHA256 " + artifact.sha256 : "metadata unavailable / omitted"}`);
      }
      if (!obs.artifacts.length) add("Artifacts: missing");
    }
    for (const failure of c.failures) add(`Proof: ${failure}`);
  }
  if (review?.notes) { add("Latest review notes · candidate-wide, not criterion acceptance", true); add(review.notes); }
  add("Review findings · historical candidate identities, not current criterion acceptance", true);
  if (!data.findings.length) add("No retained review findings");
  for (const f of data.findings) {
    add(`${f.run}/${f.attempt} · ${f.superseded ? "SUPERSEDED" : "recorded; applicability requires review"} · ${f.verdict ?? "pending"} · ${f.head}`, true);
    add(f.notes);
  }
  if (data.findings_omitted) add(`${data.findings_omitted} older findings omitted`);
  if (data.artifacts_omitted) add(`${data.artifacts_omitted} artifact metadata rows omitted; canonical validation still checked referenced bytes`);
  add("Run artifacts · registered metadata, not independent proof", true);
  if (!data.artifacts.length) add("No run artifacts registered");
  for (const artifact of data.artifacts) add(`${artifact.title} · ${artifact.path} · registered SHA256 ${artifact.sha256}`);
  if (data.worker_summary) { add("Worker summary · assertion, not acceptance", true); add(data.worker_summary); }
  return rows;
}
