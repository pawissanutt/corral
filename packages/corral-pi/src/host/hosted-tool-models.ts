// Live picker candidates only. This one-shot GET path never widens a session's provider rules.
import { parseArgs } from "node:util";
import { authDir, CredentialsUnavailable } from "../auth/paths.ts";
import { isApiProvider, validApiKey } from "../auth/providers.ts";
import { FileCredentialStore } from "../auth/store.ts";
import type { Fetch } from "../auth/types.ts";
import { hostedProvider, isApiSearchModel, isGptImageModel, validHostedModelId, type HostedTool } from "./hosted-settings.ts";

export type HostedToolModels = { status: "live" | "unsupported" | "missing_login" | "failed"; models: string[] };
const empty = (status: Exclude<HostedToolModels["status"], "live">): HostedToolModels => ({ status, models: [] });
const OPENAI_LIST = "https://api.openai.com/v1/models";
const ANTHROPIC_LIST = "https://api.anthropic.com/v1/models";
const MAX_RESPONSE_BYTES = 1024 * 1024;
const MAX_OUTPUT_BYTES = 56 * 1024;
const MAX_MODELS = 512;
const MAX_PAGES = 8;
const TIMEOUT_MS = 10_000;

function cancel(stream: ReadableStream<Uint8Array> | ReadableStreamDefaultReader<Uint8Array> | null): void {
  if (stream) void stream.cancel().catch(() => {});
}

async function readJson(response: Response, signal: AbortSignal): Promise<unknown> {
  if (Number(response.headers.get("content-length")) > MAX_RESPONSE_BYTES) {
    cancel(response.body);
    throw new Error();
  }
  const reader = response.body?.getReader();
  if (!reader) throw new Error();
  const abort = () => cancel(reader);
  signal.addEventListener("abort", abort, { once: true });
  const chunks: Uint8Array[] = [];
  let bytes = 0;
  try {
    for (;;) {
      signal.throwIfAborted();
      const { done, value } = await reader.read();
      if (done) break;
      bytes += value.byteLength;
      if (bytes > MAX_RESPONSE_BYTES) throw new Error();
      chunks.push(value);
    }
    signal.throwIfAborted();
    return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(Buffer.concat(chunks, bytes)));
  } finally {
    signal.removeEventListener("abort", abort);
    cancel(reader);
    reader.releaseLock();
  }
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

export async function listHostedToolModels(tool: HostedTool, provider: string, options: {
  dir?: string; fetch?: Fetch; signal?: AbortSignal;
} = {}): Promise<HostedToolModels> {
  // Neither existing OAuth route has a supported public account-scoped catalog.
  // The Sign in with ChatGPT /models flow requires a different OAuth app/token and scope.
  if (!hostedProvider(tool, provider) || !isApiProvider(provider)) return empty("unsupported");
  try {
    const credential = await new FileCredentialStore(options.dir ?? authDir(), provider).read(provider);
    if (credential?.type !== "api_key" || !validApiKey(credential.key)) return empty("missing_login");
    const { key } = credential;
    const anthropic = provider === "anthropic-api";
    const headers = new Headers({ accept: "application/json" });
    if (anthropic) {
      headers.set("x-api-key", key);
      headers.set("anthropic-version", "2023-06-01");
    } else headers.set("authorization", `Bearer ${key}`);

    const signal = options.signal ?? AbortSignal.timeout(TIMEOUT_MS);
    const send = options.fetch ?? globalThis.fetch;
    const result: HostedToolModels = { status: "live", models: [] };
    const seen = new Set<string>();
    const cursors = new Set<string>();
    // Selector-safe IDs are ASCII and require no JSON escapes: quotes + possible comma per ID.
    let outputBytes = JSON.stringify(result).length + 1; // Include the CLI's newline.
    let after: string | undefined;
    for (let page = 0; page < (anthropic ? MAX_PAGES : 1); page++) {
      signal.throwIfAborted();
      const url = new URL(anthropic ? ANTHROPIC_LIST : OPENAI_LIST);
      if (anthropic) {
        url.searchParams.set("limit", "100");
        if (after !== undefined) url.searchParams.set("after_id", after);
      }
      const response = await send(url.href, { method: "GET", redirect: "error", headers, signal });
      if (!response.ok || response.redirected) {
        cancel(response.body);
        return empty(response.status === 401 ? "missing_login" : "failed");
      }
      const { data, has_more, last_id } = record(await readJson(response, signal));
      if (!Array.isArray(data) || anthropic && typeof has_more !== "boolean") return empty("failed");
      for (const value of data) {
        const { id } = record(value);
        if (typeof id !== "string" || id.includes(key) || seen.has(id) ||
            (tool === "image_gen" ? !isGptImageModel(id) : !isApiSearchModel(provider, id))) continue;
        outputBytes += id.length + 3;
        if (outputBytes > MAX_OUTPUT_BYTES) return result;
        seen.add(id);
        result.models.push(id);
        if (result.models.length === MAX_MODELS) return result;
      }
      if (!anthropic || !has_more) return result;
      if (!validHostedModelId(provider, last_id) || last_id.includes(key) || cursors.has(last_id)) return empty("failed");
      cursors.add(last_id);
      after = last_id;
    }
    // Do not report a partial or empty live list if pagination could not finish within bounds.
    return empty("failed");
  } catch (error) {
    return empty(error instanceof CredentialsUnavailable ? "missing_login" : "failed");
  }
}

export async function runHostedToolModels(argv: readonly string[]): Promise<void> {
  let result = empty("failed");
  try {
    const { values } = parseArgs({ args: [...argv], options: {
      root: { type: "string", multiple: true }, tool: { type: "string", multiple: true }, provider: { type: "string", multiple: true },
    } });
    const tool = values.tool?.[0], provider = values.provider?.[0];
    // Root identifies the requesting workspace, not a configuration/auth source. Never read it.
    if (values.root?.length === 1 && values.root[0] && values.tool?.length === 1 && values.provider?.length === 1 &&
        provider && (tool === "web_search" || tool === "image_gen")) result = await listHostedToolModels(tool, provider);
  } catch { /* Do not echo bad arguments or provider failures. */ }
  process.stdout.write(`${JSON.stringify(result)}\n`);
}
