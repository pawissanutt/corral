// All provider streams (including retries, compaction and auxiliary sessions) pass here after
// credential restrictions. The authoritative balance lives in Corral, outside every transcript.
import { randomUUID, createHash } from "node:crypto";
import { calculateCost, type Api, type Model, type StreamFunction, type Usage } from "@earendil-works/pi-ai";
import { lazyStream } from "@earendil-works/pi-ai/api/lazy";

export interface BudgetReply extends Record<string, unknown> {
  admitted?: boolean;
  exhausted?: boolean;
}
export interface BudgetOptions {
  /** A subprocess lane can perform hidden additional requests; it has no finite hard bound. */
  readonly unbounded?: boolean;
  record(payload: Record<string, unknown>): Promise<BudgetReply>;
  cancel(): void;
  onStop?(): void;
}
function integer(value: number): number {
  if (!Number.isSafeInteger(value) || value < 0) throw new Error("budget_unbounded: model/usage has no finite safe bound");
  return value;
}
function nanos(value: number): number { return integer(Math.ceil(value * 1e9)); }

// Reserve the entire model context plus a bounded output, not a tokenizer estimate. This covers
// image inputs and provider serialization overhead. Admission may conservatively refuse a request
// that would actually cost less. Pricing tiers and 1h cache writes are bounded at the highest rate.
export function reservation(model: Model<Api>, output: number) {
  const input = integer(model.contextWindow);
  output = integer(output);
  if (!input || !output) throw new Error("budget_unbounded: missing context/output bound");
  const rates = [model.cost, ...model.cost.tiers ?? []];
  const prices = rates.flatMap((r) => [r.input, r.output, r.cacheRead, r.cacheWrite]);
  if (prices.some((v) => !Number.isFinite(v) || v < 0)) throw new Error("budget_unpriced: invalid model prices");
  const priced = model.provider === "llama.cpp" || prices.some((v) => v > 0);
  const inputRate = Math.max(...rates.flatMap((r) => [r.input, r.cacheRead, r.cacheWrite * 2]));
  const outputRate = Math.max(...rates.map((r) => r.output));
  return { tokens: integer(input + output), usd_nanos: nanos((input * inputRate + output * outputRate) / 1e6), priced };
}
function actual(model: Model<Api>, usage: Usage) {
  const tokens = integer(integer(usage.input) + integer(usage.output) + integer(usage.cacheRead) + integer(usage.cacheWrite));
  // Freeze this request's model pricing, rather than repricing history on resume.
  return { tokens, usd_nanos: nanos(calculateCost(model, structuredClone(usage)).total) };
}
export class RunBudget {
  private readonly active = new Set<AbortController>();
  private stopped = false;
  private last: BudgetReply | undefined;
  constructor(private readonly options: BudgetOptions) {}
  private limited(): boolean {
    const limits = this.last?.limits as { max_tokens?: number | null; max_usd_nanos?: number | null } | undefined;
    return limits === undefined || limits.max_tokens != null || limits.max_usd_nanos != null;
  }
  private stop() {
    if (this.stopped) return;
    this.stopped = true;
    this.options.onStop?.();
    for (const controller of this.active) controller.abort(new Error("budget_exhausted"));
    this.options.cancel();
  }
  async refresh(): Promise<void> {
    this.last = await this.options.record({ kind: "budget", action: "state", request: randomUUID() });
    if (this.last.exhausted) this.stop();
  }
  wrap(stream: StreamFunction): StreamFunction {
    return (model, context, options) => lazyStream(model, async () => {
      if (this.stopped) throw new Error("budget_exhausted: explicit resume with increased configured limits is required");
      options?.signal?.throwIfAborted();
      const request = randomUUID();
      const output = Math.min(options?.maxTokens ?? model.maxTokens, model.maxTokens);
      const bound = reservation(model, output);
      const controller = new AbortController();
      this.active.add(controller);
      let admitted = false;
      try {
        this.last = await this.options.record({ kind: "budget", action: "reserve", request, ...bound, unbounded: this.options.unbounded === true });
        if (this.last.admitted !== true) {
          if (this.limited()) {
            this.stop();
            throw new Error("budget_exhausted: reservation refused (remaining balance, uncertainty, unbounded transport or pricing gap); inspect budget state");
          }
          throw new Error("budget_accounting_refused: unlimited reservation could not be recorded");
        }
        admitted = true;
        if (this.last.exhausted) this.stop();
        if (this.stopped) controller.abort(new Error("budget_exhausted"));
        const signal = AbortSignal.any([controller.signal, ...(options?.signal ? [options.signal] : [])]);
        signal.throwIfAborted();
        const source = stream(model, context, { ...options, maxTokens: output, signal });
        const self = this;
        return (async function* () {
          try {
            for await (const event of source) {
              // Only successful terminal usage is known complete. Cancellation/error/deferred work
              // retains its reservation even if the provider emitted partial usage or a zero bill.
              if (event.type === "done" && event.reason !== "deferred") {
                const message = event.message;
                const response = createHash("sha256").update(JSON.stringify([
                  model.provider, model.id, message.responseId ?? request,
                ])).digest("hex");
                const usage = actual(model, message.usage);
                if (usage.tokens > 0) {
                  self.last = await self.options.record({ kind: "budget", action: "settle", request, response, ...usage });
                  if (self.last.exhausted) self.stop();
                }
              }
              yield event;
            }
          } finally { self.active.delete(controller); }
        })();
      } catch (error) {
        this.active.delete(controller);
        // Once admission has committed, even a local setup failure is conservatively uncertain.
        if (admitted) this.last = { ...this.last, uncertain: true };
        throw error;
      }
    });
  }
  state() {
    return { ...this.last, stopped: this.stopped, active: this.active.size,
      unbounded_transport: this.options.unbounded === true,
      reservation_policy: "full-model-context-plus-output", cancellation: "best-effort; provider billing may lag" };
  }
}
