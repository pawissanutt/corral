// Corral connection identity is not an upstream Pi provider or a model-family alias.
import { type DestinationRule, type ProviderId, PROVIDERS, hostRules } from "./providers.ts";

export const CONNECTIONS = {
  "anthropic-subscription": { provider: "anthropic", vendor: "anthropic", billing: "subscription", auth: "oauth" },
  "anthropic-api": { provider: "anthropic", vendor: "anthropic", billing: "metered", auth: "api_key" },
  "openai-subscription": { provider: "openai-codex", vendor: "openai", billing: "subscription", auth: "oauth" },
  "openai-api": { provider: "openai", vendor: "openai", billing: "metered", auth: "api_key" },
  "amazon-bedrock": { provider: "amazon-bedrock", vendor: "amazon-bedrock", billing: "metered", auth: "aws-profile" },
  "llama.cpp": { provider: "llama.cpp", vendor: "llama.cpp", billing: "local", auth: "local" },
} as const;
export type ConnectionId = keyof typeof CONNECTIONS;
export type ApiConnectionId = "anthropic-api" | "openai-api";

export function isConnectionId(value: unknown): value is ConnectionId {
  return typeof value === "string" && Object.hasOwn(CONNECTIONS, value);
}

export function isApiConnection(id: ConnectionId): id is ApiConnectionId {
  return CONNECTIONS[id].auth === "api_key";
}

/** Legacy provider IDs keep their subscription meaning, even with ambient API keys. */
export function resolveConnection(provider: ProviderId, selected?: ConnectionId): ConnectionId {
  const id = selected ?? (provider === "anthropic" ? "anthropic-subscription"
    : provider === "openai-codex" ? "openai-subscription"
    : provider === "openai" ? undefined : provider);
  if (!id || !isConnectionId(id) || CONNECTIONS[id].provider !== provider) {
    throw new Error("connection_invalid: unknown or incompatible connection/provider binding");
  }
  return id;
}

export function connectionRules(id: ConnectionId): readonly DestinationRule[] {
  if (!isApiConnection(id)) return hostRules(CONNECTIONS[id].provider);
  const route = PROVIDERS[CONNECTIONS[id].provider];
  return [{
    method: "POST", display: route.api,
    matches(url, headers) {
      const key = id === "anthropic-api" ? headers.get("x-api-key") : headers.get("authorization")?.replace(/^Bearer /, "");
      const otherAuth = id === "anthropic-api" ? headers.has("authorization") : headers.has("x-api-key");
      return [route.api, ...(id === "anthropic-api" ? [`${route.api}?beta=true`] : [])].includes(url)
        && typeof key === "string" && validApiKey(key) && !otherAuth
        && !headers.has("cookie") && !headers.has("proxy-authorization")
        && !headers.get("anthropic-beta")?.includes("oauth");
    },
  }];
}

/** Literal key only: never Pi's command/env syntax or an OAuth token. No secret in errors. */
export function validApiKey(key: string): boolean {
  return /^sk-[A-Za-z0-9_-]{8,4093}$/.test(key) && !key.includes("sk-ant-oat") && !key.includes("sk-ant-ort");
}

/** T-894 deliberately cannot spend. The canonical paid-episode service is a later member. */
export function requirePaidAdmission(id: ConnectionId): void {
  if (isApiConnection(id)) throw new Error("paid_admission_required: API connections are disabled until canonical metered admission is installed");
}
