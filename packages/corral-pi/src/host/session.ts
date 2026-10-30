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
import { codemode } from "../capabilities/codemode.ts";
import { HOOK_ENTRY, Hooks } from "../capabilities/hooks.ts";
import { CHECKPOINT_ENTRY, Checkpoints } from "../capabilities/checkpoints.ts";
import { type HostControls, installToolControls, parseControls } from "./tool-controls.ts";
import { type CapabilityGuidance, type Guidance, selectGuidance } from "../capabilities/guidance.ts";
import { PROVIDERS, type ProviderId, hostDestinations } from "../auth/providers.ts";
import type { AuthContext } from "../auth/types.ts";
import type { EventLog } from "../bridge/events.ts";
import { CLAUDE_SDK_API, type ClaudeLaneConfig, registerClaudeSdkApi } from "../claude/stream.ts";
import type { SubmitInput, SubmitResult } from "../bridge/protocol.ts";
import type { DeliveryHost } from "../delivery/types.ts";
import type { HostIdentity, RoleContext } from "../roles/types.ts";
import { type CorralClient, type CorralReceipt, eventRequestId, sha256Hex } from "../tools/corral.ts";
import { corralToolNames, defineCorralTool } from "../tools/define.ts";
import { installChrome } from "../ui/chrome.ts";
import { corralRenderers } from "../ui/render.ts";
import { installTodo } from "../ui/todo.ts";
import type { HostCapabilities, UiMode } from "./config.ts";
import { EXPORT_COMMAND, type SessionExport, exportSnapshot } from "./export.ts";
import { type HerdrLifecycle, answeredKeys } from "./herdr.ts";
import type { LoadedModules } from "./modules.ts";
import { type GuardedFetch, resourceLoaderOptions, restrictedSettings } from "./restrict.ts";
import { createModelCatalog } from "./catalog.ts";
import { browserSettings, browserTools } from "../extensions/browser/index.ts";
import { createLocalModels } from "./local-models.ts";
import { webTools } from "../extensions/web/index.ts";
import { configuredServers, lspToolkit, type LspToolkit } from "../extensions/lsp/index.ts";
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
import { executionRefusal } from "../roles/common/execution.ts";

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
}

type TurnOrigin = { kind: "bridge"; idempotency_key: string } | { kind: "owner_terminal" } | { kind: "unknown" };

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
    registerClaudeSdkApi(lane);
    const api = getApiProvider(CLAUDE_SDK_API);
    if (!api) throw new Error(`model_unavailable: the ${route.lane} lane is not registered`);
    stream = (current, context, options) => api.stream({ ...current, api: CLAUDE_SDK_API }, context, options);
    streamSimple = (current, context, options) => api.streamSimple({ ...current, api: CLAUDE_SDK_API }, context, options);
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
  private readonly baseLoadout: readonly string[];
  private mcp: McpAccess | undefined;
  private readonly roleContext: RoleContext;
  private readonly localTools: readonly ToolDefinition[];
  private readonly jobs: JobRuntime | undefined;
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
  private readonly budget: RunBudget;

  readonly provider: ProviderId;

  private constructor(private readonly options: HostSessionOptions) {
    this.provider = options.provider ?? "openai-codex";
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
    this.roleContext = {
      identity: options.identity, cwd: options.cwd, model: options.model,
      todo: options.capabilities ? options.capabilities.enabled.includes("todo") : options.todo === true,
      ...(options.capabilities ? { capabilities: options.capabilities } : {}),
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
    if (webEnabled && (options.identity.role === "pm" || !options.network.web || !options.capabilities?.web)) {
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
    if (lspEnabled && options.identity.role !== "worker" && options.identity.role !== "pm") {
      this.additionalLsp = lspToolkit({ root: options.cwd, servers: configuredServers(options.guidance?.settings.lsp) });
    }
    this.jobs = this.jobRuntime();
    const auxiliary = auxiliarySettings(options.identity.role, options.capabilities?.enabled ?? [], options.guidance?.settings);
    if (auxiliary.explore || auxiliary.oracle) {
      this.auxiliary = new AuxiliaryRuntime({
        settings: auxiliary, generation: options.identity.generation, provider: this.provider, model: options.model,
        sessionId: () => this.sessionId(),
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
          if (!this.auxiliaryModels) throw new Error("auxiliary model runtime is not ready");
          return runAuxiliarySession({ sdk: options.sdk, models: this.auxiliaryModels, provider: this.provider,
            cwd: options.cwd, agentDir: options.agentDir, parentLoadout: this.loadout, localTools: this.localTools }, invocation, signal);
        },
      });
    }
    this.localTools = [...(lspEnabled ? roleLocal : roleLocal.filter((tool) => !tool.name.startsWith("lsp_"))),
      ...(this.additionalLsp?.tools ?? []), ...(webEnabled && options.network.web ? webTools(options.network.web) : []),
      ...(this.jobs ? jobTools(this.jobs) : []),
      ...(this.auxiliary && auxiliary.explore ? [exploreTool(this.auxiliary)] : []),
      ...(this.auxiliary && auxiliary.oracle ? [oracleTool(this.auxiliary)] : []),
      ...(browser ? browserTools(browser, { cwd: options.cwd }) : [])];
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
   * background_shell: worker only, with its generation-bound settings and runtime directory. Like
   * todo, the role module decides its loadout: a role that allows no job ops offers no job tools.
   */
  private jobRuntime(): JobRuntime | undefined {
    const { options } = this;
    if (options.capabilities?.enabled.includes("background_shell") !== true) return undefined;
    const settings = JobSettingsSchema.safeParse(options.guidance?.settings.background_shell);
    if (options.identity.role !== "worker" || !settings.success || !options.runtimeDir) {
      throw new Error("module_invalid: background_shell needs the worker role, its settings and a runtime directory");
    }
    const ops = options.modules.role.corralOps;
    if (!ops.includes("job") || !ops.includes("job_monitor")) return undefined;
    return new JobRuntime({
      cwd: options.cwd, runtimeDir: options.runtimeDir, generation: options.identity.generation,
      settings: settings.data,
      onEvent: (kind, data) => options.events.emit(kind, data),
      call: (op, key, payload) => options.client.call(op, {
        requestId: toolRequestId(this.sessionId(), key), payload, turn: this.currentTurn(),
      }),
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
    const models = localModels?.models ?? await createModelCatalog(options.modules.auth.credentials(authContext(route.id)), {
      providers: [route.id],
    });
    const modelId = options.model.startsWith(`${route.id}/`) ? options.model.slice(route.id.length + 1) : options.model;
    const model = localModels?.model ?? models.getModel(route.id, modelId);
    if (!model) throw new Error(`model_unavailable: ${route.id}/${modelId}`);
    if (!localModels) {
      restrictProvider(models, route.id, options.network.fetch as typeof globalThis.fetch,
        { configDir: join(options.agentDir, "claude-code"), cwd: options.cwd }, options.aws);
    }
    const restricted = models.getProvider(route.id);
    if (!restricted) throw new Error("budget: bound provider is unavailable");
    if ((options.capabilities?.catalog ?? 0) >= 7) {
      models.registerNativeProvider({ ...restricted,
        stream: host.budget.wrap(restricted.stream.bind(restricted)),
        streamSimple: host.budget.wrap(restricted.streamSimple.bind(restricted)),
      });
    }
    host.auxiliaryModels = models;
    const thinkingLevel = options.effort && THINKING.has(options.effort)
      ? (options.effort as "off" | "minimal" | "low" | "medium" | "high" | "xhigh") : undefined;
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
        ...(thinkingLevel ? { thinkingLevel, scopedModels: [{ model, thinkingLevel }] } : { scopedModels: [{ model }] }),
      });
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
    if (this.options.capabilities?.enabled.includes("codemode")) {
      await codemode(this.options.sdk.createCodemodeExtension)(pi);
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
    const corral = new Set([...corralToolNames(specs), ...(this.jobs ? [JOB_MONITOR_TOOL] : [])]);
    const rich = this.options.ui !== "plain";
    for (const spec of specs) {
      const tool = defineCorralTool(spec, {
        client: this.options.client, allowedOps: this.options.modules.role.corralOps,
        sessionId: () => this.sessionId(), currentTurn: () => this.currentTurn(),
      });
      pi.registerTool(rich ? { ...tool, ...corralRenderers(tool.name) } : tool);
    }
    if (rich) {
      installChrome(pi, {
        identity: this.options.identity, provider: this.provider, model: this.options.model,
        cwd: this.options.cwd, ...(this.options.effort ? { effort: this.options.effort } : {}),
      }, this.options.events, () => this.bound, this.options.sdk.Theme);
      if (this.loadout.includes("corral_todo")) {
        installTodo(pi, this.options.identity, () => this.options.client.todoSnapshot());
      }
    }
    for (const tool of this.localTools) pi.registerTool(tool);
    // One execution check for model-issued and nested (`ctx.executeTool`) calls. A corral tool
    // runs only at top level, where its request ID is bound to a transcript tool call.
    pi.on("tool_call", (event) => {
      const allowed = new Set(this.loadout);
      const nested = event.parentToolCallId !== undefined;
      const reason = executionRefusal(event.toolName, nested, allowed, corral);
      if (!reason) return undefined;
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
    pi.on("session_before_switch", (event, ctx) =>
      refuse(event.reason === "new" ? "a new session" : "switching or importing a session", ctx.ui));
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
    pi.on("tool_result", (event) => {
      if (event.parentToolCallId === undefined) this.observeQuestion(event.toolName, event.input, event.details);
    });
    pi.on("input", (event) => {
      if (this.open && event.source === "interactive") this.ownerInput = true;
      else this.lastInputSource = event.source;
      return { action: "continue" as const };
    });
    this.options.modules.role.extend?.(pi, this.roleContext);
    // Registered after the role's own handler: upstream keeps the first skill of each name, so
    // a home or repository skill can never replace the role's packaged one (corral-worker).
    pi.on("resources_discover", () => ({ skillPaths: [...this.guidance.skillPaths] }));
    pi.on("session_shutdown", async (event) => {
      if (event.reason === "reload") await this.auxiliary?.cancelAll(); else await this.auxiliary?.close();
      await this.jobs?.shutdown();
    });
  }

  private watch(session: AgentSession): void {
    this.unsubscribe?.();
    this.unsubscribe = session.subscribe((event) => {
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
    this.turn++;
    this.open = true;
    const origin: TurnOrigin = this.nextTurnOrigin
      ?? (this.lastInputSource === "interactive" ? { kind: "owner_terminal" } : { kind: "unknown" });
    this.nextTurnOrigin = undefined;
    this.lastInputSource = undefined;
    this.ownerInput = false;
    // Before the prompt is recorded: its parent is the cursor a rewind to this turn restores.
    if (this.bound) this.checkpoint("start");
    this.report("turn_started", { origin });
    if (this.budget.state().stopped) this.report("budget_exhausted", { budget: this.budget.state() });
    this.options.herdr?.turnStarted();
  }

  /** Only corral's receipts open or settle a question in the herdr projection. */
  private observeQuestion(tool: string, input: Record<string, unknown>, details: unknown): void {
    const receipt = details as Partial<CorralReceipt> | undefined;
    const result = (receipt?.result ?? {}) as { key?: unknown; blocking?: unknown; code?: unknown };
    if (tool === "corral_ask" && receipt?.state === "committed" && receipt.replayed !== true &&
        typeof result.key === "string" && typeof input.kind === "string") {
      const blocking = typeof result.blocking === "boolean" ? result.blocking : input.blocking === true;
      this.options.herdr?.asked({ key: result.key, kind: input.kind, blocking });
    }
    const settled = receipt?.state === "committed" ||
      (receipt?.state === "rejected" && NOT_PENDING.has(String(result.code)));
    if (tool === "corral_withdraw" && settled && typeof input.key === "string") this.options.herdr?.resolved(input.key);
  }

  private turnEnded(aborted: boolean): void {
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
    if (kind === "turn_started") payload.origin = data.origin;
    if (kind === "provider_error") payload.class = data.class;
    if (kind === "budget_exhausted") payload.budget = data.budget;
    if (kind === "turn_ended" && data.owner_input === true) payload.owner_input = true;
    if (kind === "turn_ended" && data.cancelled === true) payload.cancelled = true;
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

  markAttached(mode: "interactive" | "headless"): void {
    this.attached = mode;
    this.options.events.emit(mode);
  }

  drained(): Promise<unknown> {
    return this.calls;
  }

  async close(): Promise<void> {
    const released = this.options.herdr?.close();
    // Stop the parent's follow-up loop before cancelled children can return tool results.
    const aborted = this.session.abort();
    await this.auxiliary?.close();
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
    if (input.origin === "corral-inbox") for (const key of answeredKeys(input.text)) this.options.herdr?.resolved(key);
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
    await this.session.abort();
    return { cancelled: true, turn: cancelled };
  }

  state(): Record<string, unknown> {
    const session = this.session;
    const resources = this.runtime.services.resourceLoader;
    return {
      role: this.options.identity.role,
      todo: this.loadout.includes("corral_todo"),
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
