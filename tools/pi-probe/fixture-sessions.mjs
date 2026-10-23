// Generates the committed offline Pi session fixtures with the pinned SDK on Bun
// and the fake Codex transport: a session with known synthetic usage (for
// pi-usage) and one whose last turn ends in a corral tool call and an upstream
// edit call without results (for recovery). Run through run-fixtures.sh.
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { readSubscription, readOnlyStore, ProbeFailure } from "./credentials.mjs";
import { guardedFetch, cappedPayload } from "./request-guard.mjs";
import { fakeCodexTransport, writeSyntheticAuth } from "./fake-transport.mjs";
import { checkRuntime } from "./runtime.mjs";

/** @type {import("./fake-transport.mjs").FakeStep[]} */
export const USAGE_SCRIPT = [
  { text: "USAGE_ONE", usage: { input: 1200, output: 40 } },
  { text: "USAGE_TWO", usage: { input: 476, cached: 1024, output: 60, reasoning: 16 } },
  { text: "USAGE_THREE", usage: { input: 260, cached: 512, cacheWrite: 128, output: 25, reasoning: 8 } },
];
export const DANGLING_CALLS = [
  { callId: "call_corral_gate_1", name: "corral_gate", arguments: {}, class: "corral" },
  { callId: "call_edit_1", name: "edit", class: "upstream",
    arguments: { path: "README.md", edits: [{ oldText: "fixture before", newText: "fixture after" }] } },
];

/** Pi's categories for one scripted response, derived from the script alone. @param {import("./fake-transport.mjs").FakeUsage} usage */
export function expectedUsage(usage) {
  const cached = usage.cached ?? 0;
  const cacheWrite = usage.cacheWrite ?? 0;
  return { input: usage.input, output: usage.output, cacheRead: cached, cacheWrite,
    reasoning: usage.reasoning ?? 0, totalTokens: usage.input + cached + cacheWrite + usage.output };
}

/**
 * @param {typeof import("@earendil-works/pi-coding-agent")} sdk
 * @param {{root: string, name: string, transport: ReturnType<typeof fakeCodexTransport>,
 * tools: string[], customTools?: import("@earendil-works/pi-coding-agent").ToolDefinition[],
 * extensionFactories?: import("@earendil-works/pi-coding-agent").ExtensionFactory[]}} options
 */
async function fixtureSession(sdk, options) {
  const base = join(options.root, options.name);
  const cwd = join(base, "worktree");
  mkdirSync(cwd, { recursive: true });
  const authPath = join(base, "synthetic-codex-auth.json");
  writeSyntheticAuth(authPath, Date.now());
  const { credential } = readSubscription(authPath, "codex", Date.now());
  const credentials = readOnlyStore(credential);
  const models = await sdk.ModelRuntime.create({ credentials, modelsPath: null, refreshOnCreate: false, allowModelNetwork: false });
  const provider = models.getProvider("openai-codex");
  const model = models.getModel("openai-codex", "gpt-6.1-sol");
  if (!provider || !model) throw new ProbeFailure("subscription_route_unavailable", "upstream_incompatibility");
  const fetch = guardedFetch({ credential, ledger: null, allowance: 8, receipt() {}, send: options.transport.send });
  /** @param {import("@earendil-works/pi-ai").StreamOptions | undefined} streamOptions @returns {import("@earendil-works/pi-ai").SimpleStreamOptions} */
  const bounded = (streamOptions) => ({ ...streamOptions, maxTokens: 1024, maxRetries: 0, transport: "sse", fetch,
    /** @param {unknown} payload @param {import("@earendil-works/pi-ai").Model<string>} current */
    async onPayload(payload, current) { return cappedPayload(await streamOptions?.onPayload?.(payload, current) ?? payload); } });
  /** @type {import("@earendil-works/pi-ai").StreamFunction<"openai-codex-responses",
   * import("@earendil-works/pi-ai").OpenAICodexResponsesOptions>} */
  const streamCodex = provider.stream.bind(provider);
  models.registerNativeProvider({ ...provider,
    stream(current, context, streamOptions) {
      if (current.api !== "openai-codex-responses") throw new ProbeFailure("provider_api_not_allowed");
      return streamCodex({ ...current, api: "openai-codex-responses" }, context, bounded(streamOptions));
    },
    streamSimple: (current, context, streamOptions) => provider.streamSimple(current, context, bounded(streamOptions)) });
  const settingsManager = sdk.SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false },
    cacheWarming: "off", transport: "sse", quietStartup: true });
  const agentDir = join(base, "agent");
  const services = await sdk.createAgentSessionServices({ cwd, agentDir, modelRuntime: models, settingsManager,
    resourceLoaderOptions: { noExtensions: true, noSkills: true, noPromptTemplates: true, noThemes: true,
      noContextFiles: true, disabledBuiltinExtensions: ["mcp", "codemode", "tool_search"],
      extensionFactories: options.extensionFactories ?? [] } });
  const sessionManager = sdk.SessionManager.create(cwd, join(base, "sessions"));
  const { session } = await sdk.createAgentSessionFromServices({ services, sessionManager, model,
    thinkingLevel: "off", tools: options.tools, customTools: options.customTools });
  return { session, sessionManager };
}

/** @param {import("@earendil-works/pi-coding-agent").AgentSession} session */
function settled(session) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { stop(); reject(new ProbeFailure("fixture_settle_deadline", "upstream_incompatibility")); }, 30_000);
    const stop = session.subscribe((event) => {
      if (event.type === "agent_settled") { clearTimeout(timer); stop(); resolve(event); }
    });
  });
}

/** @param {string} path */
function sessionEntries(path) {
  return readFileSync(path, "utf8").trim().split("\n").map((line) => JSON.parse(line));
}

/** @param {string} probeRoot @param {string} root @param {string} target */
export async function generate(probeRoot, root, target) {
  checkRuntime(probeRoot);
  const sdk = await import("@earendil-works/pi-coding-agent");
  const { Type } = await import("typebox");
  mkdirSync(target, { recursive: true });

  const usageTransport = fakeCodexTransport({ receipt() {}, script: USAGE_SCRIPT });
  const usage = await fixtureSession(sdk, { root, name: "usage", transport: usageTransport, tools: [] });
  for (const step of USAGE_SCRIPT) {
    const done = settled(usage.session);
    await usage.session.prompt(`Reply only ${step.text}.`);
    await done;
  }
  const usageFile = usage.sessionManager.getSessionFile();
  if (!usageFile || usageTransport.requests !== USAGE_SCRIPT.length) throw new ProbeFailure("usage_fixture_incomplete");
  copyFileSync(usageFile, join(target, "session-usage.jsonl"));
  const usages = USAGE_SCRIPT.map((step) => expectedUsage(/** @type {import("./fake-transport.mjs").FakeUsage} */ (step.usage)));
  const responses = usages.map((usage, index) => ({ responseId: `resp_fake_${index + 1}`, text: USAGE_SCRIPT[index].text, ...usage }));
  const totals = Object.fromEntries(/** @type {Array<keyof typeof usages[number]>} */ (["input", "output", "cacheRead",
    "cacheWrite", "reasoning", "totalTokens"]).map((key) => [key, usages.reduce((sum, usage) => sum + usage[key], 0)]));
  writeFileSync(join(target, "session-usage.expected.json"), JSON.stringify({
    session: "session-usage.jsonl", sessionId: usage.sessionManager.getSessionId(), provider: "openai-codex",
    model: "gpt-6.1-sol", source: "fake Codex transport, USAGE_SCRIPT in fixture-sessions.mjs",
    mapping: "Pi 1.3.0: input = input_tokens - cached_tokens - cache_write_tokens; cacheRead = cached_tokens; cacheWrite = cache_write_tokens; reasoning = reasoning_tokens; totalTokens = total_tokens",
    responses, totals }, null, 2) + "\n");

  /** @type {Array<() => void>} */
  const entered = [];
  const inFlight = new Promise((resolve) => entered.push(() => resolve(undefined)));
  const never = new Promise(() => {});
  const danglingTransport = fakeCodexTransport({ receipt() {}, script: [{ usage: { input: 300, output: 30 },
    toolCalls: DANGLING_CALLS.map(({ callId, name, arguments: args }) => ({ callId, name, arguments: args })) }] });
  const dangling = await fixtureSession(sdk, { root, name: "dangling", transport: danglingTransport,
    tools: ["edit", "corral_gate"],
    customTools: [{ name: "corral_gate", label: "corral gate (fixture)", description: "Fixture stand-in for a corral tool",
      parameters: Type.Object({}), async execute() { entered.forEach((signal) => signal()); return never; } }],
    // The edit call is held before execution, so neither call can record a result.
    extensionFactories: [(pi) => { pi.on("tool_call", async (event) => {
      if (event.toolName === "edit") { entered.forEach((signal) => signal()); await never; }
      return undefined;
    }); }] });
  void dangling.session.prompt("Run the corral gate and apply the edit.");
  await Promise.race([inFlight, new Promise((_, reject) => setTimeout(() =>
    reject(new ProbeFailure("dangling_fixture_deadline", "upstream_incompatibility")), 30_000))]);
  const danglingFile = dangling.sessionManager.getSessionFile();
  if (!danglingFile) throw new ProbeFailure("dangling_fixture_unpersisted", "upstream_incompatibility");
  // Snapshot while both calls are in flight: what a crash at this point leaves on disk.
  const snapshot = readFileSync(danglingFile, "utf8");
  writeFileSync(join(target, "session-dangling.jsonl"), snapshot);
  const entries = sessionEntries(join(target, "session-dangling.jsonl"));
  const calls = entries.filter((entry) => entry.type === "message" && entry.message.role === "assistant")
    .flatMap((entry) => entry.message.content.filter((/** @type {{type:string}} */ part) => part.type === "toolCall"));
  writeFileSync(join(target, "session-dangling.expected.json"), JSON.stringify({
    session: "session-dangling.jsonl", sessionId: dangling.sessionManager.getSessionId(),
    source: "fake Codex transport; snapshot taken while both calls were in flight, before any result",
    calls: calls.map((/** @type {{id:string,name:string,arguments:unknown}} */ call) => ({ toolCallId: call.id, name: call.name,
      class: DANGLING_CALLS.find((item) => item.name === call.name)?.class ?? "unknown", arguments: call.arguments })),
    toolResults: 0 }, null, 2) + "\n");
}

if (import.meta.main) {
  const [root, target] = process.argv.slice(2);
  if (!root || !target) throw new Error("usage: bun fixture-sessions.mjs <scratch-root> <target-dir>");
  await generate(dirname(fileURLToPath(import.meta.url)), root, target);
  console.log(JSON.stringify({ type: "fixtures_written", target }));
  process.exit(0);
}
