// Read-only host observations. Neither a tool commit nor an agent assertion is gate acceptance.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { BridgeEvent, EventLog, EventPage } from "../bridge/events.ts";
import type { CapabilitySet } from "../roles/types.ts";
import { corralSummary } from "./corral-presenter.ts";
import type { PagerLine } from "./pager.ts";
import { plainLabel } from "./panels-view.ts";
import { observedCorralResult } from "./receipt-source.ts";

/**
 * One corral operation this host observed: when, which tool, whether a Code Mode cell made it, the receipt state
 * (`no receipt` when the result carried none) and the presenter's one-line summary (`""` without a presenter).
 * Never the receipt body or the tool arguments, which may hold private text.
 */
export interface ReceiptRow { readonly at: number; readonly tool: string; readonly nested: boolean; readonly state: string; readonly summary: string }
/** Receipt states a corral result can carry; anything else reads `no receipt`. */
const RECEIPT_STATES: Readonly<Record<string, true>> = { committed: true, rejected: true, reserved: true, queued: true, uncertain: true, abandoned: true };
/** Operations kept; older ones are counted, not kept. */
const RECEIPT_RING = 24;
/** Summary characters a readable line keeps so its state still shows; the exact records keep all of it. */
const SUMMARY_CELLS = 96;

export interface InspectorSnapshot {
  readonly session: string;
  readonly boundSession: string | null;
  readonly turn: number;
  readonly turnOpen: boolean;
  readonly capabilities: Pick<CapabilitySet, "catalog" | "digest" | "preset" | "enabled"> | null;
  readonly guidance: {
    readonly projectText: boolean | null;
    readonly skillPaths: readonly string[];
    readonly promptPaths: readonly string[];
    readonly prompts: readonly string[];
    readonly skills: readonly string[];
  };
  readonly settings: readonly string[];
  readonly budget: readonly string[];
  readonly events: EventPage;
  readonly receipts: readonly ReceiptRow[];
  readonly receiptsOmitted: number;
}

/** Retains bounded observations, not receipt bodies or tool arguments that may contain private text. */
export class InspectorActivity {
  private readonly receipts: ReceiptRow[] = [];
  private omitted = 0;
  constructor(private readonly events: EventLog, private readonly changed: () => void) {}

  extend(pi: ExtensionAPI, allowed: ReadonlySet<string>): void {
    pi.on("tool_result", event => {
      if (!allowed.has(event.toolName) || !observedCorralResult(event)) return;
      const details = event.details as { state?: unknown } | undefined;
      const state = typeof details?.state === "string" && RECEIPT_STATES[details.state] === true ? details.state : "no receipt";
      const summary = corralSummary(event.toolName, event.input, { content: event.content, details: event.details }, event.isError) ?? "";
      this.receipts.push({ at: Date.now(), tool: event.toolName, nested: event.parentToolCallId !== undefined, state, summary });
      if (this.receipts.length > RECEIPT_RING) { this.receipts.shift(); this.omitted++; }
      this.changed();
    });
  }

  snapshot(): Pick<InspectorSnapshot, "events" | "receipts" | "receiptsOmitted"> {
    // Request the entire bounded ring; unlike a zero-based count, a dropped prefix is explicit.
    return { events: this.events.since(0, 512 * 8192, 512), receipts: [...this.receipts], receiptsOmitted: this.omitted };
  }
}

export function inspectorContext(info: InspectorSnapshot): string[] {
  const caps = info.capabilities;
  return [
    "Effective generation configuration",
    ...(caps ? [`Preset ${caps.preset} · catalog ${caps.catalog}`, `Snapshot digest ${caps.digest}`,
      `Enabled capabilities: ${caps.enabled.join(", ") || "none"}`] : ["Capabilities unavailable · legacy launch"]),
    `Project-text trust: ${info.guidance.projectText === null ? "unavailable · legacy launch" : info.guidance.projectText ? "enabled" : "disabled"}`,
    "Settings provenance: host restrictions + recorded launch snapshot; per-key source history unavailable",
    ...info.settings,
    "Skill search paths (not proof a skill was read)", ...info.guidance.skillPaths,
    "Loaded skill resources", ...info.guidance.skills,
    "Prompt search paths", ...info.guidance.promptPaths,
    `Loaded templates: ${info.guidance.prompts.join(", ") || "none"}`,
    "Context/accounting", ...info.budget,
    "Token counts are SDK accounting, not a measurement of guidance actually read.",
  ];
}

/** `HH:MM:SS` local; another day adds its date; an unreadable time stays as recorded. */
function clock(at: number | string, now: number): string {
  const date = new Date(at);
  if (Number.isNaN(date.getTime())) return plainLabel(String(at));
  const time = date.toTimeString().slice(0, 8), today = new Date(now);
  if (date.toDateString() === today.toDateString()) return time;
  const day = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
  return `${day} ${time}`;
}
const words = (kind: string) => plainLabel(kind.replace(/^corral_/, "").replace(/_/g, " "));
/** The presenter's own glyph, kept only where the receipt state does not say otherwise; `✓` needs a receipt. */
function receiptGlyph(row: ReceiptRow): string {
  const own = /^([✓✗○▲!…]) /.exec(row.summary)?.[1];
  if (row.state === "rejected") return "✗";
  if (row.state === "uncertain" || row.state === "abandoned") return "!";
  if (row.state === "queued" || row.state === "reserved") return "○";
  if (row.state === "no receipt") return own === "✗" ? "✗" : "!";
  return own ?? "○";
}
function stateWords(row: ReceiptRow, glyph: string): string {
  if (row.state === "uncertain" || row.state === "abandoned") return `${row.state} · outcome unknown`;
  if (row.state === "no receipt") return glyph === "✗" ? "no receipt · failed" : "no receipt · outcome unknown";
  return row.state;
}
function eventRaw(event: BridgeEvent): string {
  const omitted = event.omitted === undefined ? ""
    : ` · omitted ${plainLabel(String(event.omitted))}${typeof event.bytes === "number" ? `, ${event.bytes} bytes` : ""}`;
  return `#${event.seq} ${plainLabel(event.at)} · g${event.generation} · ${plainLabel(event.kind)}${omitted}`;
}

/**
 * Host events and the corral operations this host observed, for the receipts pager. `lines` read in words: local
 * time, the event or operation, its outcome. `raw` holds the exact records kept: sequence, generation, ISO time,
 * raw kind, tool, receipt state and the whole summary; never an event payload. Gaps, a partial page and omitted
 * operations are stated in both, and nothing without a receipt reads as success.
 */
export function receiptLines(info: InspectorSnapshot, now: number): { readonly lines: PagerLine[]; readonly raw: PagerLine[] } {
  const page = info.events, gap = page.gap;
  const lines: PagerLine[] = [{ text: `EVENTS · this host, generation ${page.generation}`, heading: true }];
  const raw: PagerLine[] = [{ text: `events · generation ${page.generation} · last #${page.last} · next #${page.next}`, heading: true }];
  for (const event of page.events) {
    lines.push({ text: `${clock(event.at, now)}  ${words(event.kind)}${event.omitted === undefined ? "" : " · payload omitted"}` });
    raw.push({ text: eventRaw(event) });
  }
  if (!page.events.length) { lines.push({ text: "○ no events in this generation yet" }); raw.push({ text: "events · none" }); }
  if (gap) {
    lines.push({ text: `! ${gap.missing} earlier event${gap.missing === 1 ? "" : "s"} of this generation left the host's log`
      + ` (#${gap.after + 1}–#${gap.resumed - 1}) · reconcile from canonical records` });
    raw.push({ text: `gap · after #${gap.after} · resumed #${gap.resumed} · ${gap.missing} missing · reconcile` });
  } else { lines.push({ text: "○ no gap in this generation's log" }); raw.push({ text: "gap · none" }); }
  if (page.more) {
    lines.push({ text: "! more events than one page holds · later events are not shown" });
    raw.push({ text: `more · true · next #${page.next}` });
  }
  lines.push({ text: "" }, { text: "CORRAL OPERATIONS · observed in this host only", heading: true });
  raw.push({ text: "" }, { text: `receipts · last ${RECEIPT_RING} kept · ${info.receipts.length} shown`, heading: true });
  for (const row of info.receipts) {
    const glyph = receiptGlyph(row);
    const said = plainLabel(row.summary.replace(/^[✓✗○▲!…] /, ""));
    const body = !said ? words(row.tool) : said.length > SUMMARY_CELLS ? `${said.slice(0, SUMMARY_CELLS - 1)}…` : said;
    lines.push({ text: `${clock(row.at, now)}  ${glyph} ${body} · ${words(row.tool)} · ${stateWords(row, glyph)}${row.nested ? " · in a Code Mode cell" : ""}` });
    raw.push({ text: `${new Date(row.at).toISOString()} · ${plainLabel(row.tool)} · ${row.state}${row.nested ? " · Code Mode child" : ""}`
      + ` · ${plainLabel(row.summary) || "no summary"}` });
  }
  if (!info.receipts.length) { lines.push({ text: "○ no corral operation observed in this host yet" }); raw.push({ text: "receipts · none" }); }
  if (info.receiptsOmitted) {
    lines.push({ text: `○ ${info.receiptsOmitted} older operation${info.receiptsOmitted === 1 ? "" : "s"} not kept` });
    raw.push({ text: `omitted · ${info.receiptsOmitted} older receipts` });
  }
  const earlier = "○ operations of earlier sessions and hosts are not shown here";
  const footer = "Committed means recorded, not a gate pass or criterion acceptance.";
  lines.push({ text: earlier }, { text: "" }, { text: footer });
  raw.push({ text: earlier }, { text: "" }, { text: footer });
  return { lines, raw };
}
