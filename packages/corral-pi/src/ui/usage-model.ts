import { z } from "zod";
import type { ContextUsage } from "@earendil-works/pi-coding-agent";
import type { Role } from "../roles/types.ts";
import type { PanelSnapshot } from "./panels-view.ts";
import type { Inspection } from "./inspection.ts";
import type { ReadState } from "./read-state.ts";
import { childHue } from "./build/contract.ts";

const Count = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
const Dollars = z.number().finite().nonnegative().nullable().optional();
const Usage = z.object({ input: Count, output: Count, cache_read: Count, cache_write: Count,
  reported_usd: Dollars, cost_usd: Dollars, estimated_usd: Dollars, unpriced_tokens: Count,
  cost_basis: z.enum(["reported", "estimated", "mixed"]).nullable() });
export const UsagePanelSchema = z.object({ source: z.string().max(16384),
  rows: z.array(z.object({ scope: z.string().max(16384), usage: Usage.nullable(),
    wall_seconds: z.number().int().nullable(), known_runs: Count, runs: Count })).max(102), models_omitted: Count });
export type UsagePanel = z.infer<typeof UsagePanelSchema>;
export const BudgetSchema = z.object({
  limits: z.object({ max_tokens: Count.nullable(), max_usd_nanos: Count.nullable() }),
  tokens: Count, usd_nanos: Count, uncertain_requests: Count, pricing_gaps: Count,
  legacy_usage_gap: z.boolean(), unbounded_requests: Count, exhausted: z.boolean(),
  stopped: z.boolean().optional(), uncertain: z.boolean().optional(), active: Count.optional(),
});
const LocalBudgetSchema = z.object({
  limits: z.object({ max_tokens: Count.nullable(), max_usd_nanos: Count.nullable() }),
  used: z.object({ tokens: Count, usd_nanos: Count }),
  reserved: z.object({ tokens: Count, usd_nanos: Count }),
  exhausted: z.boolean(), stopped: z.boolean().optional(), active: Count.optional(),
});
export interface UsageIdentity { role: Role; project: string; task?: string; run?: string; assistant?: string; build?: string }
export interface SessionRow { key: string; name: string; hue: number | undefined; tokens: number | undefined; cost: number | undefined; contextPercent: number | undefined }
export interface SessionUsage { model: string | null; effort: string; context: ContextUsage | undefined;
  rows: readonly SessionRow[]; total: { cost: number; partial: boolean } | undefined }
export function sessionCost(snapshot: PanelSnapshot | null): { total: number; partial: boolean } | undefined {
  const children = snapshot?.build?.fleet.children ?? [];
  const priced = children.filter(child => child.usage.cost !== undefined);
  if (!snapshot || (snapshot.cost === undefined && !priced.length)) return undefined;
  return { total: (snapshot.cost ?? 0) + priced.reduce((sum, child) => sum + (child.usage.cost ?? 0), 0),
    partial: snapshot.cost === undefined || children.some(child => child.usage.cost === undefined && child.usage.turns > 0) };
}
export type BudgetState = { kind: "legacy" } | { kind: "unavailable" } | { kind: "unreadable" }
  | { kind: "read"; facts: BudgetFacts } | { kind: "local"; facts: LocalBudgetFacts };
export interface BudgetFacts {
  limits: { tokens: number | null; usd: number | null };
  accounted: { tokens: number; usd: number };
  atLastReceipt: { exhausted: boolean; stopped: boolean | undefined; active: number | undefined };
  diagnostics: { unresolved: number; unbounded: number; pricingGaps: number; legacyUsageGap: boolean; uncertain: boolean };
}
export interface LocalBudgetFacts {
  limits: { tokens: number | null; usd: number | null };
  accounted: { tokens: number; usd: number };
  settled: { tokens: number; usd: number };
  reserved: { tokens: number; usd: number };
  atLastReceipt: { exhausted: boolean; stopped: boolean | undefined; active: number | undefined };
}
export function budgetState(value: unknown, catalog: number | undefined): BudgetState {
  if (catalog !== undefined && catalog < 7) return { kind: "legacy" };
  if (value == null) return { kind: "unavailable" };
  if (typeof value === "object" && !Array.isArray(value) &&
      !["limits", "tokens", "usd_nanos", "used", "reserved", "uncertain_requests", "pricing_gaps", "legacy_usage_gap", "unbounded_requests", "exhausted"].some(key => key in value)) return { kind: "unavailable" };
  const parsed = BudgetSchema.safeParse(value);
  if (!parsed.success) {
    const local = LocalBudgetSchema.safeParse(value);
    if (!local.success) return { kind: "unreadable" };
    const b = local.data, tokens = b.used.tokens + b.reserved.tokens, nanos = b.used.usd_nanos + b.reserved.usd_nanos;
    if (!Number.isSafeInteger(tokens) || !Number.isSafeInteger(nanos)) return { kind: "unreadable" };
    return { kind: "local", facts: {
      limits: { tokens: b.limits.max_tokens, usd: b.limits.max_usd_nanos === null ? null : b.limits.max_usd_nanos / 1e9 },
      accounted: { tokens, usd: nanos / 1e9 },
      settled: { tokens: b.used.tokens, usd: b.used.usd_nanos / 1e9 },
      reserved: { tokens: b.reserved.tokens, usd: b.reserved.usd_nanos / 1e9 },
      atLastReceipt: { exhausted: b.exhausted, stopped: b.stopped, active: b.active },
    } };
  }
  const b = parsed.data;
  return { kind: "read", facts: {
    limits: { tokens: b.limits.max_tokens, usd: b.limits.max_usd_nanos === null ? null : b.limits.max_usd_nanos / 1e9 },
    accounted: { tokens: b.tokens, usd: b.usd_nanos / 1e9 },
    atLastReceipt: { exhausted: b.exhausted, stopped: b.stopped, active: b.active },
    diagnostics: { unresolved: b.uncertain_requests, unbounded: b.unbounded_requests, pricingGaps: b.pricing_gaps,
      legacyUsageGap: b.legacy_usage_gap, uncertain: Boolean(b.uncertain || b.uncertain_requests || b.pricing_gaps || b.legacy_usage_gap || b.unbounded_requests) },
  } };
}
export interface CollectorUsage { state: ReadState; panel: UsagePanel | undefined; legacy: boolean }
export interface UsageModel { identity: UsageIdentity; session: SessionUsage | undefined; bound: boolean; budget: BudgetState; collector: CollectorUsage }
export function usageModel({ identity, snapshot, inspection }: { identity: UsageIdentity; snapshot: PanelSnapshot | null;
  inspection: { current: Inspection | undefined; state: ReadState } }): UsageModel {
  const children = snapshot?.build?.fleet.children ?? [];
  const total = snapshot?.build ? sessionCost(snapshot) : undefined;
  return { identity, bound: snapshot?.bound ?? false,
    budget: budgetState(snapshot?.budget, snapshot?.inspector?.capabilities?.catalog),
    collector: { state: inspection.state, panel: inspection.current?.usage, legacy: !!inspection.current && !inspection.current.usage },
    session: snapshot ? { model: snapshot.model, effort: snapshot.effort, context: snapshot.context,
      rows: [{ key: "pane", name: "Pane", hue: undefined, tokens: snapshot.tokens, cost: snapshot.cost,
        contextPercent: snapshot.context?.percent ?? undefined },
        ...children.map(child => ({ key: `child:${child.id}`, name: child.id,
          hue: snapshot.build ? childHue(snapshot.build.fleet, child.id) : undefined,
          tokens: child.usage.input + child.usage.output, cost: child.usage.cost, contextPercent: child.usage.contextPercent }))],
      total: total ? { cost: total.total, partial: total.partial } : undefined } : undefined };
}
