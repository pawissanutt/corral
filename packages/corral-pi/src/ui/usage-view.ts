// Usage (F2 or `/panels usage`): what this session used, where the run budget stood at the last receipt
// (usage-budget-view.ts), what the task/run collector stored, and that receipt's diagnostics. Each source is its own
// section with its state and freshness on its rule; the scopes are never added together. Presentation only: no reads
// here, no repricing, a cost is never a bill, and an unknown is never drawn as zero.
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { Role } from "../roles/types.ts";
import { agentColor } from "./agent-hues.ts";
import { ago, backLine, bold, clock, glyphRows, legacyRow, sectionRule, stateBanner, stateNote, stateRow, wrap } from "./panel-parts.ts";
import { splitRow } from "./panels-chrome.ts";
import { pad, plainLabel, PANEL_BLUE, PANEL_MUTED, PANEL_TEXT, PANEL_WARNING, type PanelLine } from "./panels-view.ts";
import type { ReadState } from "./read-state.ts";
import { budgetDigest, budgetSection, diagnosticItems, diagnosticsSection } from "./usage-budget-view.ts";
import { dollars, fieldRows, grouped, itemRows, muted, NARROW, plural, wall } from "./usage-format.ts";
import type { SessionUsage, UsageIdentity, UsageModel, UsagePanel } from "./usage-model.ts";

type ScopeRow = UsagePanel["rows"][number];
type Facts = NonNullable<ScopeRow["usage"]>;

/** Views with fewer body rows than this open with the three-row digest. */
const DIGEST_BELOW = 12;
const ROLE: Readonly<Record<Role, string>> = {
  worker: "worker", reviewer: "reviewer", pm: "PM", planner: "planner", plan_reviewer: "plan reviewer", assistant: "assistant", build: "build",
};

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
const COLUMN_GAP = 2;
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
/** The reading rules behind `?`, after Diagnostics; nothing above them moves when they show. */
const LEGEND = [
  "Session tokens and API-equivalent USD are counted by this pane and priced from the model catalog; they are not a bill and not a provider quota. A + marks a total with an unknown member.",
  "The run budget is canonical reservation accounting (settled plus reserved) from the last host receipt, not provider balance or quota. Cancellation is best-effort; provider billing may lag. The host stop state is what that receipt recorded, not whether the host runs now.",
  "Reported USD is transcript-reported, not verified billed metered spend. Subscription costs are API-equivalent estimates, not subscription charges.",
  "Wall time is completed-run elapsed time, not model-active time. Response latency is transcript-derived, not throughput.",
  "workers: workers with collected usage of all workers in the scope. unknown is never zero. No model rows means per-response/model accounting unavailable, not no usage.",
  "Columns that do not fit drop in this order onto the muted row under each scope: basis, wall, workers, unpriced, cache write, cache read, API-equiv. USD, reported USD, output.",
  "c token bars are relative to the largest category in each scope; the exact values stay in the table.",
];
/** The collector's closing caveat, longest first. */
const CAVEAT = [
  "Reported USD is not a verified bill · API-equiv. USD is an estimate",
  "reported: not a verified bill · API-equiv.: estimate",
  "reported ≠ billed · API-equiv. = estimate",
  "reported ≠ billed · API ≈ estimate",
];

export interface UsageViewOptions {
  /** `c`: token bars under each collector scope, the exact table kept. */
  readonly chart: boolean;
  /** `?`: the reading rules after Diagnostics and problem lines in full. */
  readonly legend: boolean;
  readonly now: number;
  /** The view's body height; below 12 the digest opens the page. 0 when not laid out yet. */
  readonly bodyRows: number;
}

/**
 * The Usage page, each row at most `width` cells: the back line, the digest on short views, then SESSION, RUN BUDGET,
 * TASK / RUN, DIAGNOSTICS (when a host or local receipt was read) and READING RULES (with `?`). Pure: renders `model` only.
 */
export function usageLines(model: UsageModel, width: number, options: UsageViewOptions): PanelLine[] {
  const lines = [
    backLine("Chat", "Usage", identityLabel(model.identity), width).line,
    ...(options.bodyRows > 0 && options.bodyRows < DIGEST_BELOW ? digest(model, width, options.now) : []),
    ...sessionSection(model.session, width),
    ...budgetSection(model.budget, width),
    ...collectorSection(model, width, options),
    ...diagnosticsSection(model.budget, width),
    ...(options.legend ? [sectionRule("Reading rules", width), ...LEGEND.flatMap(line => wrap(muted(line), width))] : []),
  ];
  return lines.map(text => ({ text: truncateToWidth(text, Math.max(1, width), "…") }));
}

/** The back line's right side: whose usage this is. */
function identityLabel(identity: UsageIdentity): string {
  if (identity.task && identity.run) return `${identity.task} / ${identity.run}`;
  if (identity.role === "assistant" && identity.assistant) return `assistant ${identity.assistant}`;
  if (identity.role === "build" && identity.build) return `build ${identity.build}`;
  const role = identity.task ? `${ROLE[identity.role]} · ${identity.task}` : ROLE[identity.role];
  return identity.project ? `${role} · ${identity.project}` : role;
}

/** The session's totals: tokens over the listed rows, cost as the status bar sums it; `partial` when a member is unknown. */
function sessionTotals(session: SessionUsage): { tokens: number | undefined; tokensPartial: boolean; cost: number | undefined; costPartial: boolean } {
  const known = session.rows.flatMap(row => row.tokens === undefined ? [] : [row.tokens]);
  const pane = session.rows[0];
  const cost = session.rows.length > 1 || session.total ? session.total : pane?.cost === undefined ? undefined : { cost: pane.cost, partial: false };
  return { tokens: known.length ? known.reduce((sum, n) => sum + n, 0) : undefined, tokensPartial: known.length < session.rows.length,
    cost: cost?.cost, costPartial: cost?.partial ?? false };
}

/** Three rows that answer the page's questions on a short view; the full sections follow, nothing is dropped. */
function digest(model: UsageModel, width: number, now: number): string[] {
  const one = (text: string, color: string) => color + truncateToWidth(plainLabel(text), Math.max(1, width), "…") + PANEL_TEXT;
  const session = model.session, out: string[] = [];
  if (!session) out.push(one("○ this pane has not reported usage yet · unknown, not zero", PANEL_MUTED));
  else {
    const t = sessionTotals(session), agents = session.rows.length - 1;
    const tokens = t.tokens === undefined ? "unknown" : grouped(t.tokens) + (t.tokensPartial ? "+" : "");
    const cost = t.cost === undefined ? "API-equiv. unknown" : `${dollars(t.cost)}${t.costPartial ? "+" : ""} estimate`;
    out.push(one(`${tokens} tokens · ${cost} · ${agents > 0 ? `this pane + ${plural(agents, "agent")}` : "this pane"}`, PANEL_TEXT));
  }
  const budget = budgetDigest(model.budget);
  out.push(one(budget.text, budget.color));
  const issues = model.budget.kind === "read" ? diagnosticItems(model.budget.facts.diagnostics, true) : [];
  const collector = collectorDigest(model.collector.state, now);
  const warn = issues.length > 0 || collector.warn;
  out.push(one(`${warn ? "!" : "○"} ${[...issues, collector.text].join(" · ")}`, warn ? PANEL_WARNING : PANEL_MUTED));
  return out;
}
function collectorDigest(state: ReadState, now: number): { text: string; warn: boolean } {
  const since = state.readAt === undefined ? undefined : ago(state.readAt, now);
  switch (state.phase) {
    case "not_applicable": return { text: "task and run accounting does not apply", warn: false };
    case "loading": return { text: "reading task accounting", warn: false };
    case "never_read": return { text: "task accounting not read yet", warn: false };
    case "failed": return { text: "task accounting could not be read", warn: true };
    case "retained": return { text: `task stale · read ${state.readAt === undefined ? "earlier" : clock(state.readAt)}`, warn: true };
    case "current": return { text: since ? `task read ${since}` : "task read", warn: false };
  }
}

function sessionSection(session: SessionUsage | undefined, width: number): string[] {
  const agents = (session?.rows.length ?? 0) > 1;
  const right = width >= 90 ? [agents ? "this pane and its agents · live" : "this pane · live", "live"]
    : !agents && width >= NARROW ? ["this pane · live", "live"] : ["live"];
  const rule = sectionRule("Session", width, right);
  if (!session) return [rule, ...glyphRows("○ This pane has not reported usage yet · unknown, not zero", width)];
  return [rule, ...(agents ? sessionTable(session, width) : sessionFields(session, width))];
}
/** This pane alone: tokens with its API-equivalent estimate, the context window and the model. */
function sessionFields(session: SessionUsage, width: number): string[] {
  const labels = width >= NARROW ? 13 : 10, pane = session.rows[0], context = session.context;
  const tokens = pane?.tokens === undefined ? muted("unknown") : grouped(pane.tokens);
  const cost = pane?.cost === undefined ? muted("unknown") : `${dollars(pane.cost)}, an estimate`;
  const joined = `${tokens} · API-equiv. ${cost}`;
  const share = context?.percent == null ? muted("percent unknown") : `${Math.round(context.percent)}%`;
  return [
    ...(visibleWidth(joined) <= width - labels - 1 ? fieldRows("tokens", joined, labels, width)
      : [...fieldRows("tokens", tokens, labels, width), ...fieldRows("API-equiv.", cost, labels, width)]),
    ...fieldRows("context", context?.tokens == null ? muted("unknown")
      : `${grouped(context.tokens)} / ${grouped(context.contextWindow)} · ${share}`, labels, width),
    ...fieldRows("model", session.model === null ? muted("unknown")
      : plainLabel(session.model) + (session.effort ? ` · effort ${plainLabel(session.effort)}` : ""), labels, width),
  ];
}
/**
 * This pane and its build children: an aligned table with a `total` row from 64 cells, else one `name … tokens · USD`
 * row each under a caption (context stays in the Agents view and the status bar). A `+` beside a total marks an
 * unknown member; it stands outside the column so the digits stay aligned.
 */
function sessionTable(session: SessionUsage, width: number): string[] {
  const rows = session.rows, totals = sessionTotals(session);
  const names = rows.map((row, i) => i === 0 ? "this pane" : `  ${row.hue === undefined ? "" : agentColor(row.hue)}${bold(plainLabel(row.name))}${PANEL_TEXT}`);
  const value = (text: string | undefined, absent = "unknown") => text === undefined ? muted(absent) : text;
  const tokens = rows.map(row => value(row.tokens === undefined ? undefined : grouped(row.tokens)));
  const usd = rows.map(row => value(row.cost === undefined ? undefined : dollars(row.cost)));
  const context = rows.map(row => value(row.contextPercent === undefined ? undefined : `${Math.round(row.contextPercent)}%`, "–"));
  const tokenTotal = value(totals.tokens === undefined ? undefined : grouped(totals.tokens));
  const usdTotal = value(totals.cost === undefined ? undefined : dollars(totals.cost));
  const widest = (head: string, values: readonly string[]) => Math.max(head.length, ...values.map(v => visibleWidth(v)));
  const nameW = Math.min(24, Math.max(14, ...names.map(n => visibleWidth(n)))), tokW = widest("tokens", [...tokens, tokenTotal]);
  const usdW = widest("API-equiv. USD", [...usd, usdTotal]), ctxW = widest("context", context);
  const right = (text: string, w: number) => " ".repeat(Math.max(0, w - visibleWidth(text))) + text;
  if (width >= NARROW && nameW + tokW + usdW + ctxW + 3 * COLUMN_GAP <= width) {
    return [
      muted(`${pad("", nameW)}  ${right("tokens", tokW)}  ${right("API-equiv. USD", usdW)}  ${right("context", ctxW)}`),
      ...rows.map((_, i) => `${pad(names[i] ?? "", nameW)}  ${right(tokens[i] ?? "", tokW)}  ${right(usd[i] ?? "", usdW)}  ${right(context[i] ?? "", ctxW)}`),
      `${pad(bold("total"), nameW)}  ${right(tokenTotal, tokW)}${totals.tokensPartial ? "+ " : "  "}${right(usdTotal, usdW)}${totals.costPartial ? "+" : ""}`,
    ];
  }
  const line = (name: string, values: string) => splitRow(truncateToWidth(name, Math.max(1, width - visibleWidth(values) - 2), "…"), values, width);
  return [
    ...rows.map((_, i) => line(names[i] ?? "", `${tokens[i] ?? ""} · ${usd[i] ?? ""}`)),
    line(bold("total"), `${tokenTotal}${totals.tokensPartial ? "+" : ""} · ${usdTotal}${totals.costPartial ? "+" : ""}`),
    ...wrap(muted("tokens · API-equiv. USD, estimates"), width),
  ];
}

/**
 * The stored collector facets. Without a task run (or a reader) it is one neutral row and nothing is read; otherwise
 * the rule carries the read note, a problem line follows when the read failed or is retained, then the table: the
 * column header once, each scope as a bold row with its numbers and its dropped columns, the omitted-rows warning,
 * then the source and the caveat.
 */
function collectorSection(model: UsageModel, width: number, options: UsageViewOptions): string[] {
  const { identity, collector } = model, state = collector.state;
  if (state.phase === "not_applicable") {
    const why = identity.task && identity.run ? "this session has no reader for stored task accounting." : `this ${ROLE[identity.role]} session has no task run.`;
    return [sectionRule("Task and run", width), ...stateRow(state, "task accounting", width, why)];
  }
  const notes = stateNote(state, options.now);
  const unbound = model.bound ? [] : notes.map(note => `${PANEL_WARNING}! host unbound${PANEL_MUTED} · ${note}`);
  const out = [sectionRule(`Task ${identity.task ?? ""} / ${identity.run ?? ""}`, width, [...unbound, ...notes]),
    ...stateBanner(state, "stored accounting", width, options.legend)];
  if (state.phase !== "current" && state.phase !== "retained") return [...out, ...stateRow(state, "stored accounting", width)];
  const panel = collector.panel;
  if (!panel) return [...out, ...legacyRow("collected usage", "run record (older collector)", width)];
  const { shown, dropped } = usageColumns(panel.rows, width), gap = " ".repeat(COLUMN_GAP);
  out.push(muted(shown.map(c => cell(c.head, c)).join(gap)));
  for (const row of panel.rows) {
    out.push(...wrap(bold(plainLabel(row.scope)), width));
    const u = row.usage;
    if (!u) {
      out.push(...itemRows(["○ usage unknown", "no collected sample", `workers ${grouped(row.known_runs)}/${grouped(row.runs)}`, `wall ${wall(row.wall_seconds)}`], width).map(muted));
      continue;
    }
    out.push(shown.map(c => { const v = c.value(row, u); return (v === "unknown" ? PANEL_MUTED : PANEL_TEXT) + cell(v, c) + PANEL_TEXT; }).join(gap));
    if (dropped.length) out.push(...itemRows(dropped.map(c => `${c.head} ${c.value(row, u)}`), width).map(muted));
    if (options.chart) {
      const bars = [["input", u.input], ["output", u.output], ["cache read", u.cache_read], ["cache write", u.cache_write]] as const;
      const max = Math.max(...bars.map(([, n]) => n), 1), numbers = Math.max(...bars.map(([, n]) => grouped(n).length));
      const room = Math.max(1, Math.min(24, width - 11 - 2 - numbers - 2));
      for (const [label, n] of bars) {
        const filled = n ? Math.max(1, Math.round(n / max * room)) : 0;
        out.push(`${PANEL_MUTED}${pad(label, 11)}  ${PANEL_BLUE}${"█".repeat(filled)}${" ".repeat(room - filled)}  ${PANEL_MUTED}${grouped(n).padStart(numbers)}${PANEL_TEXT}`);
      }
    }
  }
  if (panel.models_omitted) out.push(...glyphRows(`! ${plural(panel.models_omitted, "model row")} omitted by the row limit`, width, PANEL_WARNING));
  const caveat = CAVEAT.find(c => visibleWidth(c) <= width) ?? CAVEAT.at(-1) ?? "";
  out.push(...itemRows([`source ${plainLabel(panel.source)}`, caveat], width).map(muted));
  return out;
}
