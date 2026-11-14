// Usage's RUN BUDGET and DIAGNOSTICS sections and the digest's budget row, from the host's last accounting receipt
// or a local control plane's receipt. Accounted totals are settled plus reserved, not billed spend or provider quota;
// a stop is claimed only when the receipt recorded it, and counters a receipt does not record are never drawn as zero.
import { visibleWidth } from "@earendil-works/pi-tui";
import { glyphRows, sectionRule, wrap } from "./panel-parts.ts";
import { pad, PANEL_BLUE, PANEL_ERROR, PANEL_LINE, PANEL_MUTED, PANEL_TEXT, PANEL_WARNING } from "./panels-view.ts";
import { dollars, fieldRows, grouped, muted, NARROW, plural } from "./usage-format.ts";
import type { BudgetFacts, BudgetState } from "./usage-model.ts";

type Diagnostics = BudgetFacts["diagnostics"];

/** `used` of `limit` as a whole percent, floored so a share short of the limit never reads 100%. */
function percent(used: number, limit: number): string {
  return limit > 0 ? `${Math.floor(used / limit * 100)}%` : "zero limit";
}
export function budgetSection(budget: BudgetState, width: number): string[] {
  const receipt = budget.kind === "read" || budget.kind === "unreadable";
  const rule = sectionRule("Run budget", width, budget.kind === "local" ? ["local host receipts", "local receipts"]
    : receipt ? (width >= NARROW ? ["last host receipt", "receipt"] : ["receipt"]) : ["this host"]);
  switch (budget.kind) {
    case "legacy": return [rule, ...glyphRows("○ This host generation predates budget accounting · totals unknown, not zero", width)];
    case "unavailable": return [rule, ...glyphRows("○ No budget receipt from this host · limits and totals unknown, not zero", width)];
    case "unreadable": return [rule, ...glyphRows("! Budget receipt could not be read · limits and totals unknown", width, PANEL_WARNING)];
    case "read": return [rule, ...budgetBody(budget.facts, budget.facts.diagnostics, width)];
    case "local": {
      // A local control plane keeps settled and reserved totals apart; both stay reachable beside the accounted sum.
      const { settled, reserved } = budget.facts, labels = width < NARROW ? 10 : 13;
      return [rule, ...budgetBody(budget.facts, undefined, width),
        ...fieldRows("settled", `${grouped(settled.tokens)} tokens · ${dollars(settled.usd)}`, labels, width),
        ...fieldRows("reserved", `${grouped(reserved.tokens)} tokens · ${dollars(reserved.usd)}`, labels, width)];
    }
  }
}
/**
 * The receipt's lead row (exactly one state), then the accounted totals: one meter per configured limit, or the
 * totals beside `no limit configured`. A stop is claimed only when the receipt recorded it; one warning row closes
 * the section when a diagnostic the receipt records is not clean.
 */
function budgetBody(facts: Pick<BudgetFacts, "limits" | "accounted" | "atLastReceipt">, diagnostics: Diagnostics | undefined, width: number): string[] {
  const { limits, accounted, atLastReceipt: at } = facts, narrow = width < NARROW;
  const none = limits.tokens === null && limits.usd === null;
  const totals = `${grouped(accounted.tokens)} tokens · ${dollars(accounted.usd)}`;
  const inline = `○ no limit configured · ${totals} accounted, not billed`;
  const out: string[] = [];
  let shown = false;
  if (at.exhausted) {
    const text = at.stopped === true ? (narrow ? "✗ limit reached · host stopped" : "✗ limit reached · host stopped new requests")
      : at.stopped === false ? "! limit reached · host had not stopped at last receipt" : "! limit reached at last receipt · stop state not recorded";
    out.push(...glyphRows(text, width, at.stopped === true ? PANEL_ERROR : PANEL_WARNING));
  } else if (at.stopped === true) out.push(...glyphRows("! host stopped at last receipt", width, PANEL_WARNING));
  else if (none && !narrow && visibleWidth(inline) <= width) { out.push(muted(inline)); shown = true; }
  else out.push(...glyphRows(none ? "○ no limit configured" : "○ within limits at last receipt", width));
  if (none) {
    if (!shown) out.push(...fieldRows("accounted", totals, narrow ? 10 : 13, width), ...wrap(muted("not billed spend or provider quota"), width));
  } else {
    out.push(...meterRows("tokens", accounted.tokens, limits.tokens, grouped, width), ...meterRows("USD", accounted.usd, limits.usd, dollars, width));
    if (narrow) out.push(...wrap(muted("accounted, not billed or provider quota"), width));
  }
  const issues = diagnostics ? diagnosticItems(diagnostics, narrow) : [];
  if (issues.length) out.push(...glyphRows(`! ${issues.join(" · ")}${narrow ? "" : " · see below"}`, width, PANEL_WARNING));
  return out;
}
/** One configured limit as the status bar's meter (accent, warning from 80%, error from 95%); not configured stays a word. */
function meterRows(label: string, used: number, limit: number | null, amount: (n: number) => string, width: number): string[] {
  const narrow = width < NARROW;
  if (limit === null) return wrap(muted(`${pad(label, narrow ? 7 : 8)}not configured · ${amount(used)} accounted`), width);
  const share = limit > 0 ? used / limit : 1, cells = narrow ? 8 : 12, filled = Math.min(cells, Math.floor(share * cells));
  const color = share >= 0.95 ? PANEL_ERROR : share >= 0.8 ? PANEL_WARNING : PANEL_BLUE;
  const bar = `${color}${"▰".repeat(filled)}${PANEL_LINE}${"▱".repeat(cells - filled)}${PANEL_TEXT}`;
  return wrap(narrow ? `${pad(label, 7)}${bar} ${percent(used, limit)} · ${amount(used)} / ${amount(limit)}`
    : `${pad(label, 8)}${bar}  ${amount(used)} / ${amount(limit)} accounted · ${percent(used, limit)}`, width);
}
/** The receipt's non-clean diagnostics as short phrases; empty when every one is clean. */
export function diagnosticItems(d: Diagnostics, short: boolean): string[] {
  const items = [
    ...(d.unresolved ? [short ? `${grouped(d.unresolved)} unresolved` : `${plural(d.unresolved, "unresolved reservation")}`] : []),
    ...(d.unbounded ? [`${grouped(d.unbounded)} unbounded`] : []),
    ...(d.pricingGaps ? [plural(d.pricingGaps, "pricing gap")] : []),
    ...(d.legacyUsageGap ? [short ? "legacy" : "legacy gap"] : []),
  ];
  return items.length || !d.uncertain ? items : ["uncertain spend"];
}

/** The digest's budget row: the lead state with each configured limit's share, or the accounted totals without limits. */
export function budgetDigest(budget: BudgetState): { text: string; color: string } {
  switch (budget.kind) {
    case "legacy": return { text: "○ this host generation predates budget accounting", color: PANEL_MUTED };
    case "unavailable": return { text: "○ no budget receipt from this host · totals unknown", color: PANEL_MUTED };
    case "unreadable": return { text: "! budget receipt could not be read", color: PANEL_WARNING };
    case "read": case "local": {
      const { limits, accounted, atLastReceipt: at } = budget.facts;
      const shares = [
        ...(limits.tokens === null ? [] : [`tokens ${percent(accounted.tokens, limits.tokens)}`]),
        ...(limits.usd === null ? [] : [`USD ${percent(accounted.usd, limits.usd)}`]),
      ].join(" · ");
      if (at.exhausted && at.stopped === true) return { text: `✗ budget reached · ${shares} · host stopped`, color: PANEL_ERROR };
      if (at.exhausted) return { text: `! budget reached · ${shares} · ${at.stopped === false ? "host not stopped at receipt" : "stop not recorded"}`, color: PANEL_WARNING };
      if (at.stopped === true) return { text: `! host stopped at last receipt${shares ? ` · ${shares}` : ""}`, color: PANEL_WARNING };
      if (!shares) return { text: `○ no budget limit · ${grouped(accounted.tokens)} tokens · ${dollars(accounted.usd)} accounted${budget.kind === "local" ? " locally" : ""}`, color: PANEL_MUTED };
      return { text: `○ within limits · ${shares}`, color: PANEL_MUTED };
    }
  }
}

/**
 * DIAGNOSTICS: the last receipt's reservations, pricing and legacy gaps and stop state; nothing unless a receipt was
 * read. A local receipt records none of those counts: one neutral row says so beside the stop state.
 */
export function diagnosticsSection(budget: BudgetState, width: number): string[] {
  if (budget.kind !== "read" && budget.kind !== "local") return [];
  const at = budget.facts.atLastReceipt, narrow = width < NARROW, labels = narrow ? 13 : 15;
  const host = (at.stopped === true ? "stopped new requests at last receipt" : at.stopped === false ? "not stopped at last receipt" : "stop state not recorded")
    + (at.active === undefined ? "" : ` · ${plural(at.active, "request")} in flight`);
  const rule = sectionRule("Diagnostics", width);
  if (budget.kind === "local") {
    return [rule, ...glyphRows(`○ ${narrow ? "local receipts record no reservation, pricing or legacy counts" : "local receipts do not record unresolved reservations, pricing gaps or legacy gaps"} · host ${host}`, width)];
  }
  const d = budget.facts.diagnostics;
  if (!diagnosticItems(d, true).length) {
    return [rule, ...glyphRows(`○ ${narrow ? "no reservations, pricing or legacy gaps" : "no unresolved reservations, pricing gaps or legacy gaps"} · host ${host}`, width)];
  }
  const tone = (warn: boolean, text: string) => warn ? PANEL_WARNING + text + PANEL_TEXT : muted(text);
  const lone = !d.unresolved && !d.unbounded && !d.pricingGaps && !d.legacyUsageGap;
  return [rule,
    ...fieldRows("unresolved", tone(d.unresolved > 0 || d.unbounded > 0, narrow ? `${grouped(d.unresolved)} · ${grouped(d.unbounded)} unbounded`
      : `${plural(d.unresolved, "reservation")} · ${grouped(d.unbounded)} unbounded${d.unresolved || d.unbounded ? " · still counted" : ""}`), labels, width),
    ...fieldRows("pricing gaps", tone(d.pricingGaps > 0, d.pricingGaps ? `${grouped(d.pricingGaps)} · ${narrow ? "not free" : "unknown pricing is not free"}` : "0"), labels, width),
    ...fieldRows("legacy gap", tone(d.legacyUsageGap, d.legacyUsageGap ? `yes · ${narrow ? "spend unknown" : "historical spend unknown"}` : "no"), labels, width),
    ...(lone ? fieldRows("uncertain", tone(true, "yes · an admitted request's outcome is unknown"), labels, width) : []),
    ...fieldRows("host", tone(at.stopped === true, host), labels, width),
  ];
}
