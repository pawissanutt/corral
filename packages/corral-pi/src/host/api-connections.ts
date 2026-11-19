import type { Api, CredentialStore, Model, StreamFunction } from "@earendil-works/pi-ai";
import type { ModelRuntime } from "@earendil-works/pi-coding-agent";
import { getSupportedThinkingLevels } from "@earendil-works/pi-ai/models";
import { lazyStream } from "@earendil-works/pi-ai/api/lazy";
import { CONNECTIONS, type ApiConnectionId, connectionRules, validApiKey } from "../auth/connections.ts";
import { PROVIDERS } from "../auth/providers.ts";
import type { Fetch } from "../auth/types.ts";
import { SPENDING_CAP_REACHED, type Spending } from "../capabilities/budget.ts";
import type { ModelUsage } from "../tools/corral.ts";
import { createModelCatalog } from "./catalog.ts";
import { observeUsage } from "./model-usage.ts";
import { guardedFetch } from "./restrict.ts";
import { normalizeFailure, withProviderFailures } from "./provider-failure.ts";

/** A named API route has its own credentials and transport and shares the host's spending cap, when it has one. */
export interface ApiCatalog {
  readonly models: ModelRuntime;
  readonly model: Model<Api>;
}

export async function createApiCatalog(route: {
  connection: ApiConnectionId; model: string; effort?: string; credentials: CredentialStore; send: Fetch;
  spending: Spending | undefined; report(usage: ModelUsage): void;
}): Promise<ApiCatalog> {
  const { connection, credentials, spending } = route;
  const providerId = CONNECTIONS[connection].provider;
  const models = await createModelCatalog(credentials, { providers: [providerId], apiConnection: connection, modelsPath: null });
  const provider = models.getProvider(providerId);
  const id = route.model.startsWith(`${providerId}/`) ? route.model.slice(providerId.length + 1) : route.model;
  const model = models.getModel(providerId, id);
  if (!provider || !model || model.api !== PROVIDERS[providerId].modelApi) {
    throw new Error(`model_unavailable: ${providerId}/${id}`);
  }
  const effort = getSupportedThinkingLevels(model).find(level => level === (route.effort ?? "off"));
  if (!effort) throw new Error(`model_unavailable: unsupported effort for ${providerId}/${id}`);
  const key = async () => {
    const credential = await credentials.read(providerId);
    if (credential?.type !== "api_key" || typeof credential.key !== "string" || !validApiKey(credential.key)) {
      throw new Error(`credentials_unavailable: run corral-pi login --connection ${connection}`);
    }
    return credential.key;
  };
  const guarded = guardedFetch(route.send, undefined, providerId, connectionRules(connection));
  const native = withProviderFailures(connection, provider.streamSimple.bind(provider));
  const stream: StreamFunction = (_current, context, options) => lazyStream(model, async () => {
    const apiKey = await key();
    let refusal: string | undefined;
    const fetch: Fetch = async (input, init) => {
      try { spending?.beforeRequest(model); }
      catch (error) {
        // Preserve a local refusal even if the SDK replaces the fetch error with connection prose.
        refusal = error instanceof Error ? error.message : SPENDING_CAP_REACHED;
        throw error;
      }
      return guarded.fetch(input, init);
    };
    const pinnedOptions = {
      ...options, apiKey, headers: {}, env: {}, fetch: fetch as typeof globalThis.fetch, maxRetries: 0,
      reasoning: effort, maxTokens: Math.min(options?.maxTokens ?? model.maxTokens, model.maxTokens),
    };
    const source = native(model, context, pinnedOptions);
    return (async function* () {
      for await (const event of source) {
        if (event.type !== "error") { yield event; continue; }
        // Some provider messages echo the request auth.
        const error: typeof event.error = JSON.parse(JSON.stringify(event.error).replaceAll(apiKey, "<redacted>"));
        if (refusal) {
          error.errorMessage = refusal;
          Object.assign(error, { providerFailure: normalizeFailure(connection, { message: refusal }) });
        }
        yield { ...event, error };
      }
    })();
  });
  const metered = observeUsage(stream, "api", route.report, spending, true);
  // Do not expose the OAuth/ancillary clients from the general provider catalog.
  models.registerNativeProvider({
    id: providerId, name: `${provider.name} API`,
    auth: { apiKey: { name: "corral API connection", resolve: async () => ({ auth: { apiKey: await key() }, source: connection }) } },
    getModels: () => [model], getAllModels: () => [model], stream: metered, streamSimple: metered,
  });
  await models.refresh({ allowNetwork: false });
  return { models, model };
}
