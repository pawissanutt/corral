// Local build provider defaults.
import { PROVIDERS, type ProviderId } from "../auth/providers.ts";
import type { AuthModule } from "../auth/types.ts";
import { providerCapabilities } from "./capabilities.ts";

/** Pi 1.3.0's own provider defaults, retained for bare sessions. */
export const BARE_DEFAULT_MODEL: Readonly<Record<Exclude<ProviderId, "llama.cpp" | "openai">, string>> = {
  "openai-codex": "gpt-6.1-sol",
  anthropic: "claude-opus-4-8",
  "amazon-bedrock": "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
};

export function requireBareProvider(provider: ProviderId): void {
  if (provider === "openai") throw new Error("API connections require bound canonical metered admission");
  if (provider === "llama.cpp") throw new Error("llama.cpp requires a bound role's configured llama_cpp capability");
}

/** The former plain-Pi policy, shared by network setup and the multi-provider catalog. */
export async function bareProviders(auth: AuthModule, initial: ProviderId) {
  requireBareProvider(initial);
  return providerCapabilities(auth, initial,
    (auth.providers ?? [auth.provider]).filter(provider => PROVIDERS[provider].auth === "oauth"));
}

