import { join } from "node:path";

import type { CredentialStore, Provider } from "@earendil-works/pi-ai";
import { anthropicProvider } from "@earendil-works/pi-ai/providers/anthropic";
import { openaiProvider } from "@earendil-works/pi-ai/providers/openai";
import { openaiCodexProvider } from "@earendil-works/pi-ai/providers/openai-codex";
import type { ModelRuntime } from "@earendil-works/pi-coding-agent";

import { corralPiRoot } from "../auth/paths.ts";
import { PROVIDERS, type ApiProviderId, type ProviderId, isApiProvider, isProviderId, validApiKey } from "../auth/providers.ts";

export function nativeApiProvider(id: ApiProviderId): Provider {
  return id === "anthropic-api" ? anthropicProvider() : openaiProvider();
}

export interface CatalogPolicy {
  /** Only these providers can expose models or resolve credentials. */
  readonly providers: readonly ProviderId[];
  /** Omit for corral's corral-pi/models.json; null disables that file. */
  readonly modelsPath?: string | null;
}

export class ModelCatalogInvalid extends Error {
  readonly code = "model_catalog_invalid";
  constructor(reason: string) {
    super(`model_catalog_invalid: ${reason}`);
  }
}

/** Metadata loading must never read credentials, evaluate key commands, or refresh tokens. */
export const NO_CATALOG_CREDENTIALS: CredentialStore = {
  read: async () => undefined,
  list: async () => [],
  modify: async () => { throw new Error("catalog metadata cannot write credentials"); },
  delete: async () => { throw new Error("catalog metadata cannot delete credentials"); },
};

/**
 * Shared launch/listing policy. models.json is parsed/composed by Pi, but only model
 * metadata crosses into the execution runtime. Its auth, headers and dispatch remain
 * native, so config cannot restore paid-key auth or replace a guarded stream.
 *
 * Refresh reloads configuration offline, including when Pi's /model asks for network.
 * Callers still install restrictProvider on each approved provider before streaming.
 */
export async function createModelCatalog(
  credentials: CredentialStore, policy: CatalogPolicy,
): Promise<ModelRuntime> {
  const { ModelRuntime } = await import("@earendil-works/pi-coding-agent");
  const { InMemoryModelsStore } = await import("@earendil-works/pi-ai");
  const modelsPath = policy.modelsPath === undefined ? join(corralPiRoot(), "models.json") : policy.modelsPath;
  const approved = new Set(policy.providers);
  const matchesCredential = (id: ProviderId, entry: { type: string; key?: string }) => isApiProvider(id)
    ? entry.type === "api_key" && typeof entry.key === "string" && validApiKey(entry.key) : entry.type === "oauth";
  const scopedCredentials: CredentialStore = {
    read: async (id, options) => {
      if (!isProviderId(id) || !approved.has(id)) return undefined;
      const credential = await credentials.read(id, options);
      return credential && matchesCredential(id, credential) ? credential : undefined;
    },
    list: async (options) => (await credentials.list(options))
      .filter((entry) => isProviderId(entry.providerId) && approved.has(entry.providerId) && entry.type === PROVIDERS[entry.providerId].auth),
    modify: async (id, fn, options) => {
      if (!isProviderId(id) || !approved.has(id)) throw new Error(`provider_not_allowed: ${id}`);
      return credentials.modify(id, fn, options);
    },
    delete: async (id, options) => {
      if (!isProviderId(id) || !approved.has(id)) throw new Error(`provider_not_allowed: ${id}`);
      await credentials.delete(id, options);
    },
  };
  const loadMetadata = async () => {
    const loaded = await ModelRuntime.create({
      credentials: NO_CATALOG_CREDENTIALS,
      modelsPath,
      modelsStore: new InMemoryModelsStore(),
      refreshOnCreate: false,
      allowModelNetwork: false,
    });
    // Keep the owner-selected Astra subscription window; explicit models.json overrides win.
    const provider = openaiCodexProvider();
    const getModels = () => provider.getModels().map(model =>
      model.id === "gpt-6-astra" || model.id === "gpt-6.2-astra" ? { ...model, contextWindow: 600000 } : model);
    loaded.registerNativeProvider({ ...provider, getModels, getAllModels: getModels });
    const error = loaded.getError();
    if (error !== undefined) throw new ModelCatalogInvalid(error);
    // Native registration starts an unawaited SDK refresh. It may reload this file after
    // invalidation, so retain validated model values, not that mutable runtime's catalog.
    const models = new Map(policy.providers.map(id => {
      // A configured URL, header or price cannot override the native paid route.
      if (isApiProvider(id)) {
        const listed = nativeApiProvider(id).getModels().map(model => ({ ...model, provider: id }));
        return [id, listed] as const;
      }
      return [id, [...loaded.getModels(id)]] as const;
    }));
    return { runtime: loaded, models };
  };
  let metadata = await loadMetadata();
  const runtime = await ModelRuntime.create({
    credentials: scopedCredentials, modelsPath: null, refreshOnCreate: false, allowModelNetwork: false,
    modelsStore: new InMemoryModelsStore(),
  });
  // Native registrations survive SDK refresh/rebuildProviders. Do not re-register them
  // on refresh: the caller may have replaced their streams with guarded lane adapters.
  const refused = (id: string) => () => { throw new Error(`provider_not_allowed: ${id}`); };
  for (const provider of runtime.getProviders()) {
    const id = provider.id;
    if (isProviderId(id) && approved.has(id) && !isApiProvider(id)) {
      const getModels = () => (metadata.models.get(id) ?? []).filter(model => model.api === PROVIDERS[id].modelApi);
      runtime.registerNativeProvider({
        ...provider, auth: provider.auth.oauth ? { oauth: provider.auth.oauth } : {},
        getModels, getAllModels: getModels, refreshModels: undefined,
      });
    } else {
      const blocked = refused(id);
      runtime.registerNativeProvider({ id, name: provider.name, auth: {}, getModels: () => [], stream: blocked, streamSimple: blocked });
    }
  }
  for (const id of approved) {
    if (!isApiProvider(id)) continue;
    const blocked = refused(id);
    const getModels = () => metadata.models.get(id) ?? [];
    runtime.registerNativeProvider({ id, name: PROVIDERS[id].label, auth: {}, getModels, getAllModels: getModels,
      stream: blocked, streamSimple: blocked });
  }
  const refresh = runtime.refresh.bind(runtime);
  runtime.refresh = async (options = {}) => {
    metadata = await loadMetadata();
    return refresh({
      ...options, allowNetwork: false,
      providers: options.providers?.filter((id) => isProviderId(id) && approved.has(id)) ?? [...approved],
    });
  };
  const getError = runtime.getError.bind(runtime);
  runtime.getError = () => metadata.runtime.getError() ?? getError();
  await runtime.refresh();
  return runtime;
}
