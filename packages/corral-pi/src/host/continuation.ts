import type { Api, AssistantMessageEvent, Model, SimpleStreamOptions } from "@earendil-works/pi-ai";
import type { StreamFn } from "@earendil-works/pi-agent-core";
import { lazyStream } from "@earendil-works/pi-ai/api/lazy";
import { isProviderId, type ProviderId } from "../auth/providers.ts";
import { messageFailure } from "./provider-failure.ts";

export interface FallbackSelector {
  readonly provider: ProviderId;
  readonly model: string;
}

export type FallbackCandidates = readonly FallbackSelector[] | ((current: Model<Api>) => readonly FallbackSelector[]);

interface FallbackSelection {
  readonly model: Model<Api>;
  readonly stream: StreamFn;
  readonly reasoning?: SimpleStreamOptions["reasoning"];
}

type SelectFallback = (candidate: FallbackSelector, signal?: AbortSignal) =>
  FallbackSelection | undefined | Promise<FallbackSelection | undefined>;

type StartEvent = Extract<AssistantMessageEvent, { type: "start" }>;
type ErrorEvent = Extract<AssistantMessageEvent, { type: "error" }>;

function isQuotaFallback(error: ErrorEvent["error"], provider: ProviderId): boolean {
  const failure = messageFailure(error, provider);
  return failure.category === "subscription_quota" && !failure.replayed;
}

/** Retry only an output-free subscription quota failure, inside the same Pi request.
 * Select preflights a candidate, returning undefined if unavailable, and commits the
 * model before returning a selection. The selected stream must resolve its own auth. */
export function withSubscriptionFallback(stream: StreamFn, candidates: FallbackCandidates,
  select: SelectFallback, notice: (candidate: FallbackSelector, previous: Model<Api>) => void): StreamFn {
  if (Array.isArray(candidates) && candidates.length === 0) return stream;
  return (model, context, options) => {
    const signal = options?.signal;
    return lazyStream(model, async () => {
      let source = await stream(model, context, options);
      return {
        result: () => source.result(),
        async *[Symbol.asyncIterator]() {
          let current = model;
          let seen: Set<string> | undefined;
          for (;;) {
            let start: StartEvent | undefined;
            let quota: ErrorEvent | undefined;
            let output = false;
            for await (const event of source) {
              if (event.type === "start" && !output) { start = event; continue; }
              if (!output && event.type === "error" && event.reason === "error" && !signal?.aborted
                  && !start?.partial.content.length && !event.error.content.length && isProviderId(current.provider)
                  && isQuotaFallback(event.error, current.provider)) {
                quota = event;
                break;
              }
              if (start) { yield start; start = undefined; }
              output = true;
              yield event;
            }
            if (!quota) {
              if (start) yield start;
              return;
            }

            if (!seen) {
              seen = new Set([`${model.provider}/${model.id}`]);
              if (Array.isArray(candidates)) {
                // A persisted fallback model resumes at its position, not the list head.
                const resumeAfter = candidates.findIndex(candidate => candidate.provider === model.provider && candidate.model === model.id) + 1;
                for (let i = 0; i < resumeAfter; i++) {
                  const candidate = candidates[i]!;
                  seen.add(`${candidate.provider}/${candidate.model}`);
                }
              }
            }
            const list = typeof candidates === "function" ? candidates(current) : candidates;
            let next: FallbackSelection | undefined;
            for (const candidate of list) {
              if (signal?.aborted) break;
              const key = `${candidate.provider}/${candidate.model}`;
              if (seen.has(key)) continue;
              seen.add(key);
              try { next = await select(candidate, signal); }
              catch (error) { if (!signal?.aborted) throw error; }
              if (next) {
                notice(candidate, current);
                break;
              }
            }
            if (!next || signal?.aborted) {
              if (start) yield start;
              yield quota;
              return;
            }
            current = next.model;
            // Let the new provider's guarded route resolve auth and transport. Keep the
            // same transcript, signal, session id, and provider event hooks.
            source = await next.stream(current, context, {
              ...options, apiKey: undefined, env: undefined, headers: undefined, fetch: undefined,
              serviceTier: undefined, reasoning: next.reasoning,
            });
          }
        },
      };
    }, undefined, signal);
  };
}
