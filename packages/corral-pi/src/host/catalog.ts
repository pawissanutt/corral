import { join } from "node:path";

import type { CredentialStore, Provider } from "@earendil-works/pi-ai";
import type { ModelRuntime } from "@earendil-works/pi-coding-agent";

import { piNativeRoot } from "../auth/paths.ts";
import { PROVIDERS, type ProviderId, isProviderId } from "../auth/providers.ts";
import { SUPPLEMENTAL_MODELS } from "./supplemental-models.ts";

export interface CatalogPolicy {
  /** Only these subscription providers can expose models or resolve credentials. */
  readonly providers: readonly ProviderId[];
  /** Omit for corral's pi-native/models.json; null disables that file. */
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
  const modelsPath = policy.modelsPath === undefined ? join(piNativeRoot(), "models.json") : policy.modelsPath;
  const approved = new Set(policy.providers);
  const scopedCredentials: CredentialStore = {
    read: async (id, options) => {
      if (!isProviderId(id) || !approved.has(id)) return undefined;
      const credential = await credentials.read(id, options);
      return credential?.type === "oauth" ? credential : undefined;
    },
    list: async (options) => (await credentials.list(options))
      .filter((entry) => isProviderId(entry.providerId) && approved.has(entry.providerId) && entry.type === "oauth"),
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
    const error = loaded.getError();
    if (error !== undefined) throw new ModelCatalogInvalid(error);
    return loaded;
  };
  let metadata = await loadMetadata();
  const runtime = await ModelRuntime.create({
    credentials: scopedCredentials, modelsPath: null, refreshOnCreate: false, allowModelNetwork: false,
    modelsStore: new InMemoryModelsStore(),
  });
  // Native registrations survive SDK refresh/rebuildProviders. Do not re-register them
  // on refresh: the caller may have replaced their streams with guarded lane adapters.
  for (const provider of runtime.getProviders()) {
    if (isProviderId(provider.id) && approved.has(provider.id)) {
      const id = provider.id;
      const getModels = () => {
        const listed = metadata.getModels(id).filter((model) => model.api === PROVIDERS[id].modelApi);
        const extra = (SUPPLEMENTAL_MODELS[id] ?? [])
          .filter((model) => model.api === PROVIDERS[id].modelApi && !listed.some((entry) => entry.id === model.id));
        return [...listed, ...extra];
      };
      runtime.registerNativeProvider({
        ...provider,
        auth: provider.auth.oauth ? { oauth: provider.auth.oauth } : {},
        getModels,
        getAllModels: getModels,
        // The metadata runtime, not the native provider's remote catalog, owns this list.
        refreshModels: undefined,
      });
    } else {
      const refused = () => { throw new Error(`provider_not_allowed: ${provider.id}`); };
      const disabled: Provider = {
        id: provider.id, name: provider.name, auth: {}, getModels: () => [],
        stream: refused, streamSimple: refused,
      };
      runtime.registerNativeProvider(disabled);
    }
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
  runtime.getError = () => metadata.getError() ?? getError();
  await runtime.refresh();
  return runtime;
}
