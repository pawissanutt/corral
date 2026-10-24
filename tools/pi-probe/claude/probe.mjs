// claude-route-probe (T-306): does upstream Pi's Anthropic Pro/Max OAuth route, hosted by Bun, bill
// the subscription? Offline mode runs the same code against a fake transport; live mode needs the
// owner login (login.mjs) and a recorded authorization, and stays inside the OD5 budget.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { ProbeFailure } from "../credentials.mjs";
import { checkRuntime } from "../runtime.mjs";
import { classifyRoute } from "./classify.mjs";
import { fakeAnthropic } from "./fake-anthropic.mjs";
import { guardedFetch, ledgerPath, liveAuthorization, redact, settle } from "./guard.mjs";
import { BUDGET, DEFAULT_MODEL, OAUTH_TOKEN_MARKER, PROBE_REQUESTS, PROVIDER } from "./policy.mjs";
import { defaultStoreDir, openCredentialStore } from "./store.mjs";

/** @typedef {import("./classify.mjs").Observation} Observation */

const MAX_TOKENS = 128;
const TOOL = { name: "probe_echo", description: "Echo a short text.",
  parameters: { type: "object", properties: { text: { type: "string" } }, required: ["text"] } };

/**
 * @param {{mode: "offline" | "live", store: ReturnType<typeof openCredentialStore>,
 * send?: typeof globalThis.fetch, ledger: string | null, authorization?: string, model?: string,
 * outputDir: string, receipt: (value: Record<string, unknown>) => void, probeRoot: string}} options
 */
export async function runProbe(options) {
  const model = options.model ?? DEFAULT_MODEL;
  const { receipt } = options;
  receipt({ type: "runtime_pins", ...checkRuntime(options.probeRoot), provider: PROVIDER, model });
  const stored = await options.store.read(PROVIDER);
  if (!stored || stored.type !== "oauth") throw new ProbeFailure("login_required");
  if (typeof stored.access !== "string" || !stored.access.includes(OAUTH_TOKEN_MARKER)) {
    throw new ProbeFailure("subscription_credential_required");
  }
  const sdk = await import("@earendil-works/pi-coding-agent");
  const runtime = await sdk.ModelRuntime.create({ credentials: options.store, modelsPath: null,
    allowModelNetwork: false, refreshOnCreate: false });
  const routed = runtime.getModel(PROVIDER, model);
  if (!routed || runtime.getProvider(PROVIDER)?.auth.oauth?.isSubscription !== true) throw new ProbeFailure("subscription_route_unavailable");
  if ((await runtime.getAuth(routed))?.source !== "OAuth") throw new ProbeFailure("subscription_auth_not_resolved");
  // Pi may have refreshed the token while resolving auth; the guard must match what it will send.
  const current = await options.store.read(PROVIDER);
  if (!current || current.type !== "oauth") throw new ProbeFailure("login_required");
  receipt({ type: "route_resolved", provider: PROVIDER, model, source: "OAuth", isSubscription: true,
    credentialFile: "0600", refreshByPi: true, foreignStoreRead: false });

  /** @type {Record<string, unknown>[]} */
  const seen = [];
  /** @type {string | null} */
  let guardCode = null;
  const guard = guardedFetch({ access: current.access, model, ledger: options.ledger,
    ...(options.authorization ? { authorization: options.authorization } : {}),
    allowance: PROBE_REQUESTS, receipt(value) { seen.push(value); receipt(value); },
    ...(options.send ? { send: options.send } : {}) });
  /** @type {typeof globalThis.fetch} */
  const fetch = async (input, init) => {
    try { return await guard(input, init); } catch (error) {
      if (error instanceof ProbeFailure) guardCode = error.code;
      throw error;
    }
  };

  /** @param {string} prompt @param {boolean} tool @returns {Promise<Observation>} */
  const ask = async (prompt, tool) => {
    const before = seen.length;
    const result = await runtime.streamSimple(routed, {
      messages: [{ role: "user", content: prompt, timestamp: Date.now() }],
      ...(tool ? { tools: [TOOL] } : {}),
    }, { maxTokens: MAX_TOKENS, maxRetries: 0, timeoutMs: 30_000, cacheRetention: "none", fetch }).result();
    if (guardCode) throw new ProbeFailure(guardCode);
    const http = seen.slice(before).find((entry) => entry.type === "provider_http");
    const sequence = typeof http?.sequence === "number" ? http.sequence : 0;
    if (options.ledger && options.authorization && sequence) settle(options.ledger, options.authorization, sequence, result.usage.output);
    const error = http?.error;
    return { sequence, status: typeof http?.status === "number" ? http.status : 0,
      headers: /** @type {Record<string, string>} */ (http?.headers ?? {}),
      ...(error ? { error: /** @type {{type: string | null, message: string | null}} */ (error) } :
        result.errorMessage ? { error: { type: "adapter_error", message: redact(result.errorMessage) } } : {}),
      completed: result.stopReason === "stop" || result.stopReason === "toolUse",
      stopReason: result.stopReason, outputTokens: result.usage.output,
      toolCall: result.content.some((part) => part.type === "toolCall") };
  };

  const plain = await ask("Reply only PROBE_READY. Do not use tools.", false);
  const tool = await ask("Call the probe_echo tool with text PROBE_TOOL. Do not write any other text.", true);
  const classified = classifyRoute([plain, tool]);
  const offline = options.mode === "offline";
  const summary = {
    mode: options.mode, model, provider: PROVIDER,
    decision: offline ? "NOT RUN" : classified.decision,
    offlineSelfCheck: offline ? classified.decision : null, liveEvidence: !offline,
    billing: classified.billing, toolRoundTrip: classified.toolRoundTrip, reason: classified.reason,
    evidence: classified.evidence, observations: [plain, tool],
    budget: { limit: BUDGET, requestsDispatched: seen.filter((entry) => entry.type === "request_reserved").length,
      outputTokensReported: (plain.outputTokens ?? 0) + (tool.outputTokens ?? 0), apiKeyUsed: false, apiSpendUsd: 0 },
  };
  mkdirSync(options.outputDir, { recursive: true });
  writeFileSync(join(options.outputDir, `claude-${options.mode}-result.json`), JSON.stringify(summary, null, 2) + "\n");
  receipt({ type: "claude_route_result", ...summary, observations: undefined });
  return summary;
}

/** @param {Record<string, unknown>} value */
const print = (value) => console.log(JSON.stringify({ at: new Date().toISOString(), ...value }));

async function main() {
  const probeRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const output = process.env.PI_PROBE_OUTPUT;
  const mode = process.env.PI_PROBE_CLAUDE_MODE;
  if (!output || (mode !== "offline" && mode !== "live")) throw new ProbeFailure("probe_mode_and_output_required");
  if (mode === "offline") {
    const scratch = mkdtempSync(join(tmpdir(), "claude-probe-offline-"));
    try {
      const store = openCredentialStore(join(scratch, "auth"));
      await store.modify(PROVIDER, async () => ({ type: "oauth", access: OAUTH_TOKEN_MARKER + "01-synthetic-fixture",
        refresh: "synthetic-refresh-fixture", expires: Date.now() + 3_600_000 }));
      const fake = fakeAnthropic({ script: [{ text: "PROBE_READY" }, { tool: { input: { text: "PROBE_TOOL" } }, outputTokens: 12 }] });
      await runProbe({ mode, store, send: fake.send, ledger: null, outputDir: output, receipt: print, probeRoot });
    } finally { rmSync(scratch, { recursive: true, force: true }); }
    return;
  }
  const authorization = liveAuthorization(process.env.PI_PROBE_CLAUDE_AUTHORIZATION);
  const store = openCredentialStore(process.env.PI_PROBE_CLAUDE_AUTH_DIR || defaultStoreDir());
  await runProbe({ mode, store, ledger: ledgerPath(output, authorization), authorization, outputDir: output, receipt: print, probeRoot,
    ...(process.env.PI_PROBE_CLAUDE_MODEL ? { model: process.env.PI_PROBE_CLAUDE_MODEL } : {}) });
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  // The only failure output is a stable code; never emit provider or token text.
  main().catch((error) => {
    print({ type: "probe_stopped", decision: "NO-GO",
      reason: error instanceof ProbeFailure ? error.code : "unexpected_probe_error" });
    process.exitCode = 2;
  });
}
