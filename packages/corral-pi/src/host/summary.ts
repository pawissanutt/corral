// One request rolls the quick conversation forward, without switching its conversational model.
import { join } from "node:path";
import { homedir } from "node:os";
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { CompactionResult, ModelRuntime, SessionBeforeCompactEvent } from "@earendil-works/pi-coding-agent";
import type { Api, Model, StreamFunction } from "@earendil-works/pi-ai";
import type { ThinkingLevel } from "@earendil-works/pi-agent-core";
import type { HostConfig } from "./config.ts";
import type { LoadedModules } from "./modules.ts";
import type { RunBudget } from "../capabilities/budget.ts";
import { bedrockRoute, createAwsCredentialResolver, type AwsBinding } from "../auth/aws.ts";
import { PROVIDERS, type DestinationRule, type ProviderId, hostRules } from "../auth/providers.ts";
import { connectionRules, isApiConnection } from "../auth/connections.ts";
import type { Fetch } from "../auth/types.ts";
import { type ConnectionBinding, admitConnection, parseBinding } from "./connections.ts";
import { createModelCatalog, NO_CATALOG_CREDENTIALS } from "./catalog.ts";
import { createApiCatalog } from "./api-connections.ts";
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
  readonly binding?: ConnectionBinding;
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
  readonly binding?: ConnectionBinding;
}

/** Independent guarded transport: a different profile never inherits the session's credentials. */
export async function summaryOptions(config: HostConfig, modules: LoadedModules, send: Fetch): Promise<SummaryOptions | undefined> {
  const summary = config.summary;
  if (!summary) return undefined;
  const connection = admitConnection(summary.provider, summary.binding, true);
  let rules = isApiConnection(connection) ? connectionRules(connection) : undefined;
  let awsRoute: AwsRoute | undefined;
  if (summary.provider === "amazon-bedrock") {
    if (!summary.aws) throw new Error("credentials_unavailable: summary profile has no AWS binding");
    adoptSmithyHandlers();
    const resolver = createAwsCredentialResolver(summary.aws, homedir());
    const model = summary.model.replace(/^amazon-bedrock\//, "");
    rules = hostRules(summary.provider, bedrockRoute(summary.aws, model, await resolver.profile()));
    awsRoute = { binding: summary.aws, resolver };
    await resolver.get();
  }
  if (summary.provider === "llama.cpp") {
    if (!config.capabilities?.llama_cpp) throw new Error("summary_profile_invalid: llama.cpp is not enabled");
    rules = localInferenceRules(config.capabilities.llama_cpp);
  }
  const transport = modules.auth.transport?.(authContext(summary.provider, connection)) ?? send;
  const network = guardedFetch(transport, undefined, summary.provider, rules);
  return { ...summary, network, rules: rules ?? hostRules(summary.provider), ...(awsRoute ? { awsRoute } : {}) };
}

export async function createSummaryRequest(options: {
  readonly summary?: SummaryOptions;
  readonly modules: Pick<LoadedModules, "auth">;
  readonly capabilities?: HostSessionOptions["capabilities"];
  readonly agentDir: string;
  readonly cwd: string;
}, budget: RunBudget): Promise<SummaryRequest | undefined> {
  const summary = options.summary;
  if (!summary) return undefined;
  const { provider, binding } = summary;
  parseBinding(binding, provider, summary.model, summary.effort);
  const auth = options.modules.auth;
  if (!(auth.providers ?? [auth.provider]).includes(provider)) {
    throw new Error(`provider_not_allowed: summary profile ${provider} is unsupported by the auth module`);
  }
  if (binding?.origin === "registered" && !auth.connections?.includes(binding.connection)) {
    throw new Error("connection_unsupported_by_auth: summary profile binding is unsupported");
  }
  const connection = admitConnection(provider, binding, true);
  const credentials = provider === "llama.cpp" ? NO_CATALOG_CREDENTIALS
    : options.modules.auth.credentials(authContext(provider, connection));
  const api = isApiConnection(connection);
  const local = provider === "llama.cpp" && options.capabilities?.llama_cpp
    ? await createLocalModels(options.capabilities.llama_cpp, summary.model, summary.network.fetch) : undefined;
  const direct = api && binding ? await createApiCatalog(credentials, binding, summary.network.fetch,
    request => budget.physical(request)) : undefined;
  const models = direct?.models ?? local?.models ?? await createModelCatalog(credentials, { providers: [provider] });
  const id = summary.model.startsWith(`${provider}/`) ? summary.model.slice(provider.length + 1) : summary.model;
  const model = direct?.model ?? local?.model ?? models.getModel(provider, id);
  if (!model) throw new Error(`model_unavailable: summary ${provider}/${id}`);
  if (!direct && !local) restrictProvider(models, provider, summary.network.fetch as typeof globalThis.fetch,
    { configDir: join(options.agentDir, "summary-claude"), cwd: options.cwd,
      connection: binding?.identity ?? connection }, summary.awsRoute);
  if (!direct) {
    const source = models.getProvider(provider);
    if (!source) throw new Error("model_unavailable: summary provider not in catalog");
    models.registerNativeProvider({ ...source,
      stream: budget.wrap(source.stream.bind(source), PROVIDERS[provider].lane !== undefined),
      streamSimple: budget.wrap(source.streamSimple.bind(source), PROVIDERS[provider].lane !== undefined) });
  }
  return summaryRequest(models, model, summary);
}

function summaryRequest(models: ModelRuntime, model: Model<Api>, summary: SummaryProfile): SummaryRequest {
  return { model, profile: summary.profile, effort: summary.effort, binding: summary.binding,
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
      summaryModel: request.model.id, summaryProvider: request.model.provider,
      summaryBinding: request.binding } };
}
