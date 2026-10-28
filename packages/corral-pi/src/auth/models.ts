// Harness detection (`corral harness list`, the settings model picker and routing checks) runs
// this entry point with the providers whose corral-owned login `corral agent doctor` sees:
// `bun --no-env-file src/auth/models.ts --provider openai-codex [--provider anthropic]`, or
// `corral-pi-host models --provider ...` for a compiled installation. It prints
// Pi's built-in catalog for those providers as `{"models":[{"selector","provider","id"}]}`, where
// `selector` is the provider-qualified ID a profile names. It never reads a credential, refreshes
// a token or reaches the network.
import type { CredentialStore } from "@earendil-works/pi-ai";

import { PROVIDERS, type ProviderId, isProviderId } from "./providers.ts";

/** Listing the built-in catalog needs no credential, so the store holds none and refuses writes. */
const NO_CREDENTIALS: CredentialStore = {
  read: async () => undefined,
  list: async () => [],
  modify: async () => {
    throw new Error("model listing never writes a credential");
  },
  delete: async () => {
    throw new Error("model listing never removes a credential");
  },
};

export interface ListedModel {
  readonly selector: string;
  readonly provider: ProviderId;
  readonly id: string;
}

/** Pi's built-in models of each provider that use that provider's model API, in catalog order. */
export async function listModels(providers: readonly ProviderId[]): Promise<ListedModel[]> {
  const { ModelRuntime } = await import("@earendil-works/pi-coding-agent");
  const models = await ModelRuntime.create({
    credentials: NO_CREDENTIALS, modelsPath: null, refreshOnCreate: false, allowModelNetwork: false,
  });
  return providers.flatMap((provider) => models.getModels(provider)
    .filter((model) => model.api === PROVIDERS[provider].modelApi)
    .map((model) => ({ selector: `${provider}/${model.id}`, provider, id: model.id })));
}

export function runModels(argv: readonly string[]): void {
  process.env.PI_OFFLINE = "1";
  process.env.PI_TELEMETRY = "0";
  process.env.PI_SKIP_VERSION_CHECK = "1";
  globalThis.fetch = (async () => {
    throw new Error("network_refused: model listing is offline");
  }) as unknown as typeof globalThis.fetch;
  const providers = argv.flatMap((arg, index) => argv[index - 1] === "--provider" ? [arg] : []);
  const unknown = providers.find((provider) => !isProviderId(provider));
  if (providers.length === 0 || unknown !== undefined) {
    process.stderr.write(`corral agent models failed: ${unknown === undefined ? "no --provider given"
      : `unknown provider ${unknown}`}; use openai-codex or anthropic\n`);
    process.exit(2);
  }
  listModels(providers as ProviderId[]).then((models) => {
    process.stdout.write(`${JSON.stringify({ models })}\n`);
    process.exit(0);
  }, (error: unknown) => {
    process.stderr.write(`corral agent models failed: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(1);
  });
}

if (import.meta.main) runModels(process.argv);
