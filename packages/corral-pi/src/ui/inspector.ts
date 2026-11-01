// Read-only host observations. Neither a tool commit nor an agent assertion is gate acceptance.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { EventLog, EventPage } from "../bridge/events.ts";
import type { CapabilitySet } from "../roles/types.ts";
import { renderResultText } from "./render.ts";
import { observedCorralResult } from "./receipt-source.ts";

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
  readonly receipts: readonly string[];
  readonly receiptsOmitted: number;
}
const plainTheme = { fg: (_color: string, text: string) => text, bold: (text: string) => text };

/** Retains bounded summaries, not receipt bodies or tool arguments that may contain private text. */
export class InspectorActivity {
  private readonly receipts: string[] = [];
  private omitted = 0;
  constructor(private readonly events: EventLog, private readonly changed: () => void) {}

  extend(pi: ExtensionAPI, allowed: ReadonlySet<string>): void {
    pi.on("tool_result", event => {
      if (!allowed.has(event.toolName) || !observedCorralResult(event)) return;
      const summary = renderResultText({ content: event.content, details: event.details }, false, event.isError, plainTheme);
      const details = event.details as { state?: unknown } | undefined;
      const state = typeof details?.state === "string"
        && ["committed", "rejected", "reserved", "queued", "uncertain", "abandoned"].includes(details.state) ? details.state : "no receipt";
      this.receipts.push(`${event.toolName}${event.parentToolCallId ? " (Code Mode child)" : ""} · ${state} · ${summary}`);
      if (this.receipts.length > 24) { this.receipts.shift(); this.omitted++; }
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

export function inspectorActivity(info: InspectorSnapshot): string[] {
  const page = info.events;
  return [
    `Host events · g${page.generation} · last sequence ${page.last}`,
    ...(page.gap ? [`Event gap: ${page.gap.missing} missing before ${page.gap.resumed}; reconcile canonical records`] : ["No gap in retained host generation"]),
    ...(page.more ? ["Event page incomplete; additional observations unavailable in this view"] : []),
    ...page.events.slice(-12).map(event => `#${event.seq} ${event.at} · ${event.kind}${event.omitted ? " · payload omitted" : ""}`),
    "Corral tool receipt outcomes · observed in this host only",
    `Earlier sessions/host lifecycle receipts unavailable${info.receiptsOmitted ? `; ${info.receiptsOmitted} older tool outcomes omitted` : ""}`,
    ...info.receipts,
    ...(!info.receipts.length ? ["No tool receipt observed in this host"] : []),
    "Committed means operation recorded, not gate PASS or criterion acceptance.",
  ];
}
