// Provider availability is session policy intersected with corral-owned credentials.
// The initial model is not a capability boundary.
import type { CredentialStore } from "@earendil-works/pi-ai";
import { CredentialsUnavailable } from "../auth/paths.ts";
import { PROVIDERS, type ProviderId } from "../auth/providers.ts";
import type { AuthModule, Fetch } from "../auth/types.ts";

export interface ProviderAvailability {
  readonly provider: ProviderId;
  readonly available: boolean;
  readonly reason?: string;
}

export async function providerCapabilities(
  auth: AuthModule, initial: ProviderId, policy: readonly ProviderId[],
) {
  const stores = new Map<ProviderId, CredentialStore>();
  const availability: ProviderAvailability[] = [];
  for (const provider of policy) {
    try {
      const store = auth.credentials({
        provider, apiEndpoint: PROVIDERS[provider].api, refreshEndpoint: PROVIDERS[provider].refresh,
      });
      const credential = await store.read(provider);
      if (credential?.type !== "oauth") throw new CredentialsUnavailable(`no subscription login for ${provider}`);
      stores.set(provider, store);
      availability.push({ provider, available: true });
    } catch (error) {
      if (!(error instanceof CredentialsUnavailable) || provider === initial) throw error;
      availability.push({ provider, available: false, reason: error.message });
    }
  }
  if (!stores.has(initial)) throw new CredentialsUnavailable(`${initial} is outside this session's provider policy`);
  const storeFor = (provider: string) => {
    for (const [id, store] of stores) if (id === provider) return store;
    return undefined;
  };
  const credentials: CredentialStore = {
    async read(provider, options) { return storeFor(provider)?.read(provider, options); },
    async list(options) {
      return (await Promise.all([...stores].map(async ([provider, store]) =>
        (await store.list(options)).filter((entry) => entry.providerId === provider)))).flat();
    },
    async modify(provider, change, options) {
      const store = storeFor(provider);
      if (!store) throw new CredentialsUnavailable(`${provider} is unavailable in this session`);
      return store.modify(provider, change, options);
    },
    async delete(provider, options) { await storeFor(provider)?.delete(provider, options); },
  };
  return { providers: [...stores.keys()], credentials, availability };
}

/** Dispatch through each adapter's own transport; the caller still applies the endpoint guard. */
export function providerTransport(auth: AuthModule, providers: readonly ProviderId[], fallback: Fetch): Fetch {
  const transports = providers.map((provider) => ({
    route: PROVIDERS[provider],
    send: auth.transport?.({
      provider, apiEndpoint: PROVIDERS[provider].api, refreshEndpoint: PROVIDERS[provider].refresh,
    }) ?? fallback,
  }));
  return (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const transport = transports.find(({ route }) => url === route.refresh || url === route.api ||
      route.apiQueries.some((query) => url === `${route.api}${query}`));
    if (!transport) throw new CredentialsUnavailable("no session provider owns this destination");
    return transport.send(input, init);
  };
}
