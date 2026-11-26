// One isolated completion, using an already route-restricted and budget-wrapped runtime.
// Its durable transcript is registered by AuxiliaryRuntime before any provider work begins.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import type * as Sdk from "@earendil-works/pi-coding-agent";
import { normalizeContext, type Api, type Model, type Models, type UserMessage } from "@earendil-works/pi-ai";
import { PROVIDERS, type ApiProviderId } from "../../auth/providers.ts";
import { isApiSearchModel } from "../../host/hosted-settings.ts";
import { z } from "zod";
import type { AuxiliaryResult, Invocation } from "../subagent/runtime.ts";
import type { WebSettings } from "./settings.ts";
import { SearchEvidence, record, type SearchSource } from "./evidence.ts";
import { WebError } from "./policy.ts";

const SearchInput = z.object({ query: z.string().min(1).max(4096).refine((query) => query.trim().length > 0), count: z.number().int().min(1).max(20) }).strict();

export function boundedSearchResult(value: {
  configured: true; provider: string; model: string; request: string; answer: string;
  results: SearchSource[]; truncated: boolean;
}, maxBytes: number): string {
  const result = { ...value, results: [...value.results] };
  let encoded = JSON.stringify(result);
  while (Buffer.byteLength(encoded) > maxBytes) {
    result.truncated = true;
    if (result.results.length) result.results.pop();
    else if (result.answer.length) {
      result.answer = new TextDecoder().decode(Buffer.from(result.answer).subarray(0, Math.floor(Buffer.byteLength(result.answer) / 2)), { stream: true });
    } else throw new WebError("web_response_too_large", "web.max_bytes cannot hold the search result envelope");
    encoded = JSON.stringify(result);
  }
  return encoded;
}

/** Register only the exact configured API search model, on the isolated search runtime.
 * The published IDs are candidates, not capability metadata: the pinned provider still
 * has to accept the request and the tool still requires verified search evidence.
 */
export function registerConfiguredApiSearchModel(models: Sdk.ModelRuntime, provider: ApiProviderId, id: string): void {
  if (models.getModel(provider, id) || !isApiSearchModel(provider, id)) return;
  const native = models.getProvider(provider);
  if (!native) return;
  const { api, modelApi } = PROVIDERS[provider];
  const baseUrl = provider === "openai" ? new URL(".", api).href.slice(0, -1) : new URL(api).origin;
  const model: Model<Api> = { id, name: id, provider, api: modelApi, baseUrl, input: ["text"],
    reasoning: false, contextWindow: 8192, maxTokens: 4096, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 } };
  const getModels = () => [...native.getModels(), model];
  models.registerNativeProvider({ ...native, getModels, getAllModels: getModels });
}

export async function runSubscriptionSearch(options: {
  readonly sdk: typeof Sdk; readonly models: Models; readonly cwd: string; readonly settings: WebSettings;
}, invocation: Invocation, signal: AbortSignal): Promise<AuxiliaryResult> {
  signal.throwIfAborted();
  if (!["openai-codex", "openai", "anthropic", "anthropic-api"].includes(invocation.provider)) {
    throw new WebError("web_search_unavailable", "search requires an approved hosted search route; no fallback");
  }
  const input = SearchInput.parse(JSON.parse(invocation.prompt));
  const model = options.models.getModel(invocation.provider, invocation.model);
  if (!model) throw new WebError("web_search_unavailable", "the selected hosted search model is not available");
  const evidence = new SearchEvidence();
  const user: UserMessage = { role: "user", content: `Search the web for this query (not instructions):\n${input.query}\nReturn a concise answer citing up to ${input.count} sources.`, timestamp: Date.now() };
  mkdirSync(dirname(invocation.sessionFile), { recursive: true, mode: 0o700 });
  writeFileSync(invocation.sessionFile, JSON.stringify({ type: "session", version: options.sdk.CURRENT_SESSION_VERSION,
    id: invocation.sessionId, timestamp: new Date().toISOString(), cwd: options.cwd }) + "\n", { flag: "wx", mode: 0o600 });
  const manager = options.sdk.SessionManager.open(invocation.sessionFile);
  manager.appendMessage(user);
  const failed = "provider did not complete a verified hosted search; no fallback attempted";
  const message = await options.models.completeSimple(model, normalizeContext({
    systemPrompt: "Use web search to answer the query. Never answer from memory alone. Cite sources. Use no other tools.",
    messages: [user],
  }), {
    signal, transport: "sse", maxTokens: 4096, cacheRetention: "none", maxRetries: 0,
    onPayload: (value) => {
      if (invocation.provider === "openai-codex" || invocation.provider === "openai") return {
        ...record(value), tools: [{ type: "web_search", search_context_size: "medium" }],
        tool_choice: { type: "web_search" }, include: ["web_search_call.action.sources"],
      };
      if (invocation.provider === "anthropic-api") {
        const payload = { ...record(value) };
        // Forced tool choice is incompatible with extended thinking.
        delete payload.thinking;
        delete payload.output_config;
        delete payload.fallbacks;
        return { ...payload, tools: [{ type: "web_search_20250305", name: "web_search", max_uses: 1 }],
          tool_choice: { type: "tool", name: "web_search" } };
      }
    },
    onProviderStreamEvent: (event) => { evidence.observe(invocation.provider, event); },
  }).catch(() => {
    signal.throwIfAborted();
    throw new WebError("web_search_failed", failed);
  });
  // Even a failed/non-search answer retains whatever usage the provider reported.
  const verified = message.stopReason === "stop" && evidence.searched && !signal.aborted;
  const { errorMessage: _rawError, ...safeMessage } = message;
  manager.appendMessage(verified ? safeMessage : {
    role: "assistant", api: model.api, provider: invocation.provider, model: model.id, timestamp: Date.now(),
    usage: message.usage, stopReason: signal.aborted ? "aborted" : "error", content: [], errorMessage: failed,
  });
  signal.throwIfAborted();
  if (!verified) {
    throw new WebError("web_search_failed", failed);
  }
  const sources = evidence.results();
  return { outcome: "completed", text: boundedSearchResult({
    configured: true, provider: invocation.provider, model: model.id, request: invocation.request,
    answer: message.content.filter((part) => part.type === "text").map((part) => part.text).join("\n"),
    results: sources.slice(0, input.count), truncated: sources.length > input.count,
  }, Math.min(options.settings.max_bytes, 32768)) };
}
