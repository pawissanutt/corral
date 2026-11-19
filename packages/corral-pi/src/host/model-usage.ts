import type { StreamFunction } from "@earendil-works/pi-ai";
import { lazyStream } from "@earendil-works/pi-ai/api/lazy";
import type { ModelUsage } from "../tools/corral.ts";

/** Report a completed provider response once; bookkeeping cannot invalidate that response. */
export function observeUsage(stream: StreamFunction, billing: ModelUsage["billing"],
  report: (usage: ModelUsage) => void): StreamFunction {
  return (model, context, options) => lazyStream(model, async () => {
    const source = stream(model, context, options);
    return (async function* () {
      for await (const event of source) {
        if (event.type === "error" || (event.type === "done" && event.reason !== "deferred")) {
          const usage = (event.type === "error" ? event.error : event.message).usage;
          const usd = billing === "api" && Number.isFinite(usage.cost.total) && usage.cost.total >= 0
            ? usage.cost.total : undefined;
          if (usage.input || usage.output || usage.cacheRead || usage.cacheWrite) {
            report({ model: `${model.provider}/${model.id}`, input: usage.input, output: usage.output,
              cache_read: usage.cacheRead, cache_write: usage.cacheWrite,
              ...(usd === undefined ? {} : { usd }), billing });
          }
        }
        yield event;
      }
    })();
  });
}
