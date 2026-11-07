// Claude provider behavior through a narrow fake SDK; no network or owner credentials.
import { afterEach, expect, test } from "bun:test";
import { Type } from "typebox";
import { CLAUDE_SDK_API } from "../src/claude/stream.ts";
import { PRE_TOOL_USE_DENIAL, TOOL_EXECUTION_DENIED_MESSAGE, mapToolArgs } from "../src/claude/tools.ts";
import { OAUTH, PAID, anthropic, fakeSdk, resetSdkFixture, textReply, user } from "./support/claude-sdk-fixture.ts";

afterEach(resetSdkFixture);

test("a streamed text reply becomes one Pi assistant message", async () => {
  const run = fakeSdk(async function* () { yield* textReply("Hello there"); });
  const { send, guard } = await anthropic();
  const context = { systemPrompt: "You are a corral worker.", messages: [user("hi")] };
  const { events, message } = await send(context, { sessionId: "cache-sequence" });
  expect(events).toEqual(["start", "text_start", "text_delta", "text_delta", "text_end", "done"]);
  expect(message.content).toEqual([{ type: "text", text: "Hello there" }]);
  expect([message.stopReason, message.api, message.provider, message.model])
    .toEqual(["stop", CLAUDE_SDK_API, "anthropic", "claude-sonnet-5-5"]);
  expect([message.usage.input, message.usage.output]).toEqual([12, 5]);
  const options = run.options[0]!;
  expect([options.model, options.systemPrompt, options.permissionMode, options.persistSession, options.settingSources])
    .toEqual(["claude-sonnet-5-5", { type: "preset", preset: "claude_code", append: "You are a corral worker." }, "dontAsk",
      false, []]);
  expect(options.pathToClaudeCodeExecutable).toBe("/synthetic/claude");
  expect(JSON.stringify(run.prompts)).toContain("hi");
  // The host process itself made no request.
  expect(guard.ledger()).toEqual({ allowed: 0, refused: 0, refusedDestinations: [] });
  expect(run.closes).toBe(1);
  // Exercise the actual provider's session-scoped frontier, not only the pure renderer.
  const history = [...context.messages, message];
  await send({ ...context, messages: [...history, user("continue")] }, { sessionId: "cache-sequence" });
  await send({ ...context, messages: [...history, user("continue"),
    ...Array.from({ length: 25 }, (_, index) => ({ role: "toolResult" as const, toolName: "read",
      toolCallId: `r${index}`, content: [{ type: "text" as const, text: `result ${index}` }], isError: false, timestamp: 0 })),
    user("next")] }, { sessionId: "cache-sequence" });
  const warm = run.prompts[1]?.message.content;
  const batch = run.prompts[2]?.message.content;
  if (!Array.isArray(warm) || !Array.isArray(batch)) throw new Error("prompt blocks missing");
  const before = warm.findIndex((block) => "cache_control" in block);
  const after = batch.findIndex((block) => "cache_control" in block);
  expect(before).toBeGreaterThanOrEqual(0);
  expect(after - before + 1).toBeLessThanOrEqual(20);
  expect(after).toBeGreaterThan(before);
});

test("streamed tool_use blocks become Pi tool calls that Claude Code is denied from executing", async () => {
  const attempts: { tool: string; hook: unknown; permission: unknown }[] = [];
  const run = fakeSdk(async function* (options) {
    const calls = [
      { id: "toolu_read", name: "Read", input: { file_path: "/w/notes.md", offset: 3 } },
      { id: "toolu_corral", name: "mcp__custom-tools__corral_task_show", input: { id: "T-1", json: true } },
      { id: "toolu_managed", name: "mcp__custom-tools__bash", input: { command: "echo managed", timeout: 600, wait_ms: 0 } },
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
    { name: "bash", description: "Managed Bash", parameters: Type.Object({ command: Type.String(), timeout: Type.Number(), wait_ms: Type.Number() }) },
  ];
  const { message } = await send({ systemPrompt: "s", messages: [user("look at T-1")], tools });
  expect(message.stopReason).toBe("toolUse");
  expect(message.content).toEqual([
    { type: "toolCall", id: "toolu_read", name: "read", arguments: { path: "/w/notes.md", offset: 3 } },
    { type: "toolCall", id: "toolu_corral", name: "corral_task_show", arguments: { id: "T-1", json: true } },
    { type: "toolCall", id: "toolu_managed", name: "bash", arguments: { command: "echo managed", timeout: 600, wait_ms: 0 } },
  ]);
  // Claude Code sees Pi's built-in as its own Read and the corral tool through the deny-only MCP server.
  const options = run.options[0]!;
  expect(options.tools).toEqual(["Read"]);
  const server = (options.mcpServers?.["custom-tools"] as unknown as { instance: { tools: { name: string; handler(): Promise<unknown> }[] } });
  expect(server.instance.tools.map((tool) => tool.name)).toEqual(["corral_task_show", "bash"]);
  expect(mapToolArgs("bash", { command: "legacy", timeout: 600000, run_in_background: true }))
    .toEqual({ command: "legacy", timeout: 600, wait_ms: 0 });
  // Pi owns discovery: Claude's deferred tool search is not in this lane's loadout.
  expect((server.instance as unknown as { alwaysLoad: boolean }).alwaysLoad).toBe(true);
  expect(await server.instance.tools[0]!.handler())
    .toEqual({ content: [{ type: "text", text: TOOL_EXECUTION_DENIED_MESSAGE }], isError: true });
  // Every execution attempt was denied, and the hook stops Claude Code's own loop.
  expect(attempts.map((attempt) => attempt.tool)).toEqual(["Read", "mcp__custom-tools__corral_task_show", "mcp__custom-tools__bash"]);
  for (const attempt of attempts) {
    expect(attempt.hook).toEqual(PRE_TOOL_USE_DENIAL);
    expect(PRE_TOOL_USE_DENIAL).toEqual({ continue: false, hookSpecificOutput: {
      hookEventName: "PreToolUse", permissionDecision: "deny", permissionDecisionReason: TOOL_EXECUTION_DENIED_MESSAGE } });
    expect(attempt.permission).toEqual({ behavior: "deny", message: TOOL_EXECUTION_DENIED_MESSAGE });
  }
});

test("Pi owns compaction and unexpected SDK compaction cannot bless a replay", async () => {
  const run = fakeSdk(async function* () {
    yield { type: "system", subtype: "compact_boundary", compact_metadata: { trigger: "auto", pre_tokens: 1000 } };
    yield* textReply("must not be accepted");
  });
  const { send } = await anthropic();
  const { message } = await send({ messages: [user("continue")] });
  expect(message.stopReason).toBe("error");
  expect(message.errorMessage).toContain("claude_compaction_conflict");
  expect(message.diagnostics?.find(diagnostic => diagnostic.type === "claude_cache"))
    .toMatchObject({ details: { usageReported: false, inputTokens: null, hitRate: null } });
  expect(run.options[0]?.settings).toEqual({ autoCompactEnabled: false });
  expect(run.closes).toBe(1);
});

test("failed SDK results retain consumed usage without logging prompt or credentials", async () => {
  fakeSdk(async function* () {
    yield { type: "result", subtype: "error_max_turns", is_error: true, errors: ["synthetic failure"],
      usage: { input_tokens: 3, output_tokens: 2, cache_creation_input_tokens: 10,
        cache_creation: { ephemeral_1h_input_tokens: 10 } } };
  });
  const { send } = await anthropic();
  const { message } = await send({ messages: [user("private prompt fixture")] });
  expect(message.stopReason).toBe("error");
  expect(message.usage).toMatchObject({ input: 3, output: 2, cacheWrite: 10, cacheWrite1h: 10 });
  expect(message.diagnostics?.find(diagnostic => diagnostic.type === "claude_cache"))
    .toMatchObject({ details: { usageReported: true, promptTokens: 13, cacheWrite1hTokens: 10 } });
  expect(JSON.stringify(message.diagnostics)).not.toContain("private prompt fixture");
  expect(JSON.stringify(message.diagnostics)).not.toContain(OAUTH);
});

test("an incomplete SDK stream cannot report success", async () => {
  fakeSdk(async function* () {
    for (const event of textReply("partial")) if (event.type !== "result") yield event;
  });
  const { send } = await anthropic();
  const { message } = await send({ messages: [user("continue")] });
  expect(message.stopReason).toBe("error");
  expect(message.errorMessage).toContain("claude_query_incomplete");
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

test("retention aligns SDK and history markers, while summary requests disable caching", async () => {
  for (const retention of ["5m", "1h", "none"] as const) {
    const run = fakeSdk(async function* () { yield* textReply("ok"); });
    const { send } = await anthropic({ PATH: "/bin", CORRAL_CLAUDE_CACHE_TTL: retention,
      FORCE_PROMPT_CACHING_5M: "1", DISABLE_PROMPT_CACHING: "1" });
    const context = { messages: [user("history"), user("continue")] };
    const { message } = await send(context);
    expect(message.stopReason).toBe("stop");
    const env = run.options[0]?.env;
    expect(env?.CLAUDE_CODE_PROMPT_CACHE_TTL).toBe(retention === "none" ? "5m" : retention);
    expect(env?.DISABLE_PROMPT_CACHING).toBe(retention === "none" ? "1" : undefined);
    expect(env?.FORCE_PROMPT_CACHING_5M).toBe(retention === "5m" ? "1" : undefined);
    const content = run.prompts[0]?.message.content;
    expect(JSON.stringify(content).includes('"cache_control"')).toBe(retention !== "none");
    if (retention !== "none") expect(JSON.stringify(content)).toContain(`"ttl":"${retention}"`);
    await send(context, { cacheRetention: "none" });
    expect(run.options[1]?.env?.DISABLE_PROMPT_CACHING).toBe("1");
    expect(JSON.stringify(run.prompts[1]?.message.content)).not.toContain('"cache_control"');
  }
});
