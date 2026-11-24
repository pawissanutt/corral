// Harness detection (`corral harness list`, the settings model picker and routing checks) runs
// this entry point with the providers whose corral-owned login `corral agent doctor` sees:
// `bun --no-env-file src/auth/models.ts --provider openai-codex [--provider anthropic]
// [--provider amazon-bedrock]`, or
// `corral-pi-host models --provider ...` for a compiled installation. It prints
// Pi's configured catalog for those providers as `{"models":[{"selector","provider","id"}]}`, where
// `selector` is the provider-qualified ID. Pi's runnable catalog is offline. Only an
// explicit CLI request for antigravity-cli delegates account discovery to agy; those
// rows are not available to the host runtime, its pickers or profile resolution.
import { parseArgs } from "node:util";
import { AGY_PROVIDER, type AgyListedModel, discoverAgyModels } from "../agy/account.ts";
import { type CatalogPolicy, createModelCatalog, NO_CATALOG_CREDENTIALS } from "../host/catalog.ts";
import { HOST_PROVIDER_IDS, PROVIDERS, type ProviderId, isProviderId } from "./providers.ts";

export interface ListedModel {
  readonly selector: string;
  readonly provider: ProviderId;
  readonly id: string;
}

/**
 * Pi's configured models using each approved provider's model API, in catalog order.
 * GovCloud Bedrock IDs (`us-gov.`) are left out: the host only reaches the aws partition.
 */
export async function listModels(
  providers: readonly ProviderId[], policy: Omit<CatalogPolicy, "providers"> = {},
): Promise<ListedModel[]> {
  const models = await createModelCatalog(NO_CATALOG_CREDENTIALS, { ...policy, providers });
  return providers.flatMap((provider) => models.getModels(provider)
    .filter((model) => model.api === PROVIDERS[provider].modelApi && !model.id.startsWith("us-gov."))
    .map((model) => ({ selector: `${provider}/${model.id}`, provider, id: model.id })));
}

export function runModels(argv: readonly string[]): void {
  process.env.PI_OFFLINE = "1";
  process.env.PI_TELEMETRY = "0";
  process.env.PI_SKIP_VERSION_CHECK = "1";
  globalThis.fetch = (async () => {
    throw new Error("network_refused: model listing is offline");
  }) as unknown as typeof globalThis.fetch;
  let providers: string[];
  try {
    providers = parseArgs({ args: [...argv], options: { provider: { type: "string", multiple: true } } }).values.provider ?? [];
  } catch {
    process.stderr.write("Usage: corral-pi models --provider <id> [--provider <id>]\n");
    process.exit(2);
  }
  const unknown = providers.find((provider) => !isProviderId(provider) && provider !== AGY_PROVIDER);
  if (providers.length === 0 || unknown !== undefined) {
    process.stderr.write(`corral agent models failed: ${unknown === undefined ? "no --provider given"
      : `unknown provider ${unknown}`}; use ${[...HOST_PROVIDER_IDS, AGY_PROVIDER].join(", ")}\n`);
    process.exit(2);
  }
  const list = async () => {
    if (!providers.includes(AGY_PROVIDER)) return listModels(providers.filter(isProviderId));
    const runnable = providers.filter(isProviderId);
    const configured = runnable.length ? await listModels([...new Set(runnable)]) : [];
    const agy = discoverAgyModels();
    return providers.flatMap<ListedModel | AgyListedModel>(provider =>
      provider === AGY_PROVIDER ? agy : configured.filter(model => model.provider === provider));
  };
  list().then((models) => {
    process.stdout.write(`${JSON.stringify({ models })}\n`);
    process.exit(0);
  }, (error: unknown) => {
    process.stderr.write(`corral agent models failed: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(1);
  });
}

if (import.meta.main) runModels(process.argv.slice(2));
