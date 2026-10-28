// The Claude Agent SDK lane (T-493) through the restricted Anthropic provider, against a fake SDK
// `query`: no Claude Code process, no network. The fake plays Claude Code's part: it streams the
// model's events and, for every tool call, asks the lane's hook and permission callback whether it
// may execute the tool itself.
import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { Options } from "@anthropic-ai/claude-agent-sdk";
import { type AssistantMessage, type Context, type Credential, Type, normalizeContext } from "@earendil-works/pi-ai";

import { type SdkBoundary, overrideSdk, resolveClaudeCode } from "../src/claude/sdk.ts";
import { CLAUDE_SDK_API } from "../src/claude/stream.ts";
import { PRE_TOOL_USE_DENIAL, TOOL_EXECUTION_DENIED_MESSAGE } from "../src/claude/tools.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { restrictProvider } from "../src/host/session.ts";

const OAUTH = "sk-ant-oat01-SYNTHETIC-SUBSCRIPTION";
const PAID = "sk-ant-api03-SYNTHETIC-PAID-KEY";
const temps: string[] = [];

afterEach(() => {
  overrideSdk(undefined);
  for (const dir of temps.splice(0)) rmSync(dir, { recursive: true, force: true });
});

type Script = (options: Options, control: { interrupted: Promise<void> }) => AsyncGenerator<unknown>;

interface FakeRun {
  options: Options[];
  prompts: unknown[];
  interrupts: number;
  closes: number;
}

function fakeSdk(script: Script): FakeRun {
  const run: FakeRun = { options: [], prompts: [], interrupts: 0, closes: 0 };
  const boundary: SdkBoundary = {
    query: ({ prompt, options }) => {
      run.options.push(options);
      let interrupted!: () => void;
      const control = { interrupted: new Promise<void>((resolve) => { interrupted = resolve; }) };
      const generator = (async function* () {
        for await (const message of prompt) run.prompts.push(message);
        yield* script(options, control);
      })();
      return Object.assign(generator, {
        interrupt: async () => {
          run.interrupts++;
          interrupted();
          return undefined;
        },
        close: () => { run.closes++; },
      }) as never;
    },
    createSdkMcpServer: (options) => ({ type: "sdk", name: options.name, instance: options }) as never,
  };
  overrideSdk(boundary);
  return run;
}

async function anthropic(env: NodeJS.ProcessEnv = { PATH: "/bin" }) {
  const root = mkdtempSync(join(tmpdir(), "corral-pi-claude-sdk-"));
  temps.push(root);
  const credential: Credential = { type: "oauth", access: OAUTH, refresh: "r", expires: Date.now() + 3_600_000 };
  const { ModelRuntime } = await import("@earendil-works/pi-coding-agent");
  const models = await ModelRuntime.create({
    credentials: {
      async read(provider: string) { return provider === "anthropic" ? credential : undefined; },
      async list() { return [{ providerId: "anthropic", type: "oauth" as const }]; },
      async modify() { throw new Error("read-only"); },
      async delete() { throw new Error("read-only"); },
    },
    modelsPath: null, refreshOnCreate: false, allowModelNetwork: false,
  });
  const guard = guardedFetch(async () => new Response("unexpected", { status: 500 }), undefined, "anthropic");
  const configDir = join(root, "claude-code");
  restrictProvider(models, "anthropic", guard.fetch as typeof fetch,
    { configDir, cwd: root, env, executable: () => "/synthetic/claude" });
  const model = models.getModel("anthropic", "claude-sonnet-5-5")!;
  const provider = models.getProvider("anthropic")!;
  const send = async (context: Context, options: Record<string, unknown> = {}) => {
    const auth = await models.getAuth("anthropic");
    const stream = provider.streamSimple(model, normalizeContext(context),
      { apiKey: auth?.auth.apiKey, ...options } as never);
    const events: string[] = [];
    for await (const event of stream) events.push(event.type);
    return { events, message: await stream.result() as AssistantMessage };
  };
  return { send, guard, configDir };
}

const user = (text: string) => ({ role: "user" as const, content: text, timestamp: Date.now() });

function* textReply(text: string) {
  yield { type: "stream_event", event: { type: "message_start", message: { usage: { input_tokens: 12, output_tokens: 0 } } } };
  yield { type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } } };
  yield { type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: text.slice(0, 3) } } };
  yield { type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: text.slice(3) } } };
  yield { type: "stream_event", event: { type: "content_block_stop", index: 0 } };
  yield { type: "stream_event", event: { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 5 } } };
  yield { type: "result", subtype: "success", is_error: false, result: text, stop_reason: "end_turn", usage: { input_tokens: 12, output_tokens: 5 } };
}

test("a streamed text reply becomes one Pi assistant message", async () => {
  const run = fakeSdk(async function* () { yield* textReply("Hello there"); });
  const { send, guard } = await anthropic();
  const { events, message } = await send({ systemPrompt: "You are a corral worker.", messages: [user("hi")] });
  expect(events).toEqual(["start", "text_start", "text_delta", "text_delta", "text_end", "done"]);
  expect(message.content).toEqual([{ type: "text", text: "Hello there" }]);
  expect([message.stopReason, message.api, message.provider, message.model])
    .toEqual(["stop", CLAUDE_SDK_API, "anthropic", "claude-sonnet-5-5"]);
  expect([message.usage.input, message.usage.output]).toEqual([12, 5]);
  const options = run.options[0]!;
  expect([options.model, options.systemPrompt, options.permissionMode, options.persistSession, options.settingSources])
    .toEqual(["claude-sonnet-5-5", "You are a corral worker.", "dontAsk", false, []]);
  expect(options.pathToClaudeCodeExecutable).toBe("/synthetic/claude");
  expect(JSON.stringify(run.prompts)).toContain("hi");
  // The host process itself made no request.
  expect(guard.ledger()).toEqual({ allowed: 0, refused: 0, refusedDestinations: [] });
  expect(run.closes).toBe(1);
});

test("streamed tool_use blocks become Pi tool calls that Claude Code is denied from executing", async () => {
  const attempts: { tool: string; hook: unknown; permission: unknown }[] = [];
  const run = fakeSdk(async function* (options) {
    const calls = [
      { id: "toolu_read", name: "Read", input: { file_path: "/w/notes.md", offset: 3 } },
      { id: "toolu_corral", name: "mcp__custom-tools__corral_task_show", input: { id: "T-1", json: true } },
    ];
    for (const [index, call] of calls.entries()) {
      yield { type: "stream_event", event: { type: "content_block_start", index, content_block: { type: "tool_use", id: call.id, name: call.name, input: {} } } };
      const json = JSON.stringify(call.input);
      yield { type: "stream_event", event: { type: "content_block_delta", index, delta: { type: "input_json_delta", partial_json: json.slice(0, 5) } } };
      yield { type: "stream_event", event: { type: "content_block_delta", index, delta: { type: "input_json_delta", partial_json: json.slice(5) } } };
      yield { type: "stream_event", event: { type: "content_block_stop", index } };
    }
    yield { type: "stream_event", event: { type: "message_delta", delta: { stop_reason: "tool_use" }, usage: { output_tokens: 40 } } };
    // Claude Code would now run each tool; it asks the hook and the permission callback first.
    for (const call of calls) {
      const matcher = options.hooks?.PreToolUse?.find((entry) => new RegExp(`^(?:${entry.matcher})$`).test(call.name));
      const hook = matcher ? await matcher.hooks[0]!({ hook_event_name: "PreToolUse", tool_name: call.name, tool_input: call.input } as never,
        call.id, { signal: new AbortController().signal }) : undefined;
      const permission = await options.canUseTool?.(call.name, call.input, { signal: new AbortController().signal } as never);
      attempts.push({ tool: call.name, hook, permission });
    }
    yield { type: "result", subtype: "success", is_error: false, result: "", stop_reason: "end_turn", usage: { output_tokens: 40 } };
  });
  const { send } = await anthropic();
  const tools = [
    { name: "read", description: "Read a file", parameters: Type.Object({ path: Type.String(), offset: Type.Optional(Type.Number()) }) },
    { name: "corral_task_show", description: "Show a task", parameters: Type.Object({ id: Type.String(), json: Type.Optional(Type.Boolean()) }) },
  ];
  const { message } = await send({ systemPrompt: "s", messages: [user("look at T-1")], tools });
  expect(message.stopReason).toBe("toolUse");
  expect(message.content).toEqual([
    { type: "toolCall", id: "toolu_read", name: "read", arguments: { path: "/w/notes.md", offset: 3 } },
    { type: "toolCall", id: "toolu_corral", name: "corral_task_show", arguments: { id: "T-1", json: true } },
  ]);
  // Claude Code sees Pi's built-in as its own Read and the corral tool through the deny-only MCP server.
  const options = run.options[0]!;
  expect(options.tools).toEqual(["Read"]);
  const server = (options.mcpServers?.["custom-tools"] as unknown as { instance: { tools: { name: string; handler(): Promise<unknown> }[] } });
  expect(server.instance.tools.map((tool) => tool.name)).toEqual(["corral_task_show"]);
  expect(await server.instance.tools[0]!.handler())
    .toEqual({ content: [{ type: "text", text: TOOL_EXECUTION_DENIED_MESSAGE }], isError: true });
  // Every execution attempt was denied, and the hook stops Claude Code's own loop.
  expect(attempts.map((attempt) => attempt.tool)).toEqual(["Read", "mcp__custom-tools__corral_task_show"]);
  for (const attempt of attempts) {
    expect(attempt.hook).toEqual(PRE_TOOL_USE_DENIAL);
    expect(PRE_TOOL_USE_DENIAL).toEqual({ continue: false, hookSpecificOutput: {
      hookEventName: "PreToolUse", permissionDecision: "deny", permissionDecisionReason: TOOL_EXECUTION_DENIED_MESSAGE } });
    expect(attempt.permission).toEqual({ behavior: "deny", message: TOOL_EXECUTION_DENIED_MESSAGE });
  }
});

test("Pi's abort interrupts and closes the query", async () => {
  let streaming!: () => void;
  const started = new Promise<void>((resolve) => { streaming = resolve; });
  const run = fakeSdk(async function* (_options, control) {
    yield { type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } } };
    streaming();
    await control.interrupted;
  });
  const { send } = await anthropic();
  const abort = new AbortController();
  const pending = send({ messages: [user("long task")] }, { signal: abort.signal });
  await started;
  abort.abort();
  const { events, message } = await pending;
  expect(run.interrupts).toBe(1);
  expect(run.closes).toBe(1);
  expect(events.at(-1)).toBe("error");
  expect(message.stopReason).toBe("aborted");
});

test("the Claude Code subprocess gets the corral-owned OAuth token and no ambient Anthropic auth", async () => {
  const run = fakeSdk(async function* () { yield* textReply("ok"); });
  const { send, configDir } = await anthropic({
    PATH: "/bin", HOME: "/home/owner", ANTHROPIC_API_KEY: PAID, ANTHROPIC_AUTH_TOKEN: PAID,
    ANTHROPIC_BASE_URL: "https://proxy.invalid", CLAUDE_CODE_OAUTH_TOKEN: "sk-ant-oat01-FOREIGN",
    CLAUDE_CODE_USE_BEDROCK: "1", CLAUDE_CONFIG_DIR: "/home/owner/.claude",
  });
  await send({ messages: [user("hi")] });
  const env = run.options[0]!.env!;
  expect(env.CLAUDE_CODE_OAUTH_TOKEN).toBe(OAUTH);
  expect(env.CLAUDE_CONFIG_DIR).toBe(configDir);
  expect(Object.keys(env).filter((key) => key.startsWith("ANTHROPIC_"))).toEqual([]);
  expect(env.CLAUDE_CODE_USE_BEDROCK).toBeUndefined();
  expect(JSON.stringify(env)).not.toContain(PAID);
  expect([env.PATH, env.HOME]).toEqual(["/bin", "/home/owner"]);
});

test("a missing Claude Code platform package is refused with a stable code naming it", () => {
  const deps = { platform: "linux", arch: "x64", resolve: () => { throw new Error("not found"); }, isFile: () => false };
  expect(() => resolveClaudeCode(deps)).toThrow(/^claude_code_unavailable: .*install @anthropic-ai\/claude-agent-sdk-linux-x64 /);
  // The shipped install resolves the binary the SDK's platform package put next to it.
  expect(resolveClaudeCode()).toMatch(/@anthropic-ai\/claude-agent-sdk-linux-x64(-musl)?\/claude$/);
});
