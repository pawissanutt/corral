import type { ExtensionAPI, SessionEntry } from "@earendil-works/pi-coding-agent";
import type { Usage } from "@earendil-works/pi-ai";

type CacheCounts = Pick<Usage, "input" | "cacheRead" | "cacheWrite">;

function rate(usage: CacheCounts): string {
  const total = usage.input + usage.cacheRead + usage.cacheWrite;
  return total > 0 ? `${(usage.cacheRead / total * 100).toFixed(1)}%` : "unavailable";
}

/** Match Pi's session accounting, including nested work and compaction, rather than averaging percentages. */
export function cacheStatsText(entries: readonly SessionEntry[]): string {
  const total: CacheCounts = { input: 0, cacheRead: 0, cacheWrite: 0 };
  let latest: Usage | undefined;
  let replay: string | undefined;
  const add = (usage: Usage | undefined) => {
    if (!usage) return;
    total.input += usage.input;
    total.cacheRead += usage.cacheRead;
    total.cacheWrite += usage.cacheWrite;
  };
  for (const entry of entries) {
    if (entry.type === "message") {
      const message = entry.message;
      if (message.role === "assistant") {
        latest = message.usage;
        add(message.usage);
        const detail = message.diagnostics?.find(diagnostic => diagnostic.type === "claude_cache")?.details;
        replay = detail ? JSON.stringify(detail) : undefined;
      } else if (message.role === "toolResult") add(message.usage);
    } else if (entry.type === "usage" || entry.type === "compaction" || entry.type === "branch_summary") add(entry.usage);
  }
  return [
    "Cache hits = reads / (reads + writes + uncached input); output tokens are excluded.",
    `Latest model request: ${latest ? rate(latest) : "unavailable"}`,
    `Whole session: ${rate(total)} (all models, nested tools and summaries; cold starts included)`,
    `Session input: ${total.input} uncached, ${total.cacheWrite} written, ${total.cacheRead} read from cache.`,
    ...(replay ? [`Latest Claude query diagnostics (local state is not a provider hit): ${replay}`] : []),
  ].join("\n");
}

export function registerCacheStats(pi: ExtensionAPI): void {
  pi.registerCommand("cache-stats", {
    description: "Compare latest-request and whole-session input cache rates",
    handler: async (_args, ctx) => { ctx.ui.notify(cacheStatsText(ctx.sessionManager.getEntries()), "info"); },
  });
}
