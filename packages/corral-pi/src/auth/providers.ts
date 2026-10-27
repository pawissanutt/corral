// The subscription OAuth providers pi-native can run (OD5, T-479): each one's corral vendor,
// picker label, Pi model API, and the only endpoints its host and its login may reach. The Rust
// side (crates/corral-ops/src/agent/credentials.rs, PROVIDERS) lists the same ids and labels.

export type ProviderId = "openai-codex" | "anthropic";

export interface ProviderRoute {
  readonly id: ProviderId;
  readonly vendor: "openai" | "anthropic";
  readonly brand: string;
  readonly label: string;
  /** The Pi model API every request of this provider must use. */
  readonly modelApi: string;
  /** The model endpoint, the only one a host run may POST to besides the token refresh. */
  readonly api: string;
  /** Exact query strings Pi's client appends to the model endpoint. */
  readonly apiQueries: readonly string[];
  readonly refresh: string;
  /**
   * A marker every subscription OAuth access token of this provider carries; requests whose key
   * lacks it would be paid API traffic on the same endpoint and are refused.
   */
  readonly oauthTokenMarker?: string;
  /** The OAuth endpoints a login may POST to. */
  readonly login: readonly string[];
}

export const PROVIDERS: Readonly<Record<ProviderId, ProviderRoute>> = {
  "openai-codex": {
    id: "openai-codex",
    vendor: "openai",
    brand: "OpenAI",
    label: "OpenAI Codex (ChatGPT Plus/Pro)",
    modelApi: "openai-codex-responses",
    api: "https://chatgpt.com/backend-api/codex/responses",
    apiQueries: [],
    refresh: "https://auth.openai.com/oauth/token",
    login: [
      "https://auth.openai.com/oauth/token",
      "https://auth.openai.com/api/accounts/deviceauth/usercode",
      "https://auth.openai.com/api/accounts/deviceauth/token",
    ],
  },
  anthropic: {
    id: "anthropic",
    vendor: "anthropic",
    brand: "Anthropic",
    label: "Anthropic Claude (Pro/Max)",
    modelApi: "anthropic-messages",
    api: "https://api.anthropic.com/v1/messages",
    // Pi sends subscription OAuth requests through the SDK's beta messages client.
    apiQueries: ["?beta=true"],
    refresh: "https://platform.claude.com/v1/oauth/token",
    oauthTokenMarker: "sk-ant-oat",
    login: ["https://platform.claude.com/v1/oauth/token"],
  },
};

export const PROVIDER_IDS: readonly ProviderId[] = ["openai-codex", "anthropic"];

export function isProviderId(value: unknown): value is ProviderId {
  return typeof value === "string" && (PROVIDER_IDS as readonly string[]).includes(value);
}

/** The destinations a host run of `provider` may POST to: its model endpoint and token refresh. */
export function hostDestinations(provider: ProviderId): readonly string[] {
  const route = PROVIDERS[provider];
  return [route.api, ...route.apiQueries.map((query) => `${route.api}${query}`), route.refresh];
}
