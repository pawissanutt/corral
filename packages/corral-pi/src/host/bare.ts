// `corral pi --bare`: plain unbound Pi, exec'd by corral with argv only and no launch message.
// It loads the asset root's pinned Pi SDK and only the auth module, guards the network exactly as
// the bound host does, and opens InteractiveMode in --cwd with sessions under the corral-owned
// data root (`$XDG_DATA_HOME/corral/pi-native/bare/`). There is no store, bootstrap, generation,
// nonce, bridge socket, hello, role or delivery module here, and no corral_* tool.
import { mkdirSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import { fileURLToPath } from "node:url";

import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { AgentSessionRuntime, CreateAgentSessionRuntimeFactory } from "@earendil-works/pi-coding-agent";

import { piNativeRoot } from "../auth/paths.ts";
import { PROVIDERS, type ProviderId, isProviderId } from "../auth/providers.ts";
import { UPSTREAM_TOOLS } from "../roles/types.ts";
import { loadAuth } from "./modules.ts";
import { type GuardedFetch, guardedFetch, resourceLoaderOptions, restrictEnvironment, restrictedSettings } from "./restrict.ts";
import { compiledBundle } from "./bundle.ts";
import { checkCompiledRuntime, checkRuntime, readCompat } from "./runtime.ts";
import { restrictProvider } from "./session.ts";
import { providerCapabilities, providerTransport, type ProviderAvailability } from "./capabilities.ts";
import { createModelCatalog } from "./catalog.ts";

/** Pi 1.3.0's own default model of each provider (`defaultModelPerProvider`, not exported). */
export const BARE_DEFAULT_MODEL: Readonly<Record<Exclude<ProviderId, "llama.cpp">, string>> = {
  "openai-codex": "gpt-6.1-sol",
  anthropic: "claude-opus-4-8",
  "amazon-bedrock": "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
};

const THINKING = new Set(["off", "minimal", "low", "medium", "high", "xhigh"]);
const FLAGS = new Set(["--cwd", "--provider", "--model", "--effort"]);

export interface BareOptions {
  readonly assets: string;
  readonly cwd: string;
  readonly provider: ProviderId;
  readonly model: string;
  readonly effort?: string;
  /** The corral-owned pi-native data root; sessions and Pi's agent directory live below it. */
  readonly dataRoot: string;
}

export class BareArgsInvalid extends Error {
  readonly code = "usage_error";
  constructor(reason: string) {
    super(`usage_error: ${reason}; run by \`corral pi --bare\``);
  }
}

/** The asset root this entry was installed in. */
export function bareAssetRoot(): string {
  return fileURLToPath(new URL("../..", import.meta.url)).replace(/\/$/, "");
}

/** Parses `--cwd <dir> --provider <id> [--model M] [--effort E]`; anything else is refused. */
export function parseBareArgs(argv: readonly string[], assets: string, env: NodeJS.ProcessEnv = process.env): BareOptions {
  const flags = new Map<string, string>();
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index] ?? "";
    const value = argv[index + 1];
    if (!FLAGS.has(flag) || value === undefined || value.length === 0 || flags.has(flag)) {
      throw new BareArgsInvalid(`unexpected argument ${flag}`);
    }
    flags.set(flag, value);
  }
  const cwd = flags.get("--cwd");
  if (cwd === undefined || !isAbsolute(cwd)) throw new BareArgsInvalid("--cwd must be an absolute directory");
  const provider = flags.get("--provider") ?? "openai-codex";
  if (!isProviderId(provider)) throw new BareArgsInvalid(`unknown provider ${provider}`);
  if (provider === "llama.cpp") {
    throw new BareArgsInvalid("llama.cpp requires a bound role's configured llama_cpp capability");
  }
  const effort = flags.get("--effort");
  return {
    assets, cwd, provider, model: flags.get("--model") ?? BARE_DEFAULT_MODEL[provider],
    ...(effort ? { effort } : {}),
    dataRoot: piNativeRoot(env),
  };
}

export interface BareSession {
  readonly runtime: AgentSessionRuntime;
  readonly sessionDir: string;
  readonly network: GuardedFetch;
  readonly providers: readonly ProviderAvailability[];
}

/**
 * Installs the guarded fetch, then loads the SDK and opens one unbound runtime with Pi's own
 * tools only. `loadSdk` runs after the guard, as the bound host imports the SDK after it.
 */
export async function openBare(options: BareOptions, loadSdk: () => Promise<typeof Sdk>): Promise<BareSession> {
  const auth = await loadAuth(options.assets, options.provider);
  // Bare Pi runs only subscription OAuth providers: it carries no Amazon Bedrock binding.
  const policy = (auth.providers ?? [auth.provider]).filter((provider) => PROVIDERS[provider].auth === "oauth");
  const capabilities = await providerCapabilities(auth, options.provider, policy);
  const send = providerTransport(auth, capabilities.providers, globalThis.fetch.bind(globalThis));
  const network = guardedFetch(send, undefined, capabilities.providers);
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  const sdk = await loadSdk();
  const root = join(options.dataRoot, "bare");
  const sessionDir = join(root, "sessions");
  const agentDir = join(root, "agent");
  for (const dir of [sessionDir, agentDir]) mkdirSync(dir, { recursive: true, mode: 0o700 });
  const route = PROVIDERS[options.provider];
  const models = await createModelCatalog(capabilities.credentials, {
    providers: capabilities.providers, modelsPath: join(options.dataRoot, "models.json"),
  });
  const modelId = options.model.startsWith(`${route.id}/`) ? options.model.slice(route.id.length + 1) : options.model;
  const model = models.getModel(route.id, modelId);
  if (!model) throw new Error(`model_unavailable: ${route.id}/${modelId}`);
  // The same model lanes as the bound host: Anthropic runs through the Claude Agent SDK lane (T-493).
  for (const provider of capabilities.providers) {
    restrictProvider(models, provider, network.fetch as typeof globalThis.fetch,
      { configDir: join(agentDir, "claude-code"), cwd: options.cwd });
  }
  const thinkingLevel = options.effort && THINKING.has(options.effort)
    ? (options.effort as "off" | "minimal" | "low" | "medium" | "high" | "xhigh") : undefined;
  const tools = [...UPSTREAM_TOOLS];
  const factory: CreateAgentSessionRuntimeFactory = async ({ cwd, agentDir, sessionManager, sessionStartEvent }) => {
    const services = await sdk.createAgentSessionServices({
      cwd, agentDir, modelRuntime: models, settingsManager: sdk.SettingsManager.inMemory(restrictedSettings()),
      resourceLoaderOptions: resourceLoaderOptions([]),
    });
    const result = await sdk.createAgentSessionFromServices({
      services, sessionManager, model, tools,
      ...(sessionStartEvent ? { sessionStartEvent } : {}),
      ...(thinkingLevel ? { thinkingLevel } : {}),
    });
    return { ...result, services, diagnostics: services.diagnostics };
  };
  const runtime = await sdk.createAgentSessionRuntime(factory, {
    cwd: options.cwd, agentDir, sessionManager: sdk.SessionManager.create(options.cwd, sessionDir),
  });
  return { runtime, sessionDir, network, providers: capabilities.availability };
}

/** Runs bare Pi from `argv` on `assets` (this source tree, or a compiled host's directory). */
export async function runBare(argv: readonly string[], assets: string): Promise<void> {
  const options = parseBareArgs(argv, assets);
  restrictEnvironment(process.env);
  const compat = readCompat(options.assets);
  if (compiledBundle()) checkCompiledRuntime(compat);
  else checkRuntime(options.assets, compat);
  process.chdir(options.cwd);
  const loaded: { sdk?: typeof Sdk } = {};
  const bare = await openBare(options, async () => (loaded.sdk = await import("@earendil-works/pi-coding-agent")));
  const sdk = loaded.sdk;
  if (!sdk) throw new Error("the Pi SDK did not load");
  const terminal = new sdk.InteractiveMode(bare.runtime, { verbose: false });
  await terminal.init();
  for (const provider of bare.providers) {
    if (!provider.available) terminal.showWarning(provider.reason ?? `${provider.provider} is unavailable`);
  }
  await terminal.run();
}

/** `runBare`, reporting a refusal on stderr with exit code 2. */
export async function bareCommand(argv: readonly string[], assets: string): Promise<void> {
  try {
    await runBare(argv, assets);
  } catch (error: unknown) {
    const message = error instanceof Error ? error.message : String(error);
    process.stderr.write(`corral pi --bare stopped: ${message}\n`);
    process.exit(2);
  }
}

if (import.meta.main) await bareCommand(process.argv.slice(2), bareAssetRoot());
