// A disposable SDK host of Pi's own InteractiveMode, not a second conversation.
import { readFileSync, appendFileSync, writeFileSync, renameSync, existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { readSubscription, readOnlyStore, ProbeFailure } from "./credentials.mjs";
import { guardedFetch, cappedPayload } from "./request-guard.mjs";
import { createBridge } from "./native-bridge.mjs";
import { fixtureExtension } from "./native-tools.mjs";
import { nativeScenarios } from "./native-scenarios.mjs";

const fixtureRoot = process.env.CORRAL_TEST_HERDR_ROOT;
if (!fixtureRoot || process.env.HOME !== join(fixtureRoot, "home")) throw new ProbeFailure("private_fixture_required");
const root = fixtureRoot;
const bootstrap = JSON.parse(readFileSync(join(root, "native-bootstrap.json"), "utf8"));
const probeRoot = dirname(fileURLToPath(import.meta.url));
const pkgRoot = join(probeRoot, "output/runtime/node_modules/@earendil-works/pi-coding-agent");
/** @type {typeof import("./output/runtime/node_modules/@earendil-works/pi-coding-agent/dist/index.js")} */
const sdk = await import(pathToFileURL(join(pkgRoot, "dist/index.js")).href);
const { Type } = await import("./output/runtime/node_modules/typebox/build/index.mjs");
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
/** @type {ReturnType<typeof readSubscription> | undefined} */
let selected;

try {
  const pins = JSON.parse(readFileSync(join(probeRoot, "pins.json"), "utf8"));
  const manifest = JSON.parse(readFileSync(join(pkgRoot, "package.json"), "utf8"));
  const lock = JSON.parse(readFileSync(join(probeRoot, "output/runtime/package-lock.json"), "utf8"));
  if (process.version !== pins.node || manifest.version !== pins.version ||
      lock.packages["node_modules/@earendil-works/pi-coding-agent"].integrity !== pins.integrity) {
    throw new ProbeFailure("runtime_pin_mismatch");
  }
  selected = readSubscription(bootstrap.codexPath, "codex", Date.now());
  const credentials = readOnlyStore(selected.credential);
  const models = await sdk.ModelRuntime.create({ credentials, modelsPath: null,
    refreshOnCreate: false, allowModelNetwork: false });
  const provider = models.getProvider("openai-codex");
  const candidateModel = models.getModel("openai-codex", "gpt-6.1-sol");
  if (!provider || !candidateModel || !provider.auth.oauth?.isSubscription) throw new ProbeFailure("subscription_route_unavailable");
  const model = candidateModel;
  const fetch = guardedFetch({ credential: selected.credential, ledger: join(probeRoot, "output/live-budget.json"),
    allowance: 4, receipt });
  let holdNext = false;
  let failNext = false;
  /** @type {(() => void) | undefined} */
  let release;
  /**
   * @param {import("./output/runtime/node_modules/@earendil-works/pi-ai/dist/types.js").StreamOptions | undefined} options
   * @returns {import("./output/runtime/node_modules/@earendil-works/pi-ai/dist/types.js").SimpleStreamOptions}
   */
  function bounded(options) {
    return { ...options, maxTokens: 1024, maxRetries: 0, transport: "sse", fetch, timeoutMs: 30_000,
      /** @param {unknown} payload @param {import("./output/runtime/node_modules/@earendil-works/pi-ai/dist/types.js").Model<string>} current */
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
  /** @type {import("./output/runtime/node_modules/@earendil-works/pi-ai/dist/types.js").StreamFunction<
   * "openai-codex-responses",
   * import("./output/runtime/node_modules/@earendil-works/pi-ai/dist/api/openai-codex-responses.js").OpenAICodexResponsesOptions>} */
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
  /** @type {import("./output/runtime/node_modules/@earendil-works/pi-coding-agent/dist/index.js").CreateAgentSessionRuntimeFactory} */
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
    /** @type {import("./output/runtime/node_modules/@earendil-works/pi-ai/dist/types.js").AssistantMessage} */
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
          contextFiles: resources.getAgentsFiles().agentsFiles.length };
      }
      case "start":
        holdNext = true;
        void session.prompt("Reply only NATIVE_READY. Do not use tools.").catch(() => emit({ event: "prompt_failed" }));
        return { disposition: "submitted_not_completed" };
      case "steer": return { disposition: await session.steer("Reply only STEERED. Do not use tools.") };
      case "follow_up": return { disposition: await session.followUp("Reply only FOLLOWED. Do not use tools.") };
      case "release": if (!release) throw new ProbeFailure("no_held_provider"); release(); return { released: true };
      case "cancel": await session.abort(); return { cancelled: true, streaming: session.isStreaming };
      case "handled": await session.prompt("/probe-handled"); return { disposition: "handled" };
      case "ask":
        void session.prompt("/probe-decision").catch(() => emit({ event: "decision_failed" }));
        return { disposition: "awaiting_fixture_terminal" };
      case "permissions":
      case "mutate": {
        const tool = session.agent.state.tools.find((item) => item.name === "pm_readback");
        if (!tool) throw new ProbeFailure("readback_tool_missing");
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
        failNext = true;
        void session.prompt("Reply only FAILURE_FIXTURE.").catch(() => emit({ event: "prompt_failed" }));
        return { disposition: "submitted_not_completed", injectedFailure: true };
      default: throw new ProbeFailure("command_not_allowed");
    }
  }
  bridge = await createBridge({ root, capability: bootstrap.capability, scope,
    generation: () => state.generation, receive, receipt });
  void terminal.run().catch(() => emit({ event: "terminal_failed" }));
  const result = await nativeScenarios({ bridge, scope, state, bootstrap,
    offline: process.env.PI_PROBE_NATIVE_OFFLINE !== "0" });
  selected.verifyUnchanged();
  if (existsSync(join(root, "pi-agent/auth.json")) ||
      existsSync(join(process.env.PI_CODING_AGENT_DIR ?? "", "auth.json"))) {
    throw new ProbeFailure("unexpected_fixture_credential_file");
  }
  receipt({ type: "credential_boundary", ownerBytesAndModeUnchanged: true, refreshDisabled: true,
    credentialReads: credentials.reads, fixtureAuthFileAbsent: true });
  writeFileSync(join(root, "native-result.tmp"), JSON.stringify(result) + "\n", { mode: 0o600 });
  renameSync(join(root, "native-result.tmp"), join(root, "native-result.json"));
  await bridge.close();
  state.context?.shutdown();
} catch (error) {
  try { selected?.verifyUnchanged(); } catch { receipt({ type: "owner_boundary_changed", unchanged: false }); }
  receipt({ type: "native_probe_stopped", decision: "no-go",
    reason: error instanceof ProbeFailure ? error.code : "native_fixture_error" });
  writeFileSync(join(root, "native-result.tmp"), JSON.stringify({ decision: "no-go", nativeProofComplete: false }) + "\n", { mode: 0o600 });
  renameSync(join(root, "native-result.tmp"), join(root, "native-result.json"));
  await bridge?.close();
  process.exit(2);
}
