// `corral agent login` runs this entry point in the owner's pane with the provider the owner
// picked (`--provider openai-codex|anthropic`): one interactive subscription OAuth login through
// Pi's own flow, persisted into that provider's file in the corral-owned auth directory. Its
// network is restricted to POSTs to that provider's OAuth endpoints. Amazon Bedrock has no OAuth
// login: `corral agent login --provider amazon-bedrock` records its AWS profile binding in Rust.
import { type Interface, createInterface } from "node:readline/promises";

import type { AuthEvent, AuthInteraction, AuthPrompt } from "@earendil-works/pi-ai";

import type { Fetch } from "./types.ts";
import { CredentialsUnavailable, authDir } from "./paths.ts";
import { PROVIDERS, type ProviderId, isProviderId } from "./providers.ts";
import { FileCredentialStore } from "./store.ts";

/** The OpenAI Codex login endpoints; every provider's list is `PROVIDERS[id].login`. */
export const LOGIN_DESTINATIONS: readonly string[] = PROVIDERS["openai-codex"].login;

export class LoginNetworkRefused extends Error {
  readonly code = "network_refused";
  constructor(readonly destination: string) {
    super(`network_refused: ${destination} is not an OAuth login endpoint`);
  }
}

export function loginFetch(send: Fetch, provider: ProviderId = "openai-codex"): Fetch {
  const allowed = PROVIDERS[provider].login;
  return async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const method = (init?.method ?? (input instanceof Request ? input.method : "GET")).toUpperCase();
    if (method !== "POST" || !allowed.includes(url)) {
      const parsed = URL.parse(url);
      throw new LoginNetworkRefused(parsed ? `${parsed.origin}${parsed.pathname}` : "<unparseable>");
    }
    return send(input, { ...init, redirect: "error" });
  };
}

export interface LoginResult {
  readonly provider: ProviderId;
  readonly file: string;
}

const FIXED_FAILURES = ["Login cancelled", "State mismatch", "OAuth state mismatch", "Missing authorization code",
  "Device flow timed out"];
const NOTICE_MAX = 240;

/**
 * A login failure as a non-secret line. Upstream OAuth errors embed whole token responses and
 * response bodies, so only corral's own errors, a few fixed upstream phrases, the failing step
 * and an HTTP status ever reach the terminal.
 */
export function loginDiagnostic(error: unknown, provider: ProviderId = "openai-codex"): string {
  if (error instanceof LoginNetworkRefused || error instanceof CredentialsUnavailable || error instanceof LoginNotOAuth) {
    return error.message;
  }
  const message = error instanceof Error ? error.message : "";
  if (message.startsWith("login_failed:")) return message;
  const fixed = FIXED_FAILURES.find((text) => message.startsWith(text));
  if (fixed) return fixed;
  const step = /token exchange/i.test(message) ? "the token exchange"
    : /token refresh/i.test(message) ? "the token refresh"
    : /device (code|auth)/i.test(message) ? "the device-code request" : "the OAuth flow";
  const status = /(?:failed \(|status[ =])(\d{3})\b/.exec(message)?.[1];
  return `${PROVIDERS[provider].brand} login failed at ${step}${status ? ` (HTTP ${status})` : ""}; response content is not shown`;
}

/** Progress text from the OAuth flow, bounded and with every token-like run redacted. */
export function redactNotice(text: string): string {
  return text.replace(/[A-Za-z0-9._~+/=-]{24,}/g, "<redacted>").slice(0, NOTICE_MAX);
}

export class LoginNotOAuth extends Error {
  readonly code = "usage_error";
  constructor(readonly provider: ProviderId) {
    super(`${provider} has no OAuth login here; run \`corral agent login --provider ${provider}\`, which binds an AWS profile`);
  }
}

export async function login(
  dir: string, interaction: AuthInteraction, provider: ProviderId = "openai-codex",
): Promise<LoginResult> {
  if (PROVIDERS[provider].auth !== "oauth") throw new LoginNotOAuth(provider);
  const store = new FileCredentialStore(dir, provider);
  store.ensureDir();
  const { ModelRuntime } = await import("@earendil-works/pi-coding-agent");
  const models = await ModelRuntime.create({
    credentials: store, modelsPath: null, refreshOnCreate: false, allowModelNetwork: false,
  });
  const credential = await models.login(provider, "oauth", interaction, { agentName: "corral" });
  if (credential.type !== "oauth" || !store.present()) throw new Error("login_failed: no OAuth credential was stored");
  return { provider, file: store.file };
}

export type PickKey = "up" | "down" | "enter" | "cancel" | number;

/** One terminal read as picker keys: arrows (normal and application mode), j/k, Enter, a digit
 *  that picks its row, and q, Esc, Ctrl-C or Ctrl-D to cancel; anything else is ignored. */
export function pickKeys(chunk: string): PickKey[] {
  const keys: PickKey[] = [];
  for (let i = 0; i < chunk.length; i++) {
    const c = chunk[i];
    if (c === "\x1b") {
      const seq = chunk.slice(i + 1, i + 3);
      if (seq === "[A" || seq === "OA") keys.push("up");
      else if (seq === "[B" || seq === "OB") keys.push("down");
      else if (seq.length === 0) keys.push("cancel");
      i += seq.length === 2 && (seq[0] === "[" || seq[0] === "O") ? 2 : 0;
    } else if (c === "\r" || c === "\n") keys.push("enter");
    else if (c === "k") keys.push("up");
    else if (c === "j") keys.push("down");
    else if (c === "q" || c === "Q" || c === "\x03" || c === "\x04") keys.push("cancel");
    else if (c !== undefined && c >= "1" && c <= "9") keys.push(Number(c) - 1);
  }
  return keys;
}

export const PICK_HINT = "\u2191/\u2193 move \u00b7 Enter select \u00b7 q cancel";

export interface PickInput {
  on(event: "data", listener: (chunk: Buffer | string) => void): unknown;
  off(event: "data", listener: (chunk: Buffer | string) => void): unknown;
  setRawMode?(raw: boolean): unknown;
  resume(): unknown;
  pause(): unknown;
}

/** The same arrow-key picker as corral's provider step; resolves the chosen option's id and
 *  rejects with "Login cancelled" on q, Esc, Ctrl-C or end of input. */
export function pickOption(
  message: string, options: readonly { id: string; label: string }[], input: PickInput,
  out: { write(text: string): unknown }, signal?: AbortSignal,
): Promise<string> {
  let selected = 0;
  const draw = (redraw: boolean) => {
    if (redraw) out.write(`\x1b[${options.length + 2}A\x1b[J`);
    out.write(`${message}\r\n`);
    options.forEach((option, index) => out.write(`${index === selected ? ">" : " "} ${option.label}\r\n`));
    out.write(`${PICK_HINT}\r\n`);
  };
  return new Promise((resolve, reject) => {
    const finish = (done: () => void) => {
      input.off("data", onData);
      signal?.removeEventListener("abort", onAbort);
      input.setRawMode?.(false);
      input.pause();
      done();
    };
    const onAbort = () => finish(() => reject(signal?.reason ?? new Error("Login cancelled")));
    const onData = (chunk: Buffer | string) => {
      for (const key of pickKeys(chunk.toString())) {
        if (key === "cancel") return finish(() => reject(new Error("Login cancelled")));
        if (key === "enter") return finish(() => resolve(options[selected]!.id));
        if (typeof key === "number") {
          if (key < options.length) return finish(() => resolve(options[key]!.id));
          continue;
        }
        selected = key === "up" ? Math.max(0, selected - 1) : Math.min(options.length - 1, selected + 1);
        draw(true);
      }
    };
    if (signal?.aborted) return onAbort();
    signal?.addEventListener("abort", onAbort, { once: true });
    draw(false);
    input.setRawMode?.(true);
    input.on("data", onData);
    input.resume();
  });
}

function terminalInteraction(): AuthInteraction {
  // Text prompts share one line reader; an arrow-key pick needs the raw terminal, so the reader is
  // closed (without treating that as end of input) and reopened by the next text prompt.
  let lines: Interface | undefined;
  let handingOff = false;
  const reader = (): Interface => {
    if (lines) return lines;
    const opened = createInterface({ input: process.stdin, output: process.stderr });
    opened.once("close", () => {
      if (handingOff) return;
      process.stderr.write("corral agent login failed: the terminal input closed before the login completed\n");
      process.exit(1);
    });
    lines = opened;
    return opened;
  };
  return {
    async prompt(prompt: AuthPrompt): Promise<string> {
      if (prompt.type === "select" && process.stdin.isTTY) {
        if (lines) {
          handingOff = true;
          lines.close();
          lines = undefined;
          handingOff = false;
        }
        return pickOption(prompt.message, prompt.options, process.stdin, process.stderr, prompt.signal);
      }
      const text = reader();
      if (prompt.type === "select") {
        prompt.options.forEach((option, index) => process.stderr.write(`  ${index + 1}. ${option.label}\n`));
        const answer = (await text.question(`${prompt.message} [1] `, { signal: prompt.signal })).trim();
        return prompt.options[Number(answer || "1") - 1]?.id ?? answer;
      }
      return text.question(`${prompt.message} `, prompt.signal ? { signal: prompt.signal } : {});
    },
    notify(event: AuthEvent) {
      if (event.type === "auth_url") process.stderr.write(`Open this URL to sign in:\n  ${event.url}\n`);
      else if (event.type === "device_code") {
        process.stderr.write(`Visit ${event.verificationUri} and enter the code ${event.userCode}\n`);
      } else process.stderr.write(`${redactNotice(event.message)}\n`);
    },
  };
}

/** The login command line: `--auth-dir <dir> --provider <id>`, from Bun or the compiled host. */
export function runLogin(argv: readonly string[]): void {
  process.env.PI_OFFLINE = "1";
  process.env.PI_TELEMETRY = "0";
  process.env.PI_SKIP_VERSION_CHECK = "1";
  const arg = (flag: string) => {
    const index = argv.indexOf(flag);
    return index === -1 ? undefined : argv[index + 1];
  };
  const dir = arg("--auth-dir") ?? authDir();
  const provider = arg("--provider") ?? "openai-codex";
  if (!isProviderId(provider)) {
    process.stderr.write(`corral agent login failed: unknown provider ${provider}; use openai-codex or anthropic\n`);
    process.exit(2);
  }
  if (PROVIDERS[provider].auth !== "oauth") {
    process.stderr.write(`corral agent login failed: ${new LoginNotOAuth(provider).message}\n`);
    process.exit(2);
  }
  globalThis.fetch = loginFetch(globalThis.fetch.bind(globalThis), provider) as typeof globalThis.fetch;
  login(dir, terminalInteraction(), provider).then((result) => {
    process.stdout.write(`Logged in: ${result.provider} credentials stored in ${result.file}\n`);
    process.exit(0);
  }, (error: unknown) => {
    process.stderr.write(`corral agent login failed: ${loginDiagnostic(error, provider)}\n`);
    process.exit(1);
  });
}

if (import.meta.main) runLogin(process.argv);
