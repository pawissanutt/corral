// Usage (F2 or `/panels usage`): stored collector accounting as one aligned table with a section rule per scope,
// then the configured run budget. Read-only source tables: never reprice history or interpret SDK cost as an invoice.
import { stripTerminalSequences, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import { z } from "zod";
import type { InspectionStore } from "./inspection.ts";
import { backLine, chip, chipHeader, sectionRule, toneColor, wrap, type Tone } from "./panel-parts.ts";
import { pad, plainLabel, PANEL_BLUE, PANEL_MUTED, PANEL_TEXT, PANEL_WARNING, type PanelLine } from "./panels-view.ts";

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
type ScopeRow = z.infer<typeof UsagePanelSchema>["rows"][number];
type Facts = NonNullable<ScopeRow["usage"]>;

/** Thousands separators on an integer, independent of the host locale. */
const grouped = (value: number) => String(value).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
/** USD at the stored precision (six decimals), grouped; `unknown` when absent, never zero. */
function dollars(value: number | null | undefined): string {
  if (value == null) return "unknown";
  const [whole = "0", cents = ""] = value.toFixed(6).split(".");
  return `$${grouped(Number(whole))}.${cents}`;
}
function wall(seconds: number | null): string {
  if (seconds === null) return "unknown";
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ${String(seconds % 60).padStart(2, "0")}s`;
  return `${grouped(Math.floor(seconds / 3600))}h ${String(Math.floor(seconds % 3600 / 60)).padStart(2, "0")}m`;
}

/** Row 0 is the back line; `‹ Chat` (from column 0) is its click target. */
export function usageBackAt(x: number, y: number): boolean { return y === 0 && x >= 0 && x < visibleWidth("‹ Chat"); }

/**
 * Table columns in display order. `rank` is the keep priority: when the width cannot hold every column, the highest
 * ranks drop first (basis, wall, workers, unpriced, cache write, cache read, API-equiv. USD, reported USD, output) and
 * reappear as labelled values on a muted row under each scope, so no numeric category is lost and nothing misaligns.
 */
const COLUMNS: readonly { readonly head: string; readonly rank: number; readonly right: boolean; readonly value: (row: ScopeRow, u: Facts) => string }[] = [
  { head: "input", rank: 1, right: true, value: (_, u) => grouped(u.input) },
  { head: "output", rank: 2, right: true, value: (_, u) => grouped(u.output) },
  { head: "cache read", rank: 5, right: true, value: (_, u) => grouped(u.cache_read) },
  { head: "cache write", rank: 6, right: true, value: (_, u) => grouped(u.cache_write) },
  { head: "unpriced", rank: 7, right: true, value: (_, u) => grouped(u.unpriced_tokens) },
  { head: "reported USD", rank: 3, right: true, value: (_, u) => dollars(u.reported_usd ?? u.cost_usd) },
  { head: "API-equiv. USD", rank: 4, right: true, value: (_, u) => dollars(u.estimated_usd) },
  { head: "workers", rank: 8, right: true, value: row => `${grouped(row.known_runs)}/${grouped(row.runs)}` },
  { head: "wall", rank: 9, right: true, value: row => wall(row.wall_seconds) },
  { head: "basis", rank: 10, right: false, value: (_, u) => u.cost_basis ?? "unknown" },
];
export const COLUMN_GAP = 2;
type Column = (typeof COLUMNS)[number] & { readonly width: number };

/** The columns that fit `width`, in display order, each as wide as its header or its widest value. */
export function usageColumns(rows: readonly ScopeRow[], width: number): { readonly shown: readonly Column[]; readonly dropped: readonly Column[] } {
  const sized: Column[] = COLUMNS.map(c => ({ ...c, width: Math.max(c.head.length,
    ...rows.flatMap(row => row.usage ? [visibleWidth(c.value(row, row.usage))] : [])) }));
  const keep = new Set<Column>();
  let used = 0;
  for (const column of [...sized].sort((a, b) => a.rank - b.rank)) {
    const cost = column.width + (keep.size ? COLUMN_GAP : 0);
    if (keep.size && used + cost > width) break;
    keep.add(column); used += cost;
  }
  return { shown: sized.filter(c => keep.has(c)), dropped: sized.filter(c => !keep.has(c)) };
}
const cell = (text: string, column: Column) => {
  const clipped = truncateToWidth(text, column.width, "…");
  const room = " ".repeat(Math.max(0, column.width - visibleWidth(clipped)));
  return column.right ? room + clipped : clipped + room;
};

export interface BudgetRow { readonly label: string; readonly value: string; readonly tone: Tone }
/** The configured run budget from the host's last accounting receipt; an unreadable receipt is never unlimited or zero. */
export function budgetRows(value: unknown): { readonly rows: readonly BudgetRow[]; readonly uncertain: boolean } {
  const result = BudgetSchema.safeParse(value);
  if (!result.success) return { rows: [{ label: "Budget", value: "accounting unavailable / legacy · not zero; configured limits unknown", tone: "warn" }], uncertain: false };
  const b = result.data;
  return {
    rows: [
      { label: "Token limit", value: b.limits.max_tokens === null ? "not configured" : grouped(b.limits.max_tokens), tone: "none" },
      { label: "USD limit", value: b.limits.max_usd_nanos === null ? "not configured" : dollars(b.limits.max_usd_nanos / 1e9), tone: "none" },
      { label: "Settled+reserved", value: `${grouped(b.tokens)} tokens · ${dollars(b.usd_nanos / 1e9)} accounting USD, not billed spend`, tone: "none" },
      { label: "Unresolved", value: `${grouped(b.uncertain_requests)} ${b.uncertain_requests === 1 ? "reservation" : "reservations"} · ${grouped(b.unbounded_requests)} unbounded`, tone: b.uncertain_requests || b.unbounded_requests ? "warn" : "none" },
      { label: "Pricing gaps", value: grouped(b.pricing_gaps), tone: b.pricing_gaps ? "warn" : "none" },
      { label: "Legacy gap", value: b.legacy_usage_gap ? "yes · historical spend unknown" : "not reported", tone: b.legacy_usage_gap ? "warn" : "none" },
      { label: "State", value: `${b.exhausted ? "exhausted" : "not exhausted at last receipt"} · ${b.stopped ? "host stopped" : "host not stopped"}`, tone: b.exhausted || b.stopped ? "warn" : "none" },
    ],
    uncertain: Boolean(b.uncertain || b.uncertain_requests || b.pricing_gaps || b.legacy_usage_gap || b.unbounded_requests),
  };
}
/** `items` joined by ` · ` and broken only between items; an item wider than `width` wraps on its own. */
function itemRows(items: readonly string[], width: number): string[] {
  const out: string[] = [];
  let line = "";
  for (const item of items) {
    if (line && visibleWidth(line) + 3 + visibleWidth(item) > width) { out.push(line); line = ""; }
    line = line ? `${line} · ${item}` : item;
  }
  if (line) out.push(line);
  return out.flatMap(row => wrap(row, width));
}

/** The reading rules behind `?`; one short form of the first stays on screen. */
const LEGEND = [
  "Reported USD is transcript-reported, not verified billed metered spend. Subscription costs are API-equivalent estimates, not subscription charges.",
  "Wall time is completed-run elapsed time, not model-active time. Response latency is transcript-derived, not throughput.",
  "workers: workers with collected usage of all workers in the scope. unknown is never zero. No model rows means per-response/model accounting unavailable, not no usage.",
  "Columns that do not fit drop in this order onto the muted row under each scope: basis, wall, workers, unpriced, cache write, cache read, API-equiv. USD, reported USD, output.",
  "c token bars are relative to the largest category in each scope; the exact values stay in the table.",
  "The run budget is canonical reservation accounting from the last host receipt, not provider balance or quota. Cancellation is best-effort; provider billing may lag. No provider quota or subscription balance is available.",
];
const CAVEAT = [
  "Reported USD is transcript-reported, not a verified bill · API-equivalent USD is an estimate, not a charge",
  "Reported USD is not a verified bill · API-equiv. USD is an estimate",
  "reported: not a verified bill · API-equiv.: estimate",
  "reported ≠ billed · API-equiv. = estimate",
  "reported ≠ billed · API ≈ estimate",
];

/**
 * The Usage page, one styled row per entry, each at most `width` cells: the back line, chips with freshness, the
 * caveat (or the whole legend), the token and cost table with one section rule per scope, then the run budget.
 */
export function usageLines(store: InspectionStore, budget: unknown, bound: boolean, width: number, chart = false, legend = false): PanelLine[] {
  const rows: PanelLine[] = [];
  const push = (text: string) => rows.push({ text: truncateToWidth(text, Math.max(1, width), "…") });
  const full = (text: string) => { for (const line of wrap(text, width)) push(line); };
  const info = store.current, panel = info?.usage;

  push(backLine("Chat", "Usage", info ? `${info.task} / ${info.run}` : "", width).line);

  const chips = [chip(panel ? stripTerminalSequences(truncateToWidth(plainLabel(panel.source), 40, "…")) : "no usage source"),
    ...(panel ? [chip(`${panel.rows.length} ${panel.rows.length === 1 ? "scope" : "scopes"}`)] : []),
    ...(panel?.models_omitted ? [chip(`! ${grouped(panel.models_omitted)} model rows omitted`, "warn")] : []),
    ...(!bound ? [chip("! unbound · may be stale", "warn")] : []),
    ...(store.stale || store.error ? [chip("~ stale", "warn")] : []),
    ...(store.refreshing ? [chip("reading…")] : []),
    ...(chart ? [chip("token bars")] : [])];
  for (const line of chipHeader(chips, width, store.checkedAt, Date.now())) push(line);
  if (store.stale || store.error) full(`${PANEL_WARNING}~ STALE / unavailable: ${plainLabel(store.error ?? "canonical read required; retained values historical")}${PANEL_TEXT}`);
  if (legend) for (const line of LEGEND) full(PANEL_MUTED + line + PANEL_TEXT);
  else if (panel) push(PANEL_MUTED + (CAVEAT.find(c => visibleWidth(c) <= width) ?? CAVEAT.at(-1) ?? "") + PANEL_TEXT);

  if (!panel) full(`${PANEL_WARNING}! Collected usage unavailable / legacy · not zero${PANEL_TEXT}`);
  else {
    const { shown, dropped } = usageColumns(panel.rows, width);
    push(PANEL_MUTED + shown.map(c => cell(c.head, c)).join(" ".repeat(COLUMN_GAP)) + PANEL_TEXT);
    for (const row of panel.rows) {
      push(sectionRule(row.scope, width));
      const u = row.usage;
      if (!u) {
        for (const line of itemRows(["○ usage unknown", "no collected sample", `workers ${grouped(row.known_runs)}/${grouped(row.runs)}`, `wall ${wall(row.wall_seconds)}`], width))
          push(PANEL_MUTED + line + PANEL_TEXT);
        continue;
      }
      push(shown.map(c => { const v = c.value(row, u); return (v === "unknown" ? PANEL_MUTED : PANEL_TEXT) + cell(v, c) + PANEL_TEXT; }).join(" ".repeat(COLUMN_GAP)));
      if (dropped.length) for (const line of itemRows(dropped.map(c => `${c.head} ${c.value(row, u)}`), width)) push(PANEL_MUTED + line + PANEL_TEXT);
      if (chart) {
        const bars = [["input", u.input], ["output", u.output], ["cache read", u.cache_read], ["cache write", u.cache_write]] as const;
        const max = Math.max(...bars.map(([, n]) => n), 1), numbers = Math.max(...bars.map(([, n]) => grouped(n).length));
        const room = Math.max(1, Math.min(24, width - 11 - 2 - numbers - 2));
        for (const [label, n] of bars) {
          const filled = n ? Math.max(1, Math.round(n / max * room)) : 0;
          push(`${PANEL_MUTED}${pad(label, 11)}  ${PANEL_BLUE}${"█".repeat(filled)}${" ".repeat(room - filled)}  ${PANEL_MUTED}${grouped(n).padStart(numbers)}${PANEL_TEXT}`);
        }
      }
    }
  }

  push(sectionRule("Budget", width));
  const plan = budgetRows(budget), labels = Math.min(16, Math.max(...plan.rows.map(r => r.label.length)));
  for (const { label, value, tone } of plan.rows) {
    const color = tone === "none" ? PANEL_TEXT : toneColor(tone);
    wrap(plainLabel(value), Math.max(1, width - labels - 1)).forEach((line, i) => push(`${PANEL_MUTED}${pad(i ? "" : label, labels)} ${color}${line}${PANEL_TEXT}`));
  }
  if (plan.uncertain) full(`${PANEL_WARNING}! UNCERTAIN spend · unresolved or interrupted requests retain reservations; unknown pricing is not free${PANEL_TEXT}`);
  return rows;
}
