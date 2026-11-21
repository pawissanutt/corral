import type { StreamFunction, Usage } from "@earendil-works/pi-ai";
import { lazyStream } from "@earendil-works/pi-ai/api/lazy";
import { claudeUsageEvidence } from "../claude/events.ts";
import type { ModelUsage } from "../tools/corral.ts";

type TokenField = "input" | "output" | "cacheRead" | "cacheWrite";
type TokenEvidence = Partial<Pick<Usage, TokenField>>;

function object(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === "object" ? value as Record<string, unknown> : undefined;
}

function count(value: unknown): number | undefined {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : undefined;
}

function responsesApi(api: string): boolean {
  return api === "openai-responses" || api === "openai-codex-responses";
}

// Even a positive normalized input can conceal an unknown cache deduction.
function uncachedInput(gross: unknown, cacheRead: number | undefined, cacheWrite: number | undefined): number | undefined {
  const total = count(gross);
  return total !== undefined && cacheRead !== undefined && cacheWrite !== undefined
    ? count(total - cacheRead - cacheWrite) : undefined;
}

// Pi's normalized Usage cannot distinguish zero from absent. Snapshot only the relevant
// counters at the raw events the pinned adapters use to populate Usage.
function rawUsage(api: string, data: unknown, previous: TokenEvidence): TokenEvidence {
  const event = object(data);
  if (!event) return previous;
  if (api === "anthropic-messages") {
    const start = event.type === "message_start";
    if (!start && event.type !== "message_delta") return previous;
    const usage = object(start ? object(event.message)?.usage : event.usage);
    const tokens = start ? {} : previous;
    return {
      input: usage?.input_tokens == null ? tokens.input : count(usage.input_tokens),
      output: usage?.output_tokens == null ? tokens.output : count(usage.output_tokens),
      cacheRead: usage?.cache_read_input_tokens == null ? tokens.cacheRead : count(usage.cache_read_input_tokens),
      cacheWrite: usage?.cache_creation_input_tokens == null ? tokens.cacheWrite : count(usage.cache_creation_input_tokens),
    };
  }
  if (api === "bedrock-converse-stream") {
    const usage = object(object(event.metadata)?.usage);
    return usage ? { input: count(usage.inputTokens), output: count(usage.outputTokens),
      cacheRead: count(usage.cacheReadInputTokens), cacheWrite: count(usage.cacheWriteInputTokens) } : previous;
  }
  if (responsesApi(api)) {
    if (event.type !== "response.completed" && event.type !== "response.incomplete" &&
        !(api === "openai-codex-responses" && event.type === "response.done")) return previous;
    const usage = object(object(event.response)?.usage);
    const details = object(usage?.input_tokens_details);
    const cacheRead = count(details?.cached_tokens), cacheWrite = count(details?.cache_write_tokens);
    return { input: uncachedInput(usage?.input_tokens, cacheRead, cacheWrite),
      output: count(usage?.output_tokens), cacheRead, cacheWrite };
  }
  if (api === "openai-completions") {
    const choice = Array.isArray(event.choices) ? object(event.choices[0]) : undefined;
    const usage = object(event.usage ?? choice?.usage);
    if (!usage) return previous;
    const details = object(usage.prompt_tokens_details);
    const cacheRead = count(details?.cached_tokens ?? usage.prompt_cache_hit_tokens ?? usage.cached_tokens);
    const cacheWrite = count(details?.cache_write_tokens);
    return { input: uncachedInput(usage.prompt_tokens, cacheRead, cacheWrite),
      output: count(usage.completion_tokens), cacheRead, cacheWrite };
  }
  return previous;
}

/** Report a completed provider response once; bookkeeping cannot invalidate that response. */
export function observeUsage(stream: StreamFunction, billing: ModelUsage["billing"],
  report: (usage: ModelUsage) => void): StreamFunction {
  return (model, context, options) => lazyStream(model, async () => {
    let raw: TokenEvidence = {};
    const derivedInput = responsesApi(model.api) || model.api === "openai-completions";
    // The anthropic subscription route already tracks presence in the Claude Agent SDK lane;
    // anthropic-api, in contrast, runs Pi's normalized Messages adapter.
    const source = stream(model, context, model.provider === "anthropic" ? options : {
      ...options,
      onProviderStreamEvent(data, current) {
        raw = rawUsage(model.api, data, raw);
        return options?.onProviderStreamEvent?.(data, current);
      },
    });
    return (async function* () {
      for await (const event of source) {
        if (event.type === "error" || (event.type === "done" && event.reason !== "deferred")) {
          const message = event.type === "error" ? event.error : event.message;
          const { usage } = message;
          const evidence = claudeUsageEvidence(message);
          const known = (field: TokenField) => evidence ? evidence.has(field)
            : (raw[field] !== undefined && raw[field] === usage[field]) ||
              (usage[field] > 0 && !(field === "input" && derivedInput));
          const input = known("input"), output = known("output"), read = known("cacheRead"), write = known("cacheWrite");
          const usd = billing === "api" && Number.isFinite(usage.cost.total) && usage.cost.total >= 0
            ? usage.cost.total : undefined;
          if (input || output || read || write || (!evidence && usd !== undefined && usage.cost.totalFromProvider === true)) {
            report({ model: `${model.provider}/${model.id}`,
              ...(input ? { input: usage.input } : {}), ...(output ? { output: usage.output } : {}),
              ...(read ? { cache_read: usage.cacheRead } : {}), ...(write ? { cache_write: usage.cacheWrite } : {}),
              ...(usd === undefined ? {} : { usd }), billing });
          }
        }
        yield event;
      }
    })();
  });
}
