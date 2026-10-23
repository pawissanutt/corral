// A disposable Bun host of Pi's own InteractiveMode, not a second conversation.
import { readFileSync, appendFileSync, writeFileSync, renameSync, existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { readSubscription, readOnlyStore, ProbeFailure } from "./credentials.mjs";
import { guardedFetch, cappedPayload, liveAuthorization, liveLedger } from "./request-guard.mjs";
import { fakeCodexTransport } from "./fake-transport.mjs";
import { checkRuntime } from "./runtime.mjs";
import { createBridge } from "./native-bridge.mjs";
import { fixtureExtension } from "./native-tools.mjs";
import { nativeScenarios, classifyFailure, CRITERIA } from "./native-scenarios.mjs";

const fixtureRoot = process.env.CORRAL_TEST_HERDR_ROOT;
if (!fixtureRoot || process.env.HOME !== join(fixtureRoot, "home")) throw new ProbeFailure("private_fixture_required");
const root = fixtureRoot;
const bootstrap = JSON.parse(readFileSync(join(root, "native-bootstrap.json"), "utf8"));
const probeRoot = dirname(fileURLToPath(import.meta.url));
const live = process.env.PI_PROBE_NATIVE_OFFLINE === "0";
// Offline, every model request must go through the injected fake transport.
// Anything that reaches the global fetch instead is counted and refused.
let networkRequests = 0;
if (!live) {
  globalThis.fetch = /** @type {typeof globalThis.fetch} */ (async () => {
    networkRequests++;
    throw new ProbeFailure("network_forbidden_offline");
  });
}
/** @type {import("./native-tools.mjs").FixtureState} */
const state = { generation: 0, sessionId: "", forbiddenExecutions: 0, mutations: 0, approved: false, questionPending: false };
/** @type {Awaited<ReturnType<typeof createBridge>> | undefined} */
let bridge;
/** @type {Array<{stopReason:string, outputTokens:number}>} */
const providerOutputs = [];
/** @type {Set<string>} */
const deliveries = new Set();
/** @param {Record<string, unknown>} value */
function receipt(value) {
  appendFileSync(join(root, "native-events.jsonl"),
    JSON.stringify({ at: new Date().toISOString(), ...value }) + "\n", { mode: 0o600 });
}
/** @param {Record<string, unknown>} value */
function emit(value) { if (bridge) bridge.emit(value); else receipt(value); }
/** @param {Record<string, unknown>} value */
function finish(value) {
  writeFileSync(join(root, "native-result.tmp"), JSON.stringify(value) + "\n", { mode: 0o600 });
  renameSync(join(root, "native-result.tmp"), join(root, "native-result.json"));
}
/** @type {ReturnType<typeof readSubscription> | undefined} */
let selected;
/** @type {ReturnType<typeof checkRuntime> | undefined} */
let runtime;
/** @type {ReturnType<typeof fakeCodexTransport> | undefined} */
let transport;
const mode = live ? "live" : "offline_fake_transport";

try {
  // A live run refuses before it opens any owner credential.
  const authorization = live ? liveAuthorization(process.env.PI_PROBE_LIVE_AUTHORIZATION) : undefined;
  runtime = checkRuntime(probeRoot);
  receipt({ type: "runtime_pins", ...runtime });
  const sdk = await import("@earendil-works/pi-coding-agent");
  const { Type } = await import("typebox");
  selected = readSubscription(bootstrap.codexPath, "codex", Date.now());
  const credentials = readOnlyStore(selected.credential);
  const models = await sdk.ModelRuntime.create({ credentials, modelsPath: null,
    refreshOnCreate: false, allowModelNetwork: false });
  const provider = models.getProvider("openai-codex");
  const candidateModel = models.getModel("openai-codex", "gpt-6.1-sol");
  if (!provider || !candidateModel || !provider.auth.oauth?.isSubscription) {
    throw new ProbeFailure("subscription_route_unavailable", "upstream_incompatibility");
  }
  const model = candidateModel;
  /** @type {typeof globalThis.fetch} */
  let fetch;
  if (authorization) {
    const output = process.env.PI_PROBE_OUTPUT;
    if (!output) throw new ProbeFailure("live_ledger_directory_required");
    fetch = guardedFetch({ credential: selected.credential, ledger: liveLedger(output, authorization),
      authorization, allowance: 4, receipt });
  } else {
    transport = fakeCodexTransport({ receipt, onHold: () => emit({ event: "provider_held" }) });
    fetch = guardedFetch({ credential: selected.credential, ledger: null, allowance: 8, receipt, send: transport.send });
  }
  // Live mode keeps T-136's pre-dispatch hold and injected failure; offline,
  // the fake transport holds an open response stream and answers HTTP 503.
  let holdNext = false;
  let failNext = false;
  /** @type {(() => void) | undefined} */
  let release;
  /**
   * @param {import("@earendil-works/pi-ai").StreamOptions | undefined} options
   * @returns {import("@earendil-works/pi-ai").SimpleStreamOptions}
   */
  function bounded(options) {
    return { ...options, maxTokens: 1024, maxRetries: 0, transport: "sse", fetch, timeoutMs: 30_000,
      /** @param {unknown} payload @param {import("@earendil-works/pi-ai").Model<string>} current */
      async onPayload(payload, current) {
        if (failNext) { failNext = false; throw new ProbeFailure("injected_fixture_provider_failure"); }
        if (holdNext) {
          holdNext = false;
          await new Promise((resolve, reject) => {
            const signal = options?.signal;
            const aborted = () => { release = undefined; reject(new ProbeFailure("fixture_hold_aborted")); };
            release = () => { signal?.removeEventListener("abort", aborted); release = undefined; resolve(undefined); };
            signal?.addEventListener("abort", aborted, { once: true });
            if (signal?.aborted) aborted();
            emit({ event: "provider_held" });
          });
        }
        return cappedPayload(await options?.onPayload?.(payload, current) ?? payload);
      },
    };
  }
  /** @type {import("@earendil-works/pi-ai").StreamFunction<"openai-codex-responses",
   * import("@earendil-works/pi-ai").OpenAICodexResponsesOptions>} */
  const streamCodex = provider.stream.bind(provider);
  models.registerNativeProvider({
    ...provider,
    stream(current, context, options) {
      if (current.api !== "openai-codex-responses") throw new ProbeFailure("provider_api_not_allowed");
      return streamCodex({ ...current, api: "openai-codex-responses" }, context, bounded(options));
    },
    streamSimple: (current, context, options) => provider.streamSimple(current, context, bounded(options)),
  });
  /** @type {(() => void) | undefined} */
  let unsubscribe;
  /** @type {Record<string, string>} */
  const scope = { store: "private-fixture", root, role: "pm", project: "fixture",
    herdr_socket: bootstrap.socket, workspace: bootstrap.workspace, pane: bootstrap.pane, native_session_id: "" };
  /** @type {import("@earendil-works/pi-coding-agent").CreateAgentSessionRuntimeFactory} */
  const factory = async ({ cwd, agentDir, sessionManager, sessionStartEvent }) => {
    unsubscribe?.();
    state.generation++;
    state.sessionId = sessionManager.getSessionId();
    scope.native_session_id = state.sessionId;
    const settingsManager = sdk.SettingsManager.inMemory({
      compaction: { enabled: false }, retry: { enabled: false }, cacheWarming: "off",
      transport: "sse", defaultTools: ["pm_readback"], quietStartup: true,
    });
    const services = await sdk.createAgentSessionServices({ cwd, agentDir, modelRuntime: models, settingsManager,
      resourceLoaderOptions: {
        noExtensions: true, noSkills: true, noPromptTemplates: true, noThemes: true, noContextFiles: true,
        disabledBuiltinExtensions: ["mcp", "codemode", "tool_search"],
        extensionFactories: [fixtureExtension({ state, root, emit, Type })],
      } });
    const result = await sdk.createAgentSessionFromServices({ services, sessionManager, sessionStartEvent,
      model, thinkingLevel: "off", tools: ["pm_readback", "pm_fixture_mutate", "forbidden_deferred"],
      scopedModels: [{ model, thinkingLevel: "off" }] });
    result.session.setActiveToolsByName(["pm_readback"]);
    // Native nested execution requires an assistant parent. This explicit
    // synthetic fixture message is not provider output or live model evidence.
    /** @type {import("@earendil-works/pi-ai").AssistantMessage} */
    const syntheticParent = {
      role: "assistant", content: [{ type: "text", text: "Synthetic fixture setup; not provider output." }],
      api: model.api, provider: model.provider, model: model.id, timestamp: Date.now(), stopReason: "stop",
      usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0,
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
    };
    sessionManager.appendMessage(syntheticParent);
    result.session.agent.state.messages = [...result.session.agent.state.messages, syntheticParent];
    receipt({ type: "synthetic_fixture_parent", providerGenerated: false, generation: state.generation });
    unsubscribe = result.session.subscribe((event) => {
      if (["agent_start", "agent_end", "agent_settled", "queue_update", "ui_prompt_start", "ui_prompt_end"].includes(event.type)) {
        emit({ event: event.type, ...(event.type === "agent_settled" ? { aborted: event.aborted } : {}),
          ...(event.type === "queue_update" ? { steering: event.steering.length, followUp: event.followUp.length } : {}) });
      }
      if (event.type === "message_end" && event.message.role === "assistant") {
        const final = { stopReason: event.message.stopReason, outputTokens: event.message.usage.output };
        providerOutputs.push(final);
        emit({ event: "assistant_final", ...final });
      }
      if (event.type === "message_start" && event.message.role === "user") {
        const content = event.message.content;
        const text = typeof content === "string" ? content :
          content.filter((item) => item.type === "text").map((item) => item.text).join("\n");
        const kind = text.includes("NATIVE_READY") ? "start" : text.includes("STEERED") ? "steer" :
          text.includes("FOLLOWED") ? "follow_up" : undefined;
        if (kind) { deliveries.add(kind); emit({ event: "input_delivered", kind }); }
      }
    });
    return { ...result, services, diagnostics: services.diagnostics };
  };
  const host = await sdk.createAgentSessionRuntime(factory, { cwd: root, agentDir: join(root, "pi-agent"),
    sessionManager: sdk.SessionManager.inMemory(root) });
  const terminal = new sdk.InteractiveMode(host, { verbose: false });
  await terminal.init();
  /** @param {string} kind @param {Record<string, unknown>} data */
  async function receive(kind, data) {
    const session = host.session;
    switch (kind) {
      case "state": {
        const resources = host.services.resourceLoader;
        return { generation: state.generation, sessionId: state.sessionId, mode: state.context?.mode,
          hasUI: state.context?.hasUI, streaming: session.isStreaming, mutations: state.mutations,
          providerOutputs: [...providerOutputs], deliveries: [...deliveries],
          activeTools: session.getActiveToolNames(), forbiddenExecutions: state.forbiddenExecutions,
          skills: resources.getSkills().skills.length, prompts: resources.getPrompts().prompts.length,
          contextFiles: resources.getAgentsFiles().agentsFiles.length,
          transportRequests: transport?.requests, transportAborted: transport?.aborted,
          transportFailures: transport?.failures };
      }
      case "start":
        if (transport) transport.holdNext(); else holdNext = true;
        void session.prompt("Reply only NATIVE_READY. Do not use tools.").catch(() => emit({ event: "prompt_failed" }));
        return { disposition: "submitted_not_completed" };
      case "steer": return { disposition: await session.steer("Reply only STEERED. Do not use tools.") };
      case "follow_up": return { disposition: await session.followUp("Reply only FOLLOWED. Do not use tools.") };
      case "release":
        if (transport) transport.release();
        else if (release) release();
        else throw new ProbeFailure("no_held_provider", "upstream_incompatibility");
        return { released: true };
      case "cancel": await session.abort(); return { cancelled: true, streaming: session.isStreaming };
      case "handled": await session.prompt("/probe-handled"); return { disposition: "handled" };
      case "ask":
        void session.prompt("/probe-decision").catch(() => emit({ event: "decision_failed" }));
        return { disposition: "awaiting_fixture_terminal" };
      case "permissions":
      case "mutate": {
        const tool = session.agent.state.tools.find((item) => item.name === "pm_readback");
        if (!tool) throw new ProbeFailure("readback_tool_missing", "upstream_incompatibility");
        const result = await tool.execute("fixture-" + kind, {
          operation: kind === "permissions" ? "denials" : "mutate",
        }, undefined, undefined);
        return { details: result.details };
      }
      case "reload": await session.prompt("/probe-reload"); return { reloaded: true };
      case "switch_model": await session.setModel(model); return { model: "gpt-6.1-sol" };
      case "replace":
        await host.newSession();
        return { generation: state.generation, sessionId: state.sessionId };
      case "fail":
        if (transport) transport.failNext(); else failNext = true;
        void session.prompt("Reply only FAILURE_FIXTURE.").catch(() => emit({ event: "prompt_failed" }));
        return { disposition: "submitted_not_completed", injectedFailure: true };
      default: throw new ProbeFailure("command_not_allowed");
    }
  }
  bridge = await createBridge({ root, capability: bootstrap.capability, scope,
    generation: () => state.generation, receive, receipt });
  void terminal.run().catch(() => emit({ event: "terminal_failed" }));
  const result = await nativeScenarios({ bridge, scope, state, bootstrap, live });
  selected.verifyUnchanged();
  if (existsSync(join(root, "pi-agent/auth.json")) ||
      existsSync(join(process.env.PI_CODING_AGENT_DIR ?? "", "auth.json"))) {
    throw new ProbeFailure("unexpected_fixture_credential_file");
  }
  receipt({ type: "credential_boundary", credentialSource: live ? "owner_codex_subscription" : "synthetic_fixture",
    ownerBytesAndModeUnchanged: true, refreshDisabled: true, credentialReads: credentials.reads, fixtureAuthFileAbsent: true });
  const proven = result.allPassed && (live || networkRequests === 0);
  // Offline, the refused global fetch is the only path to a provider: the
  // guarded fetch's sole destination is the fake transport.
  finish({ mode, decision: proven ? (live ? "bounded-native-proof" : "GO") : "NO-GO", runtime, ...result,
    providerRequests: live ? undefined : networkRequests,
    transportRequests: transport?.requests, transportAborted: transport?.aborted, transportFailures: transport?.failures });
  await bridge.close();
  state.context?.shutdown();
} catch (error) {
  try { selected?.verifyUnchanged(); } catch { receipt({ type: "owner_boundary_changed", unchanged: false }); }
  const failure = { step: "host_startup", ...classifyFailure(error, !live) };
  receipt({ type: "native_probe_stopped", decision: "no-go", origin: failure.origin, reason: failure.code });
  finish({ mode, decision: "NO-GO", runtime, criteria: Object.fromEntries(CRITERIA.map((name) => [name, "NOT_RUN"])),
    steps: [], allPassed: false,
    harnessPortFailures: failure.origin === "harness_port" ? [failure] : [],
    upstreamIncompatibilities: failure.origin === "upstream_incompatibility" ? [failure] : [],
    providerRequests: live ? undefined : networkRequests });
  await bridge?.close();
  process.exit(2);
}
