import type { Api, Model, Provider } from "@earendil-works/pi-ai";
import { openAICompletionsApi } from "@earendil-works/pi-ai/api/openai-completions.lazy";
import { z } from "zod";

import type { LlamaCppSettings } from "../auth/local.ts";
import type { Fetch } from "../auth/types.ts";
import { createModelCatalog, NO_CATALOG_CREDENTIALS } from "./catalog.ts";

const ModelId = z.string().min(1).max(512).regex(/^[^\s\0]+$/);
const Catalog = z.object({ data: z.array(z.object({ id: ModelId })).max(4096) });
const CATALOG_MAX_BYTES = 1_048_576;

export class LocalModelUnavailable extends Error {
  readonly code = "local_model_unavailable";
  constructor(reason: string) {
    super(`local_model_unavailable: ${reason}`);
  }
}

async function discover(settings: LlamaCppSettings, send: Fetch): Promise<readonly string[]> {
  const response = await send(`${new URL(settings.endpoint).origin}/v1/models`, {
    method: "GET", redirect: "error", signal: AbortSignal.timeout(settings.timeout_ms),
  });
  if (!response.ok) throw new LocalModelUnavailable(`catalog returned HTTP ${response.status}`);
  const reader = response.body?.getReader();
  if (!reader) throw new LocalModelUnavailable("catalog has no body");
  const chunks: Uint8Array[] = [];
  let bytes = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      bytes += value.byteLength;
      if (bytes > CATALOG_MAX_BYTES) throw new LocalModelUnavailable("catalog exceeds 1048576 bytes");
      chunks.push(value);
    }
  } finally {
    await reader.cancel();
    reader.releaseLock();
  }
  const parsed = Catalog.safeParse(JSON.parse(Buffer.concat(chunks).toString("utf8")));
  if (!parsed.success) throw new LocalModelUnavailable("catalog must contain data entries with model IDs");
  return [...new Set(parsed.data.data.map(({ id }) => id))];
}

/** A generation-local, chat-only catalog. No upstream llama extension or model manager is enabled. */
export async function createLocalModels(settings: LlamaCppSettings, selected: string, send: Fetch) {
  const selectedId = selected.startsWith("llama.cpp/") ? selected.slice("llama.cpp/".length) : selected;
  if (!ModelId.safeParse(selectedId).success) throw new LocalModelUnavailable("invalid configured model ID");
  const runtime = await createModelCatalog(NO_CATALOG_CREDENTIALS, { providers: [], modelsPath: null });
  const origin = new URL(settings.endpoint).origin;
  // Discovery is refreshed explicitly; an individual generation keeps its configured transport.
  let ids: readonly string[] = settings.discover ? await discover(settings, send) : [selectedId];
  const metadata = (id: string): Model<"openai-completions"> => ({
    id, name: id, provider: "llama.cpp", api: "openai-completions", baseUrl: `${origin}/v1`,
    reasoning: false, input: ["text"], contextWindow: settings.context_window, maxTokens: settings.max_tokens,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    compat: {
      supportsStore: false, supportsDeveloperRole: false, supportsReasoningEffort: false,
      supportsUsageInStreaming: true, supportsStrictMode: false, maxTokensField: "max_tokens",
    },
  });
  const api = openAICompletionsApi();
  const bound = (model: Model<Api>) => {
    if (model.provider !== "llama.cpp" || model.api !== "openai-completions" || !ids.includes(model.id)) {
      throw new LocalModelUnavailable("model is not in the configured local chat catalog");
    }
    return metadata(model.id);
  };
  const provider: Provider = {
    id: "llama.cpp", name: "llama.cpp", baseUrl: `${origin}/v1`,
    auth: { apiKey: { name: "configured local inference", resolve: async () => ({
      auth: { apiKey: "local", baseUrl: `${origin}/v1` }, source: "generation configuration",
    }) } },
    getModels: () => ids.map(metadata),
    stream: (model, context, options) => api.stream(bound(model), context, {
      ...options, apiKey: "local", headers: {}, fetch: send, timeoutMs: settings.timeout_ms, maxRetries: 0,
      maxTokens: Math.min(options?.maxTokens ?? settings.max_tokens, settings.max_tokens),
    }),
    streamSimple: (model, context, options) => api.streamSimple(bound(model), context, {
      ...options, apiKey: "local", headers: {}, fetch: send, timeoutMs: settings.timeout_ms, maxRetries: 0,
      maxTokens: Math.min(options?.maxTokens ?? settings.max_tokens, settings.max_tokens),
    }),
  };
  runtime.registerNativeProvider(provider);
  const refresh = runtime.refresh.bind(runtime);
  runtime.refresh = async (options = {}) => {
    if (settings.discover && options.allowNetwork === true) ids = await discover(settings, send);
    return refresh({ ...options, providers: ["llama.cpp"], allowNetwork: false });
  };
  await runtime.refresh();
  const model = runtime.getModel("llama.cpp", selectedId);
  if (!model) throw new LocalModelUnavailable(`configured model ${selectedId} is not advertised; check the endpoint or set discover = false`);
  return { models: runtime, model };
}
