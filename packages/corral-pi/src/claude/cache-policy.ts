import type { SimpleStreamOptions } from "@earendil-works/pi-ai";

export type PromptCacheTtl = "5m" | "1h";
export type ClaudeCacheRetention = PromptCacheTtl | "none";
export const DEFAULT_CACHE_TTL: PromptCacheTtl = "5m";

/** Per-call policy (including Pi's no-cache summaries) wins over the host's explicit default. */
export function resolveCacheRetention(
  configured: unknown, request: SimpleStreamOptions["cacheRetention"],
): ClaudeCacheRetention {
  if (request === "none") return "none";
  if (request === "short") return "5m";
  if (request === "long") return "1h";
  if (configured === undefined) return DEFAULT_CACHE_TTL;
  if (configured === "none" || configured === "5m" || configured === "1h") return configured;
  throw new Error("claude_cache_retention_invalid: CORRAL_CLAUDE_CACHE_TTL must be 5m, 1h or none");
}

export function cacheLifetimeMs(ttl: PromptCacheTtl): number {
  return ttl === "1h" ? 3_600_000 : 300_000;
}
