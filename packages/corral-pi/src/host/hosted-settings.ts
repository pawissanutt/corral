import type { ApiProviderId, ProviderId } from "../auth/providers.ts";

type SearchProvider = Extract<ProviderId, "openai" | "openai-codex" | "anthropic" | "anthropic-api">;
export type ImageProvider = Extract<SearchProvider, "openai" | "openai-codex">;
export type HostedTool = "web_search" | "image_gen";
export interface HostedRoute<P extends SearchProvider = SearchProvider> { readonly provider: P; readonly model: string }
export interface HostedSettings {
  readonly web_search?: "off" | HostedRoute;
  readonly image_gen?: "off" | HostedRoute<ImageProvider>;
}

export function hostedProvider(tool: HostedTool, provider: string): provider is SearchProvider {
  return provider === "openai" || provider === "openai-codex" ||
    (tool === "web_search" && (provider === "anthropic" || provider === "anthropic-api"));
}

/** Mirror the model portion of pi_hosted_tools::valid_selector. */
export function validHostedModelId(provider: SearchProvider, model: unknown): model is string {
  return typeof model === "string" && provider.length + 1 + model.length <= 256 &&
    /^[A-Za-z0-9]/.test(model) && !/[^A-Za-z0-9._-]/.test(model) && !/^(sk-|sess-|eyj)/i.test(model);
}

export function isGptImageModel(model: string): boolean {
  // The native Images path accepts GPT Image IDs, not DALL-E or a Responses carrier.
  return model.length <= 240 && /^gpt-image-[A-Za-z0-9]/.test(model) && validHostedModelId("openai", model);
}

const nonChatPrefix = /^(?:gpt-image|chatgpt-image|dall-e|tts|whisper|text|code|babbage|davinci|ada|curie|clip|(?:omni-)?moderation|sora|computer-use|embedding)(?:-|$)/i;
const nonChatVariant = /(?:^|-)(?:audio|realtime|transcribe|tts|instruct)(?:-|$)/i;

/** /models says nothing about tool support. Drop only recognizable non-chat OpenAI IDs. */
export function isApiSearchModel(provider: ApiProviderId, id: unknown): id is string {
  return validHostedModelId(provider, id) && (provider !== "openai" || !nonChatPrefix.test(id) && !nonChatVariant.test(id));
}

/** Never interpolate an invalid selector in an error. */
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
    if (typeof value !== "string" || value.length > 256) throw error();
    const [provider, model, extra] = value.split("/");
    if (!provider || extra !== undefined || !hostedProvider(key, provider) || !validHostedModelId(provider, model)) throw error();
    if (key === "web_search") settings[key] = { provider, model };
    else if (provider === "openai" || provider === "openai-codex") settings[key] = { provider, model };
    else throw error();
  }
  return settings;
}
