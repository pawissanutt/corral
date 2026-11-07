// Guarded direct Pi clients. Only trusted host code may install the future financial adapter;
// no bootstrap field, credential, runtime override or model output is an admission receipt.
import type { CredentialStore, Model, StreamFunction } from "@earendil-works/pi-ai";
import { getSupportedThinkingLevels } from "@earendil-works/pi-ai/models";
import { lazyStream } from "@earendil-works/pi-ai/api/lazy";
import { CONNECTIONS, type ApiConnectionId, connectionRules, isApiConnection, requirePaidAdmission, validApiKey } from "../auth/connections.ts";
import { PROVIDERS } from "../auth/providers.ts";
import type { Fetch } from "../auth/types.ts";
import { createModelCatalog } from "./catalog.ts";
import { type ConnectionBinding, parseBinding } from "./connections.ts";
import { guardedFetch } from "./restrict.ts";

/** Invoked at EACH physical POST, including a pinned client's internal compatibility retry. */
export type ApiAdmission = (request: {
  readonly binding: ConnectionBinding;
  readonly model: Model<import("@earendil-works/pi-ai").Api>;
  readonly context: Parameters<StreamFunction>[1];
  readonly options: Parameters<StreamFunction>[2];
}) => Promise<void>;

export async function createApiCatalog(
  credentials: CredentialStore, binding: ConnectionBinding, send: Fetch,
  admit?: ApiAdmission,
  modelsPath: string | null = null,
) {
  if (!isApiConnection(binding.connection)) throw new Error("connection_invalid: not an API connection");
  const connection: ApiConnectionId = binding.connection;
  const providerId = CONNECTIONS[connection].provider;
  parseBinding(binding, providerId, binding.model, binding.effort ?? undefined);
  const route = PROVIDERS[providerId];
  const models = await createModelCatalog(credentials, { providers: [providerId], apiConnection: connection, modelsPath });
  const provider = models.getProvider(providerId);
  const id = binding.model.startsWith(`${providerId}/`) ? binding.model.slice(providerId.length + 1) : binding.model;
  const model = models.getModel(providerId, id);
  if (!provider || !model || model.api !== route.modelApi) throw new Error("model_unavailable: exact approved API model is not in the catalog");
  const baseUrl = connection === "anthropic-api" ? "https://api.anthropic.com" : "https://api.openai.com/v1";
  if (model.baseUrl !== baseUrl || Object.keys(model.headers ?? {}).length !== 0) throw new Error("connection_invalid: API model endpoint/header override refused");
  if (binding.effort !== null && !getSupportedThinkingLevels(model).includes(binding.effort as never)) {
    throw new Error("model_unavailable: exact approved effort is unsupported by the API model");
  }
  const key = async () => {
    const credential = await credentials.read(providerId);
    if (credential?.type !== "api_key" || typeof credential.key !== "string" || !validApiKey(credential.key)) throw new Error("credentials_unavailable: API connection requires its stored literal API key");
    return credential.key;
  };
  const guarded = guardedFetch(send, undefined, providerId, connectionRules(connection));
  const wrap = (stream: StreamFunction): StreamFunction => (current, context, options) => lazyStream(current, async () => {
    if (current.provider !== providerId || current.id !== model.id || current.api !== route.modelApi
        || current.baseUrl !== model.baseUrl || JSON.stringify(current.headers ?? {}) !== JSON.stringify(model.headers ?? {})) {
      throw new Error("connection_invalid: model override changed the bound route");
    }
    if (!admit) requirePaidAdmission(connection);
    const apiKey = await key();
    // Auth is pinned, not merely prefix checked: remove OAuth, env, headers and base-URL overrides.
    const fetch: Fetch = async (input, init) => {
      if (!admit) requirePaidAdmission(connection);
      else await admit({ binding, model, context, options });
      const headers = new Headers(init?.headers ?? (input instanceof Request ? input.headers : undefined));
      const actual = connection === "anthropic-api" ? headers.get("x-api-key") : headers.get("authorization");
      if (actual !== (connection === "anthropic-api" ? apiKey : `Bearer ${apiKey}`)) {
        throw new Error("credentials_unavailable: request auth does not match selected connection");
      }
      return guarded.fetch(input, init);
    };
    const pinnedOptions = {
      ...options, apiKey, headers: {}, env: {}, fetch: fetch as typeof globalThis.fetch, maxRetries: 0,
      reasoning: (binding.effort ?? "off") as never,
    };
    return stream({ ...model, headers: {} }, context, pinnedOptions);
  });
  // Minimal registration deliberately drops OpenAI's ChatGPT OAuth AND classifier/image clients.
  models.registerNativeProvider({
    id: providerId, name: `${provider.name} API`,
    auth: { apiKey: { name: "corral API connection", resolve: async () => ({ auth: { apiKey: await key() }, source: connection }) } },
    getModels: () => [model], getAllModels: () => [model],
    stream: wrap(provider.streamSimple.bind(provider)), streamSimple: wrap(provider.streamSimple.bind(provider)),
  });
  await models.refresh({ allowNetwork: false });
  return { models, model };
}
