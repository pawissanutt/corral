import type { Api, CredentialStore, Model, StreamFunction } from "@earendil-works/pi-ai";
import type { ModelRuntime } from "@earendil-works/pi-coding-agent";
import { lazyStream } from "@earendil-works/pi-ai/api/lazy";
import { getSupportedThinkingLevels } from "@earendil-works/pi-ai/models";
import { type ApiProviderId, PROVIDERS, validApiKey } from "../auth/providers.ts";
import type { Fetch } from "../auth/types.ts";
import type { ModelUsage } from "../tools/corral.ts";
import { nativeApiProvider } from "./catalog.ts";
import { observeUsage } from "./model-usage.ts";
import { guardedFetch, type GuardedFetch } from "./restrict.ts";
import { withProviderFailures } from "./provider-failure.ts";

export function assertApiEffort(model: Model<Api>, effort: string | undefined): void {
  if (effort !== undefined && !getSupportedThinkingLevels(model).some(level => level === effort)) {
    throw new Error(`model_unavailable: unsupported effort for ${model.provider}/${model.id}`);
  }
}

export function installApiProvider(models: ModelRuntime, id: ApiProviderId, credentials: CredentialStore,
  send: Fetch, report: (usage: ModelUsage) => void, refuse?: GuardedFetch["refuse"]): GuardedFetch {
  const provider = models.getProvider(id);
  if (!provider) throw new Error(`model_unavailable: ${id} is not in Pi's catalog`);
  const guarded = guardedFetch(send, refuse, id);
  const nativeProvider = nativeApiProvider(id);
  const native = withProviderFailures(id, nativeProvider.streamSimple.bind(nativeProvider));
  const stream: StreamFunction = (current, context, options) => lazyStream(current, async () => {
    const model = models.getModel(id, current.id);
    if (!model || current.provider !== id || current.api !== PROVIDERS[id].modelApi) {
      throw new Error(`model_unavailable: ${current.provider}/${current.id}`);
    }
    const credential = await credentials.read(id);
    if (credential?.type !== "api_key" || !validApiKey(credential.key)) {
      throw new Error(`credentials_unavailable: run corral-pi login --provider ${id}`);
    }
    const apiKey = credential.key;
    const source = native(model, context, {
      ...options, apiKey, headers: {}, env: {}, fetch: guarded.fetch as typeof globalThis.fetch, maxRetries: 0,
      maxTokens: Math.min(options?.maxTokens ?? model.maxTokens, model.maxTokens),
    });
    return (async function* () {
      for await (const event of source) {
        if (event.type !== "error") { yield event; continue; }
        // Some upstream errors echo the request auth.
        const error: typeof event.error = JSON.parse(JSON.stringify(event.error).replaceAll(apiKey, "<redacted>"));
        yield { ...event, error };
      }
    })();
  });
  const metered = observeUsage(stream, "api", report);
  models.registerNativeProvider({
    ...provider,
    auth: { apiKey: { name: PROVIDERS[id].label, resolve: async ({ credential }) => credential?.key && validApiKey(credential.key)
      ? { auth: { apiKey: credential.key }, source: "corral credential store" } : undefined } },
    stream: metered, streamSimple: metered,
  });
  return guarded;
}
