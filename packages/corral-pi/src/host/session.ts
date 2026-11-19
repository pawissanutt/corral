// One upstream AgentSessionRuntime, with local session state and engine CLI board access.
import { existsSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

import type * as Sdk from "@earendil-works/pi-coding-agent";
import type {
  AgentSession, AgentSessionRuntime, CreateAgentSessionRuntimeFactory, ExtensionAPI, SessionManager, ToolDefinition,
} from "@earendil-works/pi-coding-agent";
import type { ThinkingLevel } from "@earendil-works/pi-agent-core";

import type { ImageContent, StreamFunction } from "@earendil-works/pi-ai";
import { lazyStream } from "@earendil-works/pi-ai/api/lazy";
import { getApiProvider } from "@earendil-works/pi-ai/compat";

import type { AwsBinding, AwsCredentialResolver } from "../auth/aws.ts";
import { compactionSettings } from "../capabilities/compaction.ts";
import { codemodeTool } from "../capabilities/codemode.ts";
import { nestedChild } from "../extensions/codemode/child.ts";
import { CODE_CALL_ENTRY, CodeRuntime } from "../extensions/codemode/runtime.ts";
import { HOOK_ENTRY, Hooks } from "../capabilities/hooks.ts";
import { CHECKPOINT_ENTRY, Checkpoints } from "../capabilities/checkpoints.ts";
import { jobControl, type JobControl } from "./job-controls.ts";
import { type HostControls, installToolControls, parseControls } from "./tool-controls.ts";
import { type CapabilityGuidance, type Guidance, selectGuidance } from "../capabilities/guidance.ts";
import { PROVIDERS, type ProviderId, billingKind, hostDestinations, isApiProvider, isProviderId } from "../auth/providers.ts";
import type { AuthContext, Fetch } from "../auth/types.ts";
import type { EventLog } from "../bridge/events.ts";
import { LiveEvents } from "./live-events.ts";
import { RemoteBuild } from "./remote/build.ts";
import { cardBrief, cardDetail, questionEntries, questionStatus, sortedCards, type AnswerStatus, type CardEntry } from "./remote/cards.ts";
import { RemoteDialogs } from "./remote/dialogs.ts";
import { RemoteRegistry } from "./remote/registry.ts";
import type { TranscriptInput } from "./remote/input.ts";
import { boardBrief, capabilities, exitBrief, jobCounts, planBrief, type SessionState,
  type StateProjection, type UsageBrief } from "./remote/state.ts";
import { utf8Prefix } from "../roles/assistant/owner-action.ts";
import type { RemoteOwner } from "./remote-owner.ts";
import { CLAUDE_SDK_API, type ClaudeLaneConfig, registerClaudeSdkApi } from "../claude/stream.ts";
import { closeResidentSession, residentLifecycle } from "../claude/resident.ts";
import { BridgeRefusal, type AnswerInput, type RewindInput, type SessionInput, type SubmitResult } from "../bridge/protocol.ts";
import { listRewinds, parseRewindCommand, recordRewind, restoreRewind, rewindPoint } from "./rewind.ts";
import type { HostIdentity, RoleContext } from "../roles/types.ts";
import type { BuildContext, FleetSnapshot } from "../build/types.ts";
import { createBuildControls, type BuildControls } from "../build/host-controls.ts";
import { type CorralClient, type ModelUsage } from "../tools/corral.ts";
import { observeUsage } from "./model-usage.ts";
import { corralToolNames, defineCorralTool } from "../tools/define.ts";
import { installChrome } from "../ui/chrome.ts";
import { NativePanels } from "../ui/panels.ts";
import { persistedBases } from "../ui/review-store.ts";
import { sidebarToggleKey } from "../ui/herdr-keys.ts";
import type { PanelJob, PanelSnapshot } from "../ui/panels-view.ts";
import { buildToolRenderers } from "../ui/build/tool-renderers.ts";
import { installToolRenderers, type ToolRenderDeps } from "../ui/tool-rendering.ts";
import { installMarkRows } from "../ui/mark-rows.ts";
import { TodoLedger, installPlanCommand } from "../ui/todo.ts";
import type { HostCapabilities, ModelChoice, UiMode } from "./config.ts";
import { EXPORT_COMMAND, type SessionExport, exportSnapshot } from "./export.ts";
import { askUserTool, localQuestionAsk } from "./ask-user.ts";
import { PendingQuestions, QUESTION_ENTRY, validAnswer } from "./pending-questions.ts";
import { OwnerQuestionUi } from "../ui/owner-questions.ts";
import { registerCacheStats } from "../ui/cache-stats.ts";
import type { HerdrLifecycle } from "./herdr.ts";
import type { LoadedModules } from "./modules.ts";
import { type GuardedFetch, guardedFetch, resourceLoaderOptions, restrictedSettings } from "./restrict.ts";
import { createModelCatalog } from "./catalog.ts";
import { installModelSelection } from "./model-selection.ts";
import { interactiveModels, sessionRoutes, type SessionRoutes } from "./capabilities.ts";
import { observeTerminalInput, type InputObservation } from "./input-observation.ts";
import { browserSettings, browserTools } from "../extensions/browser/index.ts";
import { installLocalProvider, LocalModelUnavailable } from "./local-models.ts";
import { webTools } from "../extensions/web/index.ts";
import { runSubscriptionSearch } from "../extensions/web/subscription.ts";
import { streamClaudeSearch } from "../claude/search.ts";
import { configuredServers, lspToolkit, type LspToolkit } from "../extensions/lsp/index.ts";
import { capabilityRegistry } from "../extensions/registry.ts";
import { McpAccess } from "../capabilities/mcp.ts";
import { MCP_CONTROLS } from "./mcp-tools.ts";
import { toolSearch } from "./tool-search.ts";
import { JOB_MONITOR_TOOL, jobTools } from "../extensions/jobs/index.ts";
import { InputWake } from "../extensions/input-wake.ts";
import { JobRuntime } from "../extensions/jobs/runtime.ts";
import { COMMAND_ENTRY, Commands, type CommandFact } from "../extensions/jobs/commands.ts";
import { MONITOR_ENTRY, type MonitorFact } from "../extensions/jobs/monitors.ts";
import { JobSettingsSchema } from "../extensions/jobs/settings.ts";
import { AuxiliaryRuntime } from "../extensions/subagent/runtime.ts";
import { auxiliarySettings } from "../extensions/subagent/settings.ts";
import { expertTool, exploreTool } from "../extensions/subagent/index.ts";
import { oracleTool } from "../extensions/oracle/index.ts";
import { runAuxiliarySession } from "../extensions/subagent/session.ts";
import { FleetRuntime } from "../extensions/fleet/runtime.ts";
import { fleetTools } from "../extensions/fleet/tools.ts";
import { boundFleetSettings } from "../extensions/fleet/settings.ts";
import { executionRefusal, exploreLoadout } from "../roles/common/execution.ts";
import { workerDoneTool } from "../roles/worker/tools.ts";
import { transcriptPreview } from "./transcript-preview.ts";
import { ManagedEffects } from "./managed-effects.ts";
import { LocalControlPlane } from "./local.ts";
import { failureClass, messageFailure, withProviderFailures } from "./provider-failure.ts";
import { assertApiEffort, installApiProvider } from "./api-providers.ts";
import { withSubscriptionFallback } from "./continuation.ts";
import { HostQueue } from "./queue.ts";
import { MARK_ENTRY, observeTranscriptEntries, recordTranscriptMark, type MarkSink, type ModelMark, type TranscriptMark,
  type TurnStopCause, type TurnWait } from "./transcript-marks.ts";
import { TurnFacts, turnWaits } from "./turn-facts.ts";
import { RemotePrompts } from "./remote-prompts.ts";
import { createSummaryRequest, summarizeQuick, type SummaryOptions, type SummaryRequest } from "./summary.ts";

export interface HostSessionOptions {
  readonly sdk: typeof Sdk;
  readonly identity: HostIdentity;
  readonly build?: BuildContext;
  readonly paneInput?: (text: string, images?: ImageContent[]) => Promise<boolean>;
  readonly transcriptRewound?: () => void;
  readonly automatedInput?: (input: SessionInput) => Promise<SubmitResult> | undefined;
  readonly cwd: string;
  readonly agentDir: string;
  readonly model: string;
  /** The bound profile's provider; absent means OpenAI Codex, the only pre-T-479 route. */
  readonly provider?: ProviderId;
  readonly fallback?: readonly ModelChoice[];
  readonly routes?: SessionRoutes;
  readonly apiFetch?: Fetch;
  readonly sessionDir?: string;
  /** The Bedrock binding and its credential resolver; required for an AWS-profile provider. */
  readonly aws?: AwsRoute;
  readonly effort?: string;
  readonly quickSummary?: boolean;
  readonly summary?: SummaryOptions;
  readonly promptFile?: string;
  readonly todo?: boolean;
  /** The session's recorded snapshot; when present it decides todo. */
  readonly capabilities?: HostCapabilities;
  /** The snapshot's settings and project-text trust; absent loads no optional guidance. */
  readonly guidance?: CapabilityGuidance;
  /** The corral workspace root, the outermost directory whose context files may load. */
  readonly root?: string;
  /** Where `~/` in a `user_dirs` entry points; absent means the user's home directory. */
  readonly home?: string;
  /** The private directory `/corral-export` writes into; absent disables the command. */
  readonly exportDir?: string;
  /** The session's runtime directory; background job output files live under it. */
  readonly runtimeDir?: string;
  /** Absent means rich: corral's header, footer status and compact corral_* rendering. */
  readonly ui?: UiMode;
  /** Presentation-only journal entries; absent for hosts without transcript marks. */
  readonly marks?: MarkSink;
  readonly modules: LoadedModules;
  readonly client: CorralClient;
  readonly events: EventLog;
  readonly network: GuardedFetch;
  readonly sessionManager: SessionManager;
  /** The pane's herdr projection; absent for a headless host or one outside its bound pane. */
  readonly herdr?: HerdrLifecycle;
}

type TurnOrigin = { kind: "bridge" } | { kind: "owner_terminal" } | { kind: "unknown" };

const THINKING: Record<ThinkingLevel, true> = { off: true, minimal: true, low: true, medium: true, high: true, xhigh: true, max: true };
const STATE_SECTION_MAX_BYTES = 8 * 1024;

/** A module's own readback, replaced by its size once it could crowd a bridge frame. */
function bounded(value: Record<string, unknown> | null): Record<string, unknown> | null {
  const size = value === null ? 0 : Buffer.byteLength(JSON.stringify(value));
  return size > STATE_SECTION_MAX_BYTES ? { omitted: "state_too_large", bytes: size } : value;
}

type Models = Awaited<ReturnType<typeof Sdk.ModelRuntime.create>>;

export interface AwsRoute {
  readonly binding: AwsBinding;
  readonly resolver: AwsCredentialResolver;
  readonly modelIds?: ReadonlySet<string>;
}

/**
 * Re-registers the Bedrock provider so it signs only with the bound profile's credentials: Pi's
 * env-sniffing auth (Bedrock API keys, AWS_PROFILE, ambient keys, container and web-identity
 * roles) is replaced by a fixed source, and every turn resolves the resolver's unexpired keys into
 * a scoped environment before Pi builds its client, which pins them so the SDK's default chain
 * never runs. A turn without keys ends with the refusal before any request.
 */
function restrictAwsProvider(
  models: Models, provider: NonNullable<ReturnType<Models["getProvider"]>>, fetch: typeof globalThis.fetch, aws: AwsRoute,
): void {
  const route = PROVIDERS[provider.id as ProviderId];
  const stream = provider.stream.bind(provider) as StreamFunction;
  const streamSimple = provider.streamSimple.bind(provider) as StreamFunction;
  const signed = async (current: { api: string }, streamOptions: unknown) => {
    if (current.api !== route.modelApi) throw new Error("provider_api_not_allowed");
    const credentials = await aws.resolver.get();
    const { apiKey: _apiKey, bearerToken: _bearer, profile: _profile, env: _env, ...rest } =
      (streamOptions ?? {}) as Record<string, unknown>;
    return {
      ...rest, fetch, region: aws.binding.region,
      env: {
        AWS_ACCESS_KEY_ID: credentials.accessKeyId, AWS_SECRET_ACCESS_KEY: credentials.secretAccessKey,
        ...(credentials.sessionToken ? { AWS_SESSION_TOKEN: credentials.sessionToken } : {}),
        // HTTP/2 signs the :authority pseudo-header, which the guarded fetch cannot send.
        AWS_BEDROCK_FORCE_HTTP1: "1",
      },
    };
  };
  models.registerNativeProvider({
    ...provider,
    ...(aws.modelIds ? { getModels: () => provider.getModels().filter(model => aws.modelIds!.has(model.id)),
      getAllModels: () => provider.getModels().filter(model => aws.modelIds!.has(model.id)) } : {}),
    auth: { apiKey: { name: "corral AWS profile", resolve: async () => ({ auth: {}, source: "corral AWS profile" }) } },
    stream: (current, context, streamOptions) =>
      lazyStream(current, async () => stream(current, context, await signed(current, streamOptions) as never)),
    streamSimple: (current, context, streamOptions) =>
      lazyStream(current, async () => streamSimple(current, context, await signed(current, streamOptions) as never)),
  });
}

/**
 * Re-registers the bound provider so it can only run on its stored subscription OAuth
 * credential and only through the guarded fetch. Pi's API-key handler (an API key, ambient
 * environment tokens or workload federation) is dropped, so a logout or a removed credential
 * leaves the provider without auth instead of falling back to a paid key; and a request whose
 * auth is not the provider's OAuth token is refused before it is sent, since paid and
 * subscription Anthropic traffic share one endpoint. A provider with a `lane` never reaches its
 * endpoint from this process: its calls run through that lane's registered Pi API (T-493).
 */
export function restrictProvider(
  models: Models, providerId: ProviderId, fetch: typeof globalThis.fetch, lane?: ClaudeLaneConfig, aws?: AwsRoute,
): void {
  const route = PROVIDERS[providerId];
  if (isApiProvider(providerId)) throw new Error("API providers require installApiProvider");
  const provider = models.getProvider(route.id);
  if (route.auth === "aws-profile") {
    if (!provider) throw new Error(`model_unavailable: ${route.id} is not in Pi's catalog`);
    if (!aws) {
      throw new Error(`credentials_unavailable: ${route.id} has no AWS profile binding; ` +
        `run \`corral agent login --provider ${route.id}\``);
    }
    restrictAwsProvider(models, provider, fetch, aws);
    return;
  }
  if (!provider?.auth.oauth) throw new Error(`model_unavailable: ${route.id} has no subscription OAuth`);
  let stream = provider.stream.bind(provider) as StreamFunction;
  let streamSimple = provider.streamSimple.bind(provider) as StreamFunction;
  if (route.lane !== undefined) {
    if (!lane) throw new Error(`model_unavailable: ${route.id} runs through the ${route.lane} lane, which has no configuration`);
    if (lane.searchOnly) {
      stream = streamSimple = (current, context, options) => streamClaudeSearch(lane, current, context, options);
    } else {
      registerClaudeSdkApi(lane);
      const api = getApiProvider(CLAUDE_SDK_API);
      if (!api) throw new Error(`model_unavailable: the ${route.lane} lane is not registered`);
      stream = (current, context, options) => api.stream({ ...current, api: CLAUDE_SDK_API }, context, options);
      streamSimple = (current, context, options) => api.streamSimple({ ...current, api: CLAUDE_SDK_API }, context, options);
    }
  }
  const checked = (current: { api: string }, streamOptions: unknown) => {
    if (current.api !== route.modelApi) throw new Error("provider_api_not_allowed");
    const auth = (streamOptions ?? {}) as { apiKey?: unknown; headers?: Record<string, unknown> };
    const headers = Object.keys(auth.headers ?? {}).map((name) => name.toLowerCase());
    if (route.oauthTokenMarker !== undefined &&
        (typeof auth.apiKey !== "string" || !auth.apiKey.includes(route.oauthTokenMarker) ||
         headers.includes("authorization") || headers.includes("x-api-key"))) {
      throw new Error(`credentials_unavailable: ${route.id} requests must use the subscription OAuth login; ` +
        `run \`corral agent login --provider ${route.id}\``);
    }
    return { ...(streamOptions as object), fetch };
  };
  if (providerId === "anthropic" || providerId === "openai-codex") {
    stream = withProviderFailures(providerId, stream);
    streamSimple = withProviderFailures(providerId, streamSimple);
  }
  models.registerNativeProvider({
    ...provider,
    auth: { oauth: provider.auth.oauth },
    stream: (current, context, streamOptions) => stream(current, context, checked(current, streamOptions)),
    streamSimple: (current, context, streamOptions) => streamSimple(current, context, checked(current, streamOptions)),
  });
}

export function authContext(provider: ProviderId = "openai-codex"): AuthContext {
  return { provider, apiEndpoint: PROVIDERS[provider].api, refreshEndpoint: PROVIDERS[provider].refresh };
}

export function providerErrorClass(message: string): string {
  if (/network_refused/.test(message)) return "network_refused";
  // AWS errors carry their exception name, often without an HTTP status code.
  if (/credentials_unavailable|\b(AccessDeniedException|UnrecognizedClientException|ExpiredTokenException|InvalidSignatureException)\b/
    .test(message)) return "auth";
  if (/\bThrottlingException\b|throttl/i.test(message)) return "rate_limit";
  if (/\b(ServiceUnavailableException|ModelNotReadyException)\b/.test(message)) return "overload";
  if (/\b(401|403)\b|unauthori[sz]ed|forbidden|expired/i.test(message)) return "auth";
  if (/\b429\b|rate.?limit/i.test(message)) return "rate_limit";
  if (/\b5\d\d\b|overload|unavailable/i.test(message)) return "overload";
  return "unknown";
}

/** Changes "since session" compares against the HEAD each root had when this session first read it. */
const REVIEW_BASE_ENTRY = "corral_review_base";
const panelStats = (stats: { readonly tokens: { readonly total: number }; readonly cost: number }) => ({ tokens: stats.tokens.total, cost: stats.cost });

// allow: SIZE_OK - Pi lifecycle adapter.

export class HostSession {
  runtime!: AgentSessionRuntime;
  /** Interactive host API only; deliberately absent from extension/model registries. */
  readonly queue: HostQueue;
  private readonly queuedInputFacts = new WeakMap<object, { facts: TranscriptInput; transcriptId?: string }>();
  private startedInputFacts: { facts: TranscriptInput; transcriptId?: string } | undefined;
  private unobserveTranscript: (() => void) | undefined;
  private remoteUsage: { leaf: string | null; model: AgentSession["model"]; usage: UsageBrief } | undefined;
  private remoteFleetDirty = true;
  private remoteFleet: FleetSnapshot | undefined;
  readonly liveEvents: LiveEvents;
  readonly remoteRegistry = new RemoteRegistry();
  readonly remoteBuild: RemoteBuild;
  private readonly remoteDialogs: RemoteDialogs;
  readonly panels: NativePanels | undefined;
  private readonly unsubscribePanels: (() => void) | undefined;
  private terminalInput: InputObservation | undefined;
  private terminalInputSequence = 0;
  private readonly baseLoadout: readonly string[];
  private mcp: McpAccess | undefined;
  private readonly roleContext: RoleContext;
  private readonly localTools: readonly ToolDefinition[];
  private dialogPi: ExtensionAPI | undefined;
  private readonly ownerQuestions: PendingQuestions | undefined;
  private readonly remotePrompts: RemotePrompts;
  private readonly local: LocalControlPlane;
  private questionUi: OwnerQuestionUi | undefined;
  private readonly jobs: JobRuntime | undefined;
  private readonly commands: Commands | undefined;
  private readonly controlJob: JobControl | undefined;
  /** The session's Code Mode kernel and its nested-call bridge (T-683); absent when disabled. */
  private readonly code: CodeRuntime | undefined;
  private turn = 0;
  private open = false;
  /** Each is settled once, by the first turn numbered `from` or later that ends. */
  private turnWaiters: { readonly from: number; settle(aborted: boolean): void }[] = [];
  private cancelRequested: number | undefined;
  private stopCause: TurnStopCause | undefined;
  private turnError: string | null = null;
  private nextTurnOrigin: TurnOrigin | undefined;
  private lastInputSource: string | undefined;
  /** The owner typed into the pane while the current turn was open (a steer into it). */
  private ownerInput = false;
  private boundSessionId: string | undefined;
  private unsubscribe: (() => void) | undefined;
  private calls: Promise<unknown> = Promise.resolve();
  private deniedTools = 0;
  private panelStats: { readonly tokens: number; readonly cost: number } | undefined;
  private panelTokenLeaf: string | null | undefined;
  private sessionsCreated = 0;
  /** How the bound host is attached: InteractiveMode in its pane, or headless (T-577). */
  private attached: "interactive" | "headless" | undefined;
  private guidance: Guidance = { skillPaths: [], promptPaths: [], contextFiles: [] };
  private readonly settings: ReturnType<typeof restrictedSettings> & ReturnType<typeof compactionSettings>
    & { readonly steeringMode: "all"; readonly followUpMode: "all" };
  /** Ends the model's job_wait/agent_wait once input is queued for the next turn boundary. */
  private readonly inputWake = new InputWake(() => this.session.getSteeringMessages().length > 0);
  private summaryRequest: SummaryRequest | undefined;
  private readonly controls: HostControls;
  private readonly hooks: Hooks | undefined;
  private additionalLsp: LspToolkit | undefined;
  private readonly additionalLspFactory: (() => LspToolkit) | undefined;
  private readonly bashQuestions = new Map<string, string>();
  private readonly checkpoints: Checkpoints | undefined;
  private rewinding = false;
  private readonly auxiliary: AuxiliaryRuntime | undefined;
  private readonly fleet: FleetRuntime | undefined;
  private readonly buildControls: BuildControls | undefined;
  private auxiliaryModels: Models | undefined;
  private buildProviders: readonly string[] = [];
  private readonly apiNetworks: GuardedFetch[] = [];
  private searchModels: Models | undefined;
  private ledger: TodoLedger | undefined;
  private closing = false;
  private closeResult: Promise<void> | undefined;
  private readonly managedEffects = new ManagedEffects();
  private readonly marks: MarkSink;
  private readonly headlessMarkRows: MarkSink = { add: mark => {
    this.session.sessionManager.appendCustomEntry(MARK_ENTRY, mark, { immediate: true });
    return true;
  } };
  /** The transcript's own sink, once the rich UI registered its mark rows; `options.marks` replaces it. */
  private markRows: MarkSink | undefined;
  private readonly turnFacts: TurnFacts;
  private selection: ModelMark["was"];
  private readonly monitorFacts = new Map<string, MonitorFact>();

  readonly provider: ProviderId;

  private constructor(private readonly options: HostSessionOptions) {
    const host = this;
    this.liveEvents = new LiveEvents();
    this.remoteDialogs = new RemoteDialogs(() => this.liveEvents.stateChanged());
    this.provider = options.provider ?? "openai-codex";
    this.local = new LocalControlPlane(options.build?.dir ?? options.runtimeDir ?? options.agentDir, options.identity);
    this.remotePrompts = new RemotePrompts(this.liveEvents.events);
    this.marks = { add: mark => this.addMark(mark) };
    this.turnFacts = new TurnFacts(this.marks);
    this.queue = new HostQueue({
      session: () => this.session,
      enabled: () => (options.capabilities?.catalog ?? 0) >= 10 && options.capabilities?.enabled.includes("queue_control") === true,
      interactive: () => this.attached === "interactive",
      turn: next => next || !this.open ? this.turn + 1 : this.turn,
      emit: (kind, data) => options.events.emit(kind, data),
      liveInput: event => this.liveEvents.input(event),
    });
    this.buildControls = options.build ? createBuildControls({
      sdk: options.sdk, context: options.build,
      models: () => {
        if (!this.auxiliaryModels) throw new Error("build model runtime is not ready");
        return this.auxiliaryModels;
      },
      approved: () => this.buildProviders,
      parent: { provider: this.provider, model: options.model.startsWith(`${this.provider}/`) ? options.model.slice(this.provider.length + 1) : options.model,
        effort: options.effort },
      agentDir: options.agentDir, wake: this.inputWake,
      localTools: () => this.localTools.filter(tool => !this.buildControls?.tools.includes(tool)),
      deliverToParent: async text => {
        const result = await this.submitAutomated(text);
        if (result.disposition === "rejected") throw new Error(`Fleet parent delivery rejected: ${result.reason}`);
      },
      repairLand: async text => {
        const result = await this.submitLocal({ text, busyPolicy: "steer", origin: "auto" });
        // A started prompt owns its turn; a steer queued into the open turn is answered there.
        const from = result.disposition === "started" ? result.turn : result.disposition === "queued" && this.open ? this.turn : undefined;
        if (from === undefined) throw new Error(`Land repair reached no turn: ${result.reason ?? result.disposition}`);
        return new Promise(resolve => { this.turnWaiters.push({ from, settle: aborted => resolve({ aborted }) }); });
      },
      shellRefusal: options.modules.role.shellRefusal, changed: () => this.fleetChanged(),
    }) : undefined;
    this.panels = options.ui === "plain" ? undefined : new NativePanels({
      identity: options.identity, cwd: options.cwd, snapshot: () => this.panelSnapshot(),
      sidebarKey: sidebarToggleKey(process.env),
      marks: this.marks,
      ...(this.buildControls ? { build: this.buildControls.ui, deliverBuildInput: (text: string) => this.deliverBuildInput(text) } : {}),
      get fleetControl() { return options.build ? undefined : host.fleet; },
      queueControl: {
        enabled: () => this.queue.controlEnabled,
        edit: key => this.queue.edit(key),
        remove: key => this.queue.remove(key),
        move: (key, delta) => this.queue.move(key, delta),
      },
      ...(options.identity.project && options.guidance?.projectDir && options.guidance.projectDir !== options.client.workspaceRoot
        ? { projectDir: options.guidance.projectDir } : {}),
      readProjects: () => options.client.projectScopes(),
      jobControl: (target, action) => this.controlJob?.(target, action) ?? Promise.resolve({ state: "denied", reason: "managed commands are unavailable" }),
      reviewBases: persistedBases(
        () => options.sessionManager.getEntries().flatMap(entry => entry.type === "custom" && entry.customType === REVIEW_BASE_ENTRY ? [entry.data] : []),
        data => options.sessionManager.appendCustomEntry(REVIEW_BASE_ENTRY, data)),
      canRenderTool: name => exploreLoadout(this.loadout).includes(name),
      artifacts: () => options.client.artifacts(),
      artifactFile: reference => options.client.artifactFile(reference),
      rewind: input => this.rewind(input),
      exportSession: async () => (await this.reportedExport()).text,
      readHistory: request => options.client.history(request),
      readTranscript: id => this.readAgentTranscript(id),
      ...(options.identity.task ? { readTask: () => options.client.taskSnapshot() } : {}),
    });
    this.unsubscribePanels = this.panels ? options.events.on(event => {
      if (event.kind === "terminal_input" && typeof event.input_sequence === "number" && event.input_sequence >= this.terminalInputSequence) {
        this.terminalInputSequence = event.input_sequence;
        this.terminalInput = { key: String(event.key), disposition: event.disposition as InputObservation["disposition"],
          ...(typeof event.reason === "string" ? { reason: event.reason } : {}),
          ...(typeof event.policy === "string" ? { policy: event.policy } : {}) };
      }
      this.panels?.refresh();
    }) : undefined;
    const registerCapability = capabilityRegistry(() => this.session?.sessionFile);
    this.roleContext = {
      identity: options.identity, cwd: options.cwd, workspaceRoot: options.client.workspaceRoot,
      model: options.model, registerCapability,
      todo: options.capabilities ? options.capabilities.enabled.includes("todo") : options.todo === true,
      ...(options.capabilities ? { capabilities: options.capabilities } : {}),
      ...(options.build ? { build: options.build } : {}),
      ...(options.guidance?.settings.lsp ? { lspSettings: options.guidance.settings.lsp } : {}),
      ...(options.effort ? { effort: options.effort } : {}),
      ...(options.promptFile ? { promptFile: options.promptFile } : {}),
    };
    const role = options.modules.role;
    const specs = role.tools(this.roleContext);
    const roleLocal = role.localTools?.(this.roleContext) ?? [];
    const roleNames = roleLocal.map((tool) => tool.name);
    if (roleNames.join(",") !== (role.localToolNames ?? []).join(",")) {
      throw new Error(`module_invalid: local tools ${roleNames.join(",")} differ from localToolNames`);
    }
    const webEnabled = options.capabilities?.enabled.includes("web.enabled") === true;
    if (webEnabled && (!options.network.web || !options.capabilities?.web)) {
      throw new Error("module_invalid: web.enabled needs an eligible role and its configured web transport");
    }
    let browser: ReturnType<typeof browserSettings>;
    try {
      browser = browserSettings(options.identity.role, options.capabilities?.enabled ?? [], options.guidance?.settings);
    } catch (error) {
      throw new Error(`module_invalid: ${error instanceof Error ? error.message : error}`);
    }
    const lspEnabled = !options.capabilities || options.capabilities.catalog < 5
      ? options.identity.role === "worker" : options.capabilities.enabled.includes("lsp");
    if (lspEnabled && options.identity.role !== "worker") {
      this.additionalLspFactory = () => lspToolkit({ root: options.cwd, servers: configuredServers(options.guidance?.settings.lsp), registerCapability });
      this.additionalLsp = this.additionalLspFactory();
    }
    this.jobs = this.jobRuntime(registerCapability);
    this.commands = this.jobs ? new Commands({
      jobs: this.jobs,
      waitMs: JobSettingsSchema.parse(options.guidance?.settings.background_shell).wait_ms,
      journal: fact => options.sessionManager.appendCustomEntry(COMMAND_ENTRY, fact),
      monitorJournal: fact => {
        if (!fact.result && !fact.delivered) this.monitorFacts.set(fact.key, fact);
        else this.monitorFacts.delete(fact.key);
        options.sessionManager.appendCustomEntry(MONITOR_ENTRY, fact);
      },
      changed: () => { this.liveEvents.stateChanged(); this.panels?.refresh(); },
      settled: result => options.events.emit("command_settled", result),
      canNotify: () => !this.open,
      noticeSeen: key => this.commandNoticeSeen(key),
      notify: (key, text) => this.commandNotice(key, text),
    }) : undefined;
    this.code = options.capabilities?.enabled.includes("codemode") ? new CodeRuntime({
      emit: (kind, data) => { options.events.emit(kind, data); },
      journal: (data, customType = CODE_CALL_ENTRY) => { this.session.sessionManager.appendCustomEntry(customType, data); },
      history: () => options.sessionManager.getEntries().flatMap(entry =>
        entry.type === "custom" && (entry.customType === "corral_code_cell" || entry.customType === CODE_CALL_ENTRY) ? [entry.data] : []),
      notify: (key, text) => this.commandNotice(key, text),
    }) : undefined;
    const subscriptionSearch = webEnabled && options.capabilities?.web?.search_provider === "subscription";
    const auxiliary = { ...auxiliarySettings(options.identity.role, options.capabilities?.enabled ?? [], options.guidance?.settings),
      ...(subscriptionSearch ? { web_search: { model: null, timeout_secs: (options.capabilities?.web?.timeout_ms ?? 15000) / 1000 } } : {}),
    };
    const record = async (payload: Record<string, unknown>): Promise<Record<string, unknown>> => {
      options.events.emit("auxiliary", payload);
      if (payload.action !== "prepare") return {};
      if (typeof payload.request !== "string") throw new Error("Invalid auxiliary session id");
      return { session_id: payload.request, session_file: this.local.auxiliaryTranscript(payload.request) };
    };
    if (!options.build) {
      const host = this;
      const settings = boundFleetSettings(options.identity.role, options.capabilities?.enabled ?? [], options.guidance?.settings);
      this.fleet = new FleetRuntime({
        sdk: options.sdk,
        get models() {
          if (!host.auxiliaryModels) throw new Error("child model runtime is not ready");
          return host.auxiliaryModels;
        },
        get approved() { return host.buildProviders; },
        parent: { provider: this.provider, model: options.model.startsWith(`${this.provider}/`) ? options.model.slice(this.provider.length + 1) : options.model, effort: options.effort },
        context: { session: { worktree: options.cwd, name: `${options.identity.role} session` },
          dir: options.runtimeDir ?? join(options.agentDir, "runtime"),
          settings: { maxParallel: settings.maxParallel, profiles: [] } },
        agentDir: options.agentDir, bound: true, allowedKinds: settings.allowedKinds,
        maxTurns: settings.maxTurns, timeoutSecs: settings.timeoutSecs,
        ...(auxiliary.explore ? { research: { settings: auxiliary.explore } } : {}),
        get parentLoadout() { return host.loadout; },
        get localTools() { return host.localTools; },
        shellRefusal: options.modules.role.shellRefusal,
        changed: () => this.fleetChanged(), record,
        deliverToParent: async text => {
          const result = await this.submitAutomated(text);
          if (result.disposition === "rejected") throw new Error(`Fleet parent delivery rejected: ${result.reason}`);
        },
      });
    }
    if (auxiliary.explore || auxiliary.oracle || auxiliary.web_search || auxiliary.expert) {
      this.auxiliary = new AuxiliaryRuntime({
        settings: auxiliary, provider: this.provider, model: options.model,
        changed: () => this.fleetChanged(),
        sessionId: () => this.sessionId(),
        searchModel: () => {
          const model = this.runtime?.session.model;
          return model && ["openai-codex", "anthropic"].includes(model.provider) && this.searchModels?.getModel(model.provider, model.id)
            ? { provider: model.provider, model: model.id } : undefined;
        },
        record,
        execute: (invocation, signal) => {
          if (invocation.kind === "web_search") {
            if (!this.searchModels || !options.capabilities?.web) throw new Error("subscription search runtime is not ready");
            return runSubscriptionSearch({ sdk: options.sdk, models: this.searchModels, cwd: options.cwd,
              settings: options.capabilities.web }, invocation, signal);
          }
          if (!this.auxiliaryModels) throw new Error("auxiliary model runtime is not ready");
          return runAuxiliarySession({ sdk: options.sdk, models: this.auxiliaryModels, provider: this.provider,
            cwd: options.cwd, agentDir: options.agentDir,
            parentLoadout: this.loadout, localTools: this.localTools }, invocation, signal);
        },
      });
    }
    this.remoteBuild = new RemoteBuild(this.buildControls, {
      fleet: () => this.buildControls?.ui.fleet ?? this.fleet,
      children: () => this.readRemoteFleet()?.children ?? [],
      ready: () => this.remoteActionsReady,
      headless: () => this.attached === "headless",
      working: () => this.open || this.session.isStreaming,
      changed: () => this.liveEvents.stateChanged(),
      mark: mark => this.addMark(mark),
      post: entry => this.session.sendCustomMessage(entry, { triggerTurn: false }),
      error: text => this.addMark({ kind: "note", tone: "error", at: Date.now(), text }),
    });
    const dialogEnabled = options.capabilities?.enabled.includes("ask_user") === true;
    const questions = dialogEnabled || options.capabilities?.enabled.includes("bash_rules") ? new PendingQuestions({
      session: () => options.sessionManager.getSessionId(),
      entries: () => options.sessionManager.getEntries(),
      append: value => { options.sessionManager.appendCustomEntry(QUESTION_ENTRY, value, { immediate: true }); },
      changed: question => {
        if (question) {
          this.remotePrompts.changed(question);
          options.events.emit("owner_question", { id: question.id, status: question.status });
          options.herdr?.questionChanged(question.id, question.status === "pending");
        }
        this.questionUi?.changed(question);
        this.liveEvents.stateChanged();
      },
      deliver: async text => {
        const result = await this.submitLocal({ text, origin: "owner", busyPolicy: "steer" });
        return result.disposition === "started" || result.disposition === "queued";
      },
    }) : undefined;
    this.ownerQuestions = questions;
    this.localTools = [...(lspEnabled ? roleLocal : roleLocal.filter((tool) => !tool.name.startsWith("lsp_"))),
      ...(dialogEnabled && questions ? [askUserTool(async answer => {
        if (!this.dialogPi) throw new Error("dialog session is unavailable");
        this.dialogPi.sendUserMessage(answer, { deliverAs: "steer" });
        return answer;
      }, questions, () => this.questionUi?.busy === true, () => this.dialogPresentation(), localQuestionAsk(questions),
      () => this.attached === "headless")] : []),
      ...(this.additionalLsp?.tools ?? []).map(tool => ({ ...tool, execute: (...args: Parameters<ToolDefinition["execute"]>) => {
        const current = this.additionalLsp?.tools.find(candidate => candidate.name === tool.name);
        if (!current) throw new Error("LSP runtime unavailable; no effect started");
        return current.execute(...args);
      } })), ...(webEnabled && options.network.web ? webTools(options.network.web,
        subscriptionSearch ? async (id, query, count, signal) => {
          if (!this.auxiliary) throw new Error("subscription search is disabled");
          return (await this.auxiliary.run("web_search", id, JSON.stringify({ query, count }), signal)).text;
        } : undefined) : []),
      ...(this.jobs ? jobTools(this.jobs, true, this.commands, this.inputWake).filter(tool => tool.name !== "bash" || role.upstreamTools.includes("bash")) : []),
      ...(this.fleet && auxiliary.explore ? [exploreTool(this.fleet)] : []),
      ...(this.auxiliary && auxiliary.oracle ? [oracleTool(this.auxiliary)] : []),
      ...(this.auxiliary && auxiliary.expert ? [expertTool(this.auxiliary)] : []),
      ...(browser ? browserTools(browser, { cwd: options.cwd, registerCapability }) : [])];
    if (this.buildControls) this.localTools = [...this.localTools, ...this.buildControls.tools];
    if (this.fleet) this.localTools = [...this.localTools, ...fleetTools(this.fleet,
      boundFleetSettings(options.identity.role, options.capabilities?.enabled ?? [], options.guidance?.settings).allowedKinds, this.inputWake)];
    const local = this.localTools.map((tool) => tool.name);
    const mcp = options.capabilities?.enabled.includes("mcp.enabled") === true;
    if (mcp !== (options.capabilities?.mcp !== undefined)) {
      throw new Error("module_invalid: mcp.enabled needs its configured settings");
    }
    const search = options.capabilities?.enabled.includes("tool_search") === true;
    this.baseLoadout = [...role.upstreamTools.filter(name => !local.includes(name)), ...corralToolNames(specs), ...local,
      ...(mcp ? MCP_CONTROLS : []), ...(mcp && search ? ["mcp_activate"] : []), ...(search ? ["tool_search"] : []),
      ...(options.capabilities?.enabled.includes("codemode") ? ["codemode"] : [])];
    if (new Set(this.loadout).size !== this.loadout.length) throw new Error("module_invalid: the loadout names a tool twice");
    this.settings = {
      ...restrictedSettings(), ...compactionSettings(options.capabilities, options.guidance),
      enableSkillCommands: options.capabilities?.enabled.includes("skills") === true && options.guidance !== undefined,
      // Corral input (inbox envelopes, agent messages, notices, answers) steers: a run that keeps
      // working never reaches the stop where follow-ups drain, so it would hold them for the whole
      // run. Everything queued by a turn boundary arrives together. Follow-ups remain only the
      // owner's explicit queue-behind-the-run choice and drain as one batch too.
      steeringMode: "all",
      followUpMode: "all",
    };
    try {
      this.controls = parseControls(options.identity.role, options.capabilities?.enabled ?? [], options.guidance?.settings);
    } catch (error) {
      throw new Error(`module_invalid: ${error instanceof Error ? error.message : error}`);
    }
    // Recovery attributes a lost hook only through this record, so it precedes every launch.
    this.hooks = this.controls.hooks && new Hooks(this.controls.hooks, {
      cwd: options.cwd,
      record: (entry) => this.session.sessionManager.appendCustomEntry(HOOK_ENTRY, entry),
    });
    this.controlJob = this.jobs && this.commands ? jobControl({
      jobs: this.jobs, commands: this.commands,
      session: () => !this.closing && this.bound && this.session.sessionId === this.boundSessionId ? this.boundSessionId : undefined,
      allowed: tool => this.loadout.includes(tool) && this.session.getActiveToolNames().includes(tool),
      record: entry => { options.sessionManager.appendCustomEntry("corral_job_control", entry); options.events.emit("job_control", entry); },
      before: (toolCallId, job) => this.hooks?.wraps("job_kill")
        ? this.hooks.before({ tool: "job_kill", toolCallId, input: { job }, ...(this.currentTurn() === undefined ? {} : { turn: this.currentTurn() }) })
        : Promise.resolve(undefined),
      after: async (toolCallId, job, failed) => {
        if (this.hooks?.wraps("job_kill")) await this.hooks.after({ tool: "job_kill", toolCallId, input: { job },
          ...(this.currentTurn() === undefined ? {} : { turn: this.currentTurn() }) }, failed);
      },
    }) : undefined;
    const run = options.identity.run;
    if (options.capabilities?.enabled.includes("checkpoints") === true) {
      if (options.identity.role !== "worker" || !run) throw new Error("module_invalid: checkpoints needs a worker run");
      this.checkpoints = new Checkpoints({
        cwd: options.cwd, run,
        record: (entry) => this.session.sessionManager.appendCustomEntry(CHECKPOINT_ENTRY, entry),
      });
    }
  }

  /**
   * Worker jobs are receipted against their run. Interactive jobs are session-owned; Commands
   * journals their completion and monitor wakes through the session delivery path.
   */
  private jobRuntime(registerCapability: (pid: number) => () => void): JobRuntime | undefined {
    const { options } = this;
    if (options.capabilities?.enabled.includes("background_shell") !== true) return undefined;
    const settings = JobSettingsSchema.safeParse(options.guidance?.settings.background_shell);
    if (options.identity.role === "reviewer" || !settings.success || !options.runtimeDir) {
      throw new Error("module_invalid: background_shell needs an eligible role, settings and a runtime directory");
    }
    return new JobRuntime({
      cwd: options.cwd, runtimeDir: options.runtimeDir,
      settings: settings.data, registerCapability,
      environment: () => ({ ...process.env, PI_SESSION_ID: this.sessionId(), PI_SESSION_FILE: this.session.sessionFile,
        PI_PROVIDER: this.session.model?.provider, PI_MODEL: this.session.model?.id, PI_REASONING_LEVEL: this.session.thinkingLevel }),
      onPrepared: job => options.sessionManager.appendCustomEntry(COMMAND_ENTRY, {
        job, consumed: false,
      } satisfies CommandFact),
      onEvent: (kind, data) => { options.events.emit(kind, data); this.liveEvents.stateChanged(); },
    });
  }

  private stopManagedResources(): Promise<void> {
    return this.managedEffects.stopAll([
      ["mcp", async () => { await this.mcp?.close(); }],
      ["lsp", async () => { await this.additionalLsp?.shutdown(); }],
    ]);
  }

  private exportable(): boolean {
    return this.options.exportDir !== undefined && this.options.guidance !== undefined &&
      this.options.capabilities?.enabled.includes("session_export") === true &&
      this.options.identity.task !== undefined && this.options.client.configured;
  }

  /** `/corral-export`: the current snapshot as one attached offline HTML artifact. */
  async exportSession(): Promise<SessionExport> {
    const { exportDir, identity, client } = this.options;
    if (!this.exportable() || !exportDir || !identity.task) throw new Error("session_export is not enabled for this role");
    if (!this.bound) throw new Error("session is not ready");
    return exportSnapshot(this.session, this.sessionId(), identity.task, exportDir, client);
  }

  /** `/corral-export` and the artifact panel's export action share one gated, journaled path. */
  private async reportedExport(): Promise<{ readonly text: string; readonly committed: boolean }> {
    try {
      const { path } = await this.exportSession();
      this.options.events.emit("session_exported", { path });
      return { text: `corral: session export attached: ${path}`, committed: true };
    } catch (error) {
      const reason = error instanceof Error ? error.message : String(error);
      this.options.events.emit("session_export_failed", { reason: reason.slice(0, 512) });
      throw error;
    }
  }

  get loadout(): readonly string[] { return [...this.baseLoadout, ...this.mcp?.names() ?? []]; }

  private get initialTools(): readonly string[] {
    const tools = this.options.capabilities?.enabled.includes("tool_search") ? this.baseLoadout : this.loadout;
    return tools;
  }

  static async create(options: HostSessionOptions): Promise<HostSession> {
    const host = new HostSession(options);
    host.restoreTurn(options.sessionManager);
    const { sdk } = options;
    const route = PROVIDERS[host.provider];
    const local = host.provider === "llama.cpp" && options.capabilities?.enabled.includes("llama_cpp.enabled") === true
      ? options.capabilities.llama_cpp : undefined;
    if (host.provider === "llama.cpp" && local === undefined) {
      throw new Error("module_invalid: llama.cpp requires its explicit configured capability");
    }
    const extra = (options.fallback ?? []).map(choice => choice.provider)
      .filter(id => (id !== "amazon-bedrock" || options.aws) && (id !== "llama.cpp" || options.capabilities?.llama_cpp));
    const { providers, credentials } = options.routes ?? await sessionRoutes(options.modules.auth, route.id, options.identity.role, extra);
    const models = await createModelCatalog(credentials, { providers,
      ...(providers.every(isApiProvider) ? { modelsPath: null }
        : options.build?.session.bare ? { modelsPath: join(options.build.dir, "..", "..", "models.json") } : {}),
    });
    const modelId = options.model.startsWith(`${route.id}/`) ? options.model.slice(route.id.length + 1) : options.model;
    const report = (usage: ModelUsage) => host.reportUsage(usage);
    for (const id of providers) {
      if (isApiProvider(id)) {
        const send = options.modules.auth.transport?.(authContext(id)) ?? options.apiFetch ?? options.network.fetch;
        host.apiNetworks.push(installApiProvider(models, id, credentials, send, report, options.network.refuse));
        continue;
      }
      if (id === "llama.cpp") {
        const settings = options.capabilities?.llama_cpp;
        if (!settings) throw new Error("module_invalid: llama.cpp requires its explicit configured capability");
        const selected = [...(route.id === id ? [modelId] : []),
          ...(options.fallback ?? []).filter(choice => choice.provider === id).map(choice => choice.model)];
        try { await installLocalProvider(models, settings, selected, options.network.fetch); }
        catch (error) { if (route.id === id) throw error; continue; }
      } else {
        const fetch = PROVIDERS[id].auth === "oauth" ? guardedFetch(options.network.fetch, options.network.refuse, id).fetch : options.network.fetch;
        restrictProvider(models, id, fetch as typeof globalThis.fetch,
          { configDir: join(options.agentDir, "claude-code"), cwd: options.cwd, connection: id }, options.aws);
      }
      const restricted = models.getProvider(id);
      if (!restricted) throw new Error("model provider is unavailable");
      const billing = billingKind(id);
      models.registerNativeProvider({ ...restricted,
        stream: observeUsage(restricted.stream.bind(restricted), billing, report),
        streamSimple: observeUsage(restricted.streamSimple.bind(restricted), billing, report),
      });
    }
    const model = models.getModel(route.id, modelId);
    if (!model) {
      if (local) throw new LocalModelUnavailable(`configured model ${modelId} is not advertised; check the endpoint or set discover = false`);
      throw new Error(`model_unavailable: ${route.id}/${modelId}`);
    }
    if (isApiProvider(route.id)) assertApiEffort(model, options.effort);
    host.auxiliaryModels = models;
    host.summaryRequest = await createSummaryRequest(options, report);
    const interactive = interactiveModels(options.identity.role);
    host.buildProviders = interactive ? providers : [route.id];
    if (options.capabilities?.enabled.includes("web.enabled") && options.capabilities.web?.search_provider === "subscription") {
      const searchProviders = host.buildProviders.filter((id): id is ProviderId => id === "openai-codex" || id === "anthropic");
      const search = await createModelCatalog(credentials, { providers: searchProviders });
      for (const id of searchProviders) {
        const fetch = guardedFetch(options.network.fetch, (destination) => { options.network.refuse?.(destination); }, id).fetch;
        restrictProvider(search, id, fetch as typeof globalThis.fetch,
          { configDir: join(options.agentDir, "claude-search"), cwd: options.cwd, searchOnly: true });
        const restricted = search.getProvider(id);
        if (!restricted) throw new Error("subscription search provider is unavailable");
        search.registerNativeProvider({ ...restricted,
          stream: observeUsage(restricted.stream.bind(restricted), "subscription", usage => host.reportUsage(usage)),
          streamSimple: observeUsage(restricted.streamSimple.bind(restricted), "subscription", usage => host.reportUsage(usage)),
        });
      }
      host.searchModels = search;
    }
    const thinkingLevel = options.effort && Object.hasOwn(THINKING, options.effort)
      ? options.effort as ThinkingLevel : isApiProvider(route.id) ? "off" : undefined;
    const contract = options.modules.role.contract(host.roleContext);
    const factory: CreateAgentSessionRuntimeFactory = async ({ cwd, agentDir, sessionManager, sessionStartEvent }) => {
      const settingsManager = sdk.SettingsManager.inMemory(host.settings);
      const guidance = selectGuidance({
        cwd, agentDir, root: options.root ?? cwd, home: options.home ?? homedir(),
        ...(options.capabilities ? { capabilities: options.capabilities } : {}),
        ...(options.guidance ? { guidance: options.guidance } : {}),
      });
      host.guidance = guidance;
      const services = await sdk.createAgentSessionServices({
        cwd, agentDir, modelRuntime: models, settingsManager,
        resourceLoaderOptions: {
          ...resourceLoaderOptions([(pi: ExtensionAPI) => host.extension(pi)]), appendSystemPrompt: [contract],
          additionalPromptTemplatePaths: [...guidance.promptPaths],
          agentsFilesOverride: () => ({ agentsFiles: guidance.contextFiles.map((file) => ({ ...file })) }),
        },
      });
      const result = await sdk.createAgentSessionFromServices({
        services, sessionManager, model, tools: [...host.baseLoadout, ...host.mcp?.authorizedNames() ?? []],
        ...(sessionStartEvent ? { sessionStartEvent } : {}),
        ...(thinkingLevel ? { thinkingLevel } : {}),
      });
      const { session } = result;
      const switchForFallback = installModelSelection(session, models, interactive);
      if (options.fallback?.length) {
        const agent = session.agent;
        const original = agent.streamFunction;
        const fallback = withSubscriptionFallback(original, options.fallback, async (candidate, signal) => {
          const model = models.getModel(candidate.provider, candidate.model);
          if (!model) return undefined;
          try {
            if (candidate.provider === "amazon-bedrock") await options.aws?.resolver.get();
            await switchForFallback(model, signal);
          } catch (error) {
            // Preflight may find a missing login, failed refresh or absent local SDK.
            if (session.model !== model) return undefined;
            throw error;
          }
          return { model, stream: original, reasoning: session.thinkingLevel === "off" ? undefined : session.thinkingLevel };
        }, (candidate, previous) => {
          const from = `${previous.provider}/${previous.id}`, model = `${candidate.provider}/${candidate.model}`;
          const text = `Quota exhausted on ${from}; continuing on ${model}`;
          void session.sendCustomMessage({ customType: "corral_model_fallback", content: text, display: true }, { triggerTurn: false });
          options.events.emit("model_fallback", { from, model, text });
        });
        agent.streamFunction = (current, context, streamOptions) => streamOptions?.sessionId === session.sessionId
          ? fallback(current, context, streamOptions) : original(current, context, streamOptions);
      }
      result.session.setActiveToolsByName([...host.initialTools]);
      host.watch(result.session);
      host.sessionsCreated++;
      return { ...result, services, diagnostics: services.diagnostics };
    };
    host.runtime = await sdk.createAgentSessionRuntime(factory, {
      cwd: options.cwd, agentDir: options.agentDir, sessionManager: options.sessionManager,
    });
    return host;
  }

  get session(): AgentSession {
    return this.runtime.session;
  }

  sessionId(): string {
    return this.boundSessionId ?? this.session.sessionId;
  }

  get bound(): boolean {
    return this.boundSessionId !== undefined;
  }

  currentTurn(): number | undefined {
    return this.open ? this.turn : undefined;
  }

  private async extension(api: ExtensionAPI): Promise<void> {
    residentLifecycle(api);
    const pi = api;
    this.dialogPi = pi;
    registerCacheStats(pi);
    if (this.options.quickSummary) pi.on("session_before_compact", async event => {
      try {
        const model = this.session.model;
        if (!model) throw new Error("model_unavailable: quick summary has no session model");
        const request = this.summaryRequest ?? { model, effort: this.session.thinkingLevel,
          stream: ((model, context, options) => this.session.modelRuntime.streamSimple(model, context, options as never)) as StreamFunction };
        return { compaction: await summarizeQuick(this.options.sdk, event.preparation, request,
          event.signal, this.sessionId(), event.customInstructions) };
      } catch (error) {
        // Pi swallows thrown extension errors and would otherwise summarize with its default model.
        this.options.events.emit("quick_summary_failed", { reason: error instanceof Error ? error.message : String(error) });
        return { cancel: true };
      }
    });
    pi.on("model_select", event => {
      this.modelChanged(`${event.model.provider}/${event.model.id}`,
        this.session.thinkingLevel, event.source !== "restore");
    });
    pi.on("thinking_level_select", event => {
      const model = this.session.model;
      if (model) this.modelChanged(`${model.provider}/${model.id}`, event.level);
    });
    if (this.ownerQuestions) {
      this.questionUi = new OwnerQuestionUi(this.ownerQuestions, () => this.bound && !this.closing,
        message => { this.session.sessionManager.appendCustomMessageEntry(message.customType, message.content, message.display, undefined); },
        () => this.session.model?.api === CLAUDE_SDK_API, () => this.dialogPresentation());
      this.questionUi.install(pi);
      this.panels?.attachQuestions(this.questionUi.dock());
    }
    // Follows the session's current model, so a mid-session model switch changes the overlay too.
    const calibration = this.options.modules.role.calibration;
    if (calibration) pi.on("before_agent_start", event => {
      const model = this.runtime?.session.model;
      const text = model ? calibration(model.provider, model.id) : undefined;
      if (text) event.systemPromptOptions.sections.model_calibration = text;
      else delete event.systemPromptOptions.sections.model_calibration;
    });
    pi.on("session_start", () => {
      if (this.bound) this.commands?.bound();
      this.liveEvents.stateChanged();
    });
    const code = this.code;
    if (code) {
      pi.registerTool(codemodeTool(code, (names) =>
        names.filter((name) => executionRefusal(name, true, new Set(this.loadout)) === undefined)));
      pi.on("session_start", () => { code.bound(); });
      pi.on("session_shutdown", async (event) => {
        if (event.reason === "reload") await code.rebind();
        else await code.close();
      });
    }
    if (this.additionalLsp) {
      if (this.managedEffects.newLifetime("lsp")) this.additionalLsp = this.additionalLspFactory?.();
      pi.on("session_start", () => { void this.additionalLsp?.warm(); });
      pi.on("session_shutdown", async () => {
        await this.managedEffects.stop("lsp", async () => { await this.additionalLsp?.shutdown(); });
      });
    }
    const settings = this.options.capabilities?.mcp;
    const search = this.options.capabilities?.enabled.includes("tool_search") === true;
    if (settings) {
      this.managedEffects.newLifetime("mcp");
      this.mcp = new McpAccess(settings, this.options.identity.role, this.options.identity.project,
        this.options.cwd, this.options.network.mcp);
      await this.mcp.refresh();
      this.mcp.attach(pi, search);
      const access = this.mcp;
      pi.on("session_shutdown", async () => {
        await this.managedEffects.stop("mcp", () => access.close());
      });
    }
    if (search) pi.registerTool(toolSearch(pi, (name) => this.loadout.includes(name)));
    const specs = this.options.modules.role.tools(this.roleContext);
    const corral = new Set([...corralToolNames(specs), ...(this.jobs && this.options.identity.role === "worker" ? [JOB_MONITOR_TOOL] : [])]);
    const rich = this.options.ui !== "plain";
    const role = this.options.identity.role;
    this.ledger = this.loadout.includes("corral_todo")
      ? new TodoLedger(this.options.identity, async () => this.local.todoSnapshot()) : undefined;
    this.ledger?.extend(pi);
    for (const spec of specs) {
      const tool = defineCorralTool(spec, {
        client: this.options.client, identity: this.options.identity,
        drained: () => this.drained(),
        todo: async payload => {
          const result = this.local.todo(payload);
          await this.ledger?.restore();
          return result;
        },
        checkpoint: note => {
          this.session.sessionManager.appendCustomEntry("corral_checkpoint_note", { turn: this.turn, note }, { immediate: true });
          return { note, turn: this.turn };
        },
      });
      pi.registerTool(role === "worker" && spec.op === "done" && this.fleet
        ? workerDoneTool(tool, {
          attachArtifact: async ({ path, kind, title }) => {
            const task = this.options.identity.task;
            if (!task) throw new Error("No task is attached to this session");
            await this.options.client.artifact(task, path, kind, title);
          },
          task: this.options.identity.task, run: this.options.identity.run, fleet: () => this.fleet!.snapshot(),
          recordDir: join(this.options.runtimeDir ?? join(this.options.agentDir, "runtime"), "session-records"),
          ...(this.options.sessionDir && this.options.client.configured ? { usageArtifact: () => this.usageArtifact() } : {}),
        }) : tool);
    }
    if (rich) {
      installChrome(pi, {
        identity: this.options.identity, provider: this.provider, model: this.options.model,
        cwd: this.options.cwd, ...(this.options.effort ? { effort: this.options.effort } : {}),
      }, this.options.events, () => this.bound, this.options.sdk.Theme);
      if (this.ledger) installPlanCommand(pi, this.ledger, true, () => this.panels?.planSurface());
      this.panels?.install(pi, this.options.sdk);
      this.markRows = installMarkRows(pi);
      // Every other tool call is one kit row; build cards and corral operations keep their renderers.
      const panels = this.panels;
      const openView: ToolRenderDeps["openView"] = panels ? (view, select) => panels.openView(view, select) : undefined;
      installToolRenderers(pi, {
        cwd: this.options.cwd, ticker: () => this.panels?.ticker, corralSpecs: specs,
        job: id => this.commands?.inspection().find(job => job.job === id),
        code: () => this.code, ledger: () => this.ledger, ...(openView ? { openView } : {}),
      });
      const unsubscribe = this.ledger?.onChange(() => { this.liveEvents.stateChanged(); this.panels?.refresh(); });
      pi.on("session_shutdown", (event) => {
        unsubscribe?.();
        if (event.reason !== "reload") { this.unsubscribePanels?.(); this.panels?.dispose(); }
      });
    }
    for (const tool of this.localTools) {
      const cards = rich && this.buildControls?.tools.includes(tool) ? buildToolRenderers(tool.name, () => this.panels?.buildCards()) : undefined;
      pi.registerTool(cards ? { ...tool, ...cards } : tool);
    }
    // One execution check for model-issued and nested calls; a Code Mode child it refuses is
    // reported as denied with no effect.
    pi.on("tool_call", async (event) => {
      const allowed = new Set(this.loadout);
      const nested = event.parentToolCallId !== undefined;
      const child = nested ? nestedChild(event.toolName, event.toolCallId) : undefined;
      const reason = executionRefusal(event.toolName, nested, allowed);
      if (!reason) return undefined;
      if (child) child.denied = reason;
      this.deniedTools++;
      this.options.events.emit("tool_denied", { tool: event.toolName, ...(nested ? { nested: true } : {}) });
      return {
        block: true,
        reason,
      };
    });
    const fixed = this.options.modules.role.shellRefusal;
    installToolControls(pi, {
      ...(fixed ? { fixed } : {}),
      ...(this.hooks ? { hooks: this.hooks } : {}),
      ...(this.controls.bashRules ? { rules: this.controls.bashRules } : {}),
      confirmBash: command => this.confirmBash(command),
      events: this.options.events, corral, currentTurn: () => this.currentTurn(),
    });
    if (!this.loadout.includes("bash")) {
      pi.on("user_bash", () => ({
        result: { output: "bash is not in this role's loadout", exitCode: 1, cancelled: false, truncated: false },
      }));
    }
    // Corral observes, recovers and resumes only the file hello bound, so while a binding is active
    // every in-process replacement (/new, /resume, /fork, import) is refused before it happens.
    const refuse = (operation: string, ui: { notify(message: string, type?: "info" | "warning" | "error"): void }) => {
      if (!this.bound) return undefined;
      const message = `corral: ${operation} would replace this session transcript; restart corral-pi to open a different session.`;
      this.options.events.emit("session_replacement_refused", { operation, session_id: this.boundSessionId, message });
      this.notice(message, "warning", ui);
      return { cancel: true };
    };
    pi.on("session_before_switch", (event, ctx) => {
      return refuse(event.reason === "new" ? "a new session" : "switching or importing a session", ctx.ui);
    });
    pi.on("session_before_fork", (_event, ctx) => refuse("forking the session", ctx.ui));
    if (this.checkpoints) pi.registerCommand("rewind", {
      description: "Restore the worktree and conversation to the start of a turn; /rewind --list",
      handler: async (args, ctx) => {
        try {
          const input = parseRewindCommand(args);
          const result = await this.rewind(input);
          if ("list" in input) this.notice(JSON.stringify(result, null, 2), "text", ctx.ui);
          else this.notice(`Rewound to the start of turn ${input.turn}. Saved work: ${result.rescue_ref}`, "text", ctx.ui);
        } catch (error) {
          this.notice(error instanceof Error ? error.message : String(error), "error", ctx.ui);
        }
      },
    });
    if (this.buildControls) {
      const { exit, board } = this.buildControls;
      pi.registerCommand("finish", { description: "Choose what happens to this build branch", handler: async () => { if (!this.panels?.finish()) exit.open(); } });
      if (board) pi.registerCommand("board", { description: "Refresh this build session's board tasks", handler: async () => { await board.refresh(); this.panels?.showBoard(); } });
    }
    if (this.exportable()) {
      pi.registerCommand(EXPORT_COMMAND, {
        description: "Attach this session's current branch to corral as an offline HTML artifact",
        handler: async (_args, ctx) => {
          try {
            const { text, committed } = await this.reportedExport();
            this.notice(text, committed ? "text" : "warning", ctx.ui);
          } catch (error) {
            this.notice(`corral: session export failed: ${error instanceof Error ? error.message : String(error)}`, "error", ctx.ui);
          }
        },
      });
    }
    pi.on("input", async (event) => {
      if (this.closing || this.rewinding) return { action: "handled" as const };
      if (this.open && event.source === "interactive") this.ownerInput = true;
      else this.lastInputSource = event.source;
      // A build session's record keeps what the owner asked for, verbatim.
      if (event.source === "interactive" && !event.text.startsWith("/")) {
        if (await this.options.paneInput?.(event.text, event.images)) return { action: "handled" as const };
        this.buildControls?.noteOwnerInput(event.text);
      }
      return { action: "continue" as const };
    });
    this.options.modules.role.extend?.(pi, this.roleContext);
    // Registered after the role's own handler: upstream keeps the first skill of each name, so
    // a home or repository skill can never replace the role's packaged one (corral-worker).
    pi.on("resources_discover", () => ({ skillPaths: [...this.guidance.skillPaths] }));
    pi.on("session_shutdown", async (event) => {
      // InteractiveMode exits the process after runtime disposal; main's finally never runs.
      // Await the release here, but keep reporting through extension reloads.
      if (event.reason === "quit") await this.options.herdr?.close();
      if (event.reason === "reload") await this.auxiliary?.cancelAll(); else await this.auxiliary?.close();
      if (event.reason === "reload") await this.fleet?.cancelAll(); else await this.fleet?.close();
      this.commands?.pause();
      await this.jobs?.shutdown();
    });
  }

  /**
   * Presentation cannot affect prompts, session state, or tool results.
   */
  private addMark(mark: TranscriptMark): boolean {
    if (mark.kind === "note" && this.attached) this.liveEvents.note(mark.tone, mark.text, mark.at);
    return recordTranscriptMark(this.options.marks ?? this.markRows, mark, {
      attached: this.attached, plain: this.options.ui === "plain", bound: this.bound,
      build: this.options.build !== undefined || this.options.identity.role === "build",
      headless: this.options.marks ?? this.headlessMarkRows,
    });
  }

  private notice(text: string, tone: "text" | "warning" | "error",
    ui: { notify(message: string, type?: "info" | "warning" | "error"): void }): void {
    if (!this.addMark({ kind: "note", at: Date.now(), tone, text })) ui.notify(text, tone === "text" ? "info" : tone);
  }

  private modelChanged(model: string, effort: string, record = true): void {
    const was = this.selection;
    this.selection = { model, effort };
    this.liveEvents.stateChanged();
    if (!record || !was || was.model === model && was.effort === effort) return;
    // Rebuilding a session does not emit marks. Initial model/effort setup is not a change after a reply.
    if (!this.session.sessionManager.getEntries().some(entry => entry.type === "message" && entry.message.role === "assistant")) return;
    this.addMark({ kind: "model", at: Date.now(), model, effort, was });
  }

  private currentWaits(): readonly TurnWait[] {
    const cells = this.code?.state().cells;
    const ownerKeys = this.options.herdr?.readback().blocked_on;
    return turnWaits({
      jobs: this.commands?.inspection(),
      questions: this.ownerQuestions?.list(),
      ownerKeys: Array.isArray(ownerKeys) ? ownerKeys.filter((key): key is string => typeof key === "string") : [],
      cells: Array.isArray(cells) ? cells.flatMap((cell: unknown) => {
        if (typeof cell !== "object" || cell === null || !("cell" in cell) || typeof cell.cell !== "string"
          || !("detached" in cell) || cell.detached !== true) return [];
        return [{ cell: cell.cell, detached: true }];
      }) : [],
      children: [
        ...(this.auxiliary?.inspection() ?? []).map(child => ({ id: child.request, state: child.state, label: `${child.kind} ${child.request.slice(0, 12)}` })),
        ...((this.buildControls?.fleet ?? this.fleet)?.snapshot().children ?? []).map(child => ({ id: child.id, state: child.state, label: `agent ${child.id}: ${child.title}` })),
      ],
      events: [
        ...[...this.monitorFacts.values()].filter(fact => !fact.result && !fact.delivered)
          .map((fact): TurnWait => ({ kind: "event", id: fact.key, label: `monitor ${fact.key} on ${fact.job}` })),
      ],
    });
  }

  private watch(session: AgentSession): void {
    this.unsubscribe?.();
    this.unobserveTranscript?.();
    this.unobserveTranscript = observeTranscriptEntries(session.sessionManager,
      () => this.liveEvents.entry(), () => this.liveEvents.stateChanged());
    this.startedInputFacts = undefined;
    this.remoteUsage = undefined;
    session.setQueueObserver({
      ...this.queue.observer,
      lifecycle: event => {
        this.queue.observer.lifecycle(event);
        const facts = this.queue.inputFacts(event.tag);
        // Pi reports "started" just before prompting with the prepared user message,
        // which it emits before draining any queued inputs.
        if (event.event === "started") this.startedInputFacts = facts;
        else if (event.event === "rejected" && this.startedInputFacts === facts) {
          this.startedInputFacts = undefined;
        }
      },
      queue: change => {
        this.queue.observer.queue(change);
        if (change.action === "enqueue") {
          for (const { message, tag } of change.entries) {
            const facts = this.queue.inputFacts(tag);
            if (facts !== undefined) this.queuedInputFacts.set(message, facts);
          }
          // Wake waits only after the synchronous queue mutation has returned.
          if (change.mode === "steer") queueMicrotask(() => this.inputWake.notify());
        }
      },
    });
    this.terminalInputSequence = 0;
    this.terminalInput = undefined;
    observeTerminalInput(session, this.options.events);
    // Pi's Esc path calls abort directly, before a cancelled retry emits its final event.
    const abort = session.abort.bind(session);
    session.abort = () => {
      if (this.open) this.stopCause ??= this.closing ? "host" : "owner";
      return abort();
    };
    this.panelStats = undefined;
    this.selection = session.model ? { model: `${session.model.provider}/${session.model.id}`, effort: session.thinkingLevel } : undefined;
    this.unsubscribe = session.subscribe((event) => {
      if (event.type === "message_end") {
        this.panelStats = undefined;
        if (event.message.role === "assistant" && event.message.stopReason !== "aborted") {
          this.turnError = event.message.stopReason === "error" ? utf8Prefix(event.message.errorMessage || "provider_error", 512) : null;
        }
        if (event.message.role === "assistant" && event.message.stopReason === "aborted") {
          this.stopCause ??= this.closing ? "host" : "owner";
        }
        if (event.message.role === "assistant" && this.recordsTurnMarks && this.open) {
          const message = event.message;
          const miss = message.stopReason === "error" || message.stopReason === "aborted" ? undefined
            : this.options.sdk.detectCacheMiss(session.sessionManager.getEntries(), message, session.modelRuntime);
          this.turnFacts.assistant(this.turn, message.stopReason,
            message.content.flatMap(part => part.type === "toolCall" ? [part.id] : []),
            miss && (miss.missedTokens >= 20_000 || miss.missedCost >= 0.1) ? miss.missedTokens : 0);
        }
        if (event.message.role === "user") {
          // Pi persists this exact message immediately after notifying its listeners.
          const queued = this.queuedInputFacts.get(event.message);
          const tagged = queued ?? this.startedInputFacts;
          if (!queued) this.startedInputFacts = undefined;
          Object.assign(event.message, { corralInput: tagged?.facts ?? { origin: "auto" },
            ...(tagged?.transcriptId === undefined ? {} : { corralRequestId: tagged.transcriptId }) });
          const content = event.message.content;
          this.ownerQuestions?.observeUser(typeof content === "string" ? content
            : content.map(p => p.type === "text" ? p.text : "").join(""));
        }
      }
      if (event.type === "agent_start") {
        // A retry or a pre-settlement continuation can start another run before agent_settled.
        if (this.open) this.turnFacts.finish(this.turn);
        this.turnStarted();
      } else if (event.type === "agent_end" && this.recordsTurnMarks) {
        this.turnFacts.end(this.turn, Date.now(), session.getSessionStats().cost, this.currentWaits(),
          { stopCause: this.stopCause, error: !!this.turnError });
      } else if (event.type === "tool_execution_start") {
        this.turnFacts.toolStarted(this.turn, event.toolCallId);
      } else if (event.type === "agent_settled") {
        this.turnEnded(event.aborted);
        this.remoteBuild.refreshExit();
      } else if (event.type === "queue_update") {
        this.options.events.emit("queue", { steering: event.steering.length, follow_up: event.followUp.length });
      } else if (event.type === "message_end" && event.message.role === "assistant" && event.message.stopReason === "error") {
        const message = event.message.errorMessage ?? "";
        const provider = isProviderId(event.message.provider) ? event.message.provider : this.provider;
        const failure = messageFailure(event.message, provider);
        this.report("provider_error", { class: provider === "amazon-bedrock" || provider === "llama.cpp"
          ? providerErrorClass(message) : failureClass(failure),
          ...(provider === "amazon-bedrock" || provider === "llama.cpp" ? {} : { failure }) });
      }
      this.liveEvents.observe(event, this.turn);
    });
    this.options.events.emit("session_bound", { session_id: session.sessionId });
  }

  private turnStarted(): void {
    this.turn++;
    this.session.sessionManager.appendCustomEntry("corral_turn", this.turn, { immediate: true });
    this.open = true;
    this.turnError = null;
    this.stopCause = undefined;
    if (this.recordsTurnMarks) this.turnFacts.start(this.turn, Date.now(), this.session.getSessionStats().cost);
    const origin: TurnOrigin = this.nextTurnOrigin
      ?? (this.lastInputSource === "interactive" ? { kind: "owner_terminal" } : { kind: "unknown" });
    this.nextTurnOrigin = undefined;
    this.lastInputSource = undefined;
    this.ownerInput = false;
    // Before the prompt is recorded: its parent is the cursor a rewind to this turn restores.
    if (this.bound) this.checkpoint("start");
    this.report("turn_started", { origin, provider: this.session.model?.provider, model: this.session.model?.id });
    this.options.herdr?.turnStarted();
  }

  private turnEnded(aborted: boolean): void {
    if (!this.open) return;
    if (aborted) this.stopCause ??= this.closing ? "host" : "owner";
    this.open = false;
    const cancelled = this.cancelRequested === this.turn;
    this.cancelRequested = undefined;
    const ownerInput = this.ownerInput;
    this.ownerInput = false;
    if (this.bound) this.checkpoint("end");
    // agent_end normally captured the run already. A startup abort can settle without it.
    if (this.recordsTurnMarks) this.turnFacts.end(this.turn, Date.now(), this.session.getSessionStats().cost, this.currentWaits(),
      { stopCause: this.stopCause, error: !!this.turnError });
    this.report("turn_ended", { aborted, cancelled, owner_input: ownerInput });
    this.options.herdr?.turnEnded();
    this.commands?.flush();
    this.turnFacts.finish(this.turn);
    const due = this.turnWaiters.filter(waiter => waiter.from <= this.turn);
    this.turnWaiters = this.turnWaiters.filter(waiter => waiter.from > this.turn);
    for (const waiter of due) waiter.settle(aborted);
  }

  private checkpoint(phase: "start" | "end"): void {
    if (!this.checkpoints) return;
    const leaf = phase === "start" ? this.session.sessionManager.getLeafId() : undefined;
    const entry = this.checkpoints.capture(phase, this.turn, leaf);
    this.options.events.emit(entry.error ? "checkpoint_failed" : "checkpoint", {
      id: entry.id, phase, ...(entry.commit ? { commit: entry.commit } : {}), ...(entry.error ? { error: entry.error } : {}),
    });
  }

  private report(kind: "turn_started" | "turn_ended" | "provider_error", data: Record<string, unknown>): void {
    this.options.events.emit(kind, { turn: this.turn, ...data });
  }

  /** A child-targeted composer never submits its text to the parent conversation. */
  async deliverBuildInput(text: string): Promise<boolean> {
    const controls = this.buildControls;
    const target = controls?.ui.target();
    if (!controls || target === "parent" || !target || this.closing || !this.bound) return false;
    return controls.fleet.steer(target, text);
  }

  async deliverRemoteChildInput(child: string, text: string): Promise<boolean> {
    const fleet = this.buildControls?.fleet ?? this.fleet;
    if (!fleet || this.closing || this.rewinding || !this.bound || !(await fleet.steer(child, text))) return false;
    this.addMark({ kind: "child", at: Date.now(), act: "steered", child, text });
    return true;
  }

  get servesChildren(): boolean { return this.buildControls !== undefined || this.fleet !== undefined || this.auxiliary !== undefined; }

  childTranscript(child: string): string | undefined {
    const fleet = this.buildControls?.fleet ?? this.fleet;
    const current = fleet?.snapshot().children.find(entry => entry.id === child)?.sessionFile;
    if (current) return current;
    const helper = this.auxiliary?.inspection().find(entry => entry.request === child && entry.parent === this.boundSessionId);
    if (helper?.sessionFile) return helper.sessionFile;
    // A stopped build's fleet is not restarted, but its child files remain session-owned.
    return this.options.build ? join(this.options.build.dir, "children", `${child}.jsonl`) : undefined;
  }

  private get recordsTurnMarks(): boolean { return this.attached === "headless" || this.options.ui !== "plain"; }

  get remoteActionsReady(): boolean { return this.bound && !this.closing && !this.rewinding; }

  remoteUiContext() {
    return this.remoteDialogs.context(this.session.extensionRunner.getUIContext(), (text, tone) => {
      this.addMark({ kind: "note", at: Date.now(), text, tone: tone === undefined || tone === "info" ? "text" : tone });
    });
  }

  remoteCardEntries(): CardEntry[] {
    return [...questionEntries(this.ownerQuestions), ...this.remoteDialogs.entries(), ...this.remoteBuild.entries()];
  }

  absentCard(id: string): AnswerStatus {
    if (this.remoteDialogs.isExpired(id)) return "expired";
    return questionStatus(this.ownerQuestions?.status(id) ?? "missing");
  }

  markAttached(mode: "interactive" | "headless"): void {
    this.attached = mode;
    this.remoteBuild.refreshExit();
    this.liveEvents.stateChanged();
    this.options.events.emit(mode);
  }

  private usageArtifact(): string {
    const path = join(this.options.sessionDir!, "usage.json");
    const { tokens } = this.session.getSessionStats();
    writeFileSync(path, `${JSON.stringify({ transcriptTokens: tokens }, null, 2)}\n`, { mode: 0o600 });
    return path;
  }

  private reportUsage(usage: ModelUsage): void {
    if (!this.options.client.configured) return;
    this.calls = this.calls.then(() => this.options.client.usage(usage)).catch((error: unknown) => {
      const reason = error instanceof Error ? error.message : String(error);
      this.options.events.emit("usage_report_failed", { reason: reason.slice(0, 512) });
    });
  }

  drained(): Promise<unknown> {
    return this.calls;
  }

  /** Native panels frame the dialog and give its row budget; every other host keeps the plain dialog. */
  private dialogPresentation(): { surface: "panels"; rows: () => number } | { surface: "plain" } {
    const panels = this.panels;
    return panels ? { surface: "panels", rows: () => panels.dialogRows() } : { surface: "plain" };
  }


  /** Every detached Code Mode cell's notice started so far has been delivered or refused. */
  async codeNoticed(): Promise<void> {
    await this.code?.notified();
  }

  close(): Promise<void> {
    closeResidentSession(this.sessionId());
    if (this.open) this.stopCause ??= "host";
    this.closing = true;
    this.remoteDialogs.close();
    this.liveEvents.stateChanged();
    // Close both resource admission barriers synchronously, even if another cleanup fails.
    const resources = this.stopManagedResources();
    void resources.catch(() => {}); // observed below, without an early unhandled rejection
    return this.closeResult ??= this.closeOwned(resources).finally(() => {
      this.unsubscribe?.();
      this.unobserveTranscript?.();
      this.liveEvents.close();
    });
  }

  private async closeOwned(resources: Promise<void>): Promise<void> {
    this.questionUi?.close();
    this.commands?.pause();
    this.unsubscribePanels?.();
    this.panels?.dispose();
    const released = this.options.herdr?.close();
    // Stop the parent's follow-up loop before cancelled children can return tool results.
    await this.code?.close();
    const aborted = this.session.abort();
    await this.auxiliary?.close();
    await this.buildControls?.close();
    await this.fleet?.close();
    await this.jobs?.shutdown();
    await aborted;
    await resources;
    this.session.dispose();
    await released;
  }

  private restoreTurn(manager: SessionManager): void {
    for (const entry of manager.getEntries()) {
      if (entry.type === "custom" && entry.customType === "corral_turn" && typeof entry.data === "number"
        && Number.isSafeInteger(entry.data) && entry.data > this.turn) this.turn = entry.data;
    }
  }

  /** Open the local transcript and runtime. */
  async startLocal(): Promise<void> {
    const { sessionId, sessionFile, sessionManager } = this.session;
    if (!sessionFile) throw new Error("the Pi session has no transcript path");
    // Pi defers a new transcript until the first message unless an entry is flushed explicitly.
    if (!existsSync(sessionFile)) sessionManager.appendCustomEntry("corral_host_started", { pid: process.pid }, { immediate: true });
    this.boundSessionId = sessionId;
    const commandFacts: CommandFact[] = [], monitorFacts: MonitorFact[] = [];
    for (const entry of this.session.sessionManager.getEntries()) {
      if (entry.type === "custom" && entry.customType === COMMAND_ENTRY) commandFacts.push(entry.data as CommandFact);
      if (entry.type === "custom" && entry.customType === MONITOR_ENTRY) monitorFacts.push(entry.data as MonitorFact);
    }
    this.options.herdr?.bind({ sessionId, sessionFile });
    this.options.events.emit("bound", { session_id: sessionId });
    this.commands?.bound(commandFacts, monitorFacts);
    await this.questionUi?.recover();
  }

  private commandNoticeSeen(key: string): boolean {
    const marker = `[corral notice ${key}]`;
    return [...this.session.getFollowUpMessages(), ...this.session.getSteeringMessages(),
      ...this.session.messages.flatMap(message => message.role === "user" ? [typeof message.content === "string" ? message.content
        : message.content.filter(part => part.type === "text").map(part => part.text).join("\n")] : [])]
      .some(text => text.split("\n").includes(marker));
  }

  private async commandNotice(key: string, text: string): Promise<string> {
    if (this.commandNoticeSeen(key)) return "duplicate";
    const sent = await this.submitAutomated(`[corral notice ${key}]\n${text}`);
    return sent.disposition;
  }

  private submitAutomated(text: string): Promise<SubmitResult> {
    const input: SessionInput = { text, busyPolicy: "steer", origin: "auto" };
    return this.options.automatedInput?.(input) ?? this.submitLocal(input);
  }

  submitLocal(input: SessionInput, images?: ImageContent[], source?: {
    requestId?: string; queueKey?: string; text?: string; pane?: boolean;
  }): Promise<SubmitResult> {
    if (!this.bound || this.closing) return Promise.resolve({ disposition: "rejected", reason: "host_not_ready" });
    if (this.rewinding) return Promise.resolve({ disposition: "rejected", reason: "rewind_in_progress" });
    if (input.origin === "owner") this.buildControls?.noteOwnerInput(source?.text ?? input.text);
    const socketId = input.inputId ?? source?.requestId;
    const transcriptId = input.inputId ?? (input.origin === "owner" ? source?.requestId : undefined);
    const tag = this.queue.bridgeInput(input, { ...source, socketId, transcriptId });
    return this.promptInput(input, tag, images);
  }

  private promptInput(input: SessionInput,
    queueTag: unknown, images?: ImageContent[]): Promise<SubmitResult> {
    const { promise, resolve } = Promise.withResolvers<SubmitResult>();
    this.session.prompt(input.text, {
      source: "rpc", queueTag, ...(images ? { images } : {}),
      ...(input.busyPolicy === "steer" ? { streamingBehavior: "steer" as const } : {}),
      ...(input.busyPolicy === "follow_up" ? { streamingBehavior: "followUp" as const } : {}),
      preflightResult: (disposition) => {
        if (disposition !== "started") {
          resolve({ disposition });
          return;
        }
        this.nextTurnOrigin = { kind: "bridge" };
        resolve({ disposition, turn: this.turn + 1 });
      },
    }).catch((error: unknown) => {
      const message = error instanceof Error ? error.message : String(error);
      resolve({ disposition: "rejected", reason: /already processing/i.test(message) ? "busy" : message.slice(0, 256) });
    });
    return promise;
  }
  private confirmBash(command: string): string | undefined {
    const questions = this.ownerQuestions;
    const text = `Run this command?\n${command}`;
    if (!questions || text.length > 1000) return "bash rule requires owner confirmation; use a shorter command; nothing ran";
    const id = this.bashQuestions.get(command);
    if (id) {
      const prior = questions.get(id);
      if (prior?.status === "pending") return `Waiting for owner question ${id}; nothing ran`;
      this.bashQuestions.delete(command);
      if (prior?.answers?.[0]?.option === "allow") return;
      return "The owner did not allow this command; nothing ran";
    }
    const question = questions.ask(crypto.randomUUID(), [{ question: text, options: ["allow", "deny"], freeText: false }], "bash");
    this.bashQuestions.set(command, question.id);
    return `Waiting for owner question ${question.id}; nothing ran`;
  }

  pendingOwnerQuestions() {
    return this.ownerQuestions?.pending().flatMap(record => {
      const question = record.questions[0];
      return question && record.questions.length === 1 ? [{ id: record.id, question: question.question,
        options: question.options, free_text: question.freeText !== false, status: "pending" }] : [];
    }) ?? [];
  }

  async answerQuestion({ question, option, text }: AnswerInput, before: () => Promise<unknown>): Promise<string> {
    const pending = this.ownerQuestions;
    const record = pending?.get(question);
    if (!pending || !record) return "missing";
    if (record.status !== "pending") return pending.status(question);
    const choice = record.questions[0];
    if (!choice || record.questions.length !== 1 || !validAnswer(choice, { option, text })) {
      throw new BridgeRefusal("invalid_question_answer", "choose a listed option or use free text if allowed");
    }
    return pending.answer(question, [{ option, text }], before);
  }

  async replyToQuestion(text: string): Promise<boolean> {
    const pending = this.ownerQuestions;
    const records = pending?.pending() ?? [];
    const record = records[0], question = record?.questions[0];
    if (!pending || !record || records.length !== 1 || !question || record.questions.length !== 1 || text.startsWith("/")) return false;
    const option = text.trim();
    const answer = question.options.includes(option) ? { option } : { text };
    if (!validAnswer(question, answer)) return false;
    await pending.answer(record.id, [answer]);
    return true;
  }

  /** Select a model in the provider catalog already configured for this process. */
  async selectRemoteModel(name: string, effort?: string): Promise<void> {
    if (this.open || this.closing) throw new Error("remote_model_selection_unavailable");
    const slash = name.indexOf("/");
    const provider = slash < 0 ? this.session.model?.provider ?? this.provider : name.slice(0, slash);
    const id = slash < 0 ? name : name.slice(slash + 1);
    const model = this.auxiliaryModels?.getModel(provider, id);
    if (!model) throw new Error(`model_unavailable: ${name}`);
    if (effort !== undefined && !Object.hasOwn(THINKING, effort)) throw new Error("thinking_level_invalid");
    if (isProviderId(provider) && isApiProvider(provider)) assertApiEffort(model, effort);
    await this.session.setModel(model, { persist: false });
    if (effort !== undefined) this.session.setThinkingLevel(effort as ThinkingLevel, { persist: false });
    this.liveEvents.stateChanged();
  }

  /** Cause of the latest run's stop, cleared when the next run starts. */
  latestStopCause(): TurnStopCause | undefined { return this.stopCause; }

  async cancel(turn: number | undefined): Promise<Record<string, unknown>> {
    return this.cancelRun(turn, "owner");
  }

  private async cancelRun(turn: number | undefined, cause: TurnStopCause): Promise<Record<string, unknown>> {
    if (!this.open) return { cancelled: false, reason: "idle" };
    if (turn !== undefined && turn !== this.turn) return { cancelled: false, reason: "turn_not_current", turn: this.turn };
    this.stopCause ??= cause;
    this.cancelRequested = this.turn;
    const cancelled = this.turn;
    this.options.events.emit("cancel", { turn: cancelled });
    await this.session.abort();
    return { cancelled: true, turn: cancelled };
  }

  async rewind(input: RewindInput): Promise<Record<string, unknown>> {
    const snapshots = this.checkpoints;
    if (!snapshots || !this.bound || this.closing) {
      throw new BridgeRefusal("rewind_unavailable", "rewind is available in a running worker with turn checkpoints");
    }
    if (this.rewinding) throw new BridgeRefusal("busy", "a rewind is already in progress");
    const session = this.session;
    const manager = session.sessionManager;
    if ("list" in input) return listRewinds(manager);
    const point = rewindPoint(manager, input.turn);
    this.rewinding = true;
    try {
      session.clearQueue();
      if (this.open) await this.cancel(undefined);
      else await session.abort();
      const result = restoreRewind(session, snapshots, point);
      this.turn = 0;
      this.restoreTurn(manager);
      this.turnError = null;
      this.stopCause = undefined;
      this.bashQuestions.clear();
      for (const question of this.ownerQuestions?.list() ?? []) {
        if (question.status === "pending") this.options.herdr?.questionChanged(question.id, false);
      }
      this.ownerQuestions?.restore();
      this.options.events.emit("rewound", { ...result });
      this.liveEvents.rewound(result.turn);
      this.options.transcriptRewound?.();
      const file = manager.getSessionFile()!;
      try { recordRewind(file, result); }
      catch (error) { throw new Error(`rewound but could not record history; saved work is at ${result.rescue_ref}: ${String(error)}`); }
      return { ...result };
    } finally {
      this.rewinding = false;
    }
  }

  private fleetChanged(): void {
    this.remoteFleetDirty = true;
    this.liveEvents.stateChanged();
    this.panels?.refresh();
  }

  private readRemoteFleet(): FleetSnapshot | undefined {
    if (this.remoteFleetDirty) {
      this.remoteFleetDirty = false;
      this.remoteFleet = (this.buildControls?.fleet ?? this.fleet)?.snapshot();
      this.liveEvents.observeFleet(this.remoteFleet, this.auxiliary?.inspections());
    }
    return this.remoteFleet;
  }

  remoteState(owner: RemoteOwner): StateProjection {
    const session = this.session;
    const build = this.buildControls;
    const effort_levels = session.supportsThinking() ? session.getAvailableThinkingLevels() : [];
    const model = session.model ? utf8Prefix(`${session.model.provider}/${session.model.id}`, 512) : undefined;
    const { queue, queue_total } = owner.queueBrief();
    const entries = sortedCards(this.remoteCardEntries());
    const cards = entries.slice(0, 8).map(entry => cardBrief(entry.card));
    const exitState = this.remoteBuild.exitState();
    const exit = exitState ? exitBrief(exitState) : undefined;
    const board = build?.board ? boardBrief(build.board) : undefined;
    const plan = this.ledger ? planBrief(this.ledger) : undefined;
    const jobs = this.commands?.inspection() ?? [];
    const recovered = this.commands?.recovered() ?? [];
    const usage = this.remoteUsageBrief();
    const state: SessionState = {
      mode: this.attached ?? "headless", turn: this.turn, open: this.open, paused: owner.paused,
      activity: this.liveEvents.activity, last_error: this.turnError ?? undefined, model,
      effort: effort_levels.length ? session.thinkingLevel : undefined, effort_levels, usage,
      queue, queue_total, cards, exit, children: this.liveEvents.childCounts, jobs: jobCounts(jobs, recovered.length),
      board, plan, notices: this.liveEvents.notices, can: capabilities(this.remoteRegistry, {
        open: this.open, closing: this.closing, rewind: this.checkpoints !== undefined,
      }),
    };
    const journal = this.options.build ? statSync(join(this.options.build.dir, "journal.json"), { throwIfNoEntry: false }) : undefined;
    return { state, parts: {
      cards: entries.map(entry => cardDetail(entry.card)), jobs: [jobs, recovered], usage,
      context: [model, usage.context_tokens, usage.context_window, usage.context_percent],
      plan: [plan, this.ledger?.current?.revision], board: [board, build?.board?.tasks(), build?.board?.proposed()],
      record: journal ? [journal.ino, journal.mtimeMs, journal.size] : undefined,
      models: [model, state.effort, effort_levels],
      commands: [
        session.extensionRunner.getRegisteredCommands().map(command => [command.invocationName, command.description]),
        session.promptTemplates.map(prompt => prompt.name),
        this.runtime.services.resourceLoader.getSkills().skills.map(skill => skill.name),
      ],
    } };
  }

  private remoteUsageBrief(): UsageBrief {
    const session = this.session, model = session.model;
    const leaf = session.sessionManager.getLeafId();
    if (this.remoteUsage?.leaf === leaf && this.remoteUsage.model === model) return this.remoteUsage.usage;
    const stats = session.getSessionStats();
    const cost_partial = stats.tokens.total > 0 && session.sessionManager.getEntries().some(entry => {
      if (entry.type !== "message" || entry.message.role !== "assistant") return false;
      const { usage } = entry.message;
      return usage.totalTokens > 0 && (!Number.isFinite(usage.cost.total) || usage.cost.total === 0);
    });
    const usage: UsageBrief = { tokens: stats.tokens.total, cost_partial,
      cost_usd: Number.isFinite(stats.cost) && (stats.cost > 0 || !cost_partial) ? stats.cost : undefined,
      context_tokens: stats.contextUsage?.tokens ?? 0, context_window: stats.contextUsage?.contextWindow ?? 0,
      context_percent: stats.contextUsage?.percent ?? undefined };
    this.remoteUsage = { leaf, model, usage };
    return usage;
  }

  private panelSnapshot(): PanelSnapshot | null {
    if (!this.runtime) return null;
    const session = this.session;
    const leaf = session.sessionManager.getLeafId();
    if (leaf !== this.panelTokenLeaf) { this.panelTokenLeaf = leaf; this.panelStats = undefined; }
    const jobs: PanelJob[] | null = this.jobs ? [...(this.commands?.inspection() ?? []).map(detail => ({ job: detail.job, state: detail.state, detail })),
      ...this.commands?.recovered() ?? []] : null;
    return {
      ...(this.buildControls ? { build: this.buildControls.panelState() } : {}),
      ...(this.fleet ? { fleet: this.fleet.snapshot() } : {}),
      bound: this.bound, model: session.model ? `${session.model.provider}/${session.model.id}` : null,
      effort: session.thinkingLevel, effortAvailable: session.supportsThinking(),
      ...(this.terminalInput ? { input: this.terminalInput } : {}),
      streaming: session.isStreaming, context: session.getContextUsage(),
      ...(this.panelStats ??= panelStats(session.getSessionStats())), tools: session.getActiveToolNames(),
      contextFiles: this.guidance.contextFiles.map(file => file.path),
      skills: this.runtime.services.resourceLoader.getSkills().skills.map(skill => skill.name),
      ledger: this.ledger?.current ?? null, ledgerStale: this.ledger?.unavailable ?? true,
      ledgerContinued: this.ledger?.continued ?? null,
      inspector: {
        session: session.sessionId, boundSession: this.boundSessionId ?? null,
        turn: this.turn, turnOpen: this.open,
        capabilities: this.options.capabilities ?? null,
        guidance: {
          projectText: this.options.guidance?.projectText ?? null,
          skillPaths: this.guidance.skillPaths, promptPaths: this.guidance.promptPaths,
          prompts: this.runtime.services.resourceLoader.getPrompts().prompts.map(prompt => prompt.name),
          skills: this.runtime.services.resourceLoader.getSkills().skills.map(skill => `${skill.name} · ${skill.filePath}`),
        },
        // Only effective SDK settings, never raw capability/MCP settings or credentials.
        settings: Object.entries(this.settings).map(([key, value]) => `${key}: ${JSON.stringify(value)}`),
        events: this.options.events.since(0, 512 * 8192, 512),
      },
      agents: [{ id: session.sessionId, session: session.sessionId, kind: this.options.identity.role,
        state: session.isStreaming ? "streaming" : this.open ? "turn open" : "settled locally",
        model: session.model ? `${session.model.provider}/${session.model.id}` : null,
        tokens: this.panelStats.tokens, transcript: !!session.sessionFile && session.sessionId === this.boundSessionId },
        ...(this.auxiliary?.inspection() ?? []).map(child => ({ id: child.request, session: child.request,
          parent: child.parent, kind: `native ${child.kind}`, state: child.state, model: child.model, transcript: !!child.sessionFile && child.parent === this.boundSessionId }))],
      jobs, auxiliary: this.auxiliary?.state() ?? null, helpers: this.auxiliary?.inspections() ?? null,
      steering: session.getSteeringMessages().length, followUps: session.getFollowUpMessages().length,
      queue: this.queue.items(),
    };
  }

  private async readAgentTranscript(id: string): Promise<string> {
    if (!this.bound || this.session.sessionId !== this.boundSessionId) throw new Error("Transcript unavailable: stale/unbound host");
    if (id === this.session.sessionId && this.session.sessionFile) return transcriptPreview(this.session.sessionFile, id);
    const child = this.auxiliary?.inspection().find(child => child.request === id);
    if (!child?.sessionFile || child.parent !== this.boundSessionId) throw new Error("Selected agent missing or transcript ineligible");
    return transcriptPreview(child.sessionFile, child.request);
  }

  state(): Record<string, unknown> {
    const session = this.session;
    const resources = this.runtime.services.resourceLoader;
    const network = this.options.network.ledger();
    return {
      role: this.options.identity.role,
      identity: this.options.identity,
      native_children: this.auxiliary?.inspection().map(({ sessionFile: _file, ...child }) => child) ?? null,
      fleet: this.fleet?.snapshot() ?? null,
      todo: this.loadout.includes("corral_todo"),
      // The plan the sidebar renders and the model summary names: one host ledger.
      ledger: this.ledger?.current ? { plan: this.ledger.current.plan ?? null, revision: this.ledger.current.revision } : null,
      capabilities: this.options.capabilities ?? null,
      mcp: this.mcp?.state() ?? null,
      bound: this.bound,
      interactive: this.attached === "interactive",
      mode: this.attached ?? null,
      session: { bound_id: this.boundSessionId ?? null, current_id: session.sessionId, file: session.sessionFile ?? null,
        created: this.sessionsCreated },
      streaming: session.isStreaming,
      turn: { current: this.turn, open: this.open },
      last_error: this.turnError,
      last_seq: this.options.events.last,
      model: session.model ? `${session.model.provider}/${session.model.id}` : null,
      loadout: [...this.loadout],
      active_tools: session.getActiveToolNames(),
      denied_tools: this.deniedTools,
      resources: {
        extensions: resources.getExtensions().extensions.map((extension) => extension.path),
        skills: resources.getSkills().skills.length,
        prompts: resources.getPrompts().prompts.length,
        context_files: resources.getAgentsFiles().agentsFiles.length,
      },
      jobs: this.jobs?.state() ?? null,
      commands: this.commands?.state() ?? null,
      codemode: this.code?.state() ?? null,
      owner_questions: this.ownerQuestions?.list().map(q => ({ id: q.id, status: this.ownerQuestions?.status(q.id),
        session: q.session, expires: q.expires })) ?? null,
      checkpoints: this.checkpoints !== undefined,
      auxiliary: this.auxiliary?.state() ?? null,
      guidance: {
        project_text: this.options.guidance?.projectText ?? false,
        skill_paths: [...this.guidance.skillPaths], prompt_paths: [...this.guidance.promptPaths],
        context_files: this.guidance.contextFiles.map((file) => file.path),
        skills: resources.getSkills().skills.map((skill) => ({ name: skill.name, path: skill.filePath })),
        export: this.exportable(),
      },
      settings: this.settings,
      network: {
        allowed_destinations: [...new Set([...(this.options.network.destinations ?? hostDestinations(this.provider)),
          ...this.apiNetworks.flatMap(network => network.destinations ?? [])])],
        ...network,
        allowed: network.allowed + this.apiNetworks.reduce((count, guard) => count + guard.ledger().allowed, 0),
      },
      herdr: this.options.herdr?.readback() ?? null,
      auth: bounded(this.options.modules.auth.state?.() ?? null),
    };
  }
}

