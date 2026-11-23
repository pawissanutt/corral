import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import type { AuthModule } from "../auth/types.ts";
import { EventLog } from "../bridge/events.ts";
import { overrideSdk } from "../claude/sdk.ts";
import { createSdkRunner, type ChildHooks } from "../extensions/fleet/child.ts";
import { CorralClient } from "../tools/corral.ts";
import fixtureAuth from "../../test/fixtures/auth/index.ts";
import { fakeCodexTransport, requestBody } from "../../test/fixtures/auth/fake-transport.ts";
import { writeFakeEngine } from "../../test/support/fake-engine.ts";
import { buildFixtureInstall } from "../../test/support/fixture-install.ts";
import { parseRunFlags } from "./config.ts";
import { restorableFallbacks, type FallbackSettings } from "./fallback-settings.ts";
import { restoreModelChoice } from "./lifecycle.ts";
import { loadModules } from "./modules.ts";
import { guardedFetch } from "./restrict.ts";
import { HostSession } from "./session.ts";
import type { BuildProfile } from "../build/types.ts";

function fixture(piToml?: string, options?: { readonly anthropic?: boolean }) {
  const dir = mkdtempSync(join(tmpdir(), "corral-model-fallback-"));
  if (piToml !== undefined) {
    mkdirSync(join(dir, ".corral-next"), { recursive: true });
    writeFileSync(join(dir, ".corral-next", "pi.toml"), piToml);
  }
  const assets = buildFixtureInstall({ parent: dir });
  const engine = writeFakeEngine(dir, {});
  const api = fakeCodexTransport(), subscription = fakeCodexTransport();
  const apiBodies: Record<string, unknown>[] = [];
  let host: HostSession | undefined;
  const key = "sk-SYNTHETIC-fixture-fallback";
  const includeAnthropic = options?.anthropic === true;
  const auth: AuthModule = {
    provider: "openai-codex",
    providers: includeAnthropic ? ["openai-codex", "openai", "anthropic", "anthropic-api"] : ["openai-codex", "openai"],
    credentials(context) {
      if (context.provider === "openai" || (includeAnthropic && context.provider === "anthropic-api")) {
        const providerId = context.provider;
        return { read: async () => ({ type: "api_key", key }),
          list: async () => [{ type: "api_key", providerId }],
          modify: async () => { throw new Error("read-only"); }, delete: async () => undefined };
      }
      if (includeAnthropic && context.provider === "anthropic") {
        return {
          read: async () => ({ type: "oauth", access: "sk-ant-oat-SYNTHETIC", refresh: "r", expires: Date.now() + 3_600_000 }),
          list: async () => [{ providerId: "anthropic", type: "oauth" }],
          modify: async () => { throw new Error("read-only"); }, delete: async () => undefined,
        };
      }
      return fixtureAuth.credentials(context);
    },
    transport(context) {
      if (context.provider === "openai" || (includeAnthropic && context.provider === "anthropic-api")) {
        return (input, init) => {
          const headers = new Headers(init?.headers);
          if (context.provider === "openai") expect(headers.get("authorization")).toBe(`Bearer ${key}`);
          else expect(headers.get("x-api-key")).toBe(key);
          apiBodies.push(requestBody(init) as Record<string, unknown>);
          return api.send(input, init);
        };
      }
      return subscription.send;
    },
  };
  const network = guardedFetch(subscription.send, undefined, "openai-codex");
  const start = async (
    command: "run" | "chat",
    flags: string[],
    extra?: { readonly capabilities?: boolean; readonly childProfiles?: readonly BuildProfile[]; readonly resume?: string },
  ) => {
    const config = parseRunFlags(command, ["--dir", dir, "--corral", engine.path, ...flags], assets.dir,
      { CORRAL_NEXT_ROOT: dir, CORRAL_TASK: "T-1", CORRAL_RUN: "R1" }, dir);
    await host?.close();
    const sessionManager = extra?.resume ? sdk.SessionManager.open(extra.resume, config.sessionDir, dir)
      : sdk.SessionManager.create(dir, config.sessionDir);
    const launch = { provider: config.provider, model: config.model, effort: config.effort };
    const resumed = extra?.resume && restoreModelChoice(sessionManager, false,
      restorableFallbacks(launch, config.fallback, config.fallbackPolicy, config.noFallback, sessionManager.getBranch()));
    const events = new EventLog();
    host = await HostSession.create({ sdk, identity: config.identity, cwd: dir, ...(resumed || launch),
      ...(config.identity.role === "worker" ? { workerLaunch: launch } : {}),
      fallback: config.fallback, noFallback: config.noFallback, fallbackPolicy: config.fallbackPolicy,
      ...(extra?.capabilities ? { capabilities: config.capabilities, guidance: config.guidance, runtimeDir: config.runtimeDir } : {}),
      ...(extra?.childProfiles ? { childProfiles: extra.childProfiles } : {}),
      agentDir: join(dir, "agent"), ui: "plain", events, network,
      modules: { ...await loadModules(assets.dir, config.identity.role), auth }, client: new CorralClient(config),
      sessionManager });
    await host.startLocal();
    return { host, config, events };
  };
  return { dir, engine, api, subscription, apiBodies, start,
    async clean() {
      overrideSdk(undefined);
      await host?.close();
      assets.cleanup();
      rmSync(dir, { recursive: true, force: true });
    } };
}

const quota = { error: { status: 429, body: { error: { type: "usage_limit_reached" } } } };

test("worker quota skips unusable entries, selects the next model and answers within the same turn", async () => {
  const f = fixture();
  try {
    const { host, events } = await f.start("run", ["--fallback", "anthropic-api/claude-opus-5-5",
      "--fallback", "openai/not-a-model", "--fallback", "openai/gpt-5.4"]);
    const session = host.session, transcript = session.sessionFile!;
    await expect(host.selectRemoteModel("openai/gpt-5.4")).rejects.toThrow("model_unavailable");
    f.subscription.script([quota]);
    f.api.script([{ text: "Metered reply" }, { text: "Still on fallback" }]);
    await session.prompt("Continue after quota");
    expect(session.getLastAssistantText()).toBe("Metered reply");
    expect(session.sessionFile).toBe(transcript);
    expect(f.api.prompts).toEqual(["Continue after quota"]);
    expect(host.state().model).toBe("openai/gpt-5.4");
    const observed = events.since(0, 65536).events;
    expect(observed.filter(event => event.kind === "turn_started")).toHaveLength(1);
    expect(observed.filter(event => event.kind === "model_fallback")).toHaveLength(1);
    expect(session.sessionManager.getBranch().filter(entry => entry.type === "custom_message" && entry.customType === "corral_model_fallback")).toHaveLength(1);
    const paid = { provider: "openai", model: "gpt-5.4" } as const;
    expect(restoreModelChoice(sdk.SessionManager.open(transcript), false, [paid])).toEqual(paid);
    await session.prompt("Next turn");
    expect(session.getLastAssistantText()).toBe("Still on fallback");
    expect(f.subscription.requests).toBe(1);
    await host.drained();
    expect(f.engine.calls().filter(argv => argv.at(-1) === "api")).toHaveLength(2);
  } finally { await f.clean(); }
});

test("quota without fallback surfaces even when a metered key is available", async () => {
  const f = fixture();
  try {
    const { host, events } = await f.start("chat", []);
    expect(await host.session.modelRuntime.checkAuth("openai")).toBeDefined();
    f.subscription.script([quota]);
    await host.session.prompt("Unused API key");
    expect(f.api.requests).toBe(0);
    expect(host.state().model).toBe("openai-codex/gpt-6.1-sol");
    expect(events.since(0, 65536).events.some(event => event.kind === "model_fallback")).toBe(false);
    expect(host.state().last_error).toContain("subscription_quota");
  } finally { await f.clean(); }
});

test("workspace [fallback.providers] follows the active subscription provider after a model switch", async () => {
  const f = fixture('[fallback.providers]\nopenai-codex = ["openai"]\nanthropic = ["anthropic-api"]\n', { anthropic: true });
  try {
    overrideSdk({
      query: () => Object.assign((async function* () {
        yield { type: "rate_limit_event", rate_limit_info: { status: "rejected", rateLimitType: "five_hour", resetsAt: 1_800_000_600 } };
        yield { type: "assistant", error: "rate_limit", message: { content: [{ type: "text", text: "quota" }], stop_reason: null } };
      })(), { interrupt: async () => undefined, close: () => undefined }) as never,
      createSdkMcpServer: options => ({ type: "sdk", name: options.name, instance: options }) as never,
    });
    const { host } = await f.start("chat", []);
    f.subscription.script([quota]);
    f.api.script([{ text: "OpenAI API fallback" }, { text: "Anthropic API fallback" }]);

    await host.session.prompt("First quota");
    expect(host.session.getLastAssistantText()).toBe("OpenAI API fallback");
    expect(host.state().model).toBe("openai/gpt-6.1-sol");

    await host.selectRemoteModel("anthropic/claude-opus-5-5");
    await host.session.prompt("Second quota after model switch");
    expect(host.session.getLastAssistantText()).toBe("Anthropic API fallback");
    expect(host.state().model).toBe("anthropic-api/claude-opus-5-5");
  } finally { await f.clean(); }
});

test("--no-fallback disables workspace [fallback.providers] even when API credentials exist", async () => {
  const f = fixture('[fallback.providers]\nopenai-codex = ["openai"]\n');
  try {
    const { host, events } = await f.start("chat", ["--no-fallback"]);
    expect(await host.session.modelRuntime.checkAuth("openai")).toBeDefined();
    f.subscription.script([quota]);
    await host.session.prompt("Opted out");
    expect(f.api.requests).toBe(0);
    expect(host.state().model).toBe("openai-codex/gpt-6.1-sol");
    expect(events.since(0, 65536).events.some(event => event.kind === "model_fallback")).toBe(false);
    expect(host.state().last_error).toContain("subscription_quota");
  } finally { await f.clean(); }
});

test("fleet child fallback switches when children = true without forwarding priority serviceTier, and stays off when children = false", async () => {
  const f = fixture('[fallback]\nchildren = true\n[fallback.providers]\nopenai-codex = ["openai"]\n');
  try {
    const { host } = await f.start("chat", []);
    const makeHooks = (name: string, fallbacks: [string, string][]): ChildHooks => ({
      execution: { sessionFile: join(f.dir, `${name}.jsonl`), sessionId: `child-${name}`, maxTurns: 4, serviceTier: "priority" },
      onEvent() {}, onToolStart() {}, onTurn() {},
      async ask() { return ""; }, openingPrompt() { return ""; }, reviewVerdict() {},
      async message() { throw new Error("unused"); }, roster() { return ""; },
      onFallback(from, to) { fallbacks.push([from, to]); },
    });

    const enabledFallbacks: [string, string][] = [];
    const runnerOn = createSdkRunner({
      sdk, models: host.session.modelRuntime,
      context: { dir: f.dir, session: { worktree: f.dir, name: "fixture" }, settings: { maxParallel: 1, profiles: [] } },
      agentDir: join(f.dir, "agent"), localTools: [], bound: true,
      fallbackPolicy: { children: true, providers: { "openai-codex": ["openai"] } } satisfies FallbackSettings,
    });
    f.subscription.script([quota]);
    f.api.script([{ text: "Child API reply" }]);
    const ok = await runnerOn.run(
      { id: "c1", title: "Child 1", brief: "Do research", owns: [], kind: "explore" },
      { profile: "explore", provider: "openai-codex", model: "gpt-6.1-sol" },
      makeHooks("c1", enabledFallbacks),
      new AbortController().signal,
    ).done;
    expect(ok).toMatchObject({ report: "Child API reply" });
    expect(ok.failed).toBeUndefined();
    expect(enabledFallbacks).toEqual([["openai-codex/gpt-6.1-sol", "openai/gpt-6.1-sol"]]);
    expect(f.apiBodies).toHaveLength(1);
    expect(f.apiBodies[0]?.service_tier).toBeUndefined();

    const disabledFallbacks: [string, string][] = [];
    const runnerOff = createSdkRunner({
      sdk, models: host.session.modelRuntime,
      context: { dir: f.dir, session: { worktree: f.dir, name: "fixture" }, settings: { maxParallel: 1, profiles: [] } },
      agentDir: join(f.dir, "agent"), localTools: [], bound: true,
      fallbackPolicy: { children: false, providers: { "openai-codex": ["openai"] } } satisfies FallbackSettings,
    });
    f.subscription.script([quota]);
    const stopped = await runnerOff.run(
      { id: "c2", title: "Child 2", brief: "Do research", owns: [], kind: "explore" },
      { profile: "explore", provider: "openai-codex", model: "gpt-6.1-sol" },
      makeHooks("c2", disabledFallbacks),
      new AbortController().signal,
    ).done;
    expect(stopped.failed).toContain("subscription_quota");
    expect(disabledFallbacks).toEqual([]);
    expect(f.api.requests).toBe(1);
  } finally { await f.clean(); }
});

test("worker keeps fallback-only providers out of fleet child launch allowlist and model picker", async () => {
  const f = fixture('[fallback]\nchildren = false\n[fallback.providers]\nopenai-codex = ["openai"]\n');
  try {
    const { host } = await f.start("run", [], { capabilities: true });
    f.subscription.script([
      {
        toolCalls: [{
          callId: "call-bad",
          name: "task",
          arguments: { agents: [{ id: "bad", title: "Bad", brief: "Should fail", owns: ["a.ts"], profile: "openai/gpt-6.2-sol" }] },
        }],
      },
      { text: "Stopped bad child" },
    ]);
    await host.session.prompt("Try launching openai child");
    const badResult = host.session.messages.find(message => message.role === "toolResult");
    expect(JSON.stringify(badResult)).toContain("Child model provider is not approved: openai");
    expect(f.api.requests).toBe(0);

    const childHeld = Promise.withResolvers<void>();
    const releaseChild = Promise.withResolvers<void>();
    f.subscription.script([
      {
        match: "Launch default child",
        toolCalls: [{
          callId: "call-ok",
          name: "task",
          arguments: { agents: [{ id: "ok", title: "OK", brief: "Stay subscription", owns: ["b.ts"] }] },
        }],
      },
      {
        match: "Stay subscription",
        chunks: ["Child ", "done"],
        afterFirstChunk: () => {
          childHeld.resolve();
          return releaseChild.promise;
        },
      },
      { text: "Launched ok child" },
    ]);
    const promptPromise = host.session.prompt("Launch default child and inspect picker");
    await childHeld.promise;
    host.installRemoteParts();
    const childDetail = host.remoteRegistry.get("child")?.({ child: "ok" }) as { models: readonly string[] } | undefined;
    expect(childDetail?.models.some(model => model.startsWith("openai-codex/"))).toBe(true);
    expect(childDetail?.models.some(model => model.startsWith("openai/"))).toBe(false);
    releaseChild.resolve();
    await promptPromise;
    expect(f.api.requests).toBe(0);
  } finally { await f.clean(); }
});

test("worker with configured paid child profile approves that child provider", async () => {
  const f = fixture('[fallback]\nchildren = false\n[fallback.providers]\nopenai-codex = ["openai"]\n');
  try {
    const { host } = await f.start("run", [], {
      capabilities: true,
      childProfiles: [{ name: "metered", model: "openai/gpt-6.2-sol" }],
    });
    f.subscription.script([
      {
        toolCalls: [{
          callId: "call-paid",
          name: "task",
          arguments: { agents: [{ id: "paid", title: "Paid", brief: "Use metered child", owns: ["c.ts"], profile: "metered" }] },
        }],
      },
      {
        toolCalls: [{
          callId: "call-wait",
          name: "agent_wait",
          arguments: { ids: ["paid"] },
        }],
      },
      { text: "Started paid child" },
    ]);
    f.api.script([{ match: "Use metered child", text: "Child done" }]);
    await host.session.prompt("Launch metered child");
    expect(f.api.requests).toBe(1);
  } finally { await f.clean(); }
});

test("worker restarts preserve the parent's fallback and model changes without moving children to the API", async () => {
  const f = fixture('[fallback.providers]\nopenai-codex = ["openai"]\n');
  try {
    const { host, config } = await f.start("run", [], { capabilities: true });
    const transcript = host.session.sessionFile!;
    await host.selectRemoteModel("openai-codex/gpt-6.2-sol");
    f.subscription.script([quota]);
    f.api.script([{ text: "Fallback on 6.2" }]);
    await host.session.prompt("Quota after switch");
    expect(host.session.getLastAssistantText()).toBe("Fallback on 6.2");
    expect(host.state().model).toBe("openai/gpt-6.2-sol");

    const resumed = (await f.start("run", [], { capabilities: true, resume: transcript })).host;
    expect(resumed.state().model).toBe("openai/gpt-6.2-sol");
    f.api.script([
      { toolCalls: [{ callId: "paid", name: "task", arguments: {
        agents: [{ id: "paid", title: "Paid child", brief: "Must not launch", owns: [], profile: "openai/gpt-6.2-sol" }],
      } }] },
      { toolCalls: [{ callId: "start", name: "task", arguments: {
        agents: [{ id: "child", title: "Default child", brief: "Use the original worker profile", owns: [] }],
      } }] },
      { toolCalls: [{ callId: "wait", name: "agent_wait", arguments: { ids: ["child"] } }] },
      { text: "Parent finished" },
    ]);
    f.subscription.script([{ text: "Subscription child finished" }]);
    await resumed.session.prompt("Launch the default child after restart");
    expect(resumed.session.getLastAssistantText()).toBe("Parent finished");
    expect(f.subscription.prompts.at(-1)).toContain("Use the original worker profile");
    const toolResults = resumed.session.messages.filter(message => message.role === "toolResult");
    expect(JSON.stringify(toolResults)).toContain("Child model provider is not approved: openai");
    expect(f.api.requests).toBe(5); // The initial fallback and four parent requests; no paid child request.

    await resumed.selectRemoteModel("openai/gpt-5.4");
    await resumed.session.prompt("Persist the new API model");
    const restarted = (await f.start("run", [], { resume: transcript })).host;
    expect(restarted.state().model).toBe("openai/gpt-5.4");

    const reopened = sdk.SessionManager.open(transcript);
    const launch = { provider: "openai-codex", model: "gpt-6.1-sol" } as const;
    const explicit = [{ provider: "openai", model: "gpt-6.2-sol" }] as const;
    expect(restorableFallbacks(launch, explicit, undefined, false, reopened.getBranch()))
      .toEqual([{ provider: "openai", model: "gpt-5.4" }]);

    const untriggered = [
      { type: "model_change", provider: "openai-codex", modelId: "gpt-6.1-sol" },
      { type: "model_change", provider: "openai", modelId: "gpt-6.2-sol" },
    ];
    expect(restorableFallbacks(launch, undefined, config.fallbackPolicy, false, untriggered)).toEqual([]);
    expect(restorableFallbacks({ provider: "anthropic", model: "claude-opus-5-5" }, undefined, config.fallbackPolicy, false, reopened.getBranch())).toEqual([]);
    expect(restorableFallbacks(launch, undefined, config.fallbackPolicy, true, reopened.getBranch())).toEqual([]);
  } finally { await f.clean(); }
});
