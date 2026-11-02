// One upstream AgentSessionRuntime under the role's restrictions. It reports host turns
// (agent_start to agent_settled) and provider errors to corral as `event` calls, and answers
// bridge input with upstream's own dispositions: started, queued, handled or rejected.
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

import type * as Sdk from "@earendil-works/pi-coding-agent";
import type {
  AgentSession, AgentSessionRuntime, CreateAgentSessionRuntimeFactory, ExtensionAPI, SessionManager, ToolDefinition,
} from "@earendil-works/pi-coding-agent";

import type { StreamFunction } from "@earendil-works/pi-ai";
import { lazyStream } from "@earendil-works/pi-ai/api/lazy";
import { getApiProvider } from "@earendil-works/pi-ai/compat";

import type { AwsBinding, AwsCredentialResolver } from "../auth/aws.ts";
import { RunBudget } from "../capabilities/budget.ts";
import { BashPermissions } from "../capabilities/bash-rules.ts";
import { compactionSettings } from "../capabilities/compaction.ts";
import { codemodeTool } from "../capabilities/codemode.ts";
import { nestedChild } from "../extensions/codemode/child.ts";
import { CODE_CALL_ENTRY, CodeRuntime } from "../extensions/codemode/runtime.ts";
import { HOOK_ENTRY, Hooks } from "../capabilities/hooks.ts";
import { CHECKPOINT_ENTRY, Checkpoints } from "../capabilities/checkpoints.ts";
import { type HostControls, installToolControls, parseControls } from "./tool-controls.ts";
import { type CapabilityGuidance, type Guidance, selectGuidance } from "../capabilities/guidance.ts";
import { PROVIDERS, type ProviderId, hostDestinations } from "../auth/providers.ts";
import type { AuthContext } from "../auth/types.ts";
import type { EventLog } from "../bridge/events.ts";
import { CLAUDE_SDK_API, type ClaudeLaneConfig, registerClaudeSdkApi } from "../claude/stream.ts";
import type { QuestionUpdate, SubmitInput, SubmitResult } from "../bridge/protocol.ts";
import type { DeliveryHost } from "../delivery/types.ts";
import { scratchRoots, writeSettings } from "../roles/assistant/paths.ts";
import type { HostIdentity, RoleContext } from "../roles/types.ts";
import { type CorralClient, type CorralReceipt, eventRequestId, sha256Hex } from "../tools/corral.ts";
import { corralToolNames, defineCorralTool } from "../tools/define.ts";
import { installChrome } from "../ui/chrome.ts";
import { NativePanels } from "../ui/panels.ts";
import type { PanelJob, PanelSnapshot } from "../ui/panels-view.ts";
import { InspectorActivity } from "../ui/inspector.ts";
import { corralRenderers } from "../ui/render.ts";
import { TodoLedger, installTodo } from "../ui/todo.ts";
import { type DueContinuation, RequestContext, continuationText } from "./request.ts";
import type { HostCapabilities, UiMode } from "./config.ts";
import { EXPORT_COMMAND, type SessionExport, exportSnapshot } from "./export.ts";
import { askUserEnabled, askUserTool } from "./ask-user.ts";
import { type HerdrLifecycle, answeredKeys } from "./herdr.ts";
import type { LoadedModules } from "./modules.ts";
import { type GuardedFetch, guardedFetch, resourceLoaderOptions, restrictedSettings } from "./restrict.ts";
import { createModelCatalog } from "./catalog.ts";
import { installModelSelection, subscriptionRoutes } from "./subscriptions.ts";
import { observeTerminalInput, type InputObservation } from "./input-observation.ts";
import { browserSettings, browserTools } from "../extensions/browser/index.ts";
import { createLocalModels } from "./local-models.ts";
import { webTools } from "../extensions/web/index.ts";
import { runSubscriptionSearch } from "../extensions/web/subscription.ts";
import { streamClaudeSearch } from "../claude/search.ts";
import { configuredServers, lspToolkit, type LspToolkit } from "../extensions/lsp/index.ts";
import { capabilityRegistry } from "../extensions/registry.ts";
import { McpAccess } from "../capabilities/mcp.ts";
import { MCP_CONTROLS } from "./mcp-tools.ts";
import { toolSearch } from "./tool-search.ts";
import { JOB_MONITOR_TOOL, jobTools } from "../extensions/jobs/index.ts";
import { JobRuntime } from "../extensions/jobs/runtime.ts";
import { JobSettingsSchema } from "../extensions/jobs/settings.ts";
import { toolRequestId } from "../tools/corral.ts";
import { AuxiliaryRuntime } from "../extensions/subagent/runtime.ts";
import { auxiliarySettings } from "../extensions/subagent/settings.ts";
import { exploreTool } from "../extensions/subagent/index.ts";
import { oracleTool } from "../extensions/oracle/index.ts";
import { runAuxiliarySession } from "../extensions/subagent/session.ts";
import { executionRefusal, nestedCorralRead } from "../roles/common/execution.ts";
import { OWNER_INPUT_TEXT_MAX, ownerInputText, utf8Prefix } from "../roles/assistant/owner-action.ts";
import type { ConfirmUi } from "../tools/define.ts";
import { memoryContext } from "./memory.ts";

export interface HostSessionOptions {
  readonly sdk: typeof Sdk;
  readonly identity: HostIdentity;
  readonly cwd: string;
  readonly agentDir: string;
  readonly model: string;
  /** The bound profile's provider; absent means OpenAI Codex, the only pre-T-479 route. */
  readonly provider?: ProviderId;
  /** The Bedrock binding and its credential resolver; required for an AWS-profile provider. */
  readonly aws?: AwsRoute;
  readonly effort?: string;
  readonly promptFile?: string;
  readonly memoryFile?: string;
  readonly todo?: boolean;
  /** The generation's recorded snapshot; when present it decides todo. */
  readonly capabilities?: HostCapabilities;
  /** The snapshot's settings and project-text trust; absent loads no optional guidance. */
  readonly guidance?: CapabilityGuidance;
  /** The corral workspace root, the outermost directory whose context files may load. */
  readonly root?: string;
  /** Where `~/` in a `user_dirs` entry points; absent means the user's home directory. */
  readonly home?: string;
  /** The private directory `/corral-export` writes into; absent disables the command. */
  readonly exportDir?: string;
  /** The generation's runtime directory; background job output files live under it. */
  readonly runtimeDir?: string;
  /** Absent means rich: corral's header, footer status and compact corral_* rendering. */
  readonly ui?: UiMode;
  readonly modules: LoadedModules;
  readonly client: CorralClient;
  readonly events: EventLog;
  readonly network: GuardedFetch;
  readonly sessionManager: SessionManager;
  /** The pane's herdr projection; absent for a headless host or one outside its bound pane. */
  readonly herdr?: HerdrLifecycle;
  /** Present only under the `corral pi` assistant supervisor; never switches Pi files in-process. */
  readonly newAssistant?: () => void;
}

type TurnOrigin = { kind: "bridge" | "continuation"; idempotency_key: string } | { kind: "owner_terminal" }
  | { kind: "unknown" };

const THINKING = new Set(["off", "minimal", "low", "medium", "high", "xhigh"]);
const STATE_SECTION_MAX_BYTES = 8 * 1024;
/** A lifecycle report keeps its request ID, so retrying it is safe; back off from contention. */
const REPORT_ATTEMPTS = 6;
const REPORT_BACKOFF_MS = 250;
/** Withdraw refusals that mean corral already holds the question as not pending. */
const NOT_PENDING = new Set(["question_withdrawn", "question_state", "question_closed", "question_not_found"]);

/** A module's own readback, replaced by its size once it could crowd a bridge frame. */
function bounded(value: Record<string, unknown> | null): Record<string, unknown> | null {
  const size = value === null ? 0 : Buffer.byteLength(JSON.stringify(value));
  return size > STATE_SECTION_MAX_BYTES ? { omitted: "state_too_large", bytes: size } : value;
}

type Models = Awaited<ReturnType<typeof Sdk.ModelRuntime.create>>;

export interface AwsRoute {
  readonly binding: AwsBinding;
  readonly resolver: AwsCredentialResolver;
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
  if (/\b429\b|rate.?limit|quota|usage limit/i.test(message)) return "rate_limit";
  if (/\b5\d\d\b|overload|unavailable/i.test(message)) return "overload";
  return "provider";
}

export class HostSession {
  runtime!: AgentSessionRuntime;
  readonly panels: NativePanels | undefined;
  private readonly unsubscribePanels: (() => void) | undefined;
  private terminalInput: InputObservation | undefined;
  private terminalInputSequence = 0;
  private readonly baseLoadout: readonly string[];
  private mcp: McpAccess | undefined;
  private readonly roleContext: RoleContext;
  private readonly localTools: readonly ToolDefinition[];
  private dialogPi: ExtensionAPI | undefined;
  private readonly jobs: JobRuntime | undefined;
  /** The generation's Code Mode kernel and its nested-call bridge (T-683); absent when disabled. */
  private readonly code: CodeRuntime | undefined;
  private writePermissionNotice: (() => Promise<void>) | undefined;
  private writeAbort = new AbortController();
  private turn = 0;
  private open = false;
  private cancelRequested: number | undefined;
  private nextTurnOrigin: TurnOrigin | undefined;
  private lastInputSource: string | undefined;
  /** The owner typed into the pane while the current turn was open (a steer into it). */
  private ownerInput = false;
  private boundSessionId: string | undefined;
  private unsubscribe: (() => void) | undefined;
  private calls: Promise<unknown> = Promise.resolve();
  private readonly stats = { sent: 0, committed: 0, rejected: 0, failed: 0, lastError: undefined as string | undefined };
  private deniedTools = 0;
  private inspector: InspectorActivity | undefined;
  private panelTokens: number | undefined;
  private panelTokenLeaf: string | null | undefined;
  private sessionsCreated = 0;
  /** How the bound host is attached: InteractiveMode in its pane, or headless (T-577). */
  private attached: "interactive" | "headless" | undefined;
  private guidance: Guidance = { skillPaths: [], promptPaths: [], contextFiles: [] };
  private readonly settings: ReturnType<typeof restrictedSettings> & ReturnType<typeof compactionSettings>;
  private readonly controls: HostControls;
  private readonly hooks: Hooks | undefined;
  private readonly additionalLsp: LspToolkit | undefined;
  private readonly permissions: BashPermissions | undefined;
  private readonly checkpoints: Checkpoints | undefined;
  private readonly auxiliary: AuxiliaryRuntime | undefined;
  private auxiliaryModels: Models | undefined;
  private searchModels: Models | undefined;
  private readonly budget: RunBudget;
  private ledger: TodoLedger | undefined;
  /** PM and assistant with todo on: the request summary and continuation (T-679). */
  private request: RequestContext | undefined;
  /** Continuation rechecks, kept off `calls` so lifecycle reports and budget records never wait on one. */
  private continuations: Promise<void> = Promise.resolve();
  private closing = false;

  readonly provider: ProviderId;

  private constructor(private readonly options: HostSessionOptions) {
    this.provider = options.provider ?? "openai-codex";
    this.panels = options.ui === "plain" ? undefined : new NativePanels({
      identity: options.identity, cwd: options.cwd, snapshot: () => this.panelSnapshot(),
      inspection: () => options.client.inspectionSnapshot(),
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
    this.budget = new RunBudget({
      unbounded: PROVIDERS[this.provider].lane !== undefined,
      record: async (payload) => {
        if (!this.bound) throw new Error("host_not_bound");
        await this.drained();
        const event = options.events.emit("budget", payload);
        const receipt = await options.client.call("event", {
          requestId: eventRequestId(options.identity.generation, event.seq), payload, turn: this.currentTurn(),
        });
        if (receipt.state !== "committed") throw new Error(`budget record refused: ${JSON.stringify(receipt.result)}`);
        return (receipt.result ?? {}) as Record<string, unknown>;
      },
      onStop: () => {
        if (this.open) this.report("budget_exhausted", { budget: this.budget.state() });
      },
      cancel: () => { void this.cancel(this.currentTurn()); void this.auxiliary?.cancelAll(); },
    });
    const registerCapability = capabilityRegistry(() => this.session?.sessionFile);
    this.roleContext = {
      identity: options.identity, cwd: options.cwd, model: options.model, registerCapability,
      todo: options.capabilities ? options.capabilities.enabled.includes("todo") : options.todo === true,
      ...(options.capabilities ? { capabilities: options.capabilities } : {}),
      ...(options.guidance?.settings.lsp ? { lspSettings: options.guidance.settings.lsp } : {}),
      ...(options.effort ? { effort: options.effort } : {}),
      ...(options.promptFile ? { promptFile: options.promptFile } : {}),
      ...(options.identity.role === "assistant" ? {
        scratchRoots: async () => scratchRoots(await options.client.todoSnapshot(), options.identity.assistant ?? ""),
      } : {}),
      ...(options.identity.role === "assistant" && options.capabilities?.enabled.includes("write_permissions") ? {
        writePermission: {
          settings: writeSettings(options.guidance?.settings.write_permissions),
          onBound: async (notice: () => Promise<void>) => {
            this.writePermissionNotice = notice;
            if (this.bound) await notice();
          },
          sessionId: () => this.sessionId(),
          turn: () => ({ id: this.currentTurn(), signal: this.writeAbort.signal }),
          call: (requestId: string, payload: Record<string, unknown>) => options.client.call("write_permission", {
            requestId, payload, turn: this.currentTurn(),
          }),
          emit: (kind: string, data: Record<string, unknown>) => { options.events.emit(kind, data); },
        },
      } : {}),
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
      throw new Error("module_invalid: web.enabled needs an eligible role and its generation-bound web transport");
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
      this.additionalLsp = lspToolkit({ root: options.cwd, servers: configuredServers(options.guidance?.settings.lsp), registerCapability });
    }
    this.jobs = this.jobRuntime(registerCapability);
    this.code = options.capabilities?.enabled.includes("codemode") ? new CodeRuntime({
      generation: options.identity.generation,
      emit: (kind, data) => { options.events.emit(kind, data); },
      journal: (data) => { this.session.sessionManager.appendCustomEntry(CODE_CALL_ENTRY, data); },
      // A detached cell's one notice enters like bridge input: journaled delivery, its own turn origin.
      notify: async (key, text) => {
        const sent = await this.submit({ text, busyPolicy: "follow_up", idempotencyKey: key, origin: "code-cell" }, key);
        return sent.reason === undefined ? sent.disposition : `${sent.disposition}: ${sent.reason}`;
      },
    }) : undefined;
    const subscriptionSearch = webEnabled && options.capabilities?.web?.search_provider === "subscription";
    const auxiliary = { ...auxiliarySettings(options.identity.role, options.capabilities?.enabled ?? [], options.guidance?.settings),
      ...(subscriptionSearch ? { web_search: { model: null, timeout_secs: (options.capabilities?.web?.timeout_ms ?? 15000) / 1000 } } : {}),
    };
    if (auxiliary.explore || auxiliary.oracle || auxiliary.web_search) {
      this.auxiliary = new AuxiliaryRuntime({
        settings: auxiliary, generation: options.identity.generation, provider: this.provider, model: options.model,
        sessionId: () => this.sessionId(),
        searchModel: () => {
          const model = this.runtime?.session.model;
          return model && ["openai-codex", "anthropic"].includes(model.provider) && this.searchModels?.getModel(model.provider, model.id)
            ? { provider: model.provider, model: model.id } : undefined;
        },
        record: async (payload) => {
          if (!this.bound) throw new Error("host_not_bound");
          await this.drained();
          const event = options.events.emit("auxiliary", payload);
          const receipt = await options.client.call("event", {
            requestId: eventRequestId(options.identity.generation, event.seq), payload, turn: this.currentTurn(),
          });
          if (receipt.state !== "committed") throw new Error(`auxiliary record refused: ${JSON.stringify(receipt.result)}`);
          return receipt.result as Record<string, unknown>;
        },
        execute: (invocation, signal) => {
          if (invocation.kind === "web_search") {
            if (!this.searchModels || !options.capabilities?.web) throw new Error("subscription search runtime is not ready");
            return runSubscriptionSearch({ sdk: options.sdk, models: this.searchModels, cwd: options.cwd,
              settings: options.capabilities.web }, invocation, signal);
          }
          if (!this.auxiliaryModels) throw new Error("auxiliary model runtime is not ready");
          return runAuxiliarySession({ sdk: options.sdk, models: this.auxiliaryModels, provider: this.provider,
            cwd: options.cwd, agentDir: options.agentDir, parentLoadout: this.loadout, localTools: this.localTools }, invocation, signal);
        },
      });
    }
    const dialogEnabled = askUserEnabled(options.identity.role, options.capabilities?.enabled ?? []);
    this.localTools = [...(lspEnabled ? roleLocal : roleLocal.filter((tool) => !tool.name.startsWith("lsp_"))),
      ...(dialogEnabled ? [askUserTool(async (answer) => {
        if (!this.dialogPi) throw new Error("dialog session is unavailable");
        const id = options.identity.role === "assistant" ? await this.journalOwnerInput("dialog", answer) : undefined;
        const message = id ? ownerInputText(answer, id) : answer;
        this.dialogPi.sendUserMessage(message, { deliverAs: "followUp" });
        return message;
      })] : []),
      ...(this.additionalLsp?.tools ?? []), ...(webEnabled && options.network.web ? webTools(options.network.web,
        subscriptionSearch ? async (id, query, count, signal) => {
          if (!this.auxiliary) throw new Error("subscription search is disabled");
          return (await this.auxiliary.run("web_search", id, JSON.stringify({ query, count }), signal)).text;
        } : undefined) : []),
      ...(this.jobs ? jobTools(this.jobs, options.identity.role === "worker") : []),
      ...(this.auxiliary && auxiliary.explore ? [exploreTool(this.auxiliary)] : []),
      ...(this.auxiliary && auxiliary.oracle ? [oracleTool(this.auxiliary)] : []),
      ...(browser ? browserTools(browser, { cwd: options.cwd, registerCapability }) : [])];
    const local = this.localTools.map((tool) => tool.name);
    const mcp = options.capabilities?.enabled.includes("mcp.enabled") === true;
    if (mcp !== (options.capabilities?.mcp !== undefined)) {
      throw new Error("module_invalid: mcp.enabled needs its generation-bound settings");
    }
    const search = options.capabilities?.enabled.includes("tool_search") === true;
    this.baseLoadout = [...role.upstreamTools, ...corralToolNames(specs), ...local,
      ...(mcp ? MCP_CONTROLS : []), ...(mcp && search ? ["mcp_activate"] : []), ...(search ? ["tool_search"] : []),
      ...(options.capabilities?.enabled.includes("codemode") ? ["codemode"] : [])];
    if (new Set(this.loadout).size !== this.loadout.length) throw new Error("module_invalid: the loadout names a tool twice");
    this.settings = {
      ...restrictedSettings(), ...compactionSettings(options.capabilities, options.guidance),
      enableSkillCommands: options.capabilities?.enabled.includes("skills") === true && options.guidance !== undefined,
    };
    try {
      this.controls = parseControls(options.identity.role, options.capabilities?.enabled ?? [], options.guidance?.settings);
    } catch (error) {
      throw new Error(`module_invalid: ${error instanceof Error ? error.message : error}`);
    }
    // Recovery attributes a lost hook only through this record, so it precedes every launch.
    this.hooks = this.controls.hooks && new Hooks(this.controls.hooks, {
      cwd: options.cwd, generation: options.identity.generation,
      record: (entry) => this.session.sessionManager.appendCustomEntry(HOOK_ENTRY, entry),
    });
    this.permissions = this.controls.bashRules && new BashPermissions();
    const run = options.identity.run;
    if (options.capabilities?.enabled.includes("checkpoints") === true) {
      if (options.identity.role !== "worker" || !run) throw new Error("module_invalid: checkpoints needs a worker run");
      this.checkpoints = new Checkpoints({
        cwd: options.cwd, generation: options.identity.generation, run,
        record: (entry) => this.session.sessionManager.appendCustomEntry(CHECKPOINT_ENTRY, entry),
      });
    }
  }

  /**
   * Worker jobs are receipted against their run. Interactive jobs live only as long as this host;
   * they have no task-scoped monitor or durable wake.
   */
  private jobRuntime(registerCapability: (pid: number) => () => void): JobRuntime | undefined {
    const { options } = this;
    if (options.capabilities?.enabled.includes("background_shell") !== true) return undefined;
    const settings = JobSettingsSchema.safeParse(options.guidance?.settings.background_shell);
    const worker = options.identity.role === "worker";
    if ((!worker && options.identity.role !== "pm" && options.identity.role !== "assistant") || !settings.success || !options.runtimeDir) {
      throw new Error("module_invalid: background_shell needs an eligible role, settings and a runtime directory");
    }
    const ops = options.modules.role.corralOps;
    if (worker && (!ops.includes("job") || !ops.includes("job_monitor"))) return undefined;
    return new JobRuntime({
      cwd: options.cwd, runtimeDir: options.runtimeDir, generation: options.identity.generation,
      settings: settings.data, registerCapability,
      onEvent: (kind, data) => options.events.emit(kind, data),
      call: (op, key, payload) => worker ? options.client.call(op, {
        requestId: toolRequestId(this.sessionId(), key), payload, turn: this.currentTurn(),
      }) : Promise.resolve({ state: "committed", result: {} }),
    });
  }

  private exportable(): boolean {
    return this.options.exportDir !== undefined && this.options.guidance !== undefined &&
      this.options.capabilities?.enabled.includes("session_export") === true &&
      this.options.modules.role.corralOps.includes("artifact");
  }

  /** `/corral-export`: the current snapshot as one attached offline HTML artifact. */
  async exportSession(): Promise<SessionExport> {
    if (!this.exportable() || this.options.exportDir === undefined) throw new Error("session_export is not enabled for this role");
    if (!this.bound) throw new Error("host_not_bound");
    return exportSnapshot(this.session, this.sessionId(), this.options.exportDir, this.options.client);
  }

  get loadout(): readonly string[] { return [...this.baseLoadout, ...this.mcp?.names() ?? []]; }

  private get initialTools(): readonly string[] {
    return this.options.capabilities?.enabled.includes("tool_search") ? this.baseLoadout : this.loadout;
  }

  static async create(options: HostSessionOptions): Promise<HostSession> {
    const host = new HostSession(options);
    const { sdk } = options;
    const route = PROVIDERS[host.provider];
    const local = host.provider === "llama.cpp" && options.capabilities?.enabled.includes("llama_cpp.enabled") === true
      ? options.capabilities.llama_cpp : undefined;
    if (host.provider === "llama.cpp" && local === undefined) {
      throw new Error("module_invalid: llama.cpp requires its explicit generation-bound capability");
    }
    const localModels = local ? await createLocalModels(local, options.model, options.network.fetch) : undefined;
    const subscriptions = await subscriptionRoutes(options.modules.auth, route.id, options.identity.role);
    const models = localModels?.models ?? await createModelCatalog(subscriptions.credentials, {
      providers: subscriptions.providers,
    });
    const modelId = options.model.startsWith(`${route.id}/`) ? options.model.slice(route.id.length + 1) : options.model;
    const model = localModels?.model ?? models.getModel(route.id, modelId);
    if (!model) throw new Error(`model_unavailable: ${route.id}/${modelId}`);
    for (const id of subscriptions.providers) {
      if (!localModels) {
        const fetch = PROVIDERS[id].auth === "oauth"
          ? guardedFetch(options.network.fetch, (destination) => { options.network.refuse?.(destination); }, id).fetch
          : options.network.fetch;
        restrictProvider(models, id, fetch as typeof globalThis.fetch,
          { configDir: join(options.agentDir, "claude-code"), cwd: options.cwd }, options.aws);
      }
      const restricted = models.getProvider(id);
      if (!restricted) throw new Error("budget: approved provider is unavailable");
      if ((options.capabilities?.catalog ?? 0) >= 7) {
        const unbounded = PROVIDERS[id].lane !== undefined;
        models.registerNativeProvider({ ...restricted,
          stream: host.budget.wrap(restricted.stream.bind(restricted), unbounded),
          streamSimple: host.budget.wrap(restricted.streamSimple.bind(restricted), unbounded),
        });
      }
    }
    host.auxiliaryModels = models;
    if (options.capabilities?.enabled.includes("web.enabled") && options.capabilities.web?.search_provider === "subscription") {
      const providers = subscriptions.providers.filter((id) => id === "openai-codex" || id === "anthropic");
      const search = await createModelCatalog(subscriptions.credentials, { providers });
      for (const id of providers) {
        const fetch = guardedFetch(options.network.fetch, (destination) => { options.network.refuse?.(destination); }, id).fetch;
        restrictProvider(search, id, fetch as typeof globalThis.fetch,
          { configDir: join(options.agentDir, "claude-search"), cwd: options.cwd, searchOnly: true });
        const restricted = search.getProvider(id);
        if (!restricted) throw new Error("subscription search provider is unavailable");
        // Hosted searches/subprocess loops have no priced finite reservation. Hard budgets
        // fail closed rather than pretending a model-token bound covers service charges.
        search.registerNativeProvider({ ...restricted,
          stream: host.budget.wrap(restricted.stream.bind(restricted), true),
          streamSimple: host.budget.wrap(restricted.streamSimple.bind(restricted), true),
        });
      }
      host.searchModels = search;
    }
    const thinkingLevel = options.effort && THINKING.has(options.effort)
      ? (options.effort as "off" | "minimal" | "low" | "medium" | "high" | "xhigh") : undefined;
    const contract = [options.modules.role.contract(host.roleContext),
      memoryContext(options.identity.role, options.memoryFile, options.runtimeDir)].filter(Boolean).join("\n\n");
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
      if (!localModels) installModelSelection(result.session, models, subscriptions.providers);
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

  private async extension(pi: ExtensionAPI): Promise<void> {
    this.dialogPi = pi;
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
    if (this.additionalLsp) pi.on("session_shutdown", async () => { await this.additionalLsp?.shutdown(); });
    const settings = this.options.capabilities?.mcp;
    const search = this.options.capabilities?.enabled.includes("tool_search") === true;
    if (settings) {
      this.mcp = new McpAccess(settings, this.options.identity.role, this.options.identity.project,
        this.options.cwd, this.options.network.mcp);
      await this.mcp.refresh();
      this.mcp.attach(pi, search);
      const access = this.mcp;
      pi.on("session_shutdown", async () => { await access.close(); });
    }
    if (search) pi.registerTool(toolSearch(pi, (name) => this.loadout.includes(name)));
    const specs = this.options.modules.role.tools(this.roleContext);
    const corral = new Set([...corralToolNames(specs), ...(this.jobs && this.options.identity.role === "worker" ? [JOB_MONITOR_TOOL] : [])]);
    const reads = new Set(specs.filter((spec) => nestedCorralRead(spec.op)).map((spec) => spec.name));
    const rich = this.options.ui !== "plain";
    this.inspector ??= new InspectorActivity(this.options.events, () => this.panels?.refresh());
    this.inspector.extend(pi, corral);
    const role = this.options.identity.role;
    this.ledger = this.loadout.includes("corral_todo")
      ? new TodoLedger(this.options.identity, () => this.options.client.todoSnapshot()) : undefined;
    this.ledger?.extend(pi);
    this.request = this.ledger && (role === "pm" || role === "assistant") ? new RequestContext(this.ledger) : undefined;
    this.request?.extend(pi);
    for (const spec of specs) {
      const tool = defineCorralTool(spec, {
        client: this.options.client, allowedOps: this.options.modules.role.corralOps,
        sessionId: () => this.sessionId(), currentTurn: () => this.currentTurn(),
        ...(this.options.identity.role === "assistant"
          ? { confirm: (command: string, context: ConfirmUi, requestId: string) =>
            this.confirmOwnerAction(command, context, requestId) } : {}),
      });
      pi.registerTool(rich ? { ...tool, ...corralRenderers(tool.name) } : tool);
    }
    if (rich) {
      installChrome(pi, {
        identity: this.options.identity, provider: this.provider, model: this.options.model,
        cwd: this.options.cwd, ...(this.options.effort ? { effort: this.options.effort } : {}),
      }, this.options.events, () => this.bound, this.options.sdk.Theme);
      if (this.ledger) installTodo(pi, this.ledger, this.request === undefined);
      this.panels?.install(pi, this.options.sdk);
      const unsubscribe = this.ledger?.onChange(() => this.panels?.refresh());
      pi.on("session_shutdown", (event) => {
        unsubscribe?.();
        if (event.reason !== "reload") { this.unsubscribePanels?.(); this.panels?.dispose(); }
      });
    }
    for (const tool of this.localTools) pi.registerTool(tool);
    // One execution check for model-issued and nested calls; a Code Mode child it refuses is
    // reported as denied with no effect.
    pi.on("tool_call", (event) => {
      const allowed = new Set(this.loadout);
      const nested = event.parentToolCallId !== undefined;
      const child = nested ? nestedChild(event.toolName, event.toolCallId) : undefined;
      const unjournaled = corral.has(event.toolName) && !reads.has(event.toolName) && child === undefined;
      const reason = executionRefusal(event.toolName, nested, allowed, unjournaled);
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
      ...(this.permissions ? { permissions: this.permissions } : {}),
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
      const message = `corral: ${operation} is refused while this pane is bound to corral session ${this.boundSessionId}; ` +
        "corral observes and recovers only that session. Continue here, or stop the run and use `corral agent resume`.";
      this.options.events.emit("session_replacement_refused", { operation, session_id: this.boundSessionId, message });
      ui.notify(message, "warning");
      return { cancel: true };
    };
    pi.on("session_before_switch", (event, ctx) => {
      if (event.reason === "new" && this.bound && this.options.identity.role === "assistant"
          && this.options.newAssistant && ctx.mode === "tui") {
        if (this.closing) return { cancel: true };
        if (!ctx.isIdle() || this.open || this.session.isBashRunning) {
          ctx.ui.notify("corral: wait for the current work to finish before /new", "warning");
          return { cancel: true };
        }
        this.closing = true;
        this.options.events.emit("assistant_new_requested", { session_id: this.boundSessionId });
        this.options.newAssistant();
        // The supervisor, not Pi's runtime, creates the new identity after this host exits.
        return { cancel: true };
      }
      return refuse(event.reason === "new" ? "a new session" : "switching or importing a session", ctx.ui);
    });
    pi.on("session_before_fork", (_event, ctx) => refuse("forking the session", ctx.ui));
    if (this.exportable()) {
      pi.registerCommand(EXPORT_COMMAND, {
        description: "Attach this session's current branch to corral as an offline HTML artifact",
        handler: async (_args, ctx) => {
          try {
            const { path, receipt } = await this.exportSession();
            this.options.events.emit("session_exported", { path, state: receipt.state, replayed: receipt.replayed === true });
            ctx.ui.notify(`corral: session export ${receipt.state}${receipt.replayed ? " (already attached)" : ""}: ${path}`,
              receipt.state === "committed" ? "info" : "warning");
          } catch (error) {
            const reason = error instanceof Error ? error.message : String(error);
            this.options.events.emit("session_export_failed", { reason: reason.slice(0, 512) });
            ctx.ui.notify(`corral: session export failed: ${reason}`, "error");
          }
        },
      });
    }
    pi.on("tool_result", (event) => this.observeQuestion(event.toolName, event.input, event.details));
    pi.on("input", async (event) => {
      if (this.closing) return { action: "handled" as const };
      if (this.open && event.source === "interactive") this.ownerInput = true;
      else this.lastInputSource = event.source;
      // Only text the owner typed into this pane is journaled, before the model sees it.
      if (event.source !== "interactive" || this.options.identity.role !== "assistant" || event.text.startsWith("/")) {
        return { action: "continue" as const };
      }
      const id = await this.journalOwnerInput("pane", event.text);
      return id === undefined
        ? { action: "continue" as const }
        : { action: "transform" as const, text: ownerInputText(event.text, id) };
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
      await this.jobs?.shutdown();
    });
  }

  /**
   * Records owner text (typed into the pane, or the owner's y/n to a confirmation) in the
   * session's owner-input journal and returns its ID; undefined when it was not journaled,
   * so an owner action cannot cite it.
   */
  private async journalOwnerInput(
    source: "pane" | "dialog" | "confirmation", text: string, command?: string, answers?: string,
  ): Promise<string | undefined> {
    if (!this.bound) return undefined;
    const sha256 = sha256Hex(text);
    const kept = utf8Prefix(text, OWNER_INPUT_TEXT_MAX);
    const event = this.options.events.emit("owner_input", { source, sha256, bytes: Buffer.byteLength(text) });
    const payload = {
      kind: "owner_input", source, text: kept, sha256,
      ...(kept !== text ? { truncated: true } : {}), ...(command !== undefined ? { command } : {}),
      ...(answers !== undefined ? { answers } : {}),
    };
    try {
      const receipt = await this.options.client.call("event", {
        requestId: eventRequestId(this.options.identity.generation, event.seq), payload, turn: this.currentTurn(),
      });
      const id = (receipt.result as { input?: unknown } | undefined)?.input;
      if (receipt.state === "committed" && typeof id === "string") return id;
      this.options.events.emit("owner_input_unjournaled", { seq: event.seq, result: JSON.stringify(receipt.result).slice(0, 256) });
    } catch (error) {
      this.options.events.emit("owner_input_unjournaled", { seq: event.seq, reason: (error instanceof Error ? error.message : String(error)).slice(0, 256) });
    }
    return undefined;
  }

  /** Shows the owner a confirm-class command in the pane and journals the answer for the request. */
  private async confirmOwnerAction(command: string, context: ConfirmUi, requestId: string): Promise<boolean> {
    if (!context.hasUI) return false;
    const yes = await context.ui.confirm("corral: confirm owner action", command);
    return await this.journalOwnerInput("confirmation", yes ? "y" : "n", command, requestId) !== undefined;
  }

  private watch(session: AgentSession): void {
    this.unsubscribe?.();
    this.terminalInputSequence = 0;
    this.terminalInput = undefined;
    observeTerminalInput(session, this.options.events);
    this.panelTokens = undefined;
    this.unsubscribe = session.subscribe((event) => {
      if (event.type === "message_end") this.panelTokens = undefined;
      if (event.type === "agent_start") this.turnStarted();
      else if (event.type === "agent_settled") this.turnEnded(event.aborted);
      else if (event.type === "queue_update") {
        this.options.events.emit("queue", { steering: event.steering.length, follow_up: event.followUp.length });
      } else if (event.type === "message_end" && event.message.role === "assistant" && event.message.stopReason === "error") {
        const message = event.message.errorMessage ?? "";
        if (!message.includes("budget_exhausted") || !this.budget.state().stopped) this.providerError(providerErrorClass(message));
      }
    });
    this.options.events.emit("session_bound", { session_id: session.sessionId });
  }

  private turnStarted(): void {
    this.writeAbort.abort();
    this.writeAbort = new AbortController();
    this.turn++;
    this.open = true;
    const origin: TurnOrigin = this.nextTurnOrigin
      ?? (this.lastInputSource === "interactive" ? { kind: "owner_terminal" } : { kind: "unknown" });
    this.nextTurnOrigin = undefined;
    this.lastInputSource = undefined;
    this.ownerInput = false;
    // Before the prompt is recorded: its parent is the cursor a rewind to this turn restores.
    if (this.bound) this.checkpoint("start");
    this.report("turn_started", { origin, provider: this.session.model?.provider, model: this.session.model?.id });
    if (this.budget.state().stopped) this.report("budget_exhausted", { budget: this.budget.state() });
    this.options.herdr?.turnStarted();
  }

  /** Only corral's receipts open or settle a question in the herdr projection. */
  private observeQuestion(tool: string, input: Record<string, unknown>, details: unknown): void {
    const receipt = details as Partial<CorralReceipt> | undefined;
    const result = (receipt?.result ?? {}) as { key?: unknown; blocking?: unknown; code?: unknown; revision?: unknown };
    const revision = typeof result.revision === "number" ? result.revision : undefined;
    if (tool === "corral_ask" && receipt?.state === "committed" && receipt.replayed !== true &&
        typeof result.key === "string" && typeof input.kind === "string") {
      const blocking = typeof result.blocking === "boolean" ? result.blocking : input.blocking === true;
      this.options.herdr?.asked({ key: result.key, kind: input.kind, blocking, revision: revision ?? 1 });
    }
    if (tool !== "corral_withdraw" || typeof input.key !== "string") return;
    if (receipt?.state === "committed") this.options.herdr?.resolved(input.key, revision);
    else if (receipt?.state === "rejected" && NOT_PENDING.has(String(result.code))) this.options.herdr?.resolved(input.key);
  }

  /** Bridge `questions`: corral's question states, including transitions this host did not make. */
  questionsChanged(updates: readonly QuestionUpdate[]): Record<string, unknown> {
    if (!this.options.herdr) return { applied: false };
    this.options.herdr.apply(updates);
    return { applied: true, blocked_on: this.options.herdr.readback().blocked_on };
  }

  private turnEnded(aborted: boolean): void {
    this.writeAbort.abort();
    if (!this.open) return;
    this.open = false;
    const cancelled = this.cancelRequested === this.turn;
    this.cancelRequested = undefined;
    const ownerInput = this.ownerInput;
    this.ownerInput = false;
    if (this.bound) this.checkpoint("end");
    this.report("turn_ended", { aborted, cancelled, owner_input: ownerInput });
    this.options.herdr?.turnEnded();
  }

  private checkpoint(phase: "start" | "end"): void {
    if (!this.checkpoints) return;
    const leaf = phase === "start" ? this.session.sessionManager.getLeafId() : undefined;
    const entry = this.checkpoints.capture(phase, this.turn, leaf);
    this.options.events.emit(entry.error ? "checkpoint_failed" : "checkpoint", {
      id: entry.id, phase, ...(entry.commit ? { commit: entry.commit } : {}), ...(entry.error ? { error: entry.error } : {}),
    });
  }

  private providerError(errorClass: string): void {
    this.report("provider_error", { class: errorClass });
  }

  private report(kind: "turn_started" | "turn_ended" | "provider_error" | "budget_exhausted", data: Record<string, unknown>): void {
    const turn = this.turn;
    const event = this.options.events.emit(kind, { turn, ...data });
    if (!this.bound) return;
    const payload: Record<string, unknown> = { kind, turn };
    if (kind === "turn_started") {
      payload.origin = data.origin;
      payload.provider = data.provider;
      payload.model = data.model;
    }
    if (kind === "provider_error") payload.class = data.class;
    if (kind === "budget_exhausted") payload.budget = data.budget;
    if (kind === "turn_ended" && data.owner_input === true) payload.owner_input = true;
    if (kind === "turn_ended" && data.cancelled === true) payload.cancelled = true;
    const role = this.options.identity.role;
    if (kind === "turn_ended" && data.aborted === true && (role === "pm" || role === "assistant")) payload.aborted = true;
    const requestId = eventRequestId(this.options.identity.generation, event.seq);
    this.calls = this.calls.then(() => this.send(requestId, payload, turn));
  }

  private async send(requestId: string, payload: Record<string, unknown>, turn: number): Promise<void> {
    this.stats.sent++;
    let last: unknown;
    for (let attempt = 0; attempt < REPORT_ATTEMPTS; attempt++) {
      if (attempt > 0) await new Promise((resolve) => setTimeout(resolve, REPORT_BACKOFF_MS * 2 ** (attempt - 1)));
      try {
        const receipt: CorralReceipt = await this.options.client.call("event", { requestId, payload, turn });
        if (receipt.state === "rejected") {
          this.stats.rejected++;
          this.stats.lastError = JSON.stringify(receipt.result).slice(0, 256);
        } else {
          this.stats.committed++;
          if (payload.kind === "turn_ended" && receipt.state === "committed") this.turnRecorded(receipt.result, turn);
        }
        return;
      } catch (error) {
        last = error;
        this.stats.lastError = error instanceof Error ? error.message.slice(0, 256) : String(error);
      }
    }
    this.stats.failed++;
    this.stats.lastError = last instanceof Error ? last.message.slice(0, 256) : String(last);
  }

  /** Corral's request decision for ended turn `turn`: a stop is shown, a due continuation queued. */
  private turnRecorded(result: unknown, turn: number): void {
    const stopped = this.request?.stopped(result);
    if (stopped) {
      this.options.events.emit("continuation_stopped", { reason: stopped });
      this.request?.notify(`corral: automatic continuation stopped (${stopped.replace("_", " ")}); `
        + "the plan still has open steps.");
    }
    const due = this.request?.due(result);
    if (due) this.continuations = this.continuations.then(() => this.continueRequest(due, turn));
  }

  /**
   * Starts the continuation that ended turn `granted` earned, only while that turn is still the
   * latest, the pane is idle, the budget holds and the canonical plan is still the granted
   * revision, each checked again after the ledger read: a turn that started (and perhaps was
   * cancelled or failed) meanwhile voids it.
   */
  private async continueRequest(due: DueContinuation, granted: number): Promise<void> {
    const key = due.continuation.key;
    const skip = (reason: string) => { this.options.events.emit("continuation_skipped", { key, reason }); };
    const ledger = this.ledger;
    const blocked = () => this.closing ? "closing" : this.open ? "busy"
      : this.budget.state().stopped ? "budget_exhausted" : this.turn !== granted ? "superseded_by_turn" : undefined;
    const before = ledger ? blocked() : "closing";
    if (before || !ledger) return skip(before ?? "closing");
    try {
      await ledger.restore();
    } catch {
      return skip("recheck_failed");
    }
    const after = blocked();
    if (after) return skip(after);
    const current = ledger.current;
    if (current?.plan !== due.plan || current.revision !== due.revision) return skip("superseded");
    this.options.events.emit("continuation", { key, attempt: due.continuation.attempt, plan: due.plan,
      revision: due.revision });
    this.session.prompt(continuationText(due), {
      source: "rpc",
      preflightResult: (disposition) => {
        if (disposition === "started") this.nextTurnOrigin = { kind: "continuation", idempotency_key: key };
        else skip(disposition);
      },
    }).catch((error: unknown) => {
      skip(`refused: ${(error instanceof Error ? error.message : String(error)).slice(0, 128)}`);
    });
  }

  markAttached(mode: "interactive" | "headless"): void {
    this.attached = mode;
    this.options.events.emit(mode);
  }

  drained(): Promise<unknown> {
    return this.calls;
  }

  /** Every continuation recheck queued so far has decided (and, if due, submitted its prompt). */
  continued(): Promise<void> {
    return this.continuations;
  }

  /** Every detached Code Mode cell's notice started so far has been delivered or refused. */
  async codeNoticed(): Promise<void> {
    await this.code?.notified();
  }

  async close(): Promise<void> {
    this.closing = true;
    this.unsubscribePanels?.();
    this.panels?.dispose();
    const released = this.options.herdr?.close();
    // Stop the parent's follow-up loop before cancelled children can return tool results.
    this.writeAbort.abort();
    await this.code?.close();
    const aborted = this.session.abort();
    await this.auxiliary?.close();
    await this.jobs?.shutdown();
    await aborted;
    await this.mcp?.close();
    this.session.dispose();
    await released;
  }

  async hello(bindNonce: string, pane: string, ctlCapSha256: string): Promise<CorralReceipt> {
    const sessionId = this.session.sessionId;
    const sessionFile = this.session.sessionFile;
    if (!sessionFile) throw new Error("hello_failed: the Pi session has no session file");
    const receipt = await this.options.client.call("hello", {
      requestId: eventRequestId(this.options.identity.generation, 0),
      bindNonce,
      payload: {
        ctl_cap_sha256: ctlCapSha256, session_file: sessionFile, pid: process.pid,
        start_time: processStartTime(), pane,
      },
    });
    if (receipt.state !== "committed") {
      const result = receipt.result as { code?: string; reason?: string } | undefined;
      throw new Error(`hello_refused: ${result?.code ?? receipt.state}: ${result?.reason ?? ""}`);
    }
    // Session order, unlike cross-process wall clocks, is a stable recovery boundary.
    // The one-shot nonce distinguishes children that reuse this session and restart at g1.
    // No turn is admitted until this generation's committed hello has been recorded.
    this.session.sessionManager.appendCustomEntry("corral_host_bound", {
      generation: this.options.identity.generation, session_id: sessionId,
      bind_nonce_sha256: sha256Hex(bindNonce),
    });
    this.boundSessionId = sessionId;
    const bound = (receipt.result ?? {}) as { herdr_seq_epoch_ms?: unknown; questions?: unknown };
    this.options.herdr?.bind({ sessionId, sessionFile, epochMs: bound.herdr_seq_epoch_ms, questions: bound.questions });
    if ((this.options.capabilities?.catalog ?? 0) >= 7) await this.budget.refresh();
    await this.writePermissionNotice?.();
    this.options.events.emit("bound", { generation: this.options.identity.generation, session_id: sessionId });
    return receipt;
  }

  deliveryHost(): DeliveryHost {
    return {
      appendEntry: (customType, data) => {
        this.session.sessionManager.appendCustomEntry(customType, data);
      },
      sessionId: () => this.sessionId(),
    };
  }

  async submit(input: SubmitInput, requestId: string): Promise<SubmitResult> {
    if (!this.bound) return { disposition: "rejected", reason: "host_not_bound" };
    if (this.closing) return { disposition: "rejected", reason: "host_closing" };
    try {
      await this.options.modules.delivery.accept({
        requestId, idempotencyKey: input.idempotencyKey, origin: input.origin, busyPolicy: input.busyPolicy,
        text: input.text, textSha256: sha256Hex(input.text), generation: this.options.identity.generation,
      }, this.deliveryHost());
    } catch (error) {
      return { disposition: "rejected", reason: `delivery_refused: ${error instanceof Error ? error.message : error}` };
    }
    // Only a corral inbox delivery can carry the owner's answer to a bound permission question.
    const permission = input.origin === "corral-inbox" ? this.permissions?.observeAnswer(input.text) : undefined;
    if (permission) this.options.events.emit("bash_permission_answer", { decision: permission });
    if (input.origin === "corral-inbox") {
      for (const { key, revision } of answeredKeys(input.text)) this.options.herdr?.resolved(key, revision);
    }
    if (this.closing) return { disposition: "rejected", reason: "host_closing" };
    return new Promise<SubmitResult>((resolve) => {
      let settled = false;
      const settle = (result: SubmitResult) => {
        if (settled) return;
        settled = true;
        resolve(result);
      };
      this.session.prompt(input.text, {
        source: "rpc",
        ...(input.busyPolicy === "steer" ? { streamingBehavior: "steer" as const } : {}),
        ...(input.busyPolicy === "follow_up" ? { streamingBehavior: "followUp" as const } : {}),
        preflightResult: (disposition) => {
          if (disposition !== "started") {
            settle({ disposition });
            return;
          }
          this.nextTurnOrigin = { kind: "bridge", idempotency_key: input.idempotencyKey };
          settle({ disposition, turn: this.turn + 1 });
        },
      }).catch((error: unknown) => {
        const message = error instanceof Error ? error.message : String(error);
        settle({ disposition: "rejected", reason: /already processing/i.test(message) ? "busy" : message.slice(0, 256) });
      });
    });
  }

  async cancel(turn: number | undefined): Promise<Record<string, unknown>> {
    if (!this.open) return { cancelled: false, reason: "idle" };
    if (turn !== undefined && turn !== this.turn) return { cancelled: false, reason: "turn_not_current", turn: this.turn };
    this.cancelRequested = this.turn;
    const cancelled = this.turn;
    this.options.events.emit("cancel", { turn: cancelled });
    this.writeAbort.abort();
    await this.session.abort();
    return { cancelled: true, turn: cancelled };
  }

  private panelSnapshot(): PanelSnapshot | null {
    if (!this.runtime) return null;
    const session = this.session;
    const leaf = session.sessionManager.getLeafId();
    if (leaf !== this.panelTokenLeaf) { this.panelTokenLeaf = leaf; this.panelTokens = undefined; }
    const jobs: PanelJob[] | null = this.jobs ? this.jobs.inspection().map(detail => ({ job: detail.job, state: detail.state, detail })) : null;
    return {
      bound: this.bound, model: session.model ? `${session.model.provider}/${session.model.id}` : null,
      effort: session.thinkingLevel, effortAvailable: session.supportsThinking(),
      ...(this.terminalInput ? { input: this.terminalInput } : {}),
      streaming: session.isStreaming, context: session.getContextUsage(),
      tokens: this.panelTokens ??= session.getSessionStats().tokens.total, tools: session.getActiveToolNames(),
      contextFiles: this.guidance.contextFiles.map(file => file.path),
      skills: this.runtime.services.resourceLoader.getSkills().skills.map(skill => skill.name),
      ledger: this.ledger?.current ?? null, ledgerStale: this.ledger?.unavailable ?? true,
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
        budget: [(this.options.capabilities?.catalog ?? 0) >= 7
          ? `Budget: ${JSON.stringify(this.budget.state())}` : "Budget unavailable · legacy generation"],
        ...(this.inspector?.snapshot() ?? { events: this.options.events.since(0, 512 * 8192, 512), receipts: [], receiptsOmitted: 0 }),
      },
      jobs, auxiliary: this.auxiliary?.state() ?? null,
      steering: session.getSteeringMessages().length, followUps: session.getFollowUpMessages().length,
    };
  }

  state(): Record<string, unknown> {
    const session = this.session;
    const resources = this.runtime.services.resourceLoader;
    return {
      role: this.options.identity.role,
      todo: this.loadout.includes("corral_todo"),
      // The plan the widget renders and the model summary names: one host ledger.
      ledger: this.ledger?.current ? { plan: this.ledger.current.plan ?? null, revision: this.ledger.current.revision } : null,
      capabilities: this.options.capabilities ?? null,
      mcp: this.mcp?.state() ?? null,
      generation: this.options.identity.generation,
      bound: this.bound,
      interactive: this.attached === "interactive",
      mode: this.attached ?? null,
      session: { bound_id: this.boundSessionId ?? null, current_id: session.sessionId, file: session.sessionFile ?? null,
        created: this.sessionsCreated },
      streaming: session.isStreaming,
      turn: { current: this.turn, open: this.open },
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
      codemode: this.code?.state() ?? null,
      checkpoints: this.checkpoints !== undefined,
      auxiliary: this.auxiliary?.state() ?? null,
      budget: (this.options.capabilities?.catalog ?? 0) >= 7 ? this.budget.state() : { unavailable: "legacy generation predates budget accounting" },
      guidance: {
        project_text: this.options.guidance?.projectText ?? false,
        skill_paths: [...this.guidance.skillPaths], prompt_paths: [...this.guidance.promptPaths],
        context_files: this.guidance.contextFiles.map((file) => file.path),
        skills: resources.getSkills().skills.map((skill) => ({ name: skill.name, path: skill.filePath })),
        export: this.exportable(),
      },
      settings: this.settings,
      network: {
        allowed_destinations: [...(this.options.network.destinations ?? hostDestinations(this.provider))],
        ...this.options.network.ledger(),
      },
      corral_calls: { ...this.stats },
      herdr: this.options.herdr?.readback() ?? null,
      delivery: bounded(this.options.modules.delivery.state?.() ?? null),
      auth: bounded(this.options.modules.auth.state?.() ?? null),
    };
  }
}

/** Linux process start time in clock ticks since boot (/proc/self/stat field 22). */
export function processStartTime(): string {
  const stat = existsSync("/proc/self/stat") ? readFileSync("/proc/self/stat", "utf8") : "";
  const value = stat.slice(stat.lastIndexOf(")") + 2).split(" ")[19];
  return value && /^\d+$/.test(value) ? value : String(Math.floor(Date.now() / 1000 - process.uptime()));
}
