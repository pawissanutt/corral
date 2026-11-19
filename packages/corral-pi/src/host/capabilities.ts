// Provider availability is session policy intersected with corral-owned credentials.
// The initial model is not a capability boundary.
import type { CredentialStore } from "@earendil-works/pi-ai";
import { CredentialsUnavailable } from "../auth/paths.ts";
import { HOST_PROVIDER_IDS, PROVIDERS, type ProviderId, isProviderId, validApiKey } from "../auth/providers.ts";
import type { AuthModule, Fetch } from "../auth/types.ts";
import { NO_CATALOG_CREDENTIALS } from "./catalog.ts";

export function interactiveModels(role: string): boolean {
  return role === "assistant" || role === "build";
}

export function sessionRoutes(auth: AuthModule, primary: ProviderId, role: string, additional: readonly ProviderId[] = []) {
  const supported = auth.providers ?? [auth.provider];
  const policy = new Set([primary, ...additional.filter(id => supported.includes(id))]);
  if (interactiveModels(role)) for (const id of HOST_PROVIDER_IDS) {
    if ((PROVIDERS[id].auth === "oauth" || PROVIDERS[id].auth === "api_key") && supported.includes(id)) {
      policy.add(id);
    }
  }
  return providerCapabilities(auth, primary, [...policy]);
}

export interface ProviderAvailability {
  readonly provider: ProviderId;
  readonly available: boolean;
  readonly reason?: string;
}

export interface SessionRoutes {
  readonly providers: readonly ProviderId[];
  readonly credentials: CredentialStore;
  readonly availability: readonly ProviderAvailability[];
}

export async function providerCapabilities(
  auth: AuthModule, initial: ProviderId, policy: readonly ProviderId[],
): Promise<SessionRoutes> {
  const stores = new Map<ProviderId, CredentialStore>();
  const availability: ProviderAvailability[] = [];
  for (const provider of policy) {
    try {
      const kind = PROVIDERS[provider].auth;
      const store = kind === "local" || kind === "aws-profile" ? NO_CATALOG_CREDENTIALS : auth.credentials({
        provider, apiEndpoint: PROVIDERS[provider].api, refreshEndpoint: PROVIDERS[provider].refresh,
      });
      if (kind === "oauth" || kind === "api_key") {
        const credential = await store.read(provider);
        if (credential?.type !== kind || (credential.type === "api_key" && !validApiKey(credential.key))) {
          throw new CredentialsUnavailable(`no stored credential for ${provider}; run corral-pi login --provider ${provider}`);
        }
      }
      stores.set(provider, store);
      availability.push({ provider, available: true });
    } catch (error) {
      if (!(error instanceof CredentialsUnavailable) || provider === initial) throw error;
      availability.push({ provider, available: false, reason: error.message });
    }
  }
  if (!stores.has(initial)) throw new CredentialsUnavailable(`${initial} is outside this session's provider policy`);
  const storeFor = (provider: string) => isProviderId(provider) ? stores.get(provider) : undefined;
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
    return (transport?.send ?? fallback)(input, init);
  };
}
