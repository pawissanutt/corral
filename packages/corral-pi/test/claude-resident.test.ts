// Offline resident-process journeys: no engine, network, owner credentials, sleeps or timing assertions.
import { afterEach, expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Type } from "typebox";
import { type AssistantMessage, type Context, type SimpleStreamOptions, normalizeContext } from "@earendil-works/pi-ai";
import type { SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import * as sdk from "@earendil-works/pi-coding-agent";
import { ReplayCache } from "../src/claude/cache.ts";
import { type ClaudeLaneConfig, streamClaudeSdk } from "../src/claude/stream.ts";
import { ResidentSessions, type ResidentScheduler, RESIDENT_IDLE_MS, RESIDENT_MAX_ENTRIES, residentSessions } from "../src/claude/resident.ts";
import { type FailureMessage } from "../src/host/provider-failure.ts";
import { EventLog } from "../src/bridge/events.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { OAUTH, anthropic, fakeSdk, resetSdkFixture, textReply, user } from "./support/claude-sdk-fixture.ts";
import type { BuildContext } from "../src/build/types.ts";
import { createSdkRunner } from "../src/extensions/fleet/child.ts";

const registries: ResidentSessions[] = [];
afterEach(() => {
  for (const registry of registries.splice(0)) registry.closeAll();
  resetSdkFixture();
});

function* reply(message: SDKUserMessage, text = "ok") {
  for (const event of textReply(text)) yield event.type === "result" ? { ...event, user_message_uuid: message.uuid } : event;
}

function* toolReply(message: SDKUserMessage) {
  yield { type: "stream_event", event: { type: "content_block_start", index: 0,
    content_block: { type: "tool_use", id: "call-bash", name: "mcp__custom-tools__bash", input: { command: "pwd" } } } };
  yield { type: "stream_event", event: { type: "content_block_delta", index: 0,
    delta: { type: "input_json_delta", partial_json: JSON.stringify({ command: "pwd" }) } } };
  yield { type: "stream_event", event: { type: "content_block_stop", index: 0 } };
  yield { type: "stream_event", event: { type: "message_delta", delta: { stop_reason: "tool_use" } } };
  yield { type: "result", subtype: "success", result: "", user_message_uuid: message.uuid, stop_reason: "end_turn" };
}

const managedTools = [{ name: "bash", description: "Managed shell", parameters: Type.Object({ command: Type.String(), wait_ms: Type.Optional(Type.Number()) }) }];
const route = (message: AssistantMessage) => message.diagnostics?.find(d => d.type === "claude_cache")?.details;

function fakeIdleScheduler() {
  const callbacks = new Set<() => void>();
  const schedule: ResidentScheduler = (expire, delayMs) => {
    expect(delayMs).toBe(RESIDENT_IDLE_MS);
    callbacks.add(expire);
    return () => { callbacks.delete(expire); };
  };
  return { schedule, fire() {
    const expire = callbacks.values().next().value;
    if (!expire) throw new Error("no scheduled idle expiry");
    callbacks.delete(expire);
    expire();
  } };
}

async function lane(now?: () => number, shared = false, schedule = fakeIdleScheduler().schedule) {
  const setup = await anthropic();
  const residents = shared ? residentSessions : new ResidentSessions(now, schedule);
  if (!shared) registries.push(residents);
  const cache = new ReplayCache();
  const send = async (context: Context, options: SimpleStreamOptions = {}, config: Partial<ClaudeLaneConfig> = {}) => {
    const stream = streamClaudeSdk({ ...setup.config, ...config }, setup.model, normalizeContext(context),
      { apiKey: OAUTH, sessionId: "resident", ...options }, cache, residents);
    for await (const _event of stream) { /* consume Pi's stream */ }
    return await stream.result() as AssistantMessage & FailureMessage;
  };
  return { ...setup, send, residents };
}

test("two turns use one process and send only the new tool result with fresh ids", async () => {
  const run = fakeSdk(async function* (_options, control) {
    if (control.turn === 0) yield* toolReply(control.message);
    else yield* reply(control.message);
  });
  const { send } = await lane();
  const first = await send({ messages: [user("run the command")], tools: managedTools });
  const result = { role: "toolResult" as const, toolCallId: "call-bash", toolName: "bash", content: [{ type: "text" as const, text: "/work" }], isError: false, timestamp: 1 };
  const second = await send({ messages: [user("run the command"), first, result], tools: managedTools });
  expect(first.stopReason).toBe("toolUse");
  expect(second.stopReason).toBe("stop");
  expect(run.options).toHaveLength(1);
  expect(run.closes).toBe(0);
  expect(run.prompts).toHaveLength(2);
  expect(typeof run.prompts[0]?.uuid).toBe("string");
  expect(run.prompts[1]?.uuid).not.toBe(run.prompts[0]?.uuid);
  const delta = JSON.stringify(run.prompts[1]?.message.content);
  expect(delta).toContain("Tool result (mcp__custom-tools__bash, id=call-bash):");
  expect(delta).toContain("/work");
  expect(delta).not.toContain("run the command");
  expect(delta).not.toContain("cache_control");
  expect(route(first)).toMatchObject({ mode: "resident-seed", reason: "cold" });
  expect(route(second)).toMatchObject({ mode: "resident-delta", deltaMessages: 1 });
  expect(run.options[0]?.persistSession).toBe(false);
});

for (const cause of ["history", "removed", "assistant", "foreign", "system", "tools", "connection", "credential"] as const) {
  test(`${cause} changes close and reseed from Pi's full history`, async () => {
    const run = fakeSdk(async function* (_options, control) { yield* reply(control.message); });
    const { send } = await lane();
    const first = await send({ systemPrompt: "original system", messages: [user("original user")] }, {}, { connection: "first" });
    const next = await send({ systemPrompt: cause === "system" ? "new system" : "original system",
      tools: cause === "tools" ? managedTools : [],
      messages: [...(cause === "removed" ? [] : [user(cause === "history" ? "rewritten user" : "original user")]),
        cause === "assistant" ? { ...first, content: [{ type: "text", text: "rewritten assistant" }] } : first,
        ...(cause === "foreign" ? [{ ...first, model: "another-model" }] : []), user("next")] },
    cause === "credential" ? { apiKey: `${OAUTH}-refreshed` } : {}, { connection: cause === "connection" ? "second" : "first" });
    expect(next.stopReason).toBe("stop");
    expect(run.options).toHaveLength(2);
    expect(run.closeCalls).toEqual([1, 0]);
    expect(route(next)).toMatchObject({ mode: "resident-seed",
      reason: ["history", "removed", "assistant", "foreign"].includes(cause) ? "diverged" : "changed" });
    const replay = JSON.stringify(run.prompts[1]?.message.content);
    if (cause === "removed") expect(replay).not.toContain("original user");
    else expect(replay).toContain(cause === "history" ? "rewritten user" : "original user");
  });
}

for (const cause of ["failure", "abort", "incomplete", "compaction"] as const) {
  test(`${cause} on a delta discards the process and the next call seeds`, async () => {
    const active = Promise.withResolvers<void>();
    const run = fakeSdk(async function* (_options, control) {
      if (control.query > 0 || control.turn === 0) { yield* reply(control.message); return; }
      if (cause === "failure") {
        yield { type: "result", subtype: "error_during_execution", errors: ["failure"], user_message_uuid: control.message.uuid };
      } else if (cause === "compaction") {
        yield { type: "system", subtype: "compact_boundary" };
      } else {
        yield { type: "stream_event", event: { type: "message_start", message: {} } };
        if (cause === "abort") { active.resolve(); await control.interrupted; }
      }
    });
    const { send, residents } = await lane();
    const abort = new AbortController();
    const first = await send({ messages: [user("first")] });
    const pending = send({ messages: [user("first"), first, user("delta")] }, { signal: abort.signal });
    if (cause === "abort") { await active.promise; abort.abort(); }
    const failed = await pending;
    expect(route(failed)).toMatchObject({ mode: "resident-delta", deltaMessages: 1 });
    expect(failed.stopReason).toBe(cause === "abort" ? "aborted" : "error");
    expect(residents.size).toBe(0);
    expect(run.closeCalls).toEqual([1]);
    if (cause === "abort") expect(run.interrupts).toBe(1);
    const next = await send({ messages: [user("first"), first, user("retry")] });
    expect(next.stopReason).toBe("stop");
    expect(run.options).toHaveLength(2);
    expect(route(next)).toMatchObject({ mode: "resident-seed", reason: "cold" });
    // Prompts so far: the seed, the delta that failed, then the reseed carrying Pi's full history.
    expect(JSON.stringify(run.prompts[2]?.message.content)).toContain("first");
  });
}

test("idle expiry reseeds using an injected clock", async () => {
  let now = 0;
  const run = fakeSdk(async function* (_options, control) { yield* reply(control.message); });
  const { send } = await lane(() => now);
  const first = await send({ messages: [user("first")] });
  now += RESIDENT_IDLE_MS;
  const next = await send({ messages: [user("first"), first, user("next")] });
  expect(run.closeCalls).toEqual([1, 0]);
  expect(route(next)).toMatchObject({ mode: "resident-seed", reason: "idle" });
});

test("scheduled idle expiry closes immediately and its restart reason is consumed exactly once", async () => {
  const timer = fakeIdleScheduler();
  const run = fakeSdk(async function* (_options, control) {
    if (control.query === 1 && control.turn === 1) {
      yield { type: "result", subtype: "error_during_execution", errors: ["unrelated turn failure"],
        user_message_uuid: control.message.uuid };
    } else yield* reply(control.message);
  });
  const { send, residents } = await lane(() => 0, false, timer.schedule);
  const first = await send({ messages: [user("first")] });
  timer.fire();
  expect(run.closeCalls).toEqual([1]);
  expect(residents.size).toBe(0);
  const next = await send({ messages: [user("first"), first, user("next")] });
  expect(next.stopReason).toBe("stop");
  expect(route(next)).toMatchObject({ mode: "resident-seed", reason: "idle" });
  expect(run.closeCalls).toEqual([1, 0]);
  const failed = await send({ messages: [user("first"), first, user("next"), next, user("fail")] });
  expect(failed.stopReason).toBe("error");
  expect(run.closeCalls).toEqual([1, 1]);
  const retry = await send({ messages: [user("first"), first, user("next"), next, user("retry")] });
  expect(route(retry)).toMatchObject({ mode: "resident-seed", reason: "cold" });
});

for (const kind of ["off-switch", "summary"] as const) {
  test(`${kind} one-shot between idle expiry and reseed does not consume the idle reason`, async () => {
    const timer = fakeIdleScheduler();
    const run = fakeSdk(async function* (_options, control) { yield* reply(control.message); });
    const { send } = await lane(() => 0, false, timer.schedule);
    const first = await send({ messages: [user("first")] });
    timer.fire();
    const oneShot = await send({ messages: [user("one-shot")] },
      kind === "summary" ? { toolChoice: "none" } : {},
      kind === "off-switch" ? { env: { CORRAL_CLAUDE_RESIDENT: "0" } } : {});
    expect(route(oneShot)).toMatchObject({ mode: "one-shot" });
    const next = await send({ messages: [user("first"), first, user("next")] });
    expect(route(next)).toMatchObject({ mode: "resident-seed", reason: "idle" });
    expect(run.closeCalls).toEqual([1, 1, 0]);
  });
}

for (const close of ["session", "all"] as const) {
  test(`${close} close invalidates Claude acquisition while SDK setup is pending`, async () => {
    const setup = await lane();
    const ready = Promise.withResolvers<void>();
    const run = fakeSdk(async function* (_options, control) { yield* reply(control.message); }, ready.promise);
    const pending = setup.send({ messages: [user("old model")] });
    if (close === "session") setup.residents.closeSession("resident");
    else setup.residents.closeAll();
    ready.resolve();
    const result = await pending;
    expect(result.stopReason).toBe("error");
    expect(result.errorMessage).toContain("claude_resident_stale");
    expect(setup.residents.size).toBe(0);
    expect(run.options).toHaveLength(0);
    expect(run.closes).toBe(run.options.length);
  });
}

test("idle restart records drop the oldest after the registry-sized bound", async () => {
  const timer = fakeIdleScheduler();
  fakeSdk(async function* (_options, control) { yield* reply(control.message); });
  const { send } = await lane(() => 0, false, timer.schedule);
  for (let index = 0; index <= RESIDENT_MAX_ENTRIES; index++) {
    await send({ messages: [user("first")] }, { sessionId: `expired-${index}` });
    timer.fire();
  }
  const oldest = await send({ messages: [user("next")] }, { sessionId: "expired-0" });
  const retained = await send({ messages: [user("next")] }, { sessionId: "expired-1" });
  expect(route(oldest)).toMatchObject({ mode: "resident-seed", reason: "cold" });
  expect(route(retained)).toMatchObject({ mode: "resident-seed", reason: "idle" });
});

test("a stray prior-turn result fails the next call instead of becoming its reply", async () => {
  const run = fakeSdk(async function* (_options, control) {
    yield* reply(control.message);
    if (control.query === 0 && control.turn === 0) {
      yield { type: "result", subtype: "success", result: "stray reply", user_message_uuid: control.message.uuid };
    }
  });
  const { send, residents } = await lane();
  const first = await send({ messages: [user("first")] });
  const next = await send({ messages: [user("first"), first, user("next")] });
  expect(next.stopReason).toBe("error");
  expect(next.errorMessage).toContain("claude_resident_result_mismatch");
  expect(next.content).toEqual([]);
  expect(residents.size).toBe(0);
  expect(run.closeCalls).toEqual([1]);
  const retry = await send({ messages: [user("first"), first, user("retry")] });
  expect(route(retry)).toMatchObject({ mode: "resident-seed", reason: "cold" });
});

for (const id of ["wrong", undefined]) {
  test(`a ${id === undefined ? "missing" : "mismatched"} result id fails closed`, async () => {
    const run = fakeSdk(async function* () {
      yield { type: "result", subtype: "success", result: "not ours", user_message_uuid: id };
    });
    const { send, residents } = await lane();
    const result = await send({ messages: [user("hello")] });
    expect(result.stopReason).toBe("error");
    expect(result.errorMessage).toContain("claude_resident_result_mismatch");
    expect(result.content).toEqual([]);
    expect(residents.size).toBe(0);
    expect(run.closeCalls).toEqual([1]);
  });
}

test("waiting tool sessions are pinned; a full registry sends the newcomer one-shot capacity", async () => {
  const run = fakeSdk(async function* (_options, control) { yield* toolReply(control.message); });
  const { send, residents } = await lane();
  const first = await send({ messages: [user("waiting")], tools: managedTools }, { sessionId: "pinned" });
  for (let index = 1; index < RESIDENT_MAX_ENTRIES; index++) {
    await send({ messages: [user("waiting")], tools: managedTools }, { sessionId: `other-${index}` });
  }
  const overflow = await send({ messages: [user("overflow")], tools: managedTools }, { sessionId: "newcomer" });
  expect(residents.size).toBe(RESIDENT_MAX_ENTRIES);
  expect(route(overflow)).toMatchObject({ mode: "one-shot", reason: "capacity" });
  expect(run.closeCalls).toEqual([0, 0, 0, 0, 1]);
  const result = { role: "toolResult" as const, toolCallId: "call-bash", toolName: "bash", content: [], isError: false, timestamp: 1 };
  const continued = await send({ messages: [user("waiting"), first, result], tools: managedTools }, { sessionId: "pinned" });
  expect(route(continued)).toMatchObject({ mode: "resident-delta", deltaMessages: 1 });
  expect(run.options).toHaveLength(RESIDENT_MAX_ENTRIES + 1);
});

test("a busy one-shot reply makes the next resident call reseed instead of sending a delta", async () => {
  const active = Promise.withResolvers<void>();
  const release = Promise.withResolvers<void>();
  const run = fakeSdk(async function* (_options, control) {
    if (control.query === 0) { active.resolve(); await release.promise; }
    yield* reply(control.message);
  });
  const { send, residents } = await lane();
  const pending = send({ messages: [user("first")] });
  const overlapping = send({ messages: [user("concurrent")] });
  await active.promise;
  const second = await overlapping;
  expect(route(second)).toMatchObject({ mode: "one-shot", reason: "busy" });
  expect(run.closeCalls).toEqual([0, 1]);
  release.resolve();
  const first = await pending;
  expect(first.stopReason).toBe("stop");
  expect(residents.size).toBe(1);
  expect(run.closeCalls).toEqual([0, 1]);
  const next = await send({ messages: [user("first"), first, user("concurrent"), second, user("next")] });
  expect(next.stopReason).toBe("stop");
  expect(route(next)).toMatchObject({ mode: "resident-seed", reason: "diverged" });
  expect(run.closeCalls).toEqual([1, 1, 0]);
});

test("the oldest unpinned idle session is evicted to admit a seed", async () => {
  let now = 0;
  const run = fakeSdk(async function* (_options, control) { yield* reply(control.message); });
  const { send } = await lane(() => now);
  for (let index = 0; index <= RESIDENT_MAX_ENTRIES; index++) {
    const result = await send({ messages: [user("hello")] }, { sessionId: `s-${index}` });
    expect(route(result)).toMatchObject({ mode: "resident-seed", reason: "cold" });
    now++;
  }
  expect(run.closeCalls).toEqual([1, 0, 0, 0, 0]);
});

test("a process-wide quota notice before a delta frame has one-shot failure parity", async () => {
  const rate = { status: "rejected", rateLimitType: "five_hour", resetsAt: 2_000_000_000 };
  const run = fakeSdk(async function* (_options, control) {
    if (control.query === 0 && control.turn === 0) {
      yield* reply(control.message);
      // This is read between the seed's result and the next turn's first frame.
      yield { type: "rate_limit_event", rate_limit_info: rate };
      return;
    }
    if (control.query > 0) yield { type: "rate_limit_event", rate_limit_info: rate };
    yield { type: "assistant", error: "rate_limit", message: { stop_reason: "end_turn", content: [{ type: "text", text: "quota" }] } };
    yield { type: "result", subtype: "error_during_execution", api_error_status: 429,
      user_message_uuid: control.message.uuid, errors: ["generic error"] };
  });
  const { send } = await lane();
  const first = await send({ messages: [user("first")] });
  const delta = await send({ messages: [user("first"), first, user("second")] });
  const oneShot = await send({ messages: [user("second")] }, {}, { env: { CORRAL_CLAUDE_RESIDENT: "0" } });
  expect(route(delta)).toMatchObject({ mode: "resident-delta" });
  expect(delta.providerFailure).toEqual(oneShot.providerFailure);
  expect(delta.providerFailure).toMatchObject({ category: "subscription_quota", scope: "account", reset_at: new Date(2_000_000_000_000).toISOString() });
  expect(run.closeCalls).toEqual([1, 1]);
});

test("stream usage wins and cumulative model totals never become call usage", async () => {
  fakeSdk(async function* (_options, control) {
    for (const event of reply(control.message)) yield event.type === "result"
      ? { ...event, usage: { input_tokens: 9000, output_tokens: 8000 }, modelUsage: { model: { inputTokens: 10000, outputTokens: 10000 } } }
      : event;
  });
  const { send } = await lane();
  const first = await send({ messages: [user("first")] });
  const second = await send({ messages: [user("first"), first, user("second")] });
  expect(first.usage).toMatchObject({ input: 12, output: 5, totalTokens: 17 });
  expect(second.usage).toMatchObject({ input: 12, output: 5, totalTokens: 17 });
  expect(route(second)).toMatchObject({ usageReported: true, inputTokens: 12 });
});

test("without stream usage the resident result reports only its per-turn usage", async () => {
  fakeSdk(async function* (_options, control) {
    yield { type: "result", subtype: "success", result: "ok", user_message_uuid: control.message.uuid,
      usage: { input_tokens: control.turn + 1, output_tokens: 2 }, modelUsage: { model: { inputTokens: 9000 } } };
  });
  const { send } = await lane();
  const first = await send({ messages: [user("first")] });
  const second = await send({ messages: [user("first"), first, user("second")] });
  expect(second.usage).toMatchObject({ input: 2, output: 2, totalTokens: 4 });
});

for (const off of ["0", "off", "false", "no"]) {
  test(`resident switch ${off} never calls the registry and replays each query`, async () => {
    const run = fakeSdk(async function* (_options, control) { yield* reply(control.message); });
    const { send, residents } = await lane();
    residents.plan = () => { throw new Error("off switch touched the registry"); };
    const config = { env: { CORRAL_CLAUDE_RESIDENT: off } };
    const first = await send({ messages: [user("first")] }, {}, config);
    const second = await send({ messages: [user("first"), first, user("next")] }, {}, config);
    expect(first.stopReason).toBe("stop");
    expect(second.stopReason).toBe("stop");
    expect(route(second)).toMatchObject({ mode: "one-shot" });
    expect(run.options).toHaveLength(2);
    expect(run.closeCalls).toEqual([1, 1]);
    expect(JSON.stringify(run.prompts[1]?.message.content)).toContain("first");
  });
}

test("summary, untracked, no-cache and registry-less callers remain one-shot", async () => {
  const run = fakeSdk(async function* (_options, control) { yield* reply(control.message); });
  const setup = await lane();
  setup.residents.plan = () => { throw new Error("ineligible call touched the registry"); };
  for (const options of [{ toolChoice: "none" as const }, { sessionId: undefined }, { cacheRetention: "none" as const }]) {
    expect(route(await setup.send({ messages: [user("hello")] }, options))).toMatchObject({ mode: "one-shot" });
  }
  const stream = streamClaudeSdk(setup.config, setup.model, normalizeContext({ messages: [user("hello")] }),
    { apiKey: OAUTH, sessionId: "no-registry" });
  for await (const _event of stream) { /* consume */ }
  expect(route(await stream.result() as AssistantMessage)).toMatchObject({ mode: "one-shot" });
  expect(run.closeCalls).toEqual([1, 1, 1, 1]);
});

async function residentHost() {
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-resident-host-"));
  const identity = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const, task: "T-961", run: "R1", generation: 1 };
  let host: HostSession | undefined;
  const cleanup = async () => {
    try { await host?.close(); }
    finally { assets.cleanup(); rmSync(work, { recursive: true, force: true }); }
  };
  try {
    const modules = await loadModules(assets.dir, "worker");
    const transport = modules.auth.transport?.(authContext());
    if (!transport) throw new Error("fixture auth transport missing");
    const corral = writeFakeCorral(work);
    const client = new CorralClient({ corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
    host = await HostSession.create({ sdk, identity, cwd: work, agentDir: join(work, "agent"), model: "gpt-6.1-sol", modules,
      client, events: new EventLog(1), network: guardedFetch(transport, undefined, "openai-codex"), ui: "plain",
      capabilities: { catalog: 7, digest: "a".repeat(64), preset: "full", enabled: ["subagents"] },
      guidance: { projectText: false, settings: { subagents: { max_parallel: 1, model: "gpt-6.1-sol", max_turns: 1, timeout_secs: 30 } } },
      sessionManager: sdk.SessionManager.create(work, join(work, "sessions")) });
    await host.session.bindExtensions({});
    return { host, work, cleanup };
  } catch (error) {
    await cleanup();
    throw error;
  }
}

test("a committed host model change closes the resident and the next Claude call seeds", async () => {
  const { host, cleanup } = await residentHost();
  try {
    const run = fakeSdk(async function* (_options, control) { yield* reply(control.message); });
    const { send } = await lane(undefined, true);
    const first = await send({ messages: [user("first")] }, { sessionId: host.sessionId() });
    expect(residentSessions.size).toBe(1);
    const models = host.runtime.services.modelRuntime;
    const current = host.session.model;
    if (!current) throw new Error("fixture session has no selected model");
    await expect(host.session.setModel({ ...current, provider: "openai" })).rejects.toThrow("model_unavailable");
    expect(run.closeCalls).toEqual([0]);
    const replacement = (await models.getAvailable()).find(model => model.provider === current.provider && model.id !== current.id);
    if (!replacement) throw new Error("fixture catalog has no alternative model");
    await host.session.setModel(replacement);
    expect(run.closeCalls).toEqual([1]);
    expect(residentSessions.size).toBe(0);
    const next = await send({ messages: [user("first"), first, user("back to Claude")] }, { sessionId: host.sessionId() });
    expect(next.stopReason).toBe("stop");
    expect(route(next)).toMatchObject({ mode: "resident-seed", reason: "cold" });
    expect(run.closeCalls).toEqual([1, 0]);
  } finally {
    await cleanup();
  }
});

test("real host shutdown and child-session end close entries without the idle timer", async () => {
  const { host, work, cleanup } = await residentHost();
  try {
    const run = fakeSdk(async function* (_options, control) { yield* reply(control.message); });
    const { send } = await lane(undefined, true);
    await send({ messages: [user("parent")] }, { sessionId: host.sessionId() });
    await send({ messages: [user("child")] }, { sessionId: "resident-child" });
    fixtureTransport().script([{ text: "child done" }]);
    await host["auxiliary"]!["options"].execute({ request: "resident-child", sessionId: "resident-child", kind: "explore",
      provider: "openai-codex", model: "gpt-6.1-sol", prompt: "Reply briefly", sessionFile: join(work, "child.jsonl"), maxTurns: 1 }, new AbortController().signal);
    expect(run.closeCalls).toEqual([0, 1]);
    await host.session.reload();
    expect(run.closeCalls).toEqual([1, 1]);
    expect(residentSessions.size).toBe(0);
    await send({ messages: [user("parent again")] }, { sessionId: host.sessionId() });
    await host.close();
    expect(run.closeCalls).toEqual([1, 1, 1]);
  } finally {
    await cleanup();
  }
});

for (const cause of ["finish", "model-change"] as const) {
  test(`fleet child ${cause} closes the Claude resident using its SDK session id`, async () => {
    const run = fakeSdk(async function* (_options, control) { yield* reply(control.message); });
    const setup = await lane(undefined, true);
    const started = Promise.withResolvers<void>();
    const finished = Promise.withResolvers<void>();
    const messages: AssistantMessage[] = [];
    let disposed = false;
    const handlers = new Map<string, (event: unknown, context: sdk.ExtensionContext) => void | Promise<void>>();
    const session = {
      sessionId: `fleet-sdk-${cause}`, model: setup.model, thinkingLevel: "off", messages, isStreaming: false,
      subscribe() { return () => {}; },
      async bindExtensions() {},
      async prompt() {
        messages.push(await setup.send({ messages: [user("fleet work")] }, { sessionId: this.sessionId }));
        started.resolve();
        await finished.promise;
      },
      async abort() { finished.resolve(); },
      async setModel(model: typeof setup.model) {
        const previousModel = this.model;
        this.model = model;
        await handlers.get("model_select")?.({ type: "model_select", model, previousModel, source: "set" },
          { sessionManager: { getSessionId: () => this.sessionId } } as unknown as sdk.ExtensionContext);
      },
      setThinkingLevel(level: string) { this.thinkingLevel = level; },
      getLastAssistantText() { return "ok"; },
      getSessionStats() { return { tokens: { input: 12, output: 5 }, cost: 0 }; },
      getContextUsage() { return undefined; },
      dispose() { disposed = true; },
    };
    const childSdk = {
      CURRENT_SESSION_VERSION: sdk.CURRENT_SESSION_VERSION,
      SettingsManager: { inMemory: () => ({}) },
      SessionManager: { open: () => ({}) },
      async createAgentSessionServices(options: {
        resourceLoaderOptions: { extensionFactories: sdk.ExtensionFactory[] };
      }) {
        const api = { registerTool() {}, on(name: string, handler: (event: unknown, context: sdk.ExtensionContext) => void | Promise<void>) {
          handlers.set(name, handler);
        } } as unknown as sdk.ExtensionAPI;
        for (const extension of options.resourceLoaderOptions.extensionFactories) await extension(api);
        return {};
      },
      async createAgentSessionFromServices() { return { session }; },
    } as unknown as typeof sdk;
    const root = setup.config.cwd;
    const context: BuildContext = { dir: root, corral: "unused",
      settings: { maxParallel: 1, exitPolicy: "none", profiles: [] },
      session: { version: 1, id: "B-resident", name: "test", created: "", updated: "",
        repoRoot: root, worktree: root, branch: "build/test", base: null, here: true } };
    const runner = createSdkRunner({ sdk: childSdk,
      models: { getModel: (_provider: string, id: string) => ({ ...setup.model, id }) } as unknown as sdk.ModelRuntime,
      context, agentDir: root, localTools: [] });
    const handle = runner.run({ id: "fleet-report-id", title: "Child", brief: "Work", owns: [] },
      { profile: "fixture", provider: setup.model.provider, model: setup.model.id },
      { onEvent() {}, onToolStart() {}, onTurn() {}, async ask() { return "answer"; }, openingPrompt: () => "", roster: async () => "",
        async message() { throw new Error("Unused fixture messaging"); }, reviewVerdict() {} }, new AbortController().signal);
    try {
      await started.promise;
      expect(residentSessions.size).toBe(1);
      expect(run.closeCalls).toEqual([0]);
      if (cause === "model-change") {
        await handle.setModel(setup.model.provider, "new-model");
        expect(session.model.id).toBe("new-model");
        expect(run.closeCalls).toEqual([1]);
        expect(residentSessions.size).toBe(0);
      }
      finished.resolve();
      await handle.done;
      expect(disposed).toBe(true);
      expect(run.closeCalls).toEqual([1]);
      expect(residentSessions.size).toBe(0);
    } finally {
      finished.resolve();
      await handle.done;
    }
  });
}
