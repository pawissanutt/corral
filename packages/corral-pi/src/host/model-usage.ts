import type { StreamFunction } from "@earendil-works/pi-ai";
import { lazyStream } from "@earendil-works/pi-ai/api/lazy";
import type { ModelUsage } from "../tools/corral.ts";

/** Report a completed provider response once; bookkeeping cannot invalidate that response. */
export function observeUsage(stream: StreamFunction, billing: ModelUsage["billing"], report: (usage: ModelUsage) => void): StreamFunction {
  return (model, context, options) => lazyStream(model, async () => {
    const source = stream(model, context, options);
    return (async function* () {
      for await (const event of source) {
        if (event.type === "done" && event.reason !== "deferred") {
          const usage = event.message.usage;
          if (usage.input || usage.output || usage.cacheRead || usage.cacheWrite) {
            report({ model: `${model.provider}/${model.id}`, input: usage.input, output: usage.output,
              cache_read: usage.cacheRead, cache_write: usage.cacheWrite,
              ...(billing === "api" ? { usd: usage.cost.total } : {}), billing });
          }
        }
        yield event;
      }
    })();
  });
}
