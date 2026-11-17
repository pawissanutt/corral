import type { Api, Model } from "@earendil-works/pi-ai";

import type { ProviderId } from "../auth/providers.ts";

// Published metadata: https://pi.dev/models/openai-codex/gpt-6-2-astra and
// https://pi.dev/models/openai-codex/gpt-6-2-sol (API variants under /models/openai/).
const codexModels: readonly Model<"openai-codex-responses">[] = [
  {
    id: "gpt-6.2-astra", name: "GPT-6.2 Astra", api: "openai-codex-responses", provider: "openai-codex",
    baseUrl: "https://chatgpt.com/backend-api", reasoning: true, input: ["text", "image"],
    contextWindow: 272000, maxTokens: 128000,
    cost: { input: 8, output: 40, cacheRead: 0.4, cacheWrite: 10,
      tiers: [{ inputTokensAbove: 272000, input: 16, output: 60, cacheRead: 0.8, cacheWrite: 20 }] },
    thinkingLevelMap: { off: null, minimal: "low", low: "low", medium: "medium", high: "high", xhigh: "xhigh", max: "max" },
    compat: { supportsOpenAIGrammarTools: true, supportsAdditionalTools: true,
      supportsToolSearch: true, supportsMidConvoSystemMessages: true },
  },
  {
    id: "gpt-6.2-sol", name: "GPT-6.2 Sol", api: "openai-codex-responses", provider: "openai-codex",
    baseUrl: "https://chatgpt.com/backend-api", reasoning: true, input: ["text", "image"],
    contextWindow: 500000, maxTokens: 128000,
    cost: { input: 2, output: 10, cacheRead: 0.1, cacheWrite: 2.5,
      tiers: [{ inputTokensAbove: 272000, input: 4, output: 15, cacheRead: 0.2, cacheWrite: 5 }] },
    thinkingLevelMap: { off: null, minimal: "low", low: "low", medium: "medium", high: "high", xhigh: "xhigh", max: "max" },
    compat: { supportsOpenAIGrammarTools: true, supportsAdditionalTools: true,
      supportsToolSearch: true, supportsMidConvoSystemMessages: true },
  },
];

const apiModels: readonly Model<"openai-responses">[] = codexModels.map(model => ({
  ...model, provider: "openai", api: "openai-responses", baseUrl: "https://api.openai.com/v1",
  contextWindow: 272000,
  thinkingLevelMap: { ...model.thinkingLevelMap, minimal: null },
  compat: { ...model.compat, supportsStrictMode: true, supportsExplicitPromptCacheMode: true, supportsMidConvoEffort: true },
}));

/**
 * Models the providers serve that the bundled Pi catalog predates. Compose these before
 * models.json so explicit entries and overrides win; newer bundled definitions also win.
 */
export const SUPPLEMENTAL_MODELS: Readonly<Partial<Record<ProviderId, readonly Model<Api>[]>>> = {
  "openai-codex": codexModels,
  openai: apiModels,
};
