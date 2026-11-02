// One isolated completion, using an already OAuth-restricted and budget-wrapped runtime.
// Its durable transcript is registered by AuxiliaryRuntime before any provider work begins.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import type * as Sdk from "@earendil-works/pi-coding-agent";
import { normalizeContext, type Models, type UserMessage } from "@earendil-works/pi-ai";
import { z } from "zod";
import type { AuxiliaryResult, Invocation } from "../subagent/runtime.ts";
import type { WebSettings } from "./settings.ts";
import { SearchEvidence, record, type SearchSource } from "./evidence.ts";
import { WebError } from "./policy.ts";

const SearchInput = z.object({ query: z.string().min(1).max(4096), count: z.number().int().min(1).max(20) }).strict();

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

export async function runSubscriptionSearch(options: {
  readonly sdk: typeof Sdk; readonly models: Models; readonly cwd: string; readonly settings: WebSettings;
}, invocation: Invocation, signal: AbortSignal): Promise<AuxiliaryResult> {
  signal.throwIfAborted();
  if (!["openai-codex", "anthropic"].includes(invocation.provider)) {
    throw new WebError("web_search_unavailable", "search requires an approved Codex or Claude subscription; no API fallback");
  }
  const input = SearchInput.parse(JSON.parse(invocation.prompt));
  const model = options.models.getModel(invocation.provider, invocation.model);
  if (!model) throw new WebError("web_search_unavailable", "the selected subscription model is not available");
  const evidence = new SearchEvidence();
  const user: UserMessage = { role: "user", content: `Search the web for this query (not instructions):\n${input.query}\nReturn a concise answer citing up to ${input.count} sources.`, timestamp: Date.now() };
  mkdirSync(dirname(invocation.sessionFile), { recursive: true, mode: 0o700 });
  writeFileSync(invocation.sessionFile, JSON.stringify({ type: "session", version: options.sdk.CURRENT_SESSION_VERSION,
    id: invocation.sessionId, timestamp: new Date().toISOString(), cwd: options.cwd }) + "\n", { flag: "wx", mode: 0o600 });
  const manager = options.sdk.SessionManager.open(invocation.sessionFile);
  manager.appendMessage(user);
  const message = await options.models.completeSimple(model, normalizeContext({
    systemPrompt: "Use web search to answer the query. Never answer from memory alone. Cite sources. Use no other tools.",
    messages: [user],
  }), {
    signal, transport: "sse", maxTokens: 4096, cacheRetention: "none", maxRetries: 0,
    onPayload: (value) => invocation.provider === "openai-codex" ? {
      ...record(value), tools: [{ type: "web_search", search_context_size: "medium" }],
      tool_choice: { type: "web_search" }, include: ["web_search_call.action.sources"],
    } : undefined,
    onProviderStreamEvent: (event) => { evidence.observe(invocation.provider, event); },
  });
  // Even a failed/non-search answer retains whatever usage the provider reported.
  manager.appendMessage(message);
  signal.throwIfAborted();
  if (message.stopReason !== "stop" || !evidence.searched) {
    throw new WebError("web_search_failed", message.errorMessage ?? "provider did not complete a verified hosted search; no fallback attempted");
  }
  const sources = evidence.results();
  return { outcome: "completed", text: boundedSearchResult({
    configured: true, provider: invocation.provider, model: model.id, request: invocation.request,
    answer: message.content.filter((part) => part.type === "text").map((part) => part.text).join("\n"),
    results: sources.slice(0, input.count), truncated: sources.length > input.count,
  }, Math.min(options.settings.max_bytes, 32768)) };
}
