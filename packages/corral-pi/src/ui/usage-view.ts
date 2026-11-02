// Read-only source tables. Never reprice history or interpret SDK cost as an invoice.
import { z } from "zod";
import type { InspectionStore } from "./inspection.ts";
import { plainLabel, type PanelLine } from "./panels-view.ts";
const Count = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
const Dollars = z.number().finite().nonnegative().nullable().optional();
const Usage = z.object({ input: Count, output: Count, cache_read: Count, cache_write: Count,
  reported_usd: Dollars, cost_usd: Dollars, estimated_usd: Dollars, unpriced_tokens: Count,
  cost_basis: z.enum(["reported", "estimated", "mixed"]).nullable() });
export const UsagePanelSchema = z.object({ source: z.string().max(16384),
  rows: z.array(z.object({ scope: z.string().max(16384), usage: Usage.nullable(),
    wall_seconds: z.number().int().nullable(), known_runs: Count, runs: Count })).max(102), models_omitted: Count });
const BudgetSchema = z.object({
  limits: z.object({ max_tokens: Count.nullable(), max_usd_nanos: Count.nullable() }),
  tokens: Count, usd_nanos: Count, uncertain_requests: Count, pricing_gaps: Count,
  legacy_usage_gap: z.boolean(), unbounded_requests: Count, exhausted: z.boolean(),
  stopped: z.boolean().optional(), uncertain: z.boolean().optional(), active: Count.optional(),
});
function dollars(value: number | null | undefined): string { return value == null ? "unknown" : `$${value.toFixed(6)}`; }
export function budgetLines(value: unknown): string[] {
  const result = BudgetSchema.safeParse(value);
  if (!result.success) return ["Budget accounting unavailable / legacy · not zero; configured limits unknown"];
  const b = result.data;
  return ["Configured run budget · canonical reservation accounting (not provider balance/quota)",
    `Token limit: ${b.limits.max_tokens ?? "not configured"} · USD limit: ${b.limits.max_usd_nanos == null ? "not configured" : dollars(b.limits.max_usd_nanos / 1e9)}`,
    `Settled + reserved: ${b.tokens} tokens · ${dollars(b.usd_nanos / 1e9)} accounting USD, not billed spend`,
    `Unresolved reservations: ${b.uncertain_requests} · pricing gaps: ${b.pricing_gaps} · unbounded unresolved: ${b.unbounded_requests}`,
    `Legacy usage gap: ${b.legacy_usage_gap ? "yes · historical spend unknown" : "not reported"} · ${b.exhausted ? "exhausted" : "not exhausted at last receipt"} · ${b.stopped ? "host stopped" : "host not stopped"}`,
    ...(b.uncertain || b.uncertain_requests || b.pricing_gaps || b.legacy_usage_gap || b.unbounded_requests
      ? ["UNCERTAIN spend · unresolved/interrupted requests retain reservations; unknown pricing is not free"] : []),
    "Cancellation is best-effort; provider billing may lag. No provider quota or subscription balance is available."];
}
export function usageLines(store: InspectionStore, budget: unknown, bound: boolean, width: number, chart = false): PanelLine[] {
  const rows: PanelLine[] = [];
  const add = (text: string, heading = false) => rows.push({ text: plainLabel(text), heading });
  add("Measured usage · source tables", true);
  add("c chart on/off (source table retained) · r reread stored snapshot · F9 conversation");
  if (!bound) add("UNBOUND · observations may be stale");
  if (store.stale || store.error) add(`STALE / unavailable: ${store.error ?? "canonical read required; retained values historical"}`);
  if (store.refreshing) add("Reading canonical stored accounting…");
  const info = store.current;
  if (info) add(`${info.task}/${info.run} · read ${store.checkedAt ? new Date(store.checkedAt).toISOString() : "unavailable"}`);
  const panel = info?.usage;
  if (!panel) add("Collected usage unavailable / legacy · not zero");
  else {
    add(panel.source);
    add("Reported USD is transcript-reported, not verified billed metered spend. Subscription costs are API-equivalent estimates, not subscription charges.");
    add("Wall time is completed-run elapsed time, not model-active time. Response latency is transcript-derived, not throughput.");
    if (width >= 90) add("Scope │ Input │ Output │ Cache read │ Cache write │ Reported USD │ API-equivalent USD │ Unpriced tokens", true);
    for (const row of panel.rows) {
      const u = row.usage;
      add(row.scope, true);
      add(`Coverage ${row.known_runs}/${row.runs} workers known · completed wall seconds ${row.wall_seconds ?? "unavailable"}`);
      if (!u) { add("Usage unknown · no collected sample"); continue; }
      if (width >= 90) add(`${u.input} │ ${u.output} │ ${u.cache_read} │ ${u.cache_write} │ ${dollars(u.reported_usd ?? u.cost_usd)} │ ${dollars(u.estimated_usd)} │ ${u.unpriced_tokens}`);
      else {
        add(`Input ${u.input} · output ${u.output} · cache read ${u.cache_read} · cache write ${u.cache_write}`);
        add(`Reported ${dollars(u.reported_usd ?? u.cost_usd)} · API-equivalent ${dollars(u.estimated_usd)}`);
        add(`Unpriced ${u.unpriced_tokens} tokens · basis ${u.cost_basis ?? "unknown"}`);
      }
      if (chart) {
        const max = Math.max(u.input, u.output, u.cache_read, u.cache_write, 1);
        add("Token bars · relative to largest category in this source row (exact values above)");
        for (const [label, count] of [["input", u.input], ["output", u.output], ["cache read", u.cache_read], ["cache write", u.cache_write]] as const)
          add(`${label}: ${"█".repeat(Math.round(count / max * Math.max(1, Math.min(24, width - 20))))} ${count}`);
      }
    }
    if (panel.models_omitted) add(`${panel.models_omitted} model rows omitted by projection bound`);
    add("No model rows means per-response/model accounting unavailable, not no usage.");
  }
  add("Budget source · last host accounting receipt", true);
  for (const line of budgetLines(budget)) add(line);
  return rows;
}
