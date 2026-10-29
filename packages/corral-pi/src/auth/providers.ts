// The providers pi-native can run (OD5, T-479, T-504): each one's corral vendor, picker label, Pi
// model API, auth kind, and the only destinations its host and its login may reach. The Rust side
// (crates/corral-ops/src/agent/credentials.rs, PROVIDERS) lists the same ids and labels.
// OpenAI Codex and Anthropic run on a subscription OAuth login; Amazon Bedrock is the one metered
// vendor (OD-1), signed with a named AWS profile's credentials (OD-2).

export type ProviderId = "openai-codex" | "anthropic" | "amazon-bedrock";

/** How a provider authenticates: a corral-owned subscription OAuth login, or a named AWS profile. */
export type AuthKind = "oauth" | "aws-profile";

export interface ProviderRoute {
  readonly id: ProviderId;
  readonly vendor: "openai" | "anthropic" | "amazon-bedrock";
  readonly brand: string;
  readonly label: string;
  /** The Pi model API every request of this provider must use. */
  readonly modelApi: string;
  readonly auth: AuthKind;
  /**
   * The model endpoint, the only one a host run may POST to besides the token refresh. For an
   * AWS-profile route it is a template; the bound region and model make the concrete rule.
   */
  readonly api: string;
  /** Exact query strings Pi's client appends to the model endpoint. */
  readonly apiQueries: readonly string[];
  /** The OAuth token refresh endpoint; empty for an AWS-profile route, which has none. */
  readonly refresh: string;
  /**
   * A marker every subscription OAuth access token of this provider carries; requests whose key
   * lacks it would be paid API traffic on the same endpoint and are refused.
   */
  readonly oauthTokenMarker?: string;
  /** The OAuth endpoints a login may POST to; an AWS-profile route logs in from Rust, offline. */
  readonly login: readonly string[];
  /**
   * Set when model calls run in a subprocess instead of through the host's fetch (T-493): the
   * Claude Agent SDK lane spawns Claude Code, so the host itself never POSTs to `api`.
   */
  readonly lane?: "claude-agent-sdk";
}

export const PROVIDERS: Readonly<Record<ProviderId, ProviderRoute>> = {
  "openai-codex": {
    id: "openai-codex",
    vendor: "openai",
    brand: "OpenAI",
    label: "OpenAI Codex (ChatGPT Plus/Pro)",
    modelApi: "openai-codex-responses",
    auth: "oauth",
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
    auth: "oauth",
    api: "https://api.anthropic.com/v1/messages",
    // Pi sends subscription OAuth requests through the SDK's beta messages client.
    apiQueries: ["?beta=true"],
    refresh: "https://platform.claude.com/v1/oauth/token",
    oauthTokenMarker: "sk-ant-oat",
    login: ["https://platform.claude.com/v1/oauth/token"],
    lane: "claude-agent-sdk",
  },
  "amazon-bedrock": {
    id: "amazon-bedrock",
    vendor: "amazon-bedrock",
    brand: "Amazon Bedrock",
    label: "Amazon Bedrock (AWS profile, metered)",
    modelApi: "bedrock-converse-stream",
    auth: "aws-profile",
    api: "https://bedrock-runtime.<region>.amazonaws.com/model/<model>/converse-stream",
    apiQueries: [],
    refresh: "",
    login: [],
  },
};

/** The subscription OAuth providers (OD5), the policy a session's provider capabilities draw from. */
export const PROVIDER_IDS: readonly ProviderId[] = ["openai-codex", "anthropic"];

/** Every provider a host can be bound to: the subscription providers and metered Amazon Bedrock. */
export const HOST_PROVIDER_IDS: readonly ProviderId[] = [...PROVIDER_IDS, "amazon-bedrock"];

export function isProviderId(value: unknown): value is ProviderId {
  return typeof value === "string" && (HOST_PROVIDER_IDS as readonly string[]).includes(value);
}

/** One allowed destination: a method and a matcher over the raw URL and the request headers. */
export interface DestinationRule {
  readonly method: "GET" | "POST";
  readonly display: string;
  matches(url: string, headers: Headers): boolean;
}

/** The Bedrock binding and the profile facts the guard needs; never a credential. */
export interface BedrockRoute {
  readonly region: string;
  readonly model: string;
  /** The profile chain has a role_arn, so STS AssumeRole is needed. */
  readonly assumesRole: boolean;
  readonly ssoRegions: readonly string[];
}

/** An AWS commercial-partition region; GovCloud, China and ISO partitions fail closed. */
export const AWS_REGION = /^[a-z]{2}(-[a-z]+)+-[0-9]+$/;
const OTHER_PARTITION = /^(us-gov|cn|us-iso[a-z]*|eu-iso[a-z]*)-/;

export function isAwsRegion(value: string): boolean {
  return AWS_REGION.test(value) && !OTHER_PARTITION.test(value);
}

function exact(url: string): DestinationRule {
  return { method: "POST", display: url, matches: (candidate) => candidate === url };
}

function parsed(url: string): URL | undefined {
  const value = URL.parse(url);
  if (!value || value.protocol !== "https:" || value.port !== "" || value.username !== "" || value.password !== "" ||
      value.hash !== "") return undefined;
  return value;
}

function converseStream(region: string, model: string): DestinationRule {
  const host = `bedrock-runtime.${region}.amazonaws.com`;
  return {
    method: "POST",
    display: `https://${host}/model/${encodeURIComponent(model)}/converse-stream`,
    matches(url, headers) {
      const target = parsed(url);
      if (!target || target.hostname !== host || target.search !== "") return false;
      const segment = /^\/model\/([^/]+)\/converse-stream$/.exec(target.pathname)?.[1];
      let id: string;
      try {
        id = segment === undefined ? "" : decodeURIComponent(segment);
      } catch {
        return false;
      }
      // Only a SigV4-signed request may reach the model: a Bearer header is a Bedrock API key.
      return id === model && /^AWS4-HMAC-SHA256 /.test(headers.get("authorization") ?? "");
    },
  };
}

function hostOnly(method: "GET" | "POST", host: string, path: string, query: boolean): DestinationRule {
  return {
    method,
    display: `https://${host}${path}`,
    matches(url) {
      const target = parsed(url);
      return target !== undefined && target.hostname === host && target.pathname === path && (query || target.search === "");
    },
  };
}

/**
 * The rules a host run of `provider` may reach. OAuth routes keep their exact POST URLs: the model
 * endpoint and token refresh, or only the token refresh when a subprocess lane owns the model
 * traffic. Bedrock allows the bound region's ConverseStream URL for the bound model, STS only when
 * the profile assumes a role, and the SSO portal only for an SSO profile's region; never the SSO
 * OIDC token endpoint. Without a Bedrock route nothing is allowed.
 */
export function hostRules(provider: ProviderId, bedrock?: BedrockRoute): readonly DestinationRule[] {
  const route = PROVIDERS[provider];
  if (route.auth === "aws-profile") {
    if (!bedrock || !isAwsRegion(bedrock.region) || bedrock.ssoRegions.some((region) => !isAwsRegion(region))) return [];
    return [
      converseStream(bedrock.region, bedrock.model),
      ...(bedrock.assumesRole ? [hostOnly("POST", "sts.amazonaws.com", "/", false),
        hostOnly("POST", `sts.${bedrock.region}.amazonaws.com`, "/", false)] : []),
      ...[...new Set(bedrock.ssoRegions)].map((region) =>
        hostOnly("GET", `portal.sso.${region}.amazonaws.com`, "/federation/credentials", true)),
    ];
  }
  const refresh = route.refresh === "" ? [] : [route.refresh];
  if (route.lane !== undefined) return refresh.map(exact);
  return [route.api, ...route.apiQueries.map((query) => `${route.api}${query}`), ...refresh].map(exact);
}

/** The destinations a host run of `provider` may reach, as get_state shows them. */
export function hostDestinations(provider: ProviderId, bedrock?: BedrockRoute): readonly string[] {
  return hostRules(provider, bedrock).map((rule) => rule.display);
}
