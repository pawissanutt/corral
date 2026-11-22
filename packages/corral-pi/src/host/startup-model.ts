// New owner sessions select a model before touching any session state. Listing is offline;
// the real host alone resolves/refreshes credentials and installs the guarded provider streams.
import { homedir } from "node:os";
import type { AwsBinding } from "../auth/aws.ts";
import type { LlamaCppSettings } from "../auth/local.ts";
import { listModels, type ListedModel } from "../auth/models.ts";
import { authDir } from "../auth/paths.ts";
import { HOST_PROVIDER_IDS, PROVIDERS, isApiProvider, type ProviderId } from "../auth/providers.ts";
import { FileCredentialStore } from "../auth/store.ts";
import { checkLaunchFallback, qualifiedModel, type ModelChoice } from "./config.ts";

export interface StartupModel extends ListedModel {
  /** Provider display name, including the billing route. */
  readonly label: string;
  /** Can be saved as a default in this launch context without per-launch bindings. */
  readonly savable: boolean;
}

interface ListOptions {
  readonly provider?: ProviderId;
  readonly aws?: AwsBinding;
  readonly local?: LlamaCppSettings;
  readonly role?: "assistant" | "build";
  readonly modelsPath?: string;
}

async function hasBedrockProfile(aws: AwsBinding): Promise<boolean> {
  try {
    const { createAwsCredentialResolver } = await import("../auth/aws.ts");
    await createAwsCredentialResolver(aws, homedir()).profile(); // no STS, SSO calls or refresh
    return true;
  } catch { return false; }
}

/** Without a launch role, list only savable choices, in the same catalog used by the launcher. */
export async function listStartupModels(options: ListOptions = {}): Promise<StartupModel[]> {
  const providers: ProviderId[] = [];
  for (const provider of HOST_PROVIDER_IDS) {
    if (options.provider !== undefined && provider !== options.provider) continue;
    const { auth } = PROVIDERS[provider];
    if (auth === "local") {
      if (options.role === "assistant" && options.local) providers.push(provider);
    } else if (auth === "aws-profile") {
      if (options.role && options.aws && await hasBedrockProfile(options.aws)) providers.push(provider);
    } else if (new FileCredentialStore(authDir(), provider).present()) providers.push(provider);
  }
  if (!providers.length) return [];
  const modelsPath = providers.every(isApiProvider) ? null : options.modelsPath;
  const models = await listModels(providers, { modelsPath });
  const seen = new Set<string>();
  return models.flatMap(model => {
    if (!qualifiedModel(model.selector) || seen.has(model.selector)) return [];
    seen.add(model.selector);
    const savable = model.provider !== "amazon-bedrock" && model.provider !== "llama.cpp" && Buffer.byteLength(model.selector) <= 256;
    return options.role || savable ? [{ ...model, label: PROVIDERS[model.provider].label, savable }] : [];
  });
}

interface StartupOptions extends ListOptions {
  readonly role: "assistant" | "build";
  readonly model?: string;
  readonly fallback?: readonly ModelChoice[];
  readonly headless: boolean;
  readonly workspace?: { readonly root: string; readonly corral: string; readonly model: string | null };
}

export class StartupModelRequired extends Error {
  constructor() {
    super("startup_model_required: No startup model is available for a new headless session. Pass --model <provider/model>.");
  }
}

export async function resolveStartupModel(options: StartupOptions): Promise<ModelChoice | undefined> {
  if (options.model !== undefined && options.provider) {
    if (options.provider === "amazon-bedrock" && !options.aws) {
      throw new Error("Amazon Bedrock requires explicit --aws-profile and --aws-region bindings");
    }
    if (options.provider === "llama.cpp" && !options.local && options.role === "assistant") {
      throw new Error("llama.cpp requires the workspace's explicit [llama_cpp] configuration");
    }
    return { provider: options.provider, model: options.model };
  }
  // A provider-only flag explicitly requests selection within that provider, not a saved model.
  if (options.headless && options.provider) throw new StartupModelRequired();
  const saved = options.provider ? null : options.workspace ? options.workspace.model
    : (await import("./preferences.ts")).readPersonalPreferences().startupModel;
  const savedChoice = saved ? qualifiedModel(saved) : undefined;
  if (options.headless && !savedChoice) throw new StartupModelRequired();
  // A bound local route has no reliable offline model advertisement. A manually configured
  // selector is intentional; actual endpoint/model validation remains in its guarded host.
  const localDefault = savedChoice?.provider === "llama.cpp" && options.role === "assistant" && options.local;
  const available = localDefault ? [] : await listStartupModels({ ...options,
    provider: options.headless ? savedChoice?.provider : options.provider });
  if (savedChoice && (localDefault || available.some(model => model.selector === saved))) {
    checkLaunchFallback(saved!, options.fallback);
    return { provider: savedChoice.provider, model: saved! };
  }
  if (options.headless) throw new StartupModelRequired();
  const candidates = available.filter(choice => !options.fallback?.some(entry =>
    `${entry.provider}/${entry.model}` === choice.selector));
  if (!candidates.length) {
    const fix = options.provider === "amazon-bedrock"
      ? "Pass --model <amazon-bedrock/id> with a usable --aws-profile and --aws-region."
      : options.provider === "llama.cpp" ? "Configure [llama_cpp] and pass --model <llama.cpp/id>."
      : `Sign in with \`corral-pi login${options.provider ? ` --provider ${options.provider}` : " --provider <id>"}\`, or pass --model <provider/model>.`;
    throw new Error(`No startup models are available${options.provider ? ` for ${options.provider}` : ""}. ${fix}`);
  }

  let save: ((selector: string) => void | Promise<unknown>) | undefined;
  const workspace = options.workspace;
  if (workspace && candidates.some(model => model.savable)) {
    const { readWorkspaceStartup, saveWorkspaceStartup } = await import("./startup-preference.ts");
    try {
      const baseline = await readWorkspaceStartup(workspace.root, workspace.corral);
      if (baseline.model === workspace.model) {
        save = selector => saveWorkspaceStartup(workspace.root, workspace.corral, baseline, selector);
      }
    } catch { /* An unavailable writer must not prevent a once-only choice. */ }
  } else if (!workspace) {
    const { savePersonalPreference } = await import("./preferences.ts");
    save = selector => savePersonalPreference("startupModel", selector);
  }
  const { pickStartupModel } = await import("./startup-picker.ts");
  const title = savedChoice && saved ? `Saved startup model ${saved} is unavailable. Choose a startup model.` : "Choose a startup model";
  const picked = await pickStartupModel(candidates, title, save ? workspace ? "workspace" : "personal" : undefined);
  if (!picked) return undefined;
  if (picked.save) await save?.(picked.model.selector);
  return { provider: picked.model.provider, model: picked.model.selector };
}
