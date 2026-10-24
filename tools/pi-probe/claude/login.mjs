// The one interactive owner login for claude-route-probe (OD8 style): Pi's own Anthropic OAuth flow,
// credentials stored by Pi in a probe-owned 0700/0600 directory. Nothing is copied from Claude Code,
// ~/.claude, OmO or any other harness; no model request is made.
import { createInterface } from "node:readline/promises";
import { fileURLToPath } from "node:url";
import { ProbeFailure } from "../credentials.mjs";
import { liveAuthorization, redact } from "./guard.mjs";
import { OAUTH_TOKEN_MARKER, PROVIDER } from "./policy.mjs";
import { defaultStoreDir, openCredentialStore } from "./store.mjs";

/** @param {Record<string, unknown>} value */
const print = (value) => console.log(JSON.stringify({ at: new Date().toISOString(), ...value }));

/** @param {string} [method] */
export function interaction(method = "copy_code") {
  const lines = createInterface({ input: process.stdin, output: process.stdout });
  /** @type {import("@earendil-works/pi-ai").AuthInteraction & {close(): void}} */
  const value = {
    signal: AbortSignal.timeout(600_000),
    close: () => lines.close(),
    async prompt(prompt) {
      if (prompt.type === "select") {
        return prompt.options.find((option) => option.id === method)?.id ?? prompt.options[0]?.id ?? "";
      }
      return (await lines.question(prompt.message + "\n> ")).trim();
    },
    notify(event) {
      if (event.type === "auth_url") console.log(`\nOpen this URL in your browser:\n${event.url}\n${event.instructions ?? ""}\n`);
      else if (event.type === "progress" || event.type === "info") console.log(redact(event.message));
    },
  };
  return value;
}

async function main() {
  liveAuthorization(process.env.PI_PROBE_CLAUDE_AUTHORIZATION);
  if (!process.stdin.isTTY) throw new ProbeFailure("interactive_terminal_required");
  const store = openCredentialStore(process.env.PI_PROBE_CLAUDE_AUTH_DIR || defaultStoreDir());
  const existing = await store.read(PROVIDER);
  if (existing?.type === "oauth" && process.env.PI_PROBE_CLAUDE_REPLACE !== "1") {
    print({ type: "login_exists", provider: PROVIDER, replaced: false });
    return;
  }
  const sdk = await import("@earendil-works/pi-coding-agent");
  const runtime = await sdk.ModelRuntime.create({ credentials: store, modelsPath: null, allowModelNetwork: false, refreshOnCreate: false });
  const ui = interaction(process.env.PI_PROBE_CLAUDE_LOGIN_METHOD);
  try {
    await runtime.login(PROVIDER, "oauth", ui);
  } finally { ui.close(); }
  const stored = await store.read(PROVIDER);
  if (stored?.type !== "oauth") throw new ProbeFailure("login_not_stored");
  print({ type: "login_complete", provider: PROVIDER, credentialType: "oauth", subscriptionTokenShape: stored.access.includes(OAUTH_TOKEN_MARKER),
    expiresInSeconds: Math.round((stored.expires - Date.now()) / 1000), credentialFile: "0600", foreignStoreRead: false });
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    print({ type: "login_stopped", reason: error instanceof ProbeFailure ? error.code : "login_failed" });
    process.exitCode = 2;
  });
}
