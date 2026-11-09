// Advisory lifecycle controller. The service is canonical; handles are never resumed/replayed.
// Public /plan choices and native presentation are owned by the choice-experience consumer.
import type { AuxiliaryHandle, AuxiliaryRuntime, PlanReviewAssociation, ReviewMission } from "../extensions/subagent/runtime.ts";
import { sha256Hex } from "../tools/corral.ts";
import { utf8Prefix } from "../roles/assistant/owner-action.ts";
import type { WorkflowExpected } from "./planning-policy.ts";

type RecordValue = Record<string, unknown>;
export interface LocalReviewProjection extends RecordValue {
  readonly review: string; readonly workflow: string; readonly revision: number; readonly digest: string;
  readonly invocation: string; readonly lifecycle: string; readonly result_revision: number;
  readonly notice_delivered: boolean;
  readonly plan: { blob: string; bytes: number }; readonly snapshot_file: string;
  readonly context: { blob: string; bytes: number }; readonly context_file: string;
}
interface Port {
  readonly auxiliary?: AuxiliaryRuntime;
  call(id: string, payload: RecordValue): Promise<unknown>;
  event(review: LocalReviewProjection, event: RecordValue): Promise<unknown>;
  notify(key: string, text: string): Promise<boolean>;
  error(error: unknown): void;
}
const live = new Set(["prepared", "queued", "running"]);
function projection(raw: unknown): LocalReviewProjection {
  const r = raw as LocalReviewProjection;
  if (!r || !/^LR[1-9][0-9]*$/.test(r.review) || !/^W[1-9][0-9]*$/.test(r.workflow)
      || !/^[a-f0-9]{64}$/.test(r.digest) || !/^[a-f0-9]{64}$/.test(r.invocation)
      || !Number.isSafeInteger(r.result_revision)) throw new Error("invalid canonical local review projection");
  return r;
}
function mission(r: LocalReviewProjection): ReviewMission {
  const digest = (blob: string) => {
    if (!/^blob:sha256:[a-f0-9]{64}$/.test(blob)) throw new Error("invalid immutable review reference");
    return blob.slice("blob:sha256:".length);
  };
  return { association: { workflow: r.workflow, review: r.review, digest: r.digest },
    plan: { path: r.snapshot_file, digest: digest(r.plan.blob), bytes: r.plan.bytes },
    context: { path: r.context_file, digest: digest(r.context.blob), bytes: r.context.bytes } };
}
export function reviewAssociation(r: LocalReviewProjection): PlanReviewAssociation {
  return { workflow: r.workflow, review: r.review, digest: r.digest };
}
export class PlanReviews {
  private readonly handles = new Map<string, { handle: AuxiliaryHandle; settled: Promise<void> }>();
  private readonly admissions = new Map<string, Promise<LocalReviewProjection>>();
  private stopped = false;
  constructor(private readonly port: Port) {}

  /** Return after durable claim, never await inference or borrow the start tool's signal. */
  start(id: string, expected: WorkflowExpected, input: string): Promise<LocalReviewProjection> {
    const key = `${expected.workflow}/${input}`;
    const prior = this.admissions.get(key);
    if (prior) return prior;
    const result = this.startOnce(id, expected, input);
    this.admissions.set(key, result);
    return result;
  }
  private async startOnce(id: string, expected: WorkflowExpected, input: string): Promise<LocalReviewProjection> {
    if (this.stopped) throw new Error("local review controller closed");
    return this.launch(await this.port.call(id, { action: "start", expected, input }));
  }
  async launch(raw: unknown): Promise<LocalReviewProjection> {
    const r = projection(raw);
    if (r.lifecycle !== "prepared" || this.handles.has(r.invocation)) return r;
    if (!this.port.auxiliary) return projection(await this.port.event(r, { action: "settle", lifecycle: "unavailable", summary: "Explore capability/model unavailable; no child started." }));
    const claim = await this.port.event(r, { action: "claim" }) as RecordValue;
    if (claim.claimed !== true) return r;
    const claimed = projection(claim);
    if (this.stopped) return projection(await this.port.event(r, { action: "recover" }));
    const handle = this.port.auxiliary.startReview(r.invocation, mission(r));
    const settled = this.finish(claimed, handle).catch(error => this.port.error(error));
    this.handles.set(r.invocation, { handle, settled });
    return claimed;
  }
  async read(id: string, review?: string, offset = 0): Promise<unknown> {
    return this.port.call(id, { action: "read", ...(review ? { review } : {}), offset });
  }
  async cancel(id: string, expected: WorkflowExpected, review: string): Promise<unknown> {
    const r = projection(await this.port.call(id, { action: "cancel", expected, review }));
    this.cancelOwned(r);
    // Cancel returns the terminal state promptly; settlement/accounting may still be draining.
    return r;
  }
  cancelOwned(raw: unknown): void {
    const r = projection(raw);
    this.handles.get(r.invocation)?.handle.cancel();
  }
  private async finish(r: LocalReviewProjection, handle: AuxiliaryHandle): Promise<void> {
    let lifecycle = "failed", summary = "No usable local review result.", text: string | undefined;
    try {
      const result = await handle.done;
      text = result.text;
      lifecycle = result.outcome === "completed" && result.report?.status === "completed" && text.trim() && !result.truncated
        ? "completed" : result.report?.stopReason === "timeout" ? "timed_out" : result.report?.stopReason === "cancelled" ? "cancelled"
        : result.report?.stopReason === "provider_error" ? "failed" : "incomplete";
      summary = `Advisory review of r${r.revision}: ${lifecycle}. ${result.report?.stopReason ?? result.outcome}.`;
      if (Buffer.byteLength(text) > 1024 * 1024) { lifecycle = "incomplete"; summary = "Findings exceed 1 MiB; inspect auxiliary transcript. No successful review claimed."; text = undefined; }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      lifecycle = /Research timeout/.test(message) ? "timed_out" : /Research cancelled/.test(message) ? "cancelled"
        : /disabled|model_unavailable|not configured|budget_exceeded|quota_exceeded/.test(message) ? "unavailable" : "failed";
      summary = utf8Prefix(message, 1024);
    }
    if (text?.trim()) {
      let offset = 0;
      for (const part of chunks(text)) {
        await this.port.event(r, { action: "stage", offset, chunk: part });
        offset += Buffer.byteLength(part);
      }
    } else if (lifecycle === "completed") lifecycle = "incomplete";
    const terminal = projection(await this.port.event(r, { action: "settle", lifecycle, summary,
      ...(text?.trim() ? { bytes: Buffer.byteLength(text), digest: sha256Hex(text) } : {}) }));
    await this.deliver(terminal);
  }
  private async deliver(r: LocalReviewProjection): Promise<void> {
    if (live.has(r.lifecycle) || r.notice_delivered) return;
    const key = `local-plan-review:${r.workflow}:${r.review}:${r.result_revision}`;
    const text = `Local advisory ${r.review} ${r.lifecycle}; reviewed r${r.revision} (${r.digest}). `
      + "Read corral_assistant_plan_review for findings and current/stale/source labels. Not board approval or implementation consent; do not automatically re-review.";
    if (await this.port.notify(key, text)) await this.port.event(r, { action: "delivered", result_revision: r.result_revision });
  }
  /** Reconcile every row, including old terminal notices; never relaunch even a prepared row. */
  async recover(): Promise<void> {
    const list = await this.port.call("review-recovery-list", { action: "read" }) as { count?: number };
    if (!Number.isSafeInteger(list.count) || Number(list.count) < 0 || Number(list.count) > 4096) return;
    for (let i = 1; i <= Number(list.count); i++) {
      let r = projection(await this.port.call(`review-recovery-${i}`, { action: "read", review: `LR${i}` }));
      if (this.handles.has(r.invocation)) continue;
      r = projection(await this.port.event(r, { action: "recover" }));
      await this.deliver(r);
    }
  }
  cancelAll(): void {
    for (const { handle } of this.handles.values()) handle.cancel();
  }
  async close(reload = false): Promise<void> {
    this.stopped = true;
    this.cancelAll();
    await Promise.allSettled([...this.handles.values()].map(item => item.settled));
    if (reload) { this.handles.clear(); this.admissions.clear(); this.stopped = false; }
  }
}
function* chunks(text: string): Generator<string> {
  // <=16 KiB in UTF-8, never split a surrogate pair/codepoint.
  let part = "", bytes = 0;
  for (const point of text) {
    const size = Buffer.byteLength(point);
    if (bytes + size > 16384) { yield part; part = ""; bytes = 0; }
    part += point; bytes += size;
  }
  if (part) yield part;
}
