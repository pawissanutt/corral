import { expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AssistantMessage } from "@earendil-works/pi-ai";
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ExtensionAPI, ModelRuntime } from "@earendil-works/pi-coding-agent";
import type { BuildContext } from "../../build/types.ts";
import { FleetRuntime } from "./runtime.ts";

test("build text deltas do not recompute session usage, while turns and tool events do", async () => {
  const dir = mkdtempSync(join(tmpdir(), "fleet-stream-"));
  const ready = Promise.withResolvers<void>();
  const finished = Promise.withResolvers<void>();
  let observer: ((event: Sdk.AgentSessionEvent) => void) | undefined;
  let turnStart: (() => void) | undefined;
  let statsCalls = 0;
  let contextCalls = 0;
  const session = {
    messages: [],
    subscribe(changed: (event: Sdk.AgentSessionEvent) => void) { observer = changed; return () => { observer = undefined; }; },
    async bindExtensions() {},
    async prompt() { ready.resolve(); await finished.promise; },
    async abort() { finished.resolve(); },
    getLastAssistantText: () => "Done",
    getSessionStats() { statsCalls++; return { tokens: { input: 1, output: 2 }, cost: 0 }; },
    getContextUsage() { contextCalls++; return { percent: 3 }; },
    dispose() {},
  };
  const sdk = {
    CURRENT_SESSION_VERSION: 3,
    SettingsManager: { inMemory: () => ({}) },
    SessionManager: { open: () => ({}) },
    async createAgentSessionServices(options: { resourceLoaderOptions: { extensionFactories: ((api: ExtensionAPI) => void)[] } }) {
      const api = { registerTool() {}, on(name: string, handler: unknown) { if (name === "turn_start") turnStart = handler as () => void; } };
      for (const extension of options.resourceLoaderOptions.extensionFactories) extension(api as unknown as ExtensionAPI);
      return {};
    },
    async createAgentSessionFromServices() { return { session }; },
  } as unknown as typeof Sdk;
  const context: BuildContext = { dir, corral: "corral", settings: { maxParallel: 1, exitPolicy: "none", profiles: [] },
    session: { version: 1, id: "B-test", name: "test", created: "", updated: "", repoRoot: dir, worktree: dir,
      branch: "build/test", base: null, here: true } };
  const runtime = new FleetRuntime({ sdk, models: { getModel: () => ({}) } as unknown as ModelRuntime,
    approved: ["fixture"], parent: { provider: "fixture", model: "model" }, context, agentDir: dir, localTools: [],
    changed() {}, now: () => 1000 });
  const running = runtime.start("call", [{ id: "child", title: "Child", brief: "Implement", owns: ["src"] }]);
  try {
    await ready.promise;
    expect(turnStart).toBeDefined();
    turnStart!();
    expect([statsCalls, contextCalls]).toEqual([1, 1]);
    const message: AssistantMessage = { role: "assistant", api: "openai-responses", provider: "fixture", model: "model", timestamp: 1,
      content: [{ type: "text", text: "x".repeat(100) }], stopReason: "stop",
      usage: { input: 1, output: 2, cacheRead: 0, cacheWrite: 0, totalTokens: 3,
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } };
    observer!({ type: "message_start", message });
    for (let index = 0; index < 100; index++) observer!({ type: "message_update", message,
      assistantMessageEvent: { type: "text_delta", contentIndex: 0, delta: "x", partial: message } });
    expect([statsCalls, contextCalls]).toEqual([1, 1]);
    expect(runtime.snapshot().children[0]!.messages[0]!.text).toHaveLength(100);
    observer!({ type: "message_end", message });
    expect([statsCalls, contextCalls]).toEqual([2, 2]);
    const partial: AssistantMessage = { ...message,
      content: [{ type: "toolCall", id: "write-1", name: "write", arguments: { path: "src/a.ts", content: "a" } }] };
    observer!({ type: "message_update", message: partial,
      assistantMessageEvent: { type: "toolcall_start", contentIndex: 0, partial } });
    expect([statsCalls, contextCalls]).toEqual([3, 3]);
    expect(runtime.snapshot().children[0]!.messages.find(item => item.kind === "tool")).toMatchObject({
      tool: "write", phase: "requested", argsComplete: false,
    });
    observer!({ type: "tool_execution_start", toolCallId: "write-1", toolName: "write", args: {} });
    expect([statsCalls, contextCalls]).toEqual([4, 4]);
    observer!({ type: "tool_execution_update", toolCallId: "write-1", toolName: "write", args: {},
      partialResult: { content: [{ type: "text", text: "partial result" }] } });
    expect([statsCalls, contextCalls]).toEqual([5, 5]);
    expect(runtime.snapshot().children[0]!.messages.find(item => item.kind === "tool")).toMatchObject({
      tool: "write", phase: "running", partial: true, text: "partial result",
    });
    observer!({ type: "tool_execution_end", toolCallId: "write-1", toolName: "write", isError: false,
      result: { content: [{ type: "text", text: "File written" }] } });
    expect([statsCalls, contextCalls]).toEqual([6, 6]);
  } finally {
    finished.resolve();
    await running;
    await runtime.close();
    rmSync(dir, { recursive: true, force: true });
  }
});
