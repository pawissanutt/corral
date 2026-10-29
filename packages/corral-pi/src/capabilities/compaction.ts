// Upstream compaction and branch-summary settings from the generation snapshot (T-564, plan
// T-460@P3 menu 4). Omitted values keep upstream's defaults; summarization requests record their
// usage on the session entry, which corral's usage collector counts toward the run.
import type { CapabilitySet } from "../roles/types.ts";
import type { CapabilityGuidance } from "./guidance.ts";

export interface CompactionSettings {
  readonly compaction: { enabled: boolean; reserveTokens?: number; keepRecentTokens?: number };
  readonly branchSummary?: { reserveTokens?: number; skipPrompt?: boolean };
}

function count(value: unknown): number | undefined {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : undefined;
}

/** An older corral sends no settings, so its host keeps upstream's automatic compaction. */
export function compactionSettings(capabilities?: CapabilitySet, guidance?: CapabilityGuidance): CompactionSettings {
  if (!capabilities || !guidance) return { compaction: { enabled: true } };
  const configured = guidance.settings.compaction ?? {};
  const reserveTokens = count(configured.reserve_tokens);
  const keepRecentTokens = count(configured.keep_recent_tokens);
  const summary = (configured.branch_summary ?? {}) as Record<string, unknown>;
  const summaryReserve = count(summary.reserve_tokens);
  const skipPrompt = typeof summary.skip_prompt === "boolean" ? summary.skip_prompt : undefined;
  return {
    compaction: {
      enabled: capabilities.enabled.includes("compaction"),
      ...(reserveTokens !== undefined ? { reserveTokens } : {}),
      ...(keepRecentTokens !== undefined ? { keepRecentTokens } : {}),
    },
    ...(summaryReserve !== undefined || skipPrompt !== undefined
      ? {
        branchSummary: {
          ...(summaryReserve !== undefined ? { reserveTokens: summaryReserve } : {}),
          ...(skipPrompt !== undefined ? { skipPrompt } : {}),
        },
      }
      : {}),
  };
}
