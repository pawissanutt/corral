// One upstream AgentSessionRuntime under the role's restrictions. It reports host turns
// (agent_start to agent_settled) and provider errors to corral as `event` calls, and answers
// bridge input with upstream's own dispositions: started, queued, handled or rejected.
import { existsSync, readFileSync } from "node:fs";

import type * as Sdk from "@earendil-works/pi-coding-agent";
import type {
  AgentSession, AgentSessionRuntime, CreateAgentSessionRuntimeFactory, ExtensionAPI, SessionManager, ToolDefinition,
} from "@earendil-works/pi-coding-agent";

import type { OpenAICodexResponsesOptions, StreamFunction } from "@earendil-works/pi-ai";

import type { AuthContext } from "../auth/types.ts";
import type { EventLog } from "../bridge/events.ts";
import type { SubmitInput, SubmitResult } from "../bridge/protocol.ts";
import type { DeliveryHost } from "../delivery/types.ts";
import type { HostIdentity, RoleContext } from "../roles/types.ts";
import { type CorralClient, type CorralReceipt, eventRequestId, sha256Hex } from "../tools/corral.ts";
import { corralToolNames, defineCorralTool } from "../tools/define.ts";
import type { LoadedModules } from "./modules.ts";
import { type GuardedFetch, PROVIDER_API, TOKEN_REFRESH, resourceLoaderOptions, restrictedSettings } from "./restrict.ts";

export interface HostSessionOptions {
  readonly sdk: typeof Sdk;
  readonly identity: HostIdentity;
  readonly cwd: string;
  readonly agentDir: string;
  readonly model: string;
  readonly effort?: string;
  readonly promptFile?: string;
  readonly modules: LoadedModules;
  readonly client: CorralClient;
  readonly events: EventLog;
  readonly network: GuardedFetch;
  readonly sessionManager: SessionManager;
}

type TurnOrigin = { kind: "bridge"; idempotency_key: string } | { kind: "owner_terminal" } | { kind: "unknown" };

const THINKING = new Set(["off", "minimal", "low", "medium", "high", "xhigh"]);
const STATE_SECTION_MAX_BYTES = 8 * 1024;
/** A lifecycle report keeps its request ID, so retrying it is safe; back off from contention. */
const REPORT_ATTEMPTS = 6;
const REPORT_BACKOFF_MS = 250;

/** A module's own readback, replaced by its size once it could crowd a bridge frame. */
function bounded(value: Record<string, unknown> | null): Record<string, unknown> | null {
  const size = value === null ? 0 : Buffer.byteLength(JSON.stringify(value));
  return size > STATE_SECTION_MAX_BYTES ? { omitted: "state_too_large", bytes: size } : value;
}

export function authContext(): AuthContext {
  return { provider: "openai-codex", apiEndpoint: PROVIDER_API, refreshEndpoint: TOKEN_REFRESH };
}

function providerErrorClass(message: string): string {
  if (/network_refused/.test(message)) return "network_refused";
  if (/\b(401|403)\b|unauthori[sz]ed|forbidden|expired/i.test(message)) return "auth";
  if (/\b429\b|rate.?limit|quota|usage limit/i.test(message)) return "rate_limit";
  if (/\b5\d\d\b|overload|unavailable/i.test(message)) return "overload";
  return "provider";
}

export class HostSession {
  runtime!: AgentSessionRuntime;
  readonly loadout: readonly string[];
  private readonly roleContext: RoleContext;
  private readonly localTools: readonly ToolDefinition[];
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
  private interactive = false;

  private constructor(private readonly options: HostSessionOptions) {
    this.roleContext = {
      identity: options.identity, cwd: options.cwd, model: options.model,
      ...(options.effort ? { effort: options.effort } : {}),
      ...(options.promptFile ? { promptFile: options.promptFile } : {}),
    };
    const role = options.modules.role;
    const specs = role.tools(this.roleContext);
    this.localTools = role.localTools?.(this.roleContext) ?? [];
    const local = this.localTools.map((tool) => tool.name);
    if (local.join(",") !== (role.localToolNames ?? []).join(",")) {
      throw new Error(`module_invalid: local tools ${local.join(",")} differ from localToolNames`);
    }
    this.loadout = [...role.upstreamTools, ...corralToolNames(specs), ...local];
    if (new Set(this.loadout).size !== this.loadout.length) throw new Error("module_invalid: the loadout names a tool twice");
  }

  static async create(options: HostSessionOptions): Promise<HostSession> {
    const host = new HostSession(options);
    const { sdk } = options;
    const models = await sdk.ModelRuntime.create({
      credentials: options.modules.auth.credentials(authContext()),
      modelsPath: null, refreshOnCreate: false, allowModelNetwork: false,
    });
    const provider = models.getProvider("openai-codex");
    const model = models.getModel("openai-codex", options.model);
    if (!provider || !model) throw new Error(`model_unavailable: openai-codex/${options.model}`);
    const fetch = options.network.fetch as typeof globalThis.fetch;
    const streamCodex = provider.stream.bind(provider) as StreamFunction<"openai-codex-responses", OpenAICodexResponsesOptions>;
    models.registerNativeProvider({
      ...provider,
      stream(current, context, streamOptions) {
        if (current.api !== "openai-codex-responses") throw new Error("provider_api_not_allowed");
        const codexOptions = streamOptions as OpenAICodexResponsesOptions | undefined;
        return streamCodex({ ...current, api: "openai-codex-responses" }, context, { ...codexOptions, fetch });
      },
      streamSimple: (current, context, streamOptions) => provider.streamSimple(current, context, { ...streamOptions, fetch }),
    });
    const thinkingLevel = options.effort && THINKING.has(options.effort)
      ? (options.effort as "off" | "minimal" | "low" | "medium" | "high" | "xhigh") : undefined;
    const contract = options.modules.role.contract(host.roleContext);
    const factory: CreateAgentSessionRuntimeFactory = async ({ cwd, agentDir, sessionManager, sessionStartEvent }) => {
      const settingsManager = sdk.SettingsManager.inMemory(restrictedSettings());
      const services = await sdk.createAgentSessionServices({
        cwd, agentDir, modelRuntime: models, settingsManager,
        resourceLoaderOptions: { ...resourceLoaderOptions([(pi: ExtensionAPI) => host.extension(pi)]), appendSystemPrompt: [contract] },
      });
      const result = await sdk.createAgentSessionFromServices({
        services, sessionManager, model, tools: [...host.loadout],
        ...(sessionStartEvent ? { sessionStartEvent } : {}),
        ...(thinkingLevel ? { thinkingLevel, scopedModels: [{ model, thinkingLevel }] } : { scopedModels: [{ model }] }),
      });
      result.session.setActiveToolsByName([...host.loadout]);
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

  private extension(pi: ExtensionAPI): void {
    const allowed = new Set(this.loadout);
    const specs = this.options.modules.role.tools(this.roleContext);
    for (const spec of specs) {
      pi.registerTool(defineCorralTool(spec, {
        client: this.options.client, allowedOps: this.options.modules.role.corralOps,
        sessionId: () => this.sessionId(), currentTurn: () => this.currentTurn(),
      }));
    }
    for (const tool of this.localTools) pi.registerTool(tool);
    pi.on("tool_call", (event) => {
      if (allowed.has(event.toolName)) return undefined;
      this.deniedTools++;
      this.options.events.emit("tool_denied", { tool: event.toolName });
      return { block: true, reason: `${event.toolName} is not in the ${this.options.identity.role} loadout` };
    });
    if (!allowed.has("bash")) {
      pi.on("user_bash", () => ({
        result: { output: "bash is not in this role's loadout", exitCode: 1, cancelled: false, truncated: false },
      }));
    }
    pi.on("input", (event) => {
      if (this.open && event.source === "interactive") this.ownerInput = true;
      else this.lastInputSource = event.source;
      return { action: "continue" as const };
    });
    this.options.modules.role.extend?.(pi, this.roleContext);
  }

  private watch(session: AgentSession): void {
    this.unsubscribe?.();
    this.unsubscribe = session.subscribe((event) => {
      if (event.type === "agent_start") this.turnStarted();
      else if (event.type === "agent_settled") this.turnEnded(event.aborted);
      else if (event.type === "queue_update") {
        this.options.events.emit("queue", { steering: event.steering.length, follow_up: event.followUp.length });
      } else if (event.type === "message_end" && event.message.role === "assistant" && event.message.stopReason === "error") {
        this.providerError(providerErrorClass(event.message.errorMessage ?? ""));
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
    this.report("turn_started", { origin });
  }

  private turnEnded(aborted: boolean): void {
    if (!this.open) return;
    this.open = false;
    const cancelled = this.cancelRequested === this.turn;
    this.cancelRequested = undefined;
    const ownerInput = this.ownerInput;
    this.ownerInput = false;
    this.report("turn_ended", { aborted, cancelled, owner_input: ownerInput });
  }

  private providerError(errorClass: string): void {
    this.report("provider_error", { class: errorClass });
  }

  private report(kind: "turn_started" | "turn_ended" | "provider_error", data: Record<string, unknown>): void {
    const turn = this.turn;
    const event = this.options.events.emit(kind, { turn, ...data });
    if (!this.bound) return;
    const payload: Record<string, unknown> = { kind, turn };
    if (kind === "turn_started") payload.origin = data.origin;
    if (kind === "provider_error") payload.class = data.class;
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

  markInteractive(): void {
    this.interactive = true;
    this.options.events.emit("interactive");
  }

  drained(): Promise<unknown> {
    return this.calls;
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
    this.boundSessionId = sessionId;
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
      generation: this.options.identity.generation,
      bound: this.bound,
      interactive: this.interactive,
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
      settings: restrictedSettings(),
      network: { allowed_destinations: [PROVIDER_API, TOKEN_REFRESH], ...this.options.network.ledger() },
      corral_calls: { ...this.stats },
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
