// Ambient resources stay disabled. Global fetch retains provider-only endpoints; optional
// web tools use a separate public-fetch/configured-search transport with the same ledger.
import { type DestinationRule, PROVIDERS, type ProviderId, hostDestinations, hostRules } from "../auth/providers.ts";
import type { Fetch } from "../auth/types.ts";
import { createWebAccess, type WebAccess } from "../extensions/web/client.ts";
import type { Lookup } from "../extensions/web/policy.ts";
import type { WebSettings } from "../extensions/web/settings.ts";

export const PROVIDER_API = PROVIDERS["openai-codex"].api;
export const TOKEN_REFRESH = PROVIDERS["openai-codex"].refresh;
export const ALLOWED_DESTINATIONS: readonly string[] = hostDestinations("openai-codex");

/** Every built-in upstream extension (pi-coding-agent 1.3.0 `builtInExtensions`). */
export const BUILTIN_EXTENSIONS: readonly string[] = ["llama.cpp", "codemode", "tool-search", "mcp"];

/** Upstream reads these before any network feature starts; set before the SDK loads. */
export function restrictEnvironment(env: NodeJS.ProcessEnv, provider?: ProviderId): void {
  env.PI_OFFLINE = "1";
  env.PI_SKIP_VERSION_CHECK = "1";
  env.PI_TELEMETRY = "0";
  for (const key of ["PI_EXPERIMENTAL", "PI_SHARE_VIEWER_URL", "PI_INSTALLER_API_BASE", "PI_PACKAGE_DIR",
    "PI_MANAGED_INSTALL_ROOT", "PI_CODING_AGENT_DIR"]) {
    delete env[key];
  }
  // Ambient Anthropic auth (API key, auth/OAuth tokens, workload federation, base URL) would be
  // paid or foreign-store access; pi-native uses only the corral-owned subscription login.
  for (const key of Object.keys(env)) if (key.startsWith("ANTHROPIC_")) delete env[key];
  // A Bedrock host signs only with its named profile's credentials: ambient AWS keys, profiles,
  // config-file paths and Bedrock API keys never reach it or any subprocess it starts.
  if (provider === "amazon-bedrock") for (const key of Object.keys(env)) if (key.startsWith("AWS_")) delete env[key];
}

export function restrictedSettings() {
  return {
    cacheWarming: "off" as const,
    enableInstallTelemetry: false,
    enableAnalytics: false,
    enableSkillCommands: false,
    quietStartup: true,
    transport: "sse" as const,
    extensions: [] as string[],
  };
}

export function resourceLoaderOptions<F>(extensionFactories: F[]) {
  return {
    noExtensions: true,
    noSkills: true,
    noPromptTemplates: true,
    noThemes: true,
    noContextFiles: true,
    disabledBuiltinExtensions: [...BUILTIN_EXTENSIONS],
    extensionFactories,
  };
}

export interface NetworkLedger {
  readonly allowed: number;
  readonly refused: number;
  readonly refusedDestinations: readonly string[];
}

export class NetworkRefused extends Error {
  readonly code = "network_refused";
  constructor(readonly destination: string) {
    super(`network_refused: ${destination} is not a pinned provider endpoint`);
  }
}

function destination(input: string | URL | Request): { url: string; method?: string; headers?: Headers } {
  if (typeof input === "string") return { url: input };
  if (input instanceof URL) return { url: input.href };
  return { url: input.url, method: input.method, headers: input.headers };
}

/** Where a refused destination is reported: origin and path only, never a query or body. */
function redact(url: string): string {
  try {
    const parsed = new URL(url);
    return `${parsed.origin}${parsed.pathname}`;
  } catch {
    return "<unparseable>";
  }
}

export interface GuardedFetch {
  readonly fetch: Fetch;
  /** Purpose-specific GET tools; never grants provider fetch the search-origin exception. */
  readonly web?: WebAccess;
  /** MCP transport only; it cannot borrow provider credentials or public-fetch policy. */
  readonly mcp?: Fetch;
  ledger(): NetworkLedger;
  /** The allowed destinations get_state shows; absent means the provider's default rules. */
  readonly destinations?: readonly string[];
  /** Records a refusal made outside fetch (the socket tripwire) and returns the error to throw. */
  refuse?(destination: string): NetworkRefused;
}

export interface WebNetworkOptions {
  readonly settings: WebSettings;
  readonly send: Fetch;
  readonly lookup?: Lookup;
}

export interface McpNetworkOptions {
  readonly origins: readonly string[];
  readonly send: Fetch;
}

/**
 * A fetch that sends only requests matching `provider`'s rules (by default its pinned POST
 * endpoints), through `send`, without redirects. Every other destination, method or
 * authorization is refused before anything is sent.
 */
export function guardedFetch(
  send: Fetch, onRefused?: (destination: string) => void,
  provider: ProviderId | readonly ProviderId[] = "openai-codex",
  rules: readonly DestinationRule[] = (typeof provider === "string" ? [provider] : provider).flatMap((id) => hostRules(id)),
  web?: WebNetworkOptions,
  mcp?: McpNetworkOptions,
): GuardedFetch {
  let allowed = 0;
  let refused = 0;
  const refusedDestinations: string[] = [];
  const refuse = (where: string): NetworkRefused => {
    refused++;
    if (refusedDestinations.length < 32 && !refusedDestinations.includes(where)) refusedDestinations.push(where);
    onRefused?.(where);
    return new NetworkRefused(where);
  };
  const fetch: Fetch = async (input, init) => {
    const target = destination(input);
    const method = (init?.method ?? target.method ?? "GET").toUpperCase();
    const headers = new Headers(init?.headers ?? target.headers);
    if (!rules.some((rule) => rule.method === method && rule.matches(target.url, headers))) throw refuse(redact(target.url));
    allowed++;
    return send(input, { ...init, redirect: "error" });
  };
  return {
    fetch, refuse, destinations: rules.map((rule) => rule.display),
    ...(mcp ? { mcp: async (input: Parameters<Fetch>[0], init?: Parameters<Fetch>[1]) => {
      const target = destination(input);
      const url = new URL(target.url);
      const method = (init?.method ?? target.method ?? "GET").toUpperCase();
      const headers = new Headers(init?.headers ?? target.headers);
      if (!["http:", "https:"].includes(url.protocol) || url.username || url.password || url.hash ||
          !mcp.origins.includes(url.origin) || !["GET", "POST", "DELETE"].includes(method) ||
          ["authorization", "cookie", "proxy-authorization", "x-api-key"].some((key) => headers.has(key))) {
        throw refuse(redact(target.url));
      }
      allowed++;
      return mcp.send(input, { ...init, redirect: "error" });
    } } : {}),
    ...(web ? { web: createWebAccess(web.settings, web.send, {
      ...(web.lookup ? { lookup: web.lookup } : {}),
      allowed: () => { allowed++; },
      refused: (where) => { refuse(where); },
    }) } : {}),
    ledger: () => ({ allowed, refused, refusedDestinations: [...refusedDestinations] }),
  };
}
