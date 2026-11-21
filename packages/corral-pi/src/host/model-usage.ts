import type { StreamFunction } from "@earendil-works/pi-ai";
import { lazyStream } from "@earendil-works/pi-ai/api/lazy";
import { claudeUsageEvidence } from "../claude/events.ts";
import type { ModelUsage } from "../tools/corral.ts";

/** Report a completed provider response once; bookkeeping cannot invalidate that response. */
export function observeUsage(stream: StreamFunction, billing: ModelUsage["billing"],
  report: (usage: ModelUsage) => void): StreamFunction {
  return (model, context, options) => lazyStream(model, async () => {
    const source = stream(model, context, options);
    return (async function* () {
      for await (const event of source) {
        if (event.type === "error" || (event.type === "done" && event.reason !== "deferred")) {
          const message = event.type === "error" ? event.error : event.message;
          const { usage } = message;
          const evidence = claudeUsageEvidence(message);
          // Other Pi routes retain their normalized usage. For Claude, an empty set means
          // the SDK provided nothing; a reported zero is still evidence worth recording.
          if (evidence ? evidence.size > 0 : usage.input || usage.output || usage.cacheRead || usage.cacheWrite) {
            const usd = billing === "api" && Number.isFinite(usage.cost.total) && usage.cost.total >= 0
              ? usage.cost.total : undefined;
            report({ model: `${model.provider}/${model.id}`,
              ...(evidence?.has("input") !== false ? { input: usage.input } : {}),
              ...(evidence?.has("output") !== false ? { output: usage.output } : {}),
              ...(evidence?.has("cacheRead") !== false ? { cache_read: usage.cacheRead } : {}),
              ...(evidence?.has("cacheWrite") !== false ? { cache_write: usage.cacheWrite } : {}),
              ...(usd === undefined ? {} : { usd }), billing });
          }
        }
        yield event;
      }
    })();
  });
}
