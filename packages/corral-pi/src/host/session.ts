// One upstream AgentSessionRuntime, with local session state and engine CLI board access.
import { existsSync, readFileSync, realpathSync, statSync, unlinkSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { createCodeCommands } from "./code-commands.ts";
import type { CodeHandoff } from "./code-handoff.ts";

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
import { ToolApprovals } from "./tool-approvals.ts";
import { type CapabilityGuidance, type Guidance, selectGuidance } from "../capabilities/guidance.ts";
import { PROVIDERS, type ProviderId, billingKind, hostDestinations, isApiProvider, isProviderId } from "../auth/providers.ts";
import type { AuthContext, Fetch } from "../auth/types.ts";
import type { EventLog } from "../bridge/events.ts";
import { LiveEvents } from "./live-events.ts";
import { registerBoard } from "./remote/board.ts";
import { boundCandidate, RemoteChanges } from "./remote/changes.ts";
import { confinedRegularFile } from "./remote/fleet.ts";
import { archivedChildren, childDetailsChanged, registerChildren } from "./remote/children.ts";
import { commandsPart, registerCommands, typedSessionAction } from "./remote/commands.ts";
import { jobsPart, registerJobs } from "./remote/jobs.ts";
import { ModelCatalogStatus, modelsPart, registerModels } from "./remote/models.ts";
import { WorkspaceModelFavorites } from "./model-favorites.ts";
import { readPersonalPreferences } from "./preferences.ts";
import { terminalSettings } from "./terminal-settings.ts";
import { usagePart, usageTotal, type ParentUsage } from "./remote/usage.ts";
import { contextPart, planHistory, registerWorkspace, WorkspaceGit, type WorkspaceInputs } from "./remote/workspace.ts";
import { RemoteBuild } from "./remote/build.ts";
import { briefedCards, cardDetail, questionEntries, questionStatus, sortedCards } from "./remote/cards.ts";
import { RemoteDialogs } from "./remote/dialogs.ts";
import { RemoteRegistry } from "./remote/registry.ts";
import { parseChildId, type TranscriptInput } from "./remote/input.ts";
import { boardBrief, capabilities, clipText, exitBrief, jobCounts, jsonBytes, planBrief, type SessionState, type StateProjection } from "./remote/state.ts";
import { utf8Prefix } from "../roles/assistant/owner-action.ts";
import { buildParentPolicy } from "../roles/build/guard.ts";
import type { RemoteOwner } from "./remote-owner.ts";
import { CLAUDE_SDK_API, type ClaudeLaneConfig, registerClaudeSdkApi } from "../claude/stream.ts";
import { closeResidentSession, residentLifecycle } from "../claude/resident.ts";
import { BridgeRefusal, type AnswerInput, type RewindInput, type SessionInput, type SubmitResult } from "../bridge/protocol.ts";
import { listRewinds, parseRewindCommand, recordRewind, restoreRewind, rewindPoint } from "./rewind.ts";
import type { HostIdentity, RoleContext } from "../roles/types.ts";
import type { BuildContext, BuildProfile, ChildSnapshot, FleetSnapshot } from "../build/types.ts";
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
import { PLAN_ENTRY, PLAN_FEEDBACK, PLAN_KICKOFF, PLAN_OPTIONS, PLAN_SUBMIT_TOOL, PlanMode, planSubmitTool, type PlanAnswer } from "./plan-mode.ts";
import { registerPlanMode } from "./remote/plan.ts";
import { PlanReviewUi } from "../ui/plan-review.ts";
import { isToolApproval, PENDING_MAX, PendingQuestions, QUESTION_ENTRY, validAnswer } from "./pending-questions.ts";
import { OwnerQuestionUi } from "../ui/owner-questions.ts";
import { registerCacheStats } from "../ui/cache-stats.ts";
import type { HerdrLifecycle } from "./herdr.ts";
import type { LoadedModules } from "./modules.ts";
import { type GuardedFetch, guardedFetch, resourceLoaderOptions, restrictedSettings } from "./restrict.ts";
import { createModelCatalog } from "./catalog.ts";
import { installModelSelection } from "./model-selection.ts";
import { childModels, childProfileProviders, interactiveModels, sessionRoutes, type SessionRoutes } from "./capabilities.ts";
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
import type { HelperInspection } from "../extensions/subagent/inspection.ts";
import { AuxiliaryRuntime } from "../extensions/subagent/runtime.ts";
import { auxiliarySettings } from "../extensions/subagent/settings.ts";
import { expertTool, exploreTool } from "../extensions/subagent/index.ts";
import { runAuxiliarySession } from "../extensions/subagent/session.ts";
import { FleetRuntime } from "../extensions/fleet/runtime.ts";
import { fleetTools } from "../extensions/fleet/tools.ts";
import { boundFleetSettings } from "../extensions/fleet/settings.ts";
import { executionRefusal, exploreLoadout } from "../roles/common/execution.ts";
import { workerDoneTool } from "../roles/worker/tools.ts";
import { readTranscriptPage, type TranscriptRequest, type TranscriptPage } from "./transcript-preview.ts";
import { ManagedEffects } from "./managed-effects.ts";
import { LocalControlPlane } from "./local.ts";
import { failureClass, messageFailure, withProviderFailures } from "./provider-failure.ts";
import { assertApiEffort, installApiProvider } from "./api-providers.ts";
import { withSubscriptionFallback, type FallbackCandidates } from "./continuation.ts";
import { childFallbackPolicy, fallbackProvidersFor, hasProviderFallbacks, providerFallbackCandidates,
  type FallbackSettings } from "./fallback-settings.ts";
import { withAutomationPolicy, type AutomationPolicy } from "./automation-policy.ts";
import { HostQueue, type InputFacts } from "./queue.ts";
import { MARK_ENTRY, observeTranscriptEntries, recordTranscriptMark, type MarkSink, type ModelMark, type TranscriptMark,
  type TurnStopCause, type TurnWait } from "./transcript-marks.ts";
import { TurnFacts, turnWaits } from "./turn-facts.ts";
import { RemotePrompts } from "./remote-prompts.ts";
import { mergeArchivedChildren, resolveChildRecording, type TranscriptRecording } from "./transcript-resolution.ts";
import { RetainedChildren } from "./retained-children.ts";
import { createSummaryRequest, summarizeQuick, type SummaryOptions, type SummaryRequest } from "./summary.ts";

export interface HostSessionOptions {
  readonly automation?: AutomationPolicy;
  readonly automationRoot?: string;
  readonly sdk: typeof Sdk;
  readonly identity: HostIdentity;
  readonly build?: BuildContext;
  readonly codeHandoff?: CodeHandoff;
  readonly paneInput?: (text: string, images?: ImageContent[]) => Promise<boolean>;
  readonly transcriptRewound?: () => void;
  readonly automatedInput?: (input: SessionInput) => Promise<SubmitResult> | undefined;
  /** Automated input that must start now or be refused, ordered against pause; never parked while paused. */
  readonly startInput?: (input: SessionInput, answers?: PlanAnswer) => Promise<SubmitResult> | undefined;
  readonly cwd: string;
  readonly agentDir: string;
  readonly model: string;
  /** The parent's active provider on host startup; absent means OpenAI Codex. */
  readonly provider?: ProviderId;
  /** Worker helpers keep the configured launch model when the parent's saved fallback is restored. */
  readonly workerLaunch?: ModelChoice & { readonly effort?: string };
  readonly fallback?: readonly ModelChoice[];
  readonly noFallback?: boolean;
  readonly fallbackPolicy?: FallbackSettings;
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
  /** `--plan`: a session that is not already planning starts in plan mode. */
  readonly planMode?: boolean;
  /** Whether the owner paused the session, which holds automated input instead of starting it. */
  readonly paused?: () => boolean;
  /** The session's recorded snapshot; when present it decides todo. */
  readonly capabilities?: HostCapabilities;
  /** The snapshot's settings and project-text trust; absent loads no optional guidance. */
  readonly guidance?: CapabilityGuidance;
  /** The corral workspace root, the outermost directory whose context files may load. */
  readonly root?: string;
  /** Named child profiles a worker's `task` children may select; Build sessions use their own. */
  readonly childProfiles?: readonly BuildProfile[];
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

/** The SDK's refusals for a session with nothing left to summarise; neither is a failure of the caller's intent. */
const uncompactable = (error: unknown): boolean =>
  error instanceof Error && /^(Nothing to compact|Already compacted)/.test(error.message);

/** Changes "since session" compares against the HEAD each root had when this session first read it. */
const REVIEW_BASE_ENTRY = "corral_review_base";
const panelStats = (stats: { readonly tokens: { readonly total: number }; readonly cost: number }) => ({ tokens: stats.tokens.total, cost: stats.cost });

// allow: SIZE_OK - Pi lifecycle adapter.

export class HostSession {
  runtime!: AgentSessionRuntime;
  /** Interactive host API only; deliberately absent from extension/model registries. */
  readonly queue: HostQueue;
  private readonly queuedInputFacts = new WeakMap<object, InputFacts>();
  private startedInputFacts: InputFacts | undefined;
  private readonly pendingAutomation = new WeakSet<InputFacts>();
  private readonly automationInputs = new WeakSet<InputFacts>();
  private inAutomation = false;
  private automationRetired = false;
  private unobserveTranscript: (() => void) | undefined;
  private remoteUsage: { leaf: string | null; model: AgentSession["model"]; usage: ParentUsage } | undefined;
  private remoteFleetDirty = true;
  private remoteFleet: FleetSnapshot | undefined;
  private remoteChildPart: unknown;
  private remoteHelpers: readonly HelperInspection[] = [];
  readonly liveEvents: LiveEvents;
  readonly remoteRegistry = new RemoteRegistry();
  readonly remoteBuild: RemoteBuild;
  private readonly remoteChanges: RemoteChanges;
  private readonly workspaceGit: WorkspaceGit;
  private readonly archivedFleet: readonly ChildSnapshot[];
  private readonly remoteDefaultModel: string;
  private readonly remoteCatalog = new ModelCatalogStatus();
  private readonly modelFavorites: WorkspaceModelFavorites;
  private readonly buildModelsPath: string | undefined;
  private remoteJobSample: ReturnType<typeof setTimeout> | undefined;
  readonly dialogs: RemoteDialogs;
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
  /** Present for the roles that ship plan-mode prompts. */
  readonly planMode: PlanMode | undefined;
  private planReview: PlanReviewUi | undefined;
  /** The summary a clean-context reset writes in place of a model-made one. */
  private planReset: string | undefined;
  private modelSelections = 0;
  /** Inputs whose callers wait to learn whether that very input reached the transcript, by queue tag. */
  private readonly deliveries = new Map<unknown, (delivered: boolean) => void>();
  /** The user message Pi has told its listeners of and appends next, with the input it records. */
  private arriving: { readonly message: object; readonly input: InputFacts } | undefined;
  /** Callers waiting for the open turn to have settled for everyone. */
  private readonly closeWaiters: (() => void)[] = [];
  /** Inputs handed to Pi whose admission is not answered yet, by queue tag, with how to refuse each. */
  private readonly admissions = new Map<unknown, (reason: string) => void>();
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
  private cancelRequested: number | undefined;
  private stopCause: TurnStopCause | undefined;
  private turnError: string | null = null;
  private nextTurnOrigin: TurnOrigin | undefined;
  private lastInputSource: string | undefined;
  /** The owner typed into the pane while the current turn was open (a steer into it). */
  private ownerInput = false;
  private readonly retainedChildren?: RetainedChildren;
  private readonly transcriptRoot?: string;
  private boundSessionFile?: string;
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
  private readonly toolApprovals: ToolApprovals | undefined;
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
  private switchingCode = false;
  private readonly codeCommand?: (text: string) => boolean;

  handleCodeCommand(text: string): boolean {
    // Once the handoff is admitted, don't let native submissions (e.g. !bash) start new work.
    return this.switchingCode || (this.codeCommand?.(text) ?? false);
  }

  private constructor(private readonly options: HostSessionOptions) {
    const host = this;
    const code = options.build;
    const handoff = options.codeHandoff;
    if (code?.session.trustedCode === true && handoff) {
      this.codeCommand = createCodeCommands({
        dataRoot: dirname(dirname(code.dir)), current: code.session, handoff: async request => {
          this.switchingCode = true;
          try { await handoff(request); }
          finally { this.switchingCode = false; }
        },
        manager: () => this.session.sessionManager,
        ui: () => this.session.extensionRunner.getUIContext(),
        busy: () => !this.bound || this.closing || this.rewinding || this.open || this.admissions.size > 0
          || !this.session.isReadyForHandoff || this.session.getSteeringMessages().length > 0
          || this.session.getFollowUpMessages().length > 0 || this.session.sessionId !== this.boundSessionId,
      });
    }
    this.liveEvents = new LiveEvents();
    // A change here, from the app or from another session re-announces the catalog and redraws an open picker.
    this.modelFavorites = new WorkspaceModelFavorites(options.client.workspaceRoot, () => {
      this.liveEvents.stateChanged();
      this.panels?.refresh();
    });
    this.dialogs = new RemoteDialogs(() => this.liveEvents.stateChanged(), {
      entries: () => {
        const review = this.planMode?.card();
        return [...questionEntries(this.ownerQuestions), ...this.remoteBuild.entries(), ...(review ? [review] : [])];
      },
      absent: id => questionStatus(this.ownerQuestions?.status(id) ?? "missing"),
      shared: () => this.attached === "interactive",
    });
    this.provider = options.provider ?? "openai-codex";
    this.local = new LocalControlPlane(options.build?.dir ?? options.runtimeDir ?? options.agentDir, options.identity);
    const planPrompts = options.modules.role.planMode;
    this.planMode = planPrompts ? new PlanMode({
      // A Build has a directory of its own. A chat's runtime directory serves every conversation held there, so
      // its plan is kept under the transcript's id: a resumed chat finds its draft, and a new one starts without.
      get dir() {
        return options.build?.dir
          ?? join(options.runtimeDir ?? options.agentDir, "plan-mode", host.session.sessionManager.getSessionId());
      },
      cwd: options.cwd, prompts: planPrompts,
      askUser: options.capabilities?.enabled.includes("ask_user") === true,
      todo: options.capabilities ? options.capabilities.enabled.includes("todo") : options.todo === true,
      entries: () => this.session.sessionManager.getBranch(),
      leaf: () => this.session.sessionManager.getLeafId(),
      append: record => { this.session.sessionManager.appendCustomEntry(PLAN_ENTRY, record, { immediate: true }); },
      busy: () => !this.remoteActionsReady ? "Session is not accepting plan actions"
        : this.open || this.session.isStreaming || this.session.isCompacting ? "Stop the current turn or wait for it to finish" : undefined,
      blocked: () => this.modelSelections ? "Wait for model selection to finish, then choose again"
        : this.session.isCompacting ? "Wait for the running compaction to finish, then choose again"
        : options.paused?.() ? "The session is paused and would hold the plan instead of starting it. Resume it, then choose again"
        : undefined,
      stopTurn: () => this.planStopTurn(),
      compact: instructions => this.planCompact(instructions),
      reset: summary => this.planResetContext(summary),
      // Plan mode's own inputs start a run or are refused: queued into another run, an answer to a review would
      // be acknowledged before it had any effect, and could outlive that run to be drained by a later one. An
      // answer is also ordered against pause and never parked in a paused session's held queue. A `/plan`
      // request is not: it can already be running inside that order.
      submit: async (text, origin, answers) => {
        const input: SessionInput = { text, origin, busyPolicy: "prompt" };
        // Read before the answer waits its turn in the order: it holds for the conversation as it is now.
        const answer = answers && { ...answers, context: this.contextTip() };
        const result = await (answer ? options.startInput?.(input, answer) ?? this.submitOpened(input, answer)
          : this.submitOpened(input));
        return result.disposition === "started";
      },
      model: () => this.session.model ? `${this.session.model.provider}/${this.session.model.id}` : undefined,
      changed: () => { this.liveEvents.stateChanged(); this.panels?.refresh(); },
      note: (text, tone = "text") => { this.addMark({ kind: "note", tone, at: Date.now(), text }); },
      approved: summary => { this.buildControls?.noteOwnerInput(summary); },
    }) : undefined;
    const sessionRoot = options.build?.dir ?? options.sessionDir;
    this.transcriptRoot = sessionRoot;
    const retained = this.retainedChildren = sessionRoot ? new RetainedChildren(sessionRoot) : undefined;
    this.remoteDefaultModel = options.model.startsWith(`${this.provider}/`) ? options.model : `${this.provider}/${options.model}`;
    const { task, run } = options.identity, buildSession = options.build?.session;
    const reviewBases = persistedBases(
      () => options.sessionManager.getEntries().flatMap(entry => entry.type === "custom" && entry.customType === REVIEW_BASE_ENTRY ? [entry.data] : []),
      data => options.sessionManager.appendCustomEntry(REVIEW_BASE_ENTRY, data));
    this.workspaceGit = new WorkspaceGit(options.cwd, () => this.liveEvents.stateChanged());
    const archivedJournal = options.build && confinedRegularFile(options.build.dir, join(options.build.dir, "journal.json"));
    this.archivedFleet = archivedJournal ? archivedChildren(archivedJournal) : [];
    this.remoteChanges = new RemoteChanges({
      cwd: options.cwd, defaultComparison: task && run && options.client.configured ? "candidate" : "session",
      scopeRequired: options.client.configured && options.identity.role === "assistant" && !options.identity.project,
      bases: { get: root => (root === this.workspaceGit.worktree ? buildSession?.base : undefined) ?? reviewBases.get(root), set: reviewBases.set },
      children: () => this.readRemoteFleet()?.children ?? [],
      ...(options.client.configured ? {
        projects: () => options.client.projectScopes(),
        ...(task && run ? { candidate: async () => boundCandidate(await options.client.taskSnapshot(), task, run) } : {}),
      } : {}),
      changed: () => this.liveEvents.stateChanged(),
    });
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
    const childPolicy = childFallbackPolicy(options.fallbackPolicy, options.noFallback);
    const helperModel = options.workerLaunch ?? { provider: this.provider, model: options.model, effort: options.effort };
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
      parentLoadout: options.capabilities?.enabled.includes("codemode") ? ["codemode"] : [],
      localTools: () => this.localTools.filter(tool => !this.buildControls?.tools.includes(tool)),
      deliverToParent: async text => {
        const result = await this.submitAutomated(text);
        if (result.disposition === "rejected") throw new Error(`Fleet parent delivery rejected: ${result.reason}`);
      },
      shellRefusal: options.modules.role.shellRefusal, changed: () => this.fleetChanged(), retained,
      ...(childPolicy ? { fallbackPolicy: childPolicy } : {}),
      onChildFallback: ({ child, from, model }) => {
        options.events.emit("model_fallback", { child, from, model, text: `Quota exhausted on ${from}; continuing on ${model}` });
      },
    }) : undefined;
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
    const ownerDialogs = { dialogs: this.dialogs };
    this.buildModelsPath = options.build?.session.bare ? join(options.build.dir, "..", "..", "models.json") : undefined;
    const startupRoot = options.build ? options.build.session.workspaceRoot
      : options.client.configured ? options.client.workspaceRoot : undefined;
    this.panels = options.ui === "plain" ? undefined : new NativePanels({
      ...ownerDialogs,
      modelFavorites: this.modelFavorites,
      personal: readPersonalPreferences(),
      settings: terminalSettings(options.identity.role, startupRoot, options.client.executable, { modelsPath: this.buildModelsPath }),
      identity: options.identity, cwd: options.cwd, snapshot: () => this.panelSnapshot(),
      planDocument: () => {
        const plan = this.planMode;
        if (!plan) return undefined;
        const { phase, title, revision } = plan.record, approved = plan.approved;
        if (phase === "review") return { phase, title: title ?? "Plan", revision };
        return approved ? { phase: "approved", title: approved.title, revision: approved.revision } : { phase };
      },
      openPlanDocument: () => {
        if (this.planMode?.review) void this.planReview?.show();
        else void this.planReview?.showApproved();
      },
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
      reviewBases,
      canRenderTool: name => exploreLoadout(this.loadout).includes(name),
      artifacts: () => options.client.artifacts(),
      artifactFile: reference => options.client.artifactFile(reference),
      rewind: input => this.rewind(input),
      exportSession: async () => (await this.reportedExport()).text,
      readHistory: request => options.client.history(request),
      readTranscript: (id, request) => this.readAgentTranscript(id, request),
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
      ...(options.childProfiles?.length ? { childProfiles: options.childProfiles } : {}),
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
        parent: { ...helperModel, model: helperModel.model.startsWith(`${helperModel.provider}/`)
          ? helperModel.model.slice(helperModel.provider.length + 1) : helperModel.model },
        context: { session: { worktree: options.cwd, name: `${options.identity.role} session` },
          dir: options.runtimeDir ?? join(options.agentDir, "runtime"),
          settings: { maxParallel: settings.maxParallel, profiles: options.childProfiles ?? [] } },
        // A bound worker's launch cwd is authoritative, including reruns and adopted checkouts.
        buildWorktree: options.identity.role === "worker" && !!options.identity.task,
        agentDir: options.agentDir, bound: true, allowedKinds: settings.allowedKinds,
        maxTurns: settings.maxTurns, timeoutSecs: settings.timeoutSecs,
        ...(auxiliary.explore ? { research: { settings: auxiliary.explore } } : {}),
        get parentLoadout() { return host.loadout; },
        get localTools() { return host.localTools; },
        shellRefusal: options.modules.role.shellRefusal,
        changed: () => this.fleetChanged(), record, retained,
        ...(childPolicy ? { fallbackPolicy: childPolicy } : {}),
        onChildFallback: ({ child, from, model }) => {
          options.events.emit("model_fallback", { child, from, model, text: `Quota exhausted on ${from}; continuing on ${model}` });
        },
        deliverToParent: async text => {
          const result = await this.submitAutomated(text);
          if (result.disposition === "rejected") throw new Error(`Fleet parent delivery rejected: ${result.reason}`);
        },
      });
    }
    if (auxiliary.explore || auxiliary.web_search || auxiliary.expert) {
      this.auxiliary = new AuxiliaryRuntime({
        settings: auxiliary, provider: helperModel.provider, model: helperModel.model,
        changed: () => this.fleetChanged(),
        sessionId: () => this.sessionId(),
        searchModel: () => {
          const model = this.runtime?.session.model;
          return model && ["openai-codex", "anthropic"].includes(model.provider) && this.searchModels?.getModel(model.provider, model.id)
            ? { provider: model.provider, model: model.id } : undefined;
        },
        record,
        retain: retained ? (helper, file, result) => retained.helper(helper, file, result) : undefined,
        execute: (invocation, signal) => {
          if (invocation.kind === "web_search") {
            if (!this.searchModels || !options.capabilities?.web) throw new Error("subscription search runtime is not ready");
            return runSubscriptionSearch({ sdk: options.sdk, models: this.searchModels, cwd: options.cwd,
              settings: options.capabilities.web }, invocation, signal);
          }
          if (!this.auxiliaryModels) throw new Error("auxiliary model runtime is not ready");
          return runAuxiliarySession({ sdk: options.sdk, models: this.auxiliaryModels, provider: helperModel.provider,
            cwd: options.cwd, agentDir: options.agentDir,
            parentLoadout: this.loadout, localTools: this.localTools,
            ...(childPolicy ? { fallbackPolicy: childPolicy } : {}),
            onFallback: (from, model) => {
              options.events.emit("model_fallback", {
                auxiliary: invocation.request, kind: invocation.kind, from, model,
                text: `Quota exhausted on ${from}; continuing on ${model}`,
              });
            } }, invocation, signal);
        },
      });
    }
    const dialogEnabled = options.capabilities?.enabled.includes("ask_user") === true;
    const writeApprovalRole = options.identity.role === "build" || options.identity.role === "assistant";
    const questions = dialogEnabled || writeApprovalRole || options.capabilities?.enabled.includes("bash_rules") ? new PendingQuestions({
      session: () => options.sessionManager.getSessionId(),
      entries: () => options.sessionManager.getEntries(),
      append: value => { options.sessionManager.appendCustomEntry(QUESTION_ENTRY, value, { immediate: true }); },
      changed: question => {
        if (question) {
          this.toolApprovals?.observeAnswer(question);
          this.remotePrompts.changed(question);
          options.events.emit("owner_question", { id: question.id, status: question.status });
          options.herdr?.questionChanged(question.id, question.status === "pending");
        }
        this.questionUi?.changed(question);
        this.liveEvents.stateChanged();
      },
      deliver: async (text, question) => {
        const result = await this.submitLocal({ text, origin: isToolApproval(question) ? "auto" : "owner", busyPolicy: "steer" });
        return result.disposition === "started" || result.disposition === "queued";
      },
    }) : undefined;
    this.ownerQuestions = questions;
    this.toolApprovals = questions ? new ToolApprovals(questions, options.cwd, writeApprovalRole) : undefined;
    this.localTools = [...(lspEnabled ? roleLocal : roleLocal.filter((tool) => !tool.name.startsWith("lsp_"))),
      ...(dialogEnabled && questions ? [askUserTool(async answer => {
        if (!this.dialogPi) throw new Error("dialog session is unavailable");
        this.dialogPi.sendUserMessage(answer, { deliverAs: "steer" });
        return answer;
      }, questions, () => this.questionUi?.busy === true, () => this.dialogPresentation(), localQuestionAsk(questions),
      () => this.attached === "headless", () => this.attached === "interactive" ? this.dialogs : undefined)] : []),
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
    // plan_submit stays out of localTools, which children inherit; it is present in every phase so the tool list is stable.
    this.baseLoadout = [...role.upstreamTools.filter(name => !local.includes(name)), ...corralToolNames(specs), ...local,
      ...(this.planMode ? [PLAN_SUBMIT_TOOL] : []), ...(mcp ? MCP_CONTROLS : []), ...(mcp && search ? ["mcp_activate"] : []), ...(search ? ["tool_search"] : []),
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
    const activePolicy = options.noFallback ? undefined : options.fallbackPolicy;
    const childLaunchProvider = options.workerLaunch?.provider ?? route.id;
    const childProviders = childProfileProviders(options.identity.role, childLaunchProvider, options.childProfiles);
    const fallbackSeedProviders: ProviderId[] = [
      ...(!options.fallback?.length ? [route.id] : []),
      ...(activePolicy?.children ? [childLaunchProvider, ...childProviders] : []),
    ];
    const extra = [
      childLaunchProvider,
      ...(!options.noFallback ? (options.fallback ?? []).map(choice => choice.provider) : []),
      ...childProviders,
      ...fallbackProvidersFor(fallbackSeedProviders, activePolicy),
    ].filter(id => (id !== "amazon-bedrock" || options.aws) && (id !== "llama.cpp" || options.capabilities?.llama_cpp));
    const { providers, credentials } = options.routes ?? await sessionRoutes(options.modules.auth, route.id, options.identity.role, extra);
    const models = await createModelCatalog(credentials, { providers,
      ...(providers.every(isApiProvider) ? { modelsPath: null }
        : host.buildModelsPath !== undefined ? { modelsPath: host.buildModelsPath } : {}),
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
    host.remoteCatalog.observe(models, () => host.liveEvents.stateChanged());
    host.summaryRequest = await createSummaryRequest(options, report);
    const interactive = interactiveModels(options.identity.role);
    host.buildProviders = interactive
      ? providers
      : childModels(options.identity.role)
        ? providers.filter(id => id === childLaunchProvider || childProviders.includes(id))
        : [route.id];
    if (options.capabilities?.enabled.includes("web.enabled") && options.capabilities.web?.search_provider === "subscription") {
      // Search follows the parent's own model, so it needs only the providers the parent may select.
      const searchProviders = (interactive ? providers : [route.id]).filter((id): id is ProviderId => id === "openai-codex" || id === "anthropic");
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
      const switchForFallback = installModelSelection(session, models, interactive, host.modelFavorites);
      const parentFallback: FallbackCandidates | undefined = options.noFallback
        ? undefined
        : options.fallback?.length
          ? options.fallback
          : hasProviderFallbacks(options.fallbackPolicy)
            ? current => providerFallbackCandidates(current, options.fallbackPolicy)
            : undefined;
      if (parentFallback) {
        const agent = session.agent;
        const original = agent.streamFunction;
        const fallback = withSubscriptionFallback(original, parentFallback, async (candidate, signal) => {
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
    if (this.options.quickSummary || this.planMode) pi.on("session_before_compact", async event => {
      // A clean-context reset supplies its own summary and keeps no earlier entry: a null first-kept entry
      // is the session manager's own "nothing kept", which the compaction result's type does not name.
      const reset = this.planReset;
      if (reset !== undefined) {
        return { compaction: { summary: reset, firstKeptEntryId: null as unknown as string,
          tokensBefore: event.preparation.tokensBefore, details: { corral_plan_reset: true } } };
      }
      if (!this.options.quickSummary) return undefined;
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
    pi.on("session_start", () => { this.toolApprovals?.clear(); });
    if (this.ownerQuestions) {
      this.questionUi = new OwnerQuestionUi(this.ownerQuestions, () => this.bound && !this.closing,
        message => { this.session.sessionManager.appendCustomMessageEntry(message.customType, message.content, message.display, undefined); },
        () => this.session.model?.api === CLAUDE_SDK_API, () => this.dialogPresentation());
      this.questionUi.install(pi);
      this.panels?.attachQuestions(this.questionUi.dock());
    }
    const automation = this.options.automation;
    if (automation) pi.on("context_with_system", event => {
      if (!this.inAutomation || this.sessionsCreated !== 1) return;
      const messages = withAutomationPolicy(event.messages, automation);
      if (messages) return { messages };
    });
    // Follows the session's current model, so a mid-session model switch changes the overlay too.
    const calibration = this.options.modules.role.calibration;
    if (calibration && this.roleContext.build?.session.trustedCode !== true) pi.on("before_agent_start", event => {
      const model = this.runtime?.session.model;
      const text = model ? calibration(model.provider, model.id) : undefined;
      if (text) event.systemPromptOptions.sections.model_calibration = text;
      else delete event.systemPromptOptions.sections.model_calibration;
    });
    pi.on("session_start", () => {
      if (this.bound) this.commands?.bound();
      this.liveEvents.stateChanged();
    });
    const plan = this.planMode;
    if (plan) {
      // The mode reaches the model as a conversation message, so the system prompt never changes with it.
      pi.on("before_agent_start", event => {
        const message = plan.turnMessage(event.messageType === "user" ? event.prompt : "");
        return message ? { message } : undefined;
      });
      // Pi can compact inside a run and go on without before_agent_start, and a summary is not the contract.
      // Its next request is built from the transcript after this event, so the entry appended here is in it.
      pi.on("session_compact", () => {
        const message = plan.turnMessage();
        if (message) this.session.sessionManager.appendCustomMessageEntry(message.customType, message.content, message.display, message.details);
      });
      // Pi's own tree navigation can move to another branch without appending an entry or going through rewind.
      pi.on("session_tree", () => { plan.refresh(); });
      pi.registerTool(planSubmitTool(() => plan));
      if (this.options.ui !== "plain") {
        this.planReview = new PlanReviewUi(plan, {
          model: () => this.session.model ? `${this.session.model.provider}/${this.session.model.id}` : undefined,
          selectModel: () => this.panels?.selectModel() ?? false,
          notice: text => { this.addMark({ kind: "note", tone: "text", at: Date.now(), text }); },
        });
        this.planReview.install(pi);
      }
      pi.registerCommand("plan", {
        description: "Toggle plan mode; /plan <request> starts planning it, /plan off leaves",
        handler: async (args, ctx) => {
          try {
            const result = await plan.command(args);
            if (!result.review) this.notice(result.text, "text", ctx.ui);
            else if (!await this.planReview?.show()) {
              // Without the pager the plan is printed and the same choices are a plain selection, for the
              // submission that was asked about. An approval is of what was shown: with no copy to show, only
              // Keep planning is offered.
              const review = plan.review, text = plan.reviewText();
              if (text !== undefined) this.notice(`${result.text}\n\n${text}`, "text", ctx.ui);
              const options = PLAN_OPTIONS.filter(candidate => text !== undefined || candidate.id === "revise");
              const picked = await ctx.ui.select(text === undefined
                ? `${result.text} Its submitted copy is missing or was changed.` : result.text, options.map(option => option.label));
              const option = options.find(candidate => candidate.label === picked);
              if (option && plan.review === review) await plan.choose(option.id);
            }
          } catch (error) {
            this.notice(error instanceof Error ? error.message : String(error), "error", ctx.ui);
          }
        },
      });
    }
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
    const unsubscribeLedger = this.ledger?.onChange(() => { this.liveEvents.stateChanged(); this.panels?.refresh(); });
    if (unsubscribeLedger) pi.on("session_shutdown", () => { unsubscribeLedger(); });
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
      pi.on("session_shutdown", (event) => {
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
    const shellRefusal = this.options.modules.role.shellRefusal;
    const { cwd, workspaceRoot, identity, build } = this.roleContext;
    const assistantWorkspace = identity.role === "assistant" && workspaceRoot && existsSync(join(workspaceRoot, ".corral-next", "corral.toml"))
      ? workspaceRoot : undefined;
    const policy = identity.role === "build" ? buildParentPolicy(this.roleContext, shellRefusal, this.controls.bashRules, plan) : {
      fixed: shellRefusal && ((command: string) => shellRefusal(command, build?.session.bare === true)),
      writeScope: identity.role === "assistant" ? { cwd, root: cwd, workspace: assistantWorkspace } : undefined,
      rules: this.controls.bashRules, plan,
    };
    installToolControls(pi, {
      ...policy,
      ...(this.hooks ? { hooks: this.hooks } : {}),
      ...(this.toolApprovals ? { approvals: this.toolApprovals } : {}),
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
      const { exit } = this.buildControls;
      if (build?.session.trustedCode !== true) pi.registerCommand("finish", { description: "Choose what happens to this build branch", handler: async () => { if (!this.panels?.finish()) await exit.requestFinish(); } });
    }
    if (!this.options.identity.task) pi.registerCommand("board", { description: "Refresh board status and open Plan", handler: async () => { await this.buildControls?.board?.refresh(); this.panels?.showPlan(); } });
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
      if (this.closing || this.rewinding || this.switchingCode) return { action: "handled" as const };
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

  private syncBuildTitle(session: AgentSession): void {
    if (!this.buildControls) return;
    try { this.buildControls.setSessionTitle(session.sessionManager.getSessionName()); }
    catch (error) {
      this.options.events.emit("build_title_save_failed", { reason: utf8Prefix(String(error), 512) });
    }
  }

  private watch(session: AgentSession): void {
    this.unsubscribe?.();
    this.unobserveTranscript?.();
    // Reconcile names written before Corral mirrored Pi's session_info into the Build journal.
    this.syncBuildTitle(session);
    this.unobserveTranscript = observeTranscriptEntries(session.sessionManager, () => {
      this.liveEvents.entry();
      // A user message is in the transcript now; an approved plan's kickoff ends its review there, and the
      // owner's own message withdraws it.
      this.planMode?.refresh();
    }, () => this.liveEvents.stateChanged(), message => {
      // Only now is the input delivered for whoever waits on it: the append has succeeded.
      const arrived = this.arriving;
      if (arrived?.message !== message) return;
      this.arriving = undefined;
      const delivery = this.deliveries.get(arrived.input);
      if (delivery) {
        this.deliveries.delete(arrived.input);
        delivery(true);
      }
    });
    if (this.planMode) {
      // Claim the selection before Pi awaits auth. Approval and model selection cannot both win.
      const selecting = async <T>(select: () => Promise<T>): Promise<T> => {
        if (this.planMode?.approving || (session.isStreaming && this.planMode?.approved)) {
          throw new BridgeRefusal("remote_model_selection_unavailable", "Wait for plan execution to settle before changing models");
        }
        this.modelSelections++;
        try { return await select(); } finally { this.modelSelections--; }
      };
      const set = session.setModel.bind(session), cycle = session.cycleModel.bind(session);
      session.setModel = (...args) => selecting(() => set(...args));
      session.cycleModel = (...args) => selecting(() => cycle(...args));
    }
    this.startedInputFacts = undefined;
    this.inAutomation = false;
    this.remoteUsage = undefined;
    session.setQueueObserver({
      ...this.queue.observer,
      lifecycle: event => {
        this.queue.observer.lifecycle(event);
        const facts = this.queue.inputFacts(event.tag);
        // Pi reports "started" just before prompting with the prepared user message, which it emits before
        // draining any queued inputs. Only the prompt that owns the run reports it: the patched Pi refuses a
        // second prompt whose preflight overlapped before it starts, so one slot is enough.
        if (event.event === "started") this.startedInputFacts = facts;
        else if (event.event === "rejected" && this.startedInputFacts === facts) {
          this.startedInputFacts = undefined;
        }
        if (event.event === "rejected") this.admissions.get(event.tag)?.(event.reason ?? "rejected");
      },
      // Pi asks in the tick the run starts. What an answer to a plan review was given for was checked before
      // Pi's preflight, and a whole other run can have come and gone since.
      admit: tag => {
        const facts = this.queue.inputFacts(tag);
        if (this.automationRetired) return "retired";
        if (this.switchingCode) return "code_handoff_in_progress";
        return (facts && this.pendingAutomation.has(facts) ? this.automationRefusal() : undefined)
          ?? this.planAnswerRefused(facts?.planAnswer);
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
          // Even a queued owner or ordinary auto message ends the trusted scope.
          this.inAutomation = tagged !== undefined && this.automationInputs.delete(tagged);
          // The owner's own message answers a pending plan review as feedback. Like the kickoff, that is a fact
          // of the recorded message: the review comes back if a rewind or a tree move takes the message away.
          const feedback = tagged?.origin === "owner" || tagged?.origin === "pane" ? this.planMode?.review : undefined;
          Object.assign(event.message, { corralInput: tagged?.facts ?? { origin: "auto" },
            ...(tagged?.transcriptId === undefined ? {} : { corralRequestId: tagged.transcriptId }),
            // Only the message the host itself admitted as an approved plan's kickoff carries this; its text proves nothing.
            ...(tagged?.planKickoff === undefined ? {} : { [PLAN_KICKOFF]: tagged.planKickoff }),
            ...(feedback === undefined ? {} : { [PLAN_FEEDBACK]: feedback }) });
          // Its input is delivered once Pi has appended this very message, which it does next.
          this.arriving = tagged && { message: event.message, input: tagged };
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
        this.inAutomation = false;
        this.turnEnded(event.aborted);
        this.planMode?.turnEnded();
        for (const closed of this.closeWaiters.splice(0)) closed();
        this.remoteBuild.refreshExit();
      } else if (event.type === "session_info_changed") {
        // InteractiveMode /name emits this after appending session_info. It is not a prompt.
        this.syncBuildTitle(session);
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
      if (event.type === "tool_execution_end" || event.type === "agent_settled") void this.workspaceGit.refresh();
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

  get servesChildren(): boolean { return !!(this.buildControls || this.fleet || this.auxiliary || this.retainedChildren); }

  private transcriptBindingCurrent(): boolean {
    return this.bound && !this.closing && !this.rewinding && this.session.sessionId === this.boundSessionId
      && this.session.sessionFile === this.boundSessionFile;
  }

  private resolveAgentRecording(id: string, allowParent: boolean): TranscriptRecording | undefined {
    if (!this.transcriptBindingCurrent()) return undefined;
    if (id === this.boundSessionId) return allowParent && this.boundSessionFile
      ? { file: this.boundSessionFile, session: id } : undefined;
    const root = this.transcriptRoot;
    if (!root) return undefined;
    const current = (this.buildControls?.fleet ?? this.fleet)?.snapshot().children.find(entry => entry.id === id);
    if (current) {
      if (current.sessionFile && current.sessionId !== undefined) {
        return resolveChildRecording(root, id, { file: current.sessionFile, session: current.sessionId });
      }
      const recording = this.retainedChildren?.recording(id, false);
      return recording?.file === current.sessionFile ? recording : undefined;
    }
    const helper = this.auxiliary?.inspection().find(entry => entry.request === id);
    if (helper) {
      if (helper.kind === "web_search" || helper.parent !== this.boundSessionId) return undefined;
      if (helper.sessionFile) return resolveChildRecording(root, id, { file: helper.sessionFile, session: helper.request });
    }
    const archived = this.archivedFleet.find(entry => entry.id === id);
    if (archived) {
      const file = archived.sessionFile ?? join(root, "children", `${id}.jsonl`);
      if (archived.sessionId !== undefined) return resolveChildRecording(root, id, { file, session: archived.sessionId });
      // A known index that fails validation is not a legacy recording.
      try { parseChildId(id); } catch { return undefined; }
      if (this.retainedChildren?.has(id)) {
        const recording = this.retainedChildren.recording(id);
        return recording?.file === file ? recording : undefined;
      }
      return resolveChildRecording(root, id, { file, legacyBuild: true });
    }
    return this.retainedChildren?.recording(id);
  }

  childTranscript(child: string): string | undefined {
    return this.resolveAgentRecording(child, false)?.file;
  }

  private get recordsTurnMarks(): boolean { return this.attached === "headless" || this.options.ui !== "plain"; }

  get remoteActionsReady(): boolean { return this.bound && !this.closing && !this.rewinding && !this.switchingCode; }

  installRemoteParts(): void {
    const registry = this.remoteRegistry, build = this.buildControls, commands = this.commands;
    if (this.servesChildren) registerChildren(registry, {
      snapshot: () => this.readRemoteFleet(), helpers: () => this.auxiliary?.inspections() ?? [],
      fleet: () => build?.fleet ?? this.fleet, ready: () => this.remoteActionsReady,
    });
    registerJobs(registry, {
      list: () => jobsPart(commands, action => this.remoteJobAllowed(action)),
      read: commands ? (job, args) => {
        if (!commands.owns(job) && !commands.recovered().some(row => row.job === job)) {
          throw new BridgeRefusal("job_missing", "No such job in this session");
        }
        return commands.output(job, args);
      } : undefined,
      act: async (job, action) => this.remoteActionsReady && this.controlJob
        ? this.controlJob({ job, session: this.sessionId() }, action) : { state: "denied", reason: "Session is not accepting job actions" },
    });
    this.remoteChanges.register(registry);
    registry.register({ name: "usage", parse: () => undefined, run: () => usagePart(
      this.remoteUsageBrief(), this.readRemoteFleet()?.children ?? [], this.selection?.model,
      this.session.supportsThinking() ? this.session.thinkingLevel : undefined, build ? this.panelSnapshot() ?? undefined : undefined,
      this.remoteHelpers,
    ) });
    const ledger = this.ledger;
    registerWorkspace(registry, {
      context: () => this.remoteContext(), git: this.workspaceGit, ledger: ledger ? () => this.ledger ?? ledger : undefined,
      history: () => planHistory(join(this.options.build?.dir ?? this.options.runtimeDir ?? this.options.agentDir, "todo.json")),
    });
    registerModels(registry, () => modelsPart(this.session, this.remoteDefaultModel, interactiveModels(this.options.identity.role), this.remoteCatalog,
      this.modelFavorites), this.modelFavorites);
    if (build) registerBoard(registry, {
      board: build.board, record: build.liveRecord, ready: () => this.remoteActionsReady,
      unavailable: build.board && this.options.build?.session.workspaceRoot ? undefined : "This build has no board connection",
    });
    registerCommands(registry, {
      list: () => this.remoteCommands(),
      ready: () => this.remoteActionsReady && !this.open && !this.session.isStreaming && !this.session.isCompacting,
      compact: instructions => this.session.compact(instructions),
      export: this.exportable() ? () => this.reportedExport() : undefined,
    });
    if (this.planMode) registerPlanMode(registry, this.planMode);
  }

  private remoteCommands() {
    return commandsPart(this.remoteRegistry, { extensions: this.session.extensionRunner.getRegisteredCommands(),
      prompts: this.session.promptTemplates, rewind: this.checkpoints !== undefined,
      skills: this.settings.enableSkillCommands ? this.runtime.services.resourceLoader.getSkills().skills : [] });
  }

  private remoteContext(): WorkspaceInputs {
    const { worktree, branch } = this.workspaceGit;
    return { cwd: this.options.cwd, worktree, branch, context_files: this.guidance.contextFiles.map(file => file.path),
      skills: this.runtime.services.resourceLoader.getSkills().skills.map(skill => skill.name),
      prompts: this.session.promptTemplates.map(prompt => prompt.name), tools: this.session.getActiveToolNames(), mcp: this.mcp?.state() };
  }

  private remoteJobAllowed(action: "stop" | "background"): boolean {
    const tool = action === "stop" ? "job_kill" : "bash";
    return this.remoteActionsReady && this.loadout.includes(tool) && this.session.getActiveToolNames().includes(tool);
  }

  remoteUiContext() {
    return this.dialogs.context(this.session.extensionRunner.getUIContext(), (text, tone) => {
      this.addMark({ kind: "note", at: Date.now(), text, tone: tone === undefined || tone === "info" ? "text" : tone });
    });
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
    this.dialogs.close();
    clearTimeout(this.remoteJobSample);
    this.remoteChanges.close();
    this.workspaceGit.close();
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
    this.modelFavorites.close();
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
    this.boundSessionFile = sessionFile;
    const commandFacts: CommandFact[] = [], monitorFacts: MonitorFact[] = [];
    for (const entry of this.session.sessionManager.getEntries()) {
      if (entry.type === "custom" && entry.customType === COMMAND_ENTRY) commandFacts.push(entry.data as CommandFact);
      if (entry.type === "custom" && entry.customType === MONITOR_ENTRY) monitorFacts.push(entry.data as MonitorFact);
    }
    this.options.herdr?.bind({ sessionId, sessionFile });
    this.options.events.emit("bound", { session_id: sessionId });
    this.commands?.bound(commandFacts, monitorFacts);
    await this.workspaceGit.refresh();
    await this.remoteChanges.captureBase(this.workspaceGit.worktree);
    await this.ledger?.restore().catch(() => false);
    await this.questionUi?.recover();
    const plan = this.planMode;
    if (plan && this.options.planMode && plan.phase === "off") {
      this.addMark({ kind: "note", tone: "text", at: Date.now(), text: await plan.enter() });
    } else if (plan?.review !== undefined) {
      this.addMark({ kind: "note", tone: "text", at: Date.now(), text: `"${plan.record.title ?? "Plan"}" is awaiting your review: /plan` });
    }
  }

  /**
   * Stop the open turn and wait until it has settled for everyone. Pi is idle before it has run its settle hooks
   * and announced the settle, and a prompt made in between is deferred behind whatever else was deferred there;
   * the kickoff of an approved plan must not join that list.
   */
  private async planStopTurn(): Promise<void> {
    if (!this.open) return;
    // A turn Pi has already finished is not cancelled, only waited for.
    if (this.session.isStreaming) await this.cancelRun(undefined, "owner");
    if (!this.open) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      await Promise.race([
        new Promise<void>(resolve => { this.closeWaiters.push(resolve); }),
        new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error("the running turn did not settle")), 30_000); }),
      ]);
    } finally {
      clearTimeout(timer);
    }
  }

  /** Summarise the planning discussion before an approved plan runs; a session too small to compact is left as it is. */
  private async planCompact(instructions: string): Promise<void> {
    try { await this.session.compact(instructions); }
    catch (error) { if (!uncompactable(error)) throw error; }
  }

  /** Replace the model's context with `summary`. Where the SDK declines to compact, the same entry is appended directly. */
  private async planResetContext(summary: string): Promise<void> {
    this.planReset = summary;
    try { await this.session.compact(); }
    catch (error) {
      if (!uncompactable(error)) throw error;
      this.session.sessionManager.appendCompaction(summary, null, this.session.getContextUsage()?.tokens ?? 0,
        { corral_plan_reset: true }, true);
      this.session.refreshContext();
    } finally {
      this.planReset = undefined;
    }
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

  private automationRefusal(): string | undefined {
    if (!this.options.automation || this.sessionsCreated !== 1) return "unbound_automation";
    if (this.automationRetired) return "retired";
    if (this.options.paused?.()) return "paused";
    if (this.ownerQuestions?.pending().length || this.planMode?.review) return "waiting";
    if (this.open || this.session.isStreaming || this.session.isCompacting || this.rewinding) return "busy";
  }

  submitAutomation(input: SessionInput, requestId: string, nonce: unknown): Promise<SubmitResult> {
    if (input.origin !== "auto" || input.to !== undefined || input.attachments?.length) {
      return Promise.resolve({ disposition: "rejected", reason: "invalid_automation_input" });
    }
    const refused = this.automationRefusal();
    if (refused) return Promise.resolve({ disposition: "rejected", reason: refused });
    const root = this.options.automationRoot;
    if (!root || typeof nonce !== "string" || nonce.length !== 32 || /[^0-9a-f]/.test(nonce)) {
      return Promise.resolve({ disposition: "rejected", reason: "invalid_automation_permit" });
    }
    const permit = join(root, ".corral-next", "automation", `.admit-${nonce}`);
    // Prefix data before Pi's slash-command, skill, and template dispatch.
    return this.submitOpened({ ...input, busyPolicy: "prompt", text: `Automation trigger (untrusted data):\n${input.text}` },
      undefined, { permit, requestId });
  }

  retireAutomation(): { retired: boolean; reason?: string } {
    if (this.automationRetired) return { retired: true };
    const reason = this.automationRefusal();
    if (reason) return { retired: false, reason };
    if (this.queue.count("steer") || this.queue.count("follow_up")) return { retired: false, reason: "busy" };
    this.automationRetired = true;
    return { retired: true };
  }

  submitLocal(input: SessionInput, images?: ImageContent[], source?: {
    requestId?: string; queueKey?: string; text?: string; pane?: boolean; planKickoff?: string; planAnswer?: PlanAnswer;
    permit?: string;
    /** Told once whether this very input reached the transcript. */
    delivered?: (delivered: boolean) => void;
  }): Promise<SubmitResult> {
    if (this.automationRetired) return Promise.resolve({ disposition: "rejected", reason: "retired" });
    if (!this.bound || this.closing) return Promise.resolve({ disposition: "rejected", reason: "host_not_ready" });
    if (this.rewinding) return Promise.resolve({ disposition: "rejected", reason: "rewind_in_progress" });
    if (this.switchingCode) return Promise.resolve({ disposition: "rejected", reason: "code_handoff_in_progress" });
    const socketId = input.inputId ?? source?.requestId;
    const transcriptId = input.inputId ?? (input.origin === "owner" ? source?.requestId : undefined);
    if (input.origin === "owner" && (input.inputId || source?.requestId) && !source?.pane && !images?.length && !input.attachments?.length) {
      const action = typedSessionAction(this.remoteRegistry, input.text);
      if (action) return action.then<SubmitResult, SubmitResult>(result => {
        if ("text" in result && typeof result.text === "string") this.addMark({ kind: "note", tone: "text", at: Date.now(), text: result.text });
        return { disposition: "handled" };
      }, error => ({ disposition: "rejected", reason: utf8Prefix(error instanceof Error ? error.message : String(error), 256) }));
    }
    const { delivered, permit, ...tagged } = source ?? {};
    const tag = this.queue.bridgeInput(input, { ...tagged, socketId, transcriptId });
    const facts = this.queue.inputFacts(tag);
    if (permit && facts) this.pendingAutomation.add(facts);
    return this.promptInput(input, tag, images, delivered, permit ? () => {
      if (!facts) throw new Error("invalid_automation_input");
      // Pi calls this synchronously just before agent.prompt, after all async run preparation.
      if (this.options.paused?.()) throw new Error("paused");
      if (this.automationRetired) throw new Error("retired");
      if (this.ownerQuestions?.pending().length || this.planMode?.review) throw new Error("waiting");
      try { unlinkSync(permit); } catch { throw new Error("automation_permit_missing"); }
      this.automationInputs.add(facts);
    } : undefined).then(result => {
      if (input.origin === "owner" && (result.disposition === "started" || result.disposition === "queued")) {
        this.buildControls?.noteOwnerInput(source?.text ?? input.text);
      }
      return result;
    });
  }

  private promptInput(input: SessionInput, queueTag: unknown, images?: ImageContent[],
    delivery?: (delivered: boolean) => void, admit?: () => void): Promise<SubmitResult> {
    const { promise, resolve } = Promise.withResolvers<SubmitResult>();
    if (delivery) this.deliveries.set(queueTag, delivery);
    const settle = (result: SubmitResult) => {
      this.admissions.delete(queueTag);
      resolve(result);
    };
    const refuse = (message: string) => settle({ disposition: "rejected",
      reason: /already processing/i.test(message) ? "busy" : message.slice(0, 256) });
    // Pi defers a prompt made while it settles a turn: that call returns at once, and a refusal of the deferred
    // prompt is reported only through the queue's lifecycle. Without this the admission would never be answered.
    this.admissions.set(queueTag, refuse);
    this.session.prompt(input.text, {
      source: "rpc", queueTag, ...(images ? { images } : {}),
      ...(input.busyPolicy === "steer" ? { streamingBehavior: "steer" as const } : {}),
      ...(input.busyPolicy === "follow_up" ? { streamingBehavior: "followUp" as const } : {}),
      preflightResult: (disposition) => {
        if (disposition !== "started") {
          settle({ disposition });
          return;
        }
        admit?.();
        this.nextTurnOrigin = { kind: "bridge" };
        settle({ disposition, turn: this.turn + 1 });
      },
    }).catch((error: unknown) => {
      refuse(error instanceof Error ? error.message : String(error));
    }).finally(() => {
      // The run is over, or never was: an input that did not reach the transcript by now never will.
      if (delivery && this.deliveries.get(queueTag) === delivery) {
        this.deliveries.delete(queueTag);
        delivery(false);
      }
    });
    return promise;
  }

  /**
   * Start input now or refuse it, and report a start only once that very input is in the transcript. Pi
   * reports a start before its core run has taken the prompt, and another prompt can still win that run; the
   * turn is open by the time the input is recorded, so a caller ordering admissions against pause leaves no
   * gap in which a started run cannot be cancelled.
   */
  async submitOpened(input: SessionInput, answers?: PlanAnswer, source?: { permit: string; requestId: string }): Promise<SubmitResult> {
    // Checked in the same tick as the call into Pi. A turn that is open for the host is running or still
    // settling, and Pi would queue this input or defer it into its settle list, where it can be dropped.
    if (this.open) return { disposition: "rejected", reason: "busy" };
    // A rewind or a turn ordered ahead of this answer may have changed what it was given for.
    const refused = this.planAnswerRefused(answers);
    if (refused) return { disposition: "rejected", reason: refused };
    const delivered = Promise.withResolvers<boolean>();
    const result = await this.submitLocal(input, undefined, { ...source, ...(answers ? { planAnswer: answers } : {}),
      ...(answers?.kickoff ? { planKickoff: answers.card } : {}), delivered: delivered.resolve });
    if (result.disposition !== "started") return result;
    return await delivered.promise ? result : { disposition: "rejected", reason: "busy" };
  }

  /**
   * Why an input that answers a plan review cannot start: the review is no longer the open one, or the model's
   * context is not the one the answer was given in. A clean context in particular is the plan and nothing else.
   */
  private planAnswerRefused(answer: PlanAnswer | undefined): string | undefined {
    if (answer === undefined) return undefined;
    if (this.planMode?.review !== answer.card) return "the plan review this answers is no longer open";
    const model = this.session.model;
    if (answer.kickoff && (this.modelSelections || answer.model !== (model && `${model.provider}/${model.id}`))) {
      return "the model selected for this approval changed";
    }
    return this.contextTip() === answer.context ? undefined : "the conversation went on after this answer to the plan review";
  }

  /** The last entry of the current branch that the model's context is built from. */
  private contextTip(): string | undefined {
    return this.session.sessionManager.getBranch().findLast(entry => entry.type === "message" || entry.type === "custom_message"
      || entry.type === "compaction" || entry.type === "branch_summary" || entry.type === "context_edit")?.id;
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
    // Only the structured pane/app answer flow may authorize a tool call; ordinary chat is not consent.
    if (!pending || !record || records.length !== 1 || !question || record.questions.length !== 1 ||
        isToolApproval(record) || text.startsWith("/")) return false;
    const option = text.trim();
    const answer = question.options.includes(option) ? { option } : { text };
    if (!validAnswer(question, answer)) return false;
    await pending.answer(record.id, [answer]);
    return true;
  }

  /** Select a model in the provider catalog already configured for this process. */
  async selectRemoteModel(name: string, effort?: string): Promise<void> {
    if (!this.remoteActionsReady || this.open || this.session.isStreaming || this.session.isCompacting) {
      throw new BridgeRefusal("remote_model_selection_unavailable", "Wait for the current session action to finish");
    }
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
      // The branch may hold another plan review now, or none: an open review must not go on showing the old one.
      this.planMode?.refresh();
      this.toolApprovals?.clear();
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
      this.remoteFleet = mergeArchivedChildren((this.buildControls?.fleet ?? this.fleet)?.snapshot(), this.archivedFleet);
      this.remoteHelpers = this.auxiliary?.inspections() ?? [];
      this.liveEvents.observeFleet(this.remoteFleet, this.remoteHelpers);
      if (this.servesChildren) this.remoteChildPart = childDetailsChanged(this.remoteFleet?.children ?? [], this.remoteHelpers);
    }
    return this.remoteFleet;
  }

  remoteState(owner: RemoteOwner): StateProjection {
    const session = this.session;
    const build = this.buildControls;
    const effort_levels = session.supportsThinking() ? session.getAvailableThinkingLevels() : [];
    const model = session.model ? utf8Prefix(`${session.model.provider}/${session.model.id}`, 512) : undefined;
    const { queue, queue_total } = owner.queueBrief();
    const entries = sortedCards(this.dialogs.entries());
    const cards = briefedCards(entries);
    const exitState = this.remoteBuild.exitState();
    const exit = exitState ? exitBrief(exitState) : undefined;
    const board = build?.board ? boardBrief(build.board) : undefined;
    const plan = this.ledger ? planBrief(this.ledger) : undefined;
    const jobs = this.commands?.inspection() ?? [];
    const recovered = this.commands?.recovered() ?? [];
    const children = this.readRemoteFleet()?.children ?? [];
    const parent = this.remoteUsageBrief(), usageTotalRow = usageTotal(parent, children, this.remoteHelpers.length > 0);
    const { context_tokens, context_window, context_percent } = parent;
    const usage = { tokens: usageTotalRow.tokens, cost_usd: usageTotalRow.cost_usd, cost_partial: usageTotalRow.partial,
      context_tokens, context_window, context_percent };
    if (!this.closing && !this.remoteJobSample && jobs.some(job => job.state === "running")) {
      // Job output can grow outside tool events, including without any complete output line.
      this.remoteJobSample = setTimeout(() => { this.remoteJobSample = undefined; this.liveEvents.stateChanged(); }, 750);
      this.remoteJobSample.unref();
    }
    const state: SessionState = {
      mode: this.attached ?? "headless", turn: this.turn, open: this.open, paused: owner.paused,
      activity: this.liveEvents.activity, last_error: this.turnError ?? undefined, model,
      effort: effort_levels.length ? session.thinkingLevel : undefined, effort_levels, usage,
      queue, queue_total, cards, exit, children: this.liveEvents.childCounts, jobs: jobCounts(jobs, recovered.length),
      board, plan, plan_mode: this.planMode?.brief(), changes: this.remoteChanges.count, notices: this.liveEvents.notices, can: capabilities(this.remoteRegistry, {
        open: this.open || session.isStreaming || session.isCompacting, closing: this.closing, rewind: this.checkpoints !== undefined,
      }),
    };
    const journal = this.options.build ? statSync(join(this.options.build.dir, "journal.json"), { throwIfNoEntry: false }) : undefined;
    return { state, parts: {
      cards: entries.map(entry => cardDetail(entry.card)),
      children: this.remoteChildPart,
      jobs: [jobsPart(this.commands, action => this.remoteJobAllowed(action), jobs, recovered), this.commands?.state()],
      changes: this.remoteChanges.version,
      usage: [model, state.effort, parent, children.map(child => [child.id, child.usage]), this.remoteHelpers.map(helper => helper.request)],
      context: contextPart(this.remoteContext()),
      plan: this.ledger ? [this.ledger.current, this.ledger.unavailable, this.ledger.continued] : undefined,
      board: build?.board ? [build.board.tasks(), build.board.proposed()] : undefined,
      record: journal ? [journal.ino, journal.mtimeMs, journal.size, this.session.sessionManager.getLeafId()] : undefined,
      models: modelsPart(session, this.remoteDefaultModel, interactiveModels(this.options.identity.role), this.remoteCatalog, this.modelFavorites),
      commands: this.remoteCommands(),
    } };
  }

  private remoteUsageBrief(): ParentUsage {
    const session = this.session, model = session.model;
    const leaf = session.sessionManager.getLeafId();
    if (this.remoteUsage?.leaf === leaf && this.remoteUsage.model === model && this.remoteUsage.usage.turns === this.turn) return this.remoteUsage.usage;
    const stats = session.getSessionStats();
    const cost_partial = stats.tokens.total > 0 && session.sessionManager.getEntries().some(entry => {
      if (entry.type !== "message" || entry.message.role !== "assistant") return false;
      const { usage } = entry.message;
      return usage.totalTokens > 0 && (!Number.isFinite(usage.cost.total) || usage.cost.total === 0);
    });
    const usage: ParentUsage = { tokens: stats.tokens.total, cost_partial, input: stats.tokens.input,
      output: stats.tokens.output, turns: this.turn,
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
    const build = this.buildControls?.panelState();
    const fleet = mergeArchivedChildren(build?.fleet ?? this.fleet?.snapshot(), this.archivedFleet);
    return {
      ...(build && fleet ? { build: { ...build, fleet } } : fleet ? { fleet } : {}),
      bound: this.bound, model: session.model ? `${session.model.provider}/${session.model.id}` : null,
      effort: session.thinkingLevel, effortAvailable: session.supportsThinking(),
      ...(this.terminalInput ? { input: this.terminalInput } : {}),
      streaming: session.isStreaming, context: session.getContextUsage(),
      ...(this.panelStats ??= panelStats(session.getSessionStats())), tools: session.getActiveToolNames(),
      contextFiles: this.guidance.contextFiles.map(file => file.path),
      skills: this.runtime.services.resourceLoader.getSkills().skills.map(skill => skill.name),
      ledger: this.ledger?.current ?? null, ledgerStale: this.ledger?.unavailable ?? true,
      ledgerContinued: this.ledger?.continued ?? null,
      ...(this.planMode && this.planMode.phase !== "off" ? { planMode: this.planMode.phase } : {}),
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
          parent: child.parent, kind: `native ${child.kind}`, state: child.state, model: child.model, transcript: child.kind !== "web_search" && !!child.sessionFile && child.parent === this.boundSessionId }))],
      jobs, auxiliary: this.auxiliary?.state() ?? null, helpers: this.auxiliary?.inspections() ?? null,
      steering: session.getSteeringMessages().length, followUps: session.getFollowUpMessages().length,
      queue: this.queue.items(),
    };
  }

  private async readAgentTranscript(id: string, request: TranscriptRequest): Promise<TranscriptPage> {
    const recording = this.resolveAgentRecording(id, true);
    if (!recording) throw new Error("Transcript unavailable: selected agent has no recording in this bound session");
    const page = await readTranscriptPage(recording.file, recording.session, request);
    if (!this.transcriptBindingCurrent()) throw new Error("Transcript unavailable: stale host");
    if (id !== this.boundSessionId && confinedRegularFile(realpathSync(this.transcriptRoot!), recording.file) !== recording.file) {
      throw new Error("Transcript unavailable: recording left the bound session");
    }
    return page;
  }

  /** Protocol v2 status: at most 8 KiB of JSON, independent of diagnostic state. */
  controlState(paused: boolean) {
    const session = this.session;
    const identity = { bound_id: this.boundSessionId ?? null, current_id: session.sessionId, created: this.sessionsCreated };
    const questions = this.ownerQuestions?.pending() ?? [];
    return {
      role: this.options.identity.role,
      bound: this.bound,
      interactive: this.attached === "interactive",
      mode: this.attached ?? null,
      // An imported transcript can have an arbitrary ID. Never pass off a prefix as its identity.
      ...(identity.current_id.isWellFormed() && (identity.bound_id?.isWellFormed() ?? true) && jsonBytes(identity) <= 1024
        ? { session: identity } : {}),
      streaming: session.isStreaming,
      turn: { current: this.turn, open: this.open },
      paused,
      last_error: this.turnError === null ? null : clipText(this.turnError.toWellFormed(), 512),
      last_seq: this.options.events.last,
      model: session.model ? clipText(`${session.model.provider}/${session.model.id}`.toWellFormed(), 512) : null,
      owner_questions_total: questions.length,
      owner_questions: questions.slice(0, PENDING_MAX).map(({ id, questions: prompts, status }) => ({
        // The original `answer` command takes one choice. Use cards for multi-question records.
        ...(prompts.length === 1 && /^[A-Za-z0-9._:-]{1,160}$/.test(id) ? { id } : {}),
        // Rust decodes Unicode scalar values, not isolated UTF-16 code units.
        question: clipText(prompts[0]!.question.toWellFormed(), 512), status,
      })),
    };
  }

  /** In-process diagnostics; not a session socket response. */
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

