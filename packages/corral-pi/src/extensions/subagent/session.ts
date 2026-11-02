// Real SDK adapter shared by explore and oracle. It reuses the parent's already restricted
// ModelRuntime, but never its extension set, Corral client, hooks or session identity.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ModelRuntime, ToolDefinition } from "@earendil-works/pi-coding-agent";
import { resourceLoaderOptions, restrictedSettings } from "../../host/restrict.ts";
import { executionRefusal, exploreLoadout } from "../../roles/common/execution.ts";
import type { AuxiliaryResult, Invocation } from "./runtime.ts";
import type { HelperEvent } from "./inspection.ts";

/** Project only visible response text and tool activity; never forward thinking or signatures. */
export function observeHelperEvent(event: Sdk.AgentSessionEvent, emit: (event: HelperEvent) => void): void {
  try { projectHelperEvent(event, emit); }
  catch (error) {
    // A projection/UI failure is not a provider failure and cannot abort a billable child.
    try { emit({ type: "inspection_error", text: `Activity projection unavailable: ${error instanceof Error ? error.message : String(error)}` }); }
    catch { /* The observer itself is broken; execution and canonical accounting must continue. */ }
  }
}
function projectHelperEvent(event: Sdk.AgentSessionEvent, emit: (event: HelperEvent) => void): void {
  if (event.type === "message_start" && event.message.role === "assistant") emit({ type: "assistant_start" });
  else if (event.type === "message_update" && event.assistantMessageEvent.type === "text_delta") {
    const blocks = event.assistantMessageEvent.partial.content.filter(block => block.type === "text").map(block => block.text);
    emit({ type: "assistant_delta", text: event.assistantMessageEvent.delta, ...(blocks.length > 1 ? { textBlocks: blocks } : {}) });
  } else if (event.type === "tool_execution_start") {
    emit({ type: "tool", id: event.toolCallId, name: event.toolName, phase: "running" });
  } else if (event.type === "tool_execution_end") {
    emit({ type: "tool", id: event.toolCallId, name: event.toolName, phase: event.isError ? "error" : "completed" });
  } else if (event.type === "message_end") {
    const message = event.message;
    if (message.role === "assistant") {
      const blocks = message.content.filter(block => block.type === "text").map(block => block.text);
      emit({ type: "assistant_end", text: blocks.join("\n"), ...(blocks.length > 1 ? { textBlocks: blocks } : {}), errorMessage: message.errorMessage,
        stopReason: message.stopReason, hasToolCalls: message.content.some(block => block.type === "toolCall") });
      for (const block of message.content) if (block.type === "toolCall") {
        emit({ type: "tool", id: block.id, name: block.name, phase: "requested", arguments: JSON.stringify(block.arguments) });
      }
      if (message.stopReason === "error" || message.stopReason === "aborted") emit({ type: "error", text: message.errorMessage ?? "Helper response interrupted." });
    } else if (message.role === "toolResult") {
      emit({ type: "tool", id: message.toolCallId, name: message.toolName, phase: message.isError ? "error" : "completed",
        text: message.content.filter(block => block.type === "text").map(block => block.text).join("\n") });
    }
  }
}

export interface AuxiliarySessionOptions {
  readonly sdk: typeof Sdk;
  readonly models: ModelRuntime;
  readonly provider: string;
  readonly cwd: string;
  readonly agentDir: string;
  readonly parentLoadout: readonly string[];
  readonly localTools: readonly ToolDefinition[];
}
export async function runAuxiliarySession(options: AuxiliarySessionOptions, invocation: Invocation, signal: AbortSignal): Promise<AuxiliaryResult> {
  signal.throwIfAborted();
  const { sdk } = options;
  if (invocation.serviceTier && (invocation.kind !== "explore" || options.provider !== "openai-codex")) {
    throw new Error("Priority service tier is restricted to openai-codex Explore sessions");
  }
  const model = options.models.getModel(options.provider, invocation.model);
  if (!model) throw new Error(`model_unavailable: ${options.provider}/${invocation.model}`);
  const loadout = invocation.kind === "explore" ? exploreLoadout(options.parentLoadout) : [];
  const allowed = new Set(loadout);
  let turns = 0;
  const extension = (pi: ExtensionAPI) => {
    for (const tool of options.localTools.filter((tool) => allowed.has(tool.name))) pi.registerTool(tool);
    pi.on("tool_call", (event) => {
      const reason = executionRefusal(event.toolName, event.parentToolCallId !== undefined, allowed);
      return reason ? { block: true, reason } : undefined;
    });
    pi.on("user_bash", () => ({ result: { output: "auxiliary sessions cannot run bash", exitCode: 1, cancelled: false, truncated: false } }));
    // The terminal tool-only turn is bounded even when a provider repeatedly asks to use tools.
    pi.on("turn_start", (_event, ctx) => {
      if (signal.aborted || ++turns > invocation.maxTurns) void ctx.abort();
    });
  };
  mkdirSync(dirname(invocation.sessionFile), { recursive: true, mode: 0o700 });
  // Never overwrite an interrupted/canonical transcript or silently retry billable work.
  writeFileSync(invocation.sessionFile, JSON.stringify({ type: "session", version: sdk.CURRENT_SESSION_VERSION,
    id: invocation.sessionId, timestamp: new Date().toISOString(), cwd: options.cwd }) + "\n", { flag: "wx", mode: 0o600 });
  const manager = sdk.SessionManager.open(invocation.sessionFile);
  const services = await sdk.createAgentSessionServices({
    cwd: options.cwd, agentDir: join(options.agentDir, "auxiliary"), modelRuntime: options.models,
    settingsManager: sdk.SettingsManager.inMemory({ ...restrictedSettings(), retry: { enabled: false }, compaction: { enabled: false } }),
    resourceLoaderOptions: {
      ...resourceLoaderOptions([extension]),
      agentsFilesOverride: () => ({ agentsFiles: [] }),
      systemPromptOverride: () => invocation.kind === "explore"
        ? "Research the supplied question using only your read-only tools. Do not implement changes, invoke shell commands or delegate. Return concise findings with file references."
        : "You are a tool-free consultant. Answer the supplied question using only the provided context. Do not request tools.",
    },
  });
  const { session } = await sdk.createAgentSessionFromServices({ services, sessionManager: manager, model,
    tools: loadout, scopedModels: [{ model, thinkingLevel: "off" }], thinkingLevel: "off" });
  if (invocation.serviceTier) {
    // Keep the SDK stream, guarded provider and budget wrapper; scope the option to this child.
    const stream = session.agent.streamFunction;
    session.agent.streamFunction = (current, context, streamOptions) => {
      const requestOptions = { ...streamOptions, serviceTier: invocation.serviceTier };
      return stream(current, context, requestOptions);
    };
  }
  const observe = invocation.observe;
  const unsubscribe = observe ? session.subscribe(event => observeHelperEvent(event, observe)) : () => {};
  const abort = () => { void session.abort(); };
  signal.addEventListener("abort", abort, { once: true });
  try {
    signal.throwIfAborted();
    await session.bindExtensions({ mode: "print", abortHandler: abort });
    await session.prompt(invocation.prompt, { source: "rpc" });
    const failed = session.messages.some((message) => message.role === "assistant" && ["error", "aborted"].includes(message.stopReason));
    return { text: session.getLastAssistantText() ?? "No text response", outcome: signal.aborted || failed ? "uncertain" : "completed" };
  } finally {
    signal.removeEventListener("abort", abort);
    unsubscribe();
    session.dispose();
  }
}
