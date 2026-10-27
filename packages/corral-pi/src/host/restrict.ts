// Restricted resources: no MCP, inherited skills, prompt templates, context files, themes,
// telemetry, bug-report upload, cache warmer or remote catalog, and a guarded fetch that
// reaches only the bound provider's model API and that provider's token-refresh endpoint.
import { PROVIDERS, type ProviderId, hostDestinations } from "../auth/providers.ts";
import type { Fetch } from "../auth/types.ts";

export const PROVIDER_API = PROVIDERS["openai-codex"].api;
export const TOKEN_REFRESH = PROVIDERS["openai-codex"].refresh;
export const ALLOWED_DESTINATIONS: readonly string[] = hostDestinations("openai-codex");

/** Every built-in upstream extension (pi-coding-agent 1.3.0 `builtInExtensions`). */
export const BUILTIN_EXTENSIONS: readonly string[] = ["llama.cpp", "codemode", "tool-search", "mcp"];

/** Upstream reads these before any network feature starts; set before the SDK loads. */
export function restrictEnvironment(env: NodeJS.ProcessEnv): void {
  env.PI_OFFLINE = "1";
  env.PI_SKIP_VERSION_CHECK = "1";
  env.PI_TELEMETRY = "0";
  for (const key of ["PI_EXPERIMENTAL", "PI_SHARE_VIEWER_URL", "PI_INSTALLER_API_BASE", "PI_PACKAGE_DIR",
    "PI_MANAGED_INSTALL_ROOT", "PI_CODING_AGENT_DIR"]) {
    delete env[key];
  }
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

function destination(input: string | URL | Request): { url: string; method?: string } {
  if (typeof input === "string") return { url: input };
  if (input instanceof URL) return { url: input.href };
  return { url: input.url, method: input.method };
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
  ledger(): NetworkLedger;
}

/**
 * A fetch that sends only POSTs to `provider`'s pinned endpoints, through `send`, without
 * redirects. Every other provider's endpoints are refused like any other destination.
 */
export function guardedFetch(
  send: Fetch, onRefused?: (destination: string) => void, provider: ProviderId = "openai-codex",
): GuardedFetch {
  const destinations = hostDestinations(provider);
  let allowed = 0;
  let refused = 0;
  const refusedDestinations: string[] = [];
  const fetch: Fetch = async (input, init) => {
    const target = destination(input);
    const method = (init?.method ?? target.method ?? "GET").toUpperCase();
    if (method !== "POST" || !destinations.includes(target.url)) {
      refused++;
      const where = redact(target.url);
      if (refusedDestinations.length < 32 && !refusedDestinations.includes(where)) refusedDestinations.push(where);
      onRefused?.(where);
      throw new NetworkRefused(where);
    }
    allowed++;
    return send(input, { ...init, redirect: "error" });
  };
  return { fetch, ledger: () => ({ allowed, refused, refusedDestinations: [...refusedDestinations] }) };
}
