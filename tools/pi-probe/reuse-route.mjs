// Prerequisite to native-session tests: verify authorized access-token reuse
// without weakening the output cap or touching the owner's login.
import { statSync, existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { ProbeFailure, readSubscription, readOnlyStore } from "./credentials.mjs";
import { cappedPayload, guardedFetch, liveAuthorization, liveLedger } from "./request-guard.mjs";
import { checkRuntime, readPins } from "./runtime.mjs";

/** @param {Record<string, unknown>} value */
const receipt = (value) => console.log(JSON.stringify({ at: new Date().toISOString(), ...value }));

async function run() {
  const fixture = process.env.CORRAL_TEST_HERDR_ROOT;
  const codexPath = process.env.PI_PROBE_CODEX_AUTH;
  const piPath = process.env.PI_PROBE_OMO_AUTH;
  const output = process.env.PI_PROBE_OUTPUT;
  if (!fixture || !codexPath || !piPath || !output || statSync(fixture).mode % 512 !== 0o700 ||
      process.env.PI_OFFLINE !== "1" || process.env.HOME !== join(fixture, "home")) {
    throw new ProbeFailure("private_fixture_required");
  }
  const authorization = liveAuthorization(process.env.PI_PROBE_LIVE_AUTHORIZATION);
  const probeRoot = resolve(new URL(".", import.meta.url).pathname);
  const pins = readPins(probeRoot);
  receipt({ type: "runtime_pins", ...checkRuntime(probeRoot), provider: pins.provider, model: pins.model });

  let selected;
  /** @type {ReadonlyArray<readonly ["codex" | "pi", string]>} */
  const sources = [["codex", codexPath], ["pi", piPath]];
  for (const [source, path] of sources) {
    try {
      selected = readSubscription(path, source, Date.now());
      break;
    } catch (error) {
      if (error instanceof ProbeFailure) {
        receipt({ type: "credential_candidate", source, compatible: false, reason: error.code });
      } else if (error instanceof Error && "code" in error && error.code === "ENOENT") {
        receipt({ type: "credential_candidate", source, compatible: false, reason: "not_present" });
      } else throw error;
    }
  }
  if (!selected) throw new ProbeFailure("compatible_unexpired_subscription_unavailable");
  receipt({ type: "credential_reuse", source: selected.source, sourceMode: selected.mode.toString(8),
    readOnly: true, inMemory: true, refreshTokenRetained: false, refreshDisabled: true,
    providerScope: "openai-codex", ownerLoginModified: false });
  try {
    const sdk = await import("@earendil-works/pi-coding-agent");
    const credentials = readOnlyStore(selected.credential);
    const runtime = await sdk.ModelRuntime.create({
      credentials, modelsPath: null, allowModelNetwork: false, refreshOnCreate: false,
    });
    const model = runtime.getModel(pins.provider, pins.model);
    const provider = runtime.getProvider(pins.provider);
    if (!model || provider?.auth.oauth?.isSubscription !== true) throw new ProbeFailure("subscription_route_unavailable");
    const auth = await runtime.getAuth(model);
    if (auth?.source !== "OAuth") throw new ProbeFailure("subscription_auth_not_resolved");
    const controller = new AbortController();
    const deadline = setTimeout(() => controller.abort(), 45_000);
    let outputCapRejected = false;
    let httpStatus = null;
    const fetch = guardedFetch({
      credential: selected.credential,
      ledger: liveLedger(output, authorization),
      authorization,
      allowance: 1,
      receipt(value) {
        if (value.type === "provider_http") {
          outputCapRejected = value.outputCapRejected === true;
          httpStatus = value.status;
        }
        receipt(value);
      },
    });
    try {
      // Direct provider hooks fail closed. Extension event handlers alone can
      // swallow errors and are deliberately not the budget boundary.
      const stream = runtime.streamSimple(model, {
        messages: [{ role: "user", content: "Reply only PROBE_READY. Do not use tools.", timestamp: Date.now() }],
      }, {
        maxTokens: 1024, transport: "sse", maxRetries: 0,
        timeoutMs: 30_000, signal: controller.signal, cacheRetention: "none",
        onPayload: cappedPayload, fetch,
      });
      const result = await stream.result();
      const completed = result.stopReason === "stop" && result.usage.output <= 1024;
      receipt({ type: "reuse_route_result", decision: completed ? "route_ready" : "no-go",
        stopReason: result.stopReason, outputTokens: result.usage.output,
        credentialReads: credentials.reads, httpStatus, outputCapRejected,
        reason: completed ? "subscription_reuse_completed" :
          outputCapRejected ? "subscription_backend_rejected_required_output_cap" : "subscription_request_not_completed",
        apiSpendUsd: 0, nativeCapabilityProof: false });
      if (!completed) process.exitCode = 2;
    } finally { clearTimeout(deadline); }
  } finally {
    selected.verifyUnchanged();
    if (existsSync(join(process.env.PI_CODING_AGENT_DIR ?? "", "auth.json"))) {
      throw new ProbeFailure("unexpected_fixture_credential_file");
    }
    receipt({ type: "credential_boundary", ownerBytesAndModeUnchanged: true,
      fixtureAuthFileAbsent: true, ownerRefreshOrLogoutInvoked: false });
  }
}

// The only failure output is a stable code; never emit provider/token errors.
try { await run(); } catch (error) {
  receipt({ type: "probe_stopped", decision: "no-go",
    reason: error instanceof ProbeFailure ? error.code : "unexpected_probe_error" });
  process.exitCode = 2;
}
