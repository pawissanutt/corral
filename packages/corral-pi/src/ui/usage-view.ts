// Usage (F2 or `/panels usage`): what this session used, live, under its own section rule. Presentation only: no reads here, no repricing, a cost is never a bill, and an unknown is never drawn as zero.
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import type { Role } from "../roles/types.ts";
import { agentColor } from "./agent-hues.ts";
import { backLine, bold, glyphRows, sectionRule, wrap } from "./panel-parts.ts";
import { splitRow } from "./panels-chrome.ts";
import { pad, plainLabel, PANEL_MUTED, PANEL_TEXT, type PanelLine } from "./panels-view.ts";
import { dollars, fieldRows, grouped, muted, NARROW, plural } from "./usage-format.ts";
import type { SessionUsage, UsageIdentity, UsageModel } from "./usage-model.ts";

/** Views with fewer body rows than this open with the digest row. */
const DIGEST_BELOW = 12;
const ROLE: Readonly<Record<Role, string>> = {
  worker: "worker", reviewer: "reviewer", assistant: "assistant", build: "build",
};

/** Row 0 is the back line; `‹ Chat` (from column 0) is its click target. */
export function usageBackAt(x: number, y: number): boolean { return y === 0 && x >= 0 && x < visibleWidth("‹ Chat"); }

const COLUMN_GAP = 2;
/** The reading rules behind `?`, after the sections; nothing above them moves when they show. */
const LEGEND = [
  "Session tokens and API-equivalent USD are counted by this pane and priced from the model catalog; they are not a bill and not a provider quota. A + marks a total with an unknown member.",
];

export interface UsageViewOptions {
  /** `?`: the reading rules after the sections. */
  readonly legend: boolean;
  /** The view's body height; below 12 the digest opens the page. 0 when not laid out yet. */
  readonly bodyRows: number;
}

/**
 * The Usage page, each row at most `width` cells: the back line, the digest on short views, then SESSION and
 * READING RULES (with `?`). Pure: renders `model` only.
 */
export function usageLines(model: UsageModel, width: number, options: UsageViewOptions): PanelLine[] {
  const lines = [
    backLine("Chat", "Usage", identityLabel(model.identity), width).line,
    ...(options.bodyRows > 0 && options.bodyRows < DIGEST_BELOW ? digest(model.session, width) : []),
    ...sessionSection(model.session, width),
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

/** One row with the session's totals on a short view; the full section follows, nothing is dropped. */
function digest(session: SessionUsage | undefined, width: number): string[] {
  const one = (text: string, color: string) => color + truncateToWidth(plainLabel(text), Math.max(1, width), "…") + PANEL_TEXT;
  if (!session) return [one("○ this pane has not reported usage yet · unknown, not zero", PANEL_MUTED)];
  const t = sessionTotals(session), agents = session.rows.length - 1;
  const tokens = t.tokens === undefined ? "unknown" : grouped(t.tokens) + (t.tokensPartial ? "+" : "");
  const cost = t.cost === undefined ? "API-equiv. unknown" : `${dollars(t.cost)}${t.costPartial ? "+" : ""} estimate`;
  return [one(`${tokens} tokens · ${cost} · ${agents > 0 ? `this pane + ${plural(agents, "agent")}` : "this pane"}`, PANEL_TEXT)];
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
