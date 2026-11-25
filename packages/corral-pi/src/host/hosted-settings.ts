import type { ProviderId } from "../auth/providers.ts";

type SearchProvider = Extract<ProviderId, "openai" | "openai-codex" | "anthropic" | "anthropic-api">;
export type ImageProvider = Extract<SearchProvider, "openai" | "openai-codex">;
export interface HostedRoute<P extends SearchProvider = SearchProvider> { readonly provider: P; readonly model: string }
export interface HostedSettings {
  readonly web_search?: "off" | HostedRoute;
  readonly image_gen?: "off" | HostedRoute<ImageProvider>;
}

/** Mirror pi_hosted_tools::valid_selector. Never interpolate an invalid selector in an error. */
export function parseHostedTools(raw: unknown): HostedSettings | undefined {
  if (raw === undefined) return undefined;
  if (!raw || typeof raw !== "object" || Array.isArray(raw) || Object.keys(raw).some(k => k !== "web_search" && k !== "image_gen")) {
    throw new Error(".corral-next/pi.toml: invalid [hosted_tools] table");
  }
  const settings: { web_search?: HostedSettings["web_search"]; image_gen?: HostedSettings["image_gen"] } = {};
  for (const [key, value] of Object.entries(raw)) {
    const error = () => new Error(`.corral-next/pi.toml: invalid hosted_tools.${key} selector`);
    if (key !== "web_search" && key !== "image_gen") throw error();
    if (value === "off") { settings[key] = value; continue; }
    if (typeof value !== "string" || value.length > 256 || /[^a-zA-Z0-9/._-]/.test(value)) throw error();
    const [provider, model, extra] = value.split("/");
    if (!model || extra !== undefined || !/^[A-Za-z0-9]/.test(model) || /^(sk-|sess-|eyj)/i.test(model)) throw error();
    if (provider === "openai" || provider === "openai-codex") settings[key] = { provider, model };
    else if (key === "web_search" && (provider === "anthropic" || provider === "anthropic-api")) settings[key] = { provider, model };
    else throw error();
  }
  return settings;
}
