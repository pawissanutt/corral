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
import { checkRuntime, readCompat } from "./runtime.ts";
import { authContext, restrictProvider } from "./session.ts";

/** Pi 1.3.0's own default model of each provider (`defaultModelPerProvider`, not exported). */
export const BARE_DEFAULT_MODEL: Readonly<Record<ProviderId, string>> = {
  "openai-codex": "gpt-6.1-sol",
  anthropic: "claude-opus-4-8",
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
}

/**
 * Installs the guarded fetch, then loads the SDK and opens one unbound runtime with Pi's own
 * tools only. `loadSdk` runs after the guard, as the bound host imports the SDK after it.
 */
export async function openBare(options: BareOptions, loadSdk: () => Promise<typeof Sdk>): Promise<BareSession> {
  const auth = await loadAuth(options.assets, options.provider);
  const context = authContext(options.provider);
  const send = auth.transport?.(context) ?? globalThis.fetch.bind(globalThis);
  const network = guardedFetch(send, undefined, options.provider);
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  const sdk = await loadSdk();
  const root = join(options.dataRoot, "bare");
  const sessionDir = join(root, "sessions");
  const agentDir = join(root, "agent");
  for (const dir of [sessionDir, agentDir]) mkdirSync(dir, { recursive: true, mode: 0o700 });
  const route = PROVIDERS[options.provider];
  const models = await sdk.ModelRuntime.create({
    credentials: auth.credentials(context), modelsPath: null, refreshOnCreate: false, allowModelNetwork: false,
  });
  const modelId = options.model.startsWith(`${route.id}/`) ? options.model.slice(route.id.length + 1) : options.model;
  const model = models.getModel(route.id, modelId);
  if (!model) throw new Error(`model_unavailable: ${route.id}/${modelId}`);
  restrictProvider(models, route.id, network.fetch as typeof globalThis.fetch);
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
      ...(thinkingLevel ? { thinkingLevel, scopedModels: [{ model, thinkingLevel }] } : { scopedModels: [{ model }] }),
    });
    return { ...result, services, diagnostics: services.diagnostics };
  };
  const runtime = await sdk.createAgentSessionRuntime(factory, {
    cwd: options.cwd, agentDir, sessionManager: sdk.SessionManager.create(options.cwd, sessionDir),
  });
  return { runtime, sessionDir, network };
}

async function main(): Promise<void> {
  const options = parseBareArgs(process.argv.slice(2), bareAssetRoot());
  restrictEnvironment(process.env);
  checkRuntime(options.assets, readCompat(options.assets));
  process.chdir(options.cwd);
  const loaded: { sdk?: typeof Sdk } = {};
  const bare = await openBare(options, async () => (loaded.sdk = await import("@earendil-works/pi-coding-agent")));
  const sdk = loaded.sdk;
  if (!sdk) throw new Error("the Pi SDK did not load");
  const terminal = new sdk.InteractiveMode(bare.runtime, { verbose: false });
  await terminal.init();
  await terminal.run();
}

if (import.meta.main) {
  main().catch((error: unknown) => {
    const message = error instanceof Error ? error.message : String(error);
    process.stderr.write(`corral pi --bare stopped: ${message}\n`);
    process.exit(2);
  });
}
