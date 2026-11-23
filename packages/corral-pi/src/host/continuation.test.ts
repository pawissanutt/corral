import { expect, test } from "bun:test";
import { createAssistantMessageEventStream, normalizeContext, type Api, type AssistantMessage,
  type AssistantMessageEvent, type AssistantMessageEventStream, type Model, type SimpleStreamOptions } from "@earendil-works/pi-ai";
import type { StreamFn } from "@earendil-works/pi-agent-core";
import { withSubscriptionFallback, type FallbackSelector } from "./continuation.ts";
import type { ProviderFailure } from "./provider-failure.ts";

const price = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 };
const context = normalizeContext({ messages: [{ role: "user", content: "prompt", timestamp: 0 }] });
const codex = model("openai-codex", "primary", "openai-codex-responses");
const claude = model("anthropic", "secondary", "anthropic-messages");
const metered = model("openai", "final", "openai-responses");
const missing = { provider: "anthropic-api", model: "missing" } as const;

function model(provider: FallbackSelector["provider"], id: string, api: Api) {
  return { provider, id, api, name: id, baseUrl: "https://unused.invalid", input: ["text"],
    reasoning: true, cost: price, contextWindow: 1000, maxTokens: 100 } satisfies Model<Api>;
}

function selector(model: { provider: FallbackSelector["provider"]; id: string }): FallbackSelector {
  return { provider: model.provider, model: model.id };
}

function message(model: Model<Api>, stopReason: AssistantMessage["stopReason"] = "stop"): AssistantMessage {
  return { role: "assistant", provider: model.provider, api: model.api, model: model.id, content: [],
    stopReason, timestamp: 0, usage: { ...price, totalTokens: 0, cost: { ...price, total: 0 } } };
}

function quota(model: Model<Api>): Extract<AssistantMessageEvent, { type: "error" }> {
  const failure: ProviderFailure = { version: 1, scope: "account", category: "subscription_quota",
    connection: model.provider === "anthropic" ? "anthropic-subscription" : "openai-subscription" };
  const error = { ...message(model, "error"), errorMessage: "subscription quota refused", providerFailure: failure };
  return { type: "error", reason: "error", error };
}

function stub(...events: AssistantMessageEvent[]): AssistantMessageEventStream {
  const source = createAssistantMessageEventStream();
  for (const event of events) source.push(event);
  source.end();
  return source;
}

async function eventsOf(source: AssistantMessageEventStream): Promise<AssistantMessageEvent[]> {
  const events: AssistantMessageEvent[] = [];
  for await (const event of source) events.push(event);
  return events;
}

test("ordered preflight and a second quota switch the same request without forwarding either start", async () => {
  const signal = new AbortController().signal;
  const onPayload = () => undefined;
  const options: SimpleStreamOptions = { signal, sessionId: "session", apiKey: "old-credential", onPayload,
    env: { OLD_PROVIDER_ENV: "1" }, headers: { Authorization: "old-authorization" }, serviceTier: "priority",
    fetch: async () => { throw new Error("old provider fetch must not be reused"); }, reasoning: "high" };
  const final = message(metered);
  const finalStart = { type: "start", partial: message(metered) } as const;
  const done = { type: "done", reason: "stop", message: final } as const;
  const steps: string[] = [];
  const stream = withSubscriptionFallback((model, sameContext, sameOptions) => {
    expect(model).toBe(codex);
    expect(sameContext).toBe(context);
    expect(sameOptions).toBe(options);
    return stub({ type: "start", partial: message(codex) }, quota(codex));
  }, [missing, selector(claude), missing, selector(metered)], async (candidate, sameSignal) => {
    expect(sameSignal).toBe(signal);
    steps.push(`select:${candidate.model}`);
    if (candidate === missing) return undefined;
    const selected = candidate.provider === "anthropic" ? claude : metered;
    const reasoning = selected === claude ? "low" : undefined;
    const stream: StreamFn = (actual, sameContext, redispatched) => {
      steps.push(`request:${actual.id}`);
      expect(actual).toBe(selected);
      expect(sameContext).toBe(context);
      expect(redispatched?.signal).toBe(signal);
      expect(redispatched?.sessionId).toBe(options.sessionId);
      expect(redispatched?.onPayload).toBe(onPayload);
      expect(redispatched?.apiKey).toBeUndefined();
      expect(redispatched?.headers).toBeUndefined();
      expect(redispatched?.env).toBeUndefined();
      expect(redispatched?.fetch).toBeUndefined();
      expect(redispatched?.serviceTier).toBeUndefined();
      expect(redispatched?.reasoning).toBe(reasoning);
      return selected === claude ? stub({ type: "start", partial: message(claude) }, quota(claude)) : stub(finalStart, done);
    };
    return { model: selected, stream, reasoning };
  }, candidate => steps.push(`notice:${candidate.model}`));

  const response = await stream(codex, context, options);
  expect(await eventsOf(response)).toEqual([finalStart, done]);
  expect(await response.result()).toBe(final);
  expect(steps).toEqual(["select:missing", "select:secondary", "notice:secondary", "request:secondary",
    "select:final", "notice:final", "request:final"]);
});

test("a restored fallback resumes after itself and retains its error if the rest are unusable", async () => {
  const start = { type: "start", partial: message(claude) } as const;
  const failure = quota(claude);
  const tried: FallbackSelector[] = [];
  const notices: FallbackSelector[] = [];
  const stream = withSubscriptionFallback(() => stub(start, failure), [selector(codex), selector(claude), missing],
    candidate => { tried.push(candidate); return undefined; }, candidate => notices.push(candidate));

  const response = await stream(claude, context);
  expect(await eventsOf(response)).toEqual([start, failure]);
  expect(await response.result()).toBe(failure.error);
  expect(tried).toEqual([missing]);
  expect(notices).toEqual([]);
});

test("quota prose alone or actual output must not trigger fallback", async () => {
  const start = { type: "start", partial: message(codex) } as const;
  const partial = message(codex);
  partial.content.push({ type: "text", text: "partial answer" });
  const delta = { type: "text_delta", contentIndex: 0, delta: "partial answer", partial } as const;
  const terminalContent = quota(codex);
  terminalContent.error.content.push(...partial.content);
  const tried: FallbackSelector[] = [];
  for (const events of [
    [start, { type: "error", reason: "error", error: { ...message(codex, "error"), errorMessage: "usage limit reached" } }],
    [start, delta, quota(codex)],
    [start, terminalContent],
  ] satisfies AssistantMessageEvent[][]) {
    const stream = withSubscriptionFallback(() => stub(...events), [selector(metered)],
      candidate => { tried.push(candidate); return undefined; }, () => {});
    expect(await eventsOf(await stream(codex, context))).toEqual(events);
  }
  expect(tried).toEqual([]);
});

test("cancellation before or during preflight never dispatches the next model", async () => {
  const failure = quota(codex);
  for (const alreadyAborted of [true, false]) {
    const controller = new AbortController();
    if (alreadyAborted) controller.abort();
    const tried: FallbackSelector[] = [];
    const notices: FallbackSelector[] = [];
    const stream = withSubscriptionFallback(() => stub(failure), [missing, selector(metered)], async candidate => {
      tried.push(candidate);
      controller.abort();
      controller.signal.throwIfAborted();
      return undefined;
    }, candidate => notices.push(candidate));

    expect(await eventsOf(await stream(codex, context, { signal: controller.signal }))).toEqual([failure]);
    expect(tried).toEqual(alreadyAborted ? [] : [missing]);
    expect(notices).toEqual([]);
  }
});

test("an empty fallback list keeps the original stream function", () => {
  const stream = () => stub(quota(codex));
  expect(withSubscriptionFallback(stream, [], () => undefined, () => {})).toBe(stream);
});

test("dynamic fallback resolver tracks the failing model provider, chains providers, and avoids cycles", async () => {
  const policy: Record<string, readonly FallbackSelector["provider"][]> = {
    "openai-codex": ["anthropic"],
    anthropic: ["openai-codex", "openai"],
  };
  const selectModel = (provider: FallbackSelector["provider"]) =>
    provider === "openai-codex" ? codex : provider === "anthropic" ? claude : metered;
  const steps: string[] = [];
  const stream = withSubscriptionFallback(
    () => stub({ type: "start", partial: message(codex) }, quota(codex)),
    current => (policy[current.provider] ?? []).map(provider => ({ provider, model: selectModel(provider).id })),
    candidate => {
      steps.push(`select:${candidate.provider}`);
      const selected = selectModel(candidate.provider);
      const next: StreamFn = actual => actual.provider === "openai"
        ? stub({ type: "start", partial: message(metered) }, { type: "done", reason: "stop", message: message(metered) })
        : stub({ type: "start", partial: message(actual) }, quota(actual));
      return { model: selected, stream: next };
    },
    (candidate, previous) => steps.push(`notice:${previous.provider}->${candidate.provider}`),
  );

  const response = await stream(codex, context);
  const events = await eventsOf(response);
  expect(events.at(-1)).toMatchObject({ type: "done", reason: "stop" });
  expect(steps).toEqual([
    "select:anthropic",
    "notice:openai-codex->anthropic",
    "select:openai",
    "notice:anthropic->openai",
  ]);
});
