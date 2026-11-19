// One request rolls the quick conversation forward, without switching its conversational model.
import { join } from "node:path";
import { homedir } from "node:os";
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { CompactionResult, ModelRuntime, SessionBeforeCompactEvent } from "@earendil-works/pi-coding-agent";
import type { Api, Model, StreamFunction } from "@earendil-works/pi-ai";
import type { ThinkingLevel } from "@earendil-works/pi-agent-core";
import type { HostConfig } from "./config.ts";
import type { LoadedModules } from "./modules.ts";
import type { ModelUsage } from "../tools/corral.ts";
import { observeUsage } from "./model-usage.ts";
import { bedrockRoute, createAwsCredentialResolver, type AwsBinding } from "../auth/aws.ts";
import { type DestinationRule, type ProviderId, billingKind, hostRules, isApiProvider } from "../auth/providers.ts";
import type { Fetch } from "../auth/types.ts";
import { createModelCatalog, NO_CATALOG_CREDENTIALS } from "./catalog.ts";
import { assertApiEffort, installApiProvider } from "./api-providers.ts";
import { createLocalModels } from "./local-models.ts";
import { localInferenceRules } from "../auth/local.ts";
import { guardedFetch, type GuardedFetch } from "./restrict.ts";
import { authContext, restrictProvider, type AwsRoute, type HostSessionOptions } from "./session.ts";
import { adoptSmithyHandlers } from "./smithy.ts";

export interface SummaryProfile {
  readonly profile: string;
  readonly model: string;
  readonly provider: ProviderId;
  readonly effort?: string;
  readonly aws?: AwsBinding;
}
export interface SummaryOptions extends SummaryProfile {
  readonly network: GuardedFetch;
  readonly awsRoute?: AwsRoute;
  readonly rules: readonly DestinationRule[];
}
export interface SummaryRequest {
  readonly model: Model<Api>;
  readonly stream: StreamFunction;
  readonly effort?: string;
  readonly profile?: string;
}

/** Independent guarded transport: a different profile never inherits the session's credentials. */
export async function summaryOptions(config: HostConfig, modules: LoadedModules, send: Fetch): Promise<SummaryOptions | undefined> {
  const summary = config.summary;
  if (!summary) return undefined;
  let rules: readonly DestinationRule[] | undefined;
  let awsRoute: AwsRoute | undefined;
  if (summary.provider === "amazon-bedrock") {
    if (!summary.aws) throw new Error("credentials_unavailable: summary profile has no AWS binding");
    adoptSmithyHandlers();
    const resolver = createAwsCredentialResolver(summary.aws, homedir());
    const model = summary.model.replace(/^amazon-bedrock\//, "");
    rules = hostRules(summary.provider, bedrockRoute(summary.aws, model, await resolver.profile()));
    awsRoute = { binding: summary.aws, resolver };
  }
  if (summary.provider === "llama.cpp") {
    if (!config.capabilities?.llama_cpp) throw new Error("summary_profile_invalid: llama.cpp is not enabled");
    rules = localInferenceRules(config.capabilities.llama_cpp);
  }
  const transport = modules.auth.transport?.(authContext(summary.provider)) ?? send;
  const network = guardedFetch(transport, undefined, summary.provider, rules);
  return { ...summary, network, rules: rules ?? hostRules(summary.provider), ...(awsRoute ? { awsRoute } : {}) };
}

export async function createSummaryRequest(options: {
  readonly summary?: SummaryOptions;
  readonly modules: Pick<LoadedModules, "auth">;
  readonly capabilities?: HostSessionOptions["capabilities"];
  readonly agentDir: string;
  readonly cwd: string;
}, report: (usage: ModelUsage) => void = () => {}): Promise<SummaryRequest | undefined> {
  const summary = options.summary;
  if (!summary) return undefined;
  const { provider } = summary;
  const auth = options.modules.auth;
  if (!(auth.providers ?? [auth.provider]).includes(provider)) {
    throw new Error(`provider_not_allowed: summary profile ${provider} is unsupported by the auth module`);
  }
  const credentials = provider === "llama.cpp" ? NO_CATALOG_CREDENTIALS
    : auth.credentials(authContext(provider));
  const local = provider === "llama.cpp" && options.capabilities?.llama_cpp
    ? await createLocalModels(options.capabilities.llama_cpp, summary.model, summary.network.fetch) : undefined;
  const models = local?.models ?? await createModelCatalog(credentials, { providers: [provider],
    ...(isApiProvider(provider) ? { modelsPath: null } : {}) });
  const id = summary.model.startsWith(`${provider}/`) ? summary.model.slice(provider.length + 1) : summary.model;
  const model = local?.model ?? models.getModel(provider, id);
  if (!model) throw new Error(`model_unavailable: summary ${provider}/${id}`);
  if (isApiProvider(provider)) {
    assertApiEffort(model, summary.effort);
    installApiProvider(models, provider, credentials, summary.network.fetch, report);
  } else {
    if (!local) restrictProvider(models, provider, summary.network.fetch as typeof globalThis.fetch,
      { configDir: join(options.agentDir, "summary-claude"), cwd: options.cwd, connection: provider }, summary.awsRoute);
    const source = models.getProvider(provider);
    if (!source) throw new Error("model_unavailable: summary provider not in catalog");
    const wrap = (stream: StreamFunction) => observeUsage(stream, billingKind(provider), report);
    models.registerNativeProvider({ ...source, stream: wrap(source.stream.bind(source)),
      streamSimple: wrap(source.streamSimple.bind(source)) });
  }
  return summaryRequest(models, model, summary);
}

function summaryRequest(models: ModelRuntime, model: Model<Api>, summary: SummaryProfile): SummaryRequest {
  return { model, profile: summary.profile, effort: summary.effort ?? (isApiProvider(summary.provider) ? "off" : undefined),
    stream: (current, context, options) => models.streamSimple(current, context, options as never) };
}

export async function summarizeQuick(
  sdk: Pick<typeof Sdk, "generateSummaryWithUsage">, preparation: SessionBeforeCompactEvent["preparation"],
  request: SummaryRequest, signal: AbortSignal, sessionId: string, customInstructions?: string,
): Promise<CompactionResult> {
  if (!preparation.firstKeptEntryId) throw new Error("First kept entry has no UUID - session may need migration");
  const messages = preparation.turnPrefixMessages.length === 0 ? preparation.messagesToSummarize
    : [...preparation.messagesToSummarize, ...preparation.turnPrefixMessages];
  const result = await sdk.generateSummaryWithUsage(messages, request.model, preparation.settings.reserveTokens,
    undefined, undefined, signal, customInstructions, preparation.previousSummary,
    request.effort as ThinkingLevel | undefined, request.stream, undefined, undefined, undefined, sessionId);
  const modifiedFiles = [...new Set([...preparation.fileOps.written, ...preparation.fileOps.edited])];
  const readFiles = [...preparation.fileOps.read].filter(file => !preparation.fileOps.written.has(file)
    && !preparation.fileOps.edited.has(file));
  const files = `${readFiles.length ? `\n\n<read-files>\n${readFiles.join("\n")}\n</read-files>` : ""}${
    modifiedFiles.length ? `\n\n<modified-files>\n${modifiedFiles.join("\n")}\n</modified-files>` : ""}`;
  return { summary: result.text + files, firstKeptEntryId: preparation.firstKeptEntryId,
    tokensBefore: preparation.tokensBefore, usage: result.usage,
    details: { readFiles, modifiedFiles, summaryProfile: request.profile,
      summaryModel: request.model.id, summaryProvider: request.model.provider } };
}
