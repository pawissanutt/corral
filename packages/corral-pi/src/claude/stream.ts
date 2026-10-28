// The Claude Agent SDK lane (T-493): a Pi API provider whose every model call runs one `query()`
// of the official SDK, which spawns Claude Code on the subscription. Claude Code only plans: its
// tool calls stream back as Pi tool calls, and Pi executes them. Ported from senpi's
// anthropic-subscription stream.js and options.js (MIT), per-turn lane only.
import { mkdirSync } from "node:fs";

import type { Options, Query } from "@anthropic-ai/claude-agent-sdk";
import {
  type AssistantMessageEventStream, type Model, type SimpleStreamOptions, type TranscriptContext,
  createAssistantMessageEventStream, getCurrentSystemPrompt, getCurrentTools,
} from "@earendil-works/pi-ai";
import { registerApiProvider } from "@earendil-works/pi-ai/compat";

import { buildCustomToolServers } from "./custom-tools.ts";
import { type StreamEvent, applyStreamEvent, assistantFailure, emptyOutput, mapStopReason, resultFailure, updateUsage }
  from "./events.ts";
import { buildPromptBlocks, promptStream } from "./prompt.ts";
import { loadSdk, resolveClaudeCode } from "./sdk.ts";
import { HOST_TOOL_DENIAL_HOOKS, canUseTool, resolveSdkTools } from "./tools.ts";

/** The Pi API id of this lane; Anthropic models are re-addressed to it by the restricted provider. */
export const CLAUDE_SDK_API = "claude-agent-sdk";

export interface ClaudeLaneConfig {
  /** A host-owned Claude Code config directory, so Claude Code never reads or writes `~/.claude`. */
  readonly configDir: string;
  readonly env?: NodeJS.ProcessEnv;
  readonly cwd?: string;
  readonly executable?: () => string;
}

const ADAPTIVE_MARKERS = ["opus-4-6", "opus-4-7", "opus-4-8", "opus-5", "sonnet-4-6", "sonnet-5", "fable-5", "mythos-5"];
const XHIGH_MARKERS = ["opus-4-7", "opus-4-8", "opus-5", "sonnet-5", "fable-5", "mythos-5"];
const THINKING_BUDGETS = { minimal: 2048, low: 8192, medium: 16384, high: 31999 } as const;

/**
 * The subprocess environment: the host's, without any ambient Anthropic or Claude Code auth, with
 * the corral-owned OAuth token as the only credential and a private config directory.
 */
export function claudeEnvironment(parent: NodeJS.ProcessEnv, token: string, configDir: string): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(parent)) {
    if (value === undefined || key.startsWith("ANTHROPIC_") || key.startsWith("CLAUDE_CODE_") ||
        key === "CLAUDE_CONFIG_DIR" || key.startsWith("AWS_BEARER_TOKEN_BEDROCK")) continue;
    env[key] = value;
  }
  env.CLAUDE_CODE_OAUTH_TOKEN = token;
  env.CLAUDE_CONFIG_DIR = configDir;
  env.CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC = "1";
  env.CLAUDE_AGENT_SDK_CLIENT_APP = "corral-pi";
  return env;
}

function thinkingOptions(model: Model<string>, options: SimpleStreamOptions | undefined): Partial<Options> {
  const reasoning = options?.reasoning;
  if (!reasoning) return {};
  const id = model.id.toLowerCase();
  if (ADAPTIVE_MARKERS.some((marker) => id.includes(marker))) {
    const effort = reasoning === "minimal" || reasoning === "low" ? "low"
      : reasoning === "xhigh" ? (XHIGH_MARKERS.some((marker) => id.includes(marker)) ? "xhigh" : "max") : reasoning;
    return { thinking: { type: "adaptive", display: "summarized" }, effort };
  }
  const level = reasoning === "xhigh" || reasoning === "max" ? "high" : reasoning;
  const custom = options?.thinkingBudgets?.[level];
  return { maxThinkingTokens: typeof custom === "number" && custom > 0 ? custom : THINKING_BUDGETS[level] };
}

export function streamClaudeSdk(
  config: ClaudeLaneConfig, model: Model<string>, context: TranscriptContext, options?: SimpleStreamOptions,
): AssistantMessageEventStream {
  const stream = createAssistantMessageEventStream();
  void (async () => {
    const output = emptyOutput(model);
    const blocks: Parameters<typeof applyStreamEvent>[0]["blocks"] = [];
    let query: Query | undefined;
    let closed = false;
    let aborted = false;
    let started = false;
    let sawStreamEvent = false;
    const close = () => {
      if (closed || !query) return;
      closed = true;
      query.close();
    };
    const interrupt = () => {
      if (!query) return;
      void query.interrupt().catch(() => undefined).finally(close);
    };
    const onAbort = () => {
      aborted = true;
      interrupt();
    };
    if (options?.signal?.aborted) onAbort();
    else options?.signal?.addEventListener("abort", onAbort, { once: true });
    try {
      const token = options?.apiKey;
      if (typeof token !== "string" || token.length === 0) {
        throw new Error("credentials_unavailable: no subscription OAuth token for the Claude Agent SDK lane");
      }
      const executable = (config.executable ?? resolveClaudeCode)();
      const sdk = await loadSdk();
      mkdirSync(config.configDir, { recursive: true, mode: 0o700 });
      const messages = context.messages;
      const toolLess = options?.toolChoice === "none";
      const tools = resolveSdkTools(toolLess ? [] : getCurrentTools(messages));
      const mcpServers = await buildCustomToolServers(tools.customTools);
      const queryOptions: Options = {
        cwd: config.cwd ?? process.cwd(),
        model: model.id,
        tools: tools.sdkTools,
        permissionMode: "dontAsk",
        includePartialMessages: true,
        canUseTool,
        hooks: HOST_TOOL_DENIAL_HOOKS,
        systemPrompt: getCurrentSystemPrompt(messages),
        settingSources: [],
        strictMcpConfig: true,
        persistSession: false,
        pathToClaudeCodeExecutable: executable,
        env: claudeEnvironment(config.env ?? process.env, token, config.configDir),
        ...(mcpServers ? { mcpServers } : {}),
        ...(toolLess ? { maxTurns: 1 } : {}),
        ...thinkingOptions(model, options),
      };
      query = sdk.query({ prompt: promptStream(buildPromptBlocks(messages, tools.customToolNameToSdk)), options: queryOptions });
      if (aborted) interrupt();
      for await (const message of query) {
        const failure = message.type === "assistant" ? assistantFailure(message)
          : message.type === "result" ? resultFailure(message) : undefined;
        if (failure) throw failure;
        if (!started) {
          stream.push({ type: "start", partial: output });
          started = true;
        }
        if (message.type === "stream_event") {
          sawStreamEvent = true;
          applyStreamEvent({ model, output, blocks, stream, customToolNameToPi: tools.customToolNameToPi },
            message.event as StreamEvent);
        } else if (message.type === "result" && message.subtype === "success") {
          updateUsage(model, output, message.usage);
          // A terminal result never downgrades a tool-use turn, or Pi would not run the calls.
          if (message.stop_reason != null && output.stopReason !== "toolUse") output.stopReason = mapStopReason(message.stop_reason);
          if (!sawStreamEvent) output.content.push({ type: "text", text: message.result });
        }
      }
      if (aborted || options?.signal?.aborted) {
        output.stopReason = "aborted";
        output.errorMessage = "Operation aborted";
        stream.push({ type: "error", reason: "aborted", error: output });
      } else {
        const reason = output.stopReason === "toolUse" ? "toolUse" : output.stopReason === "length" ? "length" : "stop";
        stream.push({ type: "done", reason, message: output });
      }
    } catch (error) {
      output.stopReason = aborted || options?.signal?.aborted ? "aborted" : "error";
      output.errorMessage = error instanceof Error ? error.message : String(error);
      stream.push({ type: "error", reason: output.stopReason, error: output });
    } finally {
      options?.signal?.removeEventListener("abort", onAbort);
      close();
      stream.end();
    }
  })();
  return stream;
}

export function registerClaudeSdkApi(config: ClaudeLaneConfig): void {
  const run = (model: Model<string>, context: TranscriptContext, options?: SimpleStreamOptions) =>
    streamClaudeSdk(config, model, context, options);
  registerApiProvider({ api: CLAUDE_SDK_API, stream: run, streamSimple: run }, "corral-claude-sdk");
}
