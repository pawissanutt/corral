import type { Model } from "@earendil-works/pi-ai";

import type { ProviderId } from "../auth/providers.ts";

/**
 * Models the subscription providers serve that the bundled Pi catalog predates. They resolve
 * only for their own provider and model API, so the network guard and lane checks still apply.
 * A model the models.json file or the bundled catalog defines wins over an entry here.
 */
export const SUPPLEMENTAL_MODELS: Readonly<Partial<Record<ProviderId, readonly Model<any>[]>>> = {
  "openai-codex": [{
    id: "gpt-6.1-terra",
    name: "GPT-6.1 Terra",
    api: "openai-codex-responses",
    provider: "openai-codex",
    baseUrl: "https://chatgpt.com/backend-api",
    reasoning: true,
    input: ["text", "image"],
    cost: { input: 1, output: 5, cacheRead: 0.05, cacheWrite: 1.25 },
    contextWindow: 500000,
    maxTokens: 128000,
    thinkingLevelMap: { off: "none", minimal: "low", low: "low", medium: "medium", high: "high", xhigh: "xhigh", max: "max" },
    compat: {
      supportsOpenAIGrammarTools: true, supportsAdditionalTools: true,
      supportsToolSearch: true, supportsMidConvoSystemMessages: true,
    },
  } as Model<any>],
  anthropic: [],
};
