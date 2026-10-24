// `corral agent login` runs this entry point in the owner's pane: one interactive OpenAI Codex
// (ChatGPT subscription) OAuth login through Pi's own flow, persisted into the corral-owned auth
// directory. Its network is restricted to POSTs to the OAuth token and device-code endpoints.
import { createInterface } from "node:readline/promises";

import type { AuthEvent, AuthInteraction, AuthPrompt } from "@earendil-works/pi-ai";

import type { Fetch } from "./types.ts";
import { CredentialsUnavailable, authDir } from "./paths.ts";
import { FileCredentialStore } from "./store.ts";

export const LOGIN_DESTINATIONS: readonly string[] = [
  "https://auth.openai.com/oauth/token",
  "https://auth.openai.com/api/accounts/deviceauth/usercode",
  "https://auth.openai.com/api/accounts/deviceauth/token",
];

export class LoginNetworkRefused extends Error {
  readonly code = "network_refused";
  constructor(readonly destination: string) {
    super(`network_refused: ${destination} is not an OAuth login endpoint`);
  }
}

export function loginFetch(send: Fetch): Fetch {
  return async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const method = (init?.method ?? (input instanceof Request ? input.method : "GET")).toUpperCase();
    if (method !== "POST" || !LOGIN_DESTINATIONS.includes(url)) {
      const parsed = URL.parse(url);
      throw new LoginNetworkRefused(parsed ? `${parsed.origin}${parsed.pathname}` : "<unparseable>");
    }
    return send(input, { ...init, redirect: "error" });
  };
}

export interface LoginResult {
  readonly provider: "openai-codex";
  readonly file: string;
}

const FIXED_FAILURES = ["Login cancelled", "State mismatch", "Missing authorization code", "Device flow timed out"];
const NOTICE_MAX = 240;

/**
 * A login failure as a non-secret line. Upstream OAuth errors embed whole token responses and
 * response bodies, so only corral's own errors, a few fixed upstream phrases, the failing step
 * and an HTTP status ever reach the terminal.
 */
export function loginDiagnostic(error: unknown): string {
  if (error instanceof LoginNetworkRefused || error instanceof CredentialsUnavailable) return error.message;
  const message = error instanceof Error ? error.message : "";
  if (message.startsWith("login_failed:")) return message;
  const fixed = FIXED_FAILURES.find((text) => message.startsWith(text));
  if (fixed) return fixed;
  const step = /token exchange/.test(message) ? "the token exchange"
    : /token refresh/.test(message) ? "the token refresh"
    : /device (code|auth)/i.test(message) ? "the device-code request" : "the OAuth flow";
  const status = /(?:failed \(|status )(\d{3})\b/.exec(message)?.[1];
  return `OpenAI login failed at ${step}${status ? ` (HTTP ${status})` : ""}; response content is not shown`;
}

/** Progress text from the OAuth flow, bounded and with every token-like run redacted. */
export function redactNotice(text: string): string {
  return text.replace(/[A-Za-z0-9._~+/=-]{24,}/g, "<redacted>").slice(0, NOTICE_MAX);
}

export async function login(dir: string, interaction: AuthInteraction): Promise<LoginResult> {
  const store = new FileCredentialStore(dir);
  store.ensureDir();
  const { ModelRuntime } = await import("@earendil-works/pi-coding-agent");
  const models = await ModelRuntime.create({
    credentials: store, modelsPath: null, refreshOnCreate: false, allowModelNetwork: false,
  });
  const credential = await models.login("openai-codex", "oauth", interaction, { agentName: "corral" });
  if (credential.type !== "oauth" || !store.present()) throw new Error("login_failed: no OAuth credential was stored");
  return { provider: "openai-codex", file: store.file };
}

function terminalInteraction(): AuthInteraction {
  const lines = createInterface({ input: process.stdin, output: process.stderr });
  lines.once("close", () => {
    process.stderr.write("corral agent login failed: the terminal input closed before the login completed\n");
    process.exit(1);
  });
  return {
    async prompt(prompt: AuthPrompt): Promise<string> {
      if (prompt.type === "select") {
        prompt.options.forEach((option, index) => process.stderr.write(`  ${index + 1}. ${option.label}\n`));
        const answer = (await lines.question(`${prompt.message} [1] `, { signal: prompt.signal })).trim();
        return prompt.options[Number(answer || "1") - 1]?.id ?? answer;
      }
      return lines.question(`${prompt.message} `, prompt.signal ? { signal: prompt.signal } : {});
    },
    notify(event: AuthEvent) {
      if (event.type === "auth_url") process.stderr.write(`Open this URL to sign in:\n  ${event.url}\n`);
      else if (event.type === "device_code") {
        process.stderr.write(`Visit ${event.verificationUri} and enter the code ${event.userCode}\n`);
      } else process.stderr.write(`${redactNotice(event.message)}\n`);
    },
  };
}

if (import.meta.main) {
  process.env.PI_OFFLINE = "1";
  process.env.PI_TELEMETRY = "0";
  process.env.PI_SKIP_VERSION_CHECK = "1";
  const index = process.argv.indexOf("--auth-dir");
  const dir = index === -1 ? authDir() : process.argv[index + 1];
  globalThis.fetch = loginFetch(globalThis.fetch.bind(globalThis)) as typeof globalThis.fetch;
  login(dir ?? authDir(), terminalInteraction()).then((result) => {
    process.stdout.write(`Logged in: ${result.provider} credentials stored in ${result.file}\n`);
    process.exit(0);
  }, (error: unknown) => {
    process.stderr.write(`corral agent login failed: ${loginDiagnostic(error)}\n`);
    process.exit(1);
  });
}
