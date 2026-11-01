// A separate, search-only SDK query. The ordinary coding lane still denies every execution.
import { mkdirSync } from "node:fs";
import type { Options, Query } from "@anthropic-ai/claude-agent-sdk";
import { createAssistantMessageEventStream, type AssistantMessageEventStream, type Model,
  type SimpleStreamOptions, type TranscriptContext } from "@earendil-works/pi-ai";
import { assistantFailure, emptyOutput, resultFailure, updateUsage } from "./events.ts";
import { buildPromptBlocks, promptStream } from "./prompt.ts";
import { loadSdk, resolveClaudeCode } from "./sdk.ts";
import { claudeEnvironment, claudeSystemPrompt, type ClaudeLaneConfig } from "./stream.ts";

export function streamClaudeSearch(
  config: ClaudeLaneConfig, model: Model<string>, context: TranscriptContext, options?: SimpleStreamOptions,
): AssistantMessageEventStream {
  const stream = createAssistantMessageEventStream();
  void (async () => {
    const output = emptyOutput(model);
    let query: Query | undefined;
    let closed = false;
    let completed = false;
    const close = () => { if (query && !closed) { closed = true; query.close(); } };
    const abort = () => { if (query) void query.interrupt().catch(() => undefined).finally(close); };
    options?.signal?.addEventListener("abort", abort, { once: true });
    try {
      options?.signal?.throwIfAborted();
      const token = options?.apiKey;
      if (typeof token !== "string" || !token.includes("sk-ant-oat")) throw new Error("credentials_unavailable: subscription OAuth required");
      const sdk = await loadSdk();
      mkdirSync(config.configDir, { recursive: true, mode: 0o700 });
      const queryOptions: Options = {
        cwd: config.cwd ?? process.cwd(), model: model.id, tools: ["WebSearch"], allowedTools: ["WebSearch"],
        permissionMode: "dontAsk", settingSources: [], strictMcpConfig: true, persistSession: false,
        maxTurns: 3, pathToClaudeCodeExecutable: (config.executable ?? resolveClaudeCode)(),
        env: claudeEnvironment(config.env ?? process.env, token, config.configDir),
        systemPrompt: claudeSystemPrompt(context.messages),
        canUseTool: async (name, input) => name === "WebSearch"
          ? { behavior: "allow", updatedInput: input }
          : { behavior: "deny", message: "Only WebSearch is permitted in this query" },
        hooks: { PreToolUse: [{ hooks: [async (input) => {
          const allowed = input.hook_event_name === "PreToolUse" && input.tool_name === "WebSearch";
          return { continue: allowed, hookSpecificOutput: {
            hookEventName: "PreToolUse", permissionDecision: allowed ? "allow" : "deny",
            permissionDecisionReason: "Search-only query; all other tools are denied",
          } };
        }] }] },
      };
      options?.signal?.throwIfAborted();
      query = sdk.query({ prompt: promptStream(buildPromptBlocks(context.messages, new Map())), options: queryOptions });
      if (options?.signal?.aborted) abort();
      stream.push({ type: "start", partial: output });
      for await (const message of query) {
        await options?.onProviderStreamEvent?.(message, model);
        // Error results (including max-turn exhaustion) can still report consumed quota.
        if (message.type === "result") updateUsage(model, output, message.usage);
        const failure = message.type === "assistant" ? assistantFailure(message)
          : message.type === "result" ? resultFailure(message) : undefined;
        if (failure) throw failure;
        if (message.type === "result" && message.subtype === "success") {
          completed = true;
          output.content = [{ type: "text", text: message.result }];
        }
      }
      options?.signal?.throwIfAborted();
      if (!completed) throw new Error("web_search_failed: Claude query ended without a terminal result");
      stream.push({ type: "done", reason: "stop", message: output });
    } catch (error) {
      output.stopReason = options?.signal?.aborted ? "aborted" : "error";
      output.errorMessage = error instanceof Error ? error.message : String(error);
      stream.push({ type: "error", reason: output.stopReason, error: output });
    } finally {
      options?.signal?.removeEventListener("abort", abort);
      close();
      stream.end();
    }
  })();
  return stream;
}
