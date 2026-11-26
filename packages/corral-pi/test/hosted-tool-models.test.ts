import { afterEach, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { FileCredentialStore } from "../src/auth/store.ts";
import type { ApiProviderId } from "../src/auth/providers.ts";
import type { Fetch } from "../src/auth/types.ts";
import { listHostedToolModels } from "../src/host/hosted-tool-models.ts";

const KEY = "sk-SYNTHETIC-discovery-key";
const temps: string[] = [];
afterEach(() => { for (const dir of temps.splice(0)) rmSync(dir, { recursive: true, force: true }); });
function temp() { const dir = mkdtempSync(join(tmpdir(), "corral-tool-models-")); temps.push(dir); return dir; }
async function login(provider: ApiProviderId, dir = temp()) {
  await new FileCredentialStore(dir, provider).modify(provider, async () => ({ type: "api_key", key: KEY }));
  return dir;
}
function page(ids: readonly unknown[], extra: object = {}) {
  return Response.json({ data: ids.map(id => ({ id, raw_field: KEY })), ...extra });
}

test("OpenAI API lists only selector-safe search candidates or the shared GPT Image family", async () => {
  const dir = await login("openai");
  const fetch: Fetch = async (url, init) => {
    expect(url).toBe("https://api.openai.com/v1/models");
    expect(init?.method).toBe("GET");
    expect(init?.redirect).toBe("error");
    const headers = new Headers(init?.headers);
    expect(headers.get("authorization")).toBe(`Bearer ${KEY}`);
    expect(headers.has("x-api-key")).toBeFalse();
    return page(["gpt-new-chat", "gpt-image-future-V2", "gpt-image", "dall-e-3", "gpt-4o-audio-preview", "gpt-4o-realtime-preview",
      "gpt-4o-mini-transcribe", "tts-1", "whisper-1", "text-embedding-3-small", "omni-moderation-latest", "davinci-002",
      "o-new-version", "gpt-new-chat", "other/new", "gpt\nnext", "Sess-secret", KEY, `echo-${KEY}`, null, "x".repeat(250)]);
  };
  expect(await listHostedToolModels("web_search", "openai", { dir, fetch })).toEqual({ status: "live", models: ["gpt-new-chat", "o-new-version"] });
  expect(await listHostedToolModels("image_gen", "openai", { dir, fetch })).toEqual({ status: "live", models: ["gpt-image-future-V2"] });
  expect(await listHostedToolModels("image_gen", "openai", { dir, fetch: async () => page(["dall-e-3"]) })).toEqual({ status: "live", models: [] });
});

test("Anthropic API follows only its fixed Models endpoint and validated pagination cursors", async () => {
  const dir = await login("anthropic-api");
  const urls: string[] = [];
  const fetch: Fetch = async (url, init) => {
    urls.push(String(url));
    const headers = new Headers(init?.headers);
    expect(headers.get("x-api-key")).toBe(KEY);
    expect(headers.get("anthropic-version")).toBe("2023-06-01");
    expect(headers.has("authorization")).toBeFalse();
    expect(init?.redirect).toBe("error");
    return urls.length === 1 ? page(["claude-new", "claude-after"], { has_more: true, last_id: "claude-after" })
      : page(["claude-after", "claude-even-newer", "eyJtoken"], { has_more: false });
  };
  expect(await listHostedToolModels("web_search", "anthropic-api", { dir, fetch })).toEqual({
    status: "live", models: ["claude-new", "claude-after", "claude-even-newer"],
  });
  expect(urls).toEqual(["https://api.anthropic.com/v1/models?limit=100", "https://api.anthropic.com/v1/models?limit=100&after_id=claude-after"]);

  let repeats = 0;
  expect(await listHostedToolModels("web_search", "anthropic-api", { dir, fetch: async () => {
    repeats++;
    return page(["claude-after"], { has_more: true, last_id: "claude-after" });
  } })).toEqual({ status: "failed", models: [] });
  expect(repeats).toBe(2);
});

test("one-shot failures stay typed and do not expose provider bodies or errors", async () => {
  const dir = await login("openai");
  let sends = 0;
  const noNetwork: Fetch = async () => { sends++; throw new Error(KEY); };
  expect(await listHostedToolModels("web_search", "anthropic-api", { dir, fetch: noNetwork })).toEqual({ status: "missing_login", models: [] });
  for (const provider of ["openai-codex", "anthropic"]) {
    expect(await listHostedToolModels("web_search", provider, { dir, fetch: noNetwork })).toEqual({ status: "unsupported", models: [] });
  }
  expect(await listHostedToolModels("image_gen", "anthropic-api", { dir, fetch: noNetwork })).toEqual({ status: "unsupported", models: [] });
  expect(sends).toBe(0);
  for (const [status, expected] of [[401, "missing_login"], [403, "failed"], [429, "failed"], [503, "failed"]] as const) {
    expect(await listHostedToolModels("web_search", "openai", { dir, fetch: async () => new Response(KEY, { status }) }))
      .toEqual({ status: expected, models: [] });
  }
  for (const fetch of [noNetwork, async () => Response.redirect("https://example.invalid/model-redirect"),
    async () => new Response(KEY), async () => Response.json({ data: KEY }),
    async () => new Response(new ReadableStream({ start(controller) { controller.enqueue(new Uint8Array(1024 * 1024 + 1)); controller.close(); } })),
  ]) {
    expect(await listHostedToolModels("web_search", "openai", { dir, fetch })).toEqual({ status: "failed", models: [] });
  }
  const abort = new AbortController();
  let stop!: () => void;
  const reading = new Promise<void>(resolve => { stop = resolve; });
  const result = listHostedToolModels("web_search", "openai", { dir, signal: abort.signal,
    fetch: async () => new Response(new ReadableStream({ pull() { stop(); } })),
  });
  await reading;
  abort.abort(new Error(KEY));
  expect(await result).toEqual({ status: "failed", models: [] });
});

test("live picker output is bounded by both entry count and framed JSON bytes", async () => {
  const dir = await login("openai");
  const catalog = Array.from({ length: 513 }, (_, i) => `gpt-new-${i}`);
  const list = (ids: readonly string[]) => listHostedToolModels("web_search", "openai", { dir, fetch: async () => page(ids) });
  expect(await list(catalog)).toEqual({ status: "live", models: catalog.slice(0, 512) });
  const large = await list(catalog.map(id => id.padEnd(249, "x")));
  expect(large.status).toBe("live");
  expect(large.models.length).toBeGreaterThan(0);
  expect(large.models.length).toBeLessThanOrEqual(511);
  expect(Buffer.byteLength(`${JSON.stringify(large)}\n`)).toBeLessThanOrEqual(56 * 1024);
});

test("the host CLI emits only the one-shot JSON object without consulting workspace or ambient auth", () => {
  const dir = temp();
  const run = spawnSync(process.execPath, ["--no-env-file", "src/cli.ts", "hosted-tool-models", "--root", join(dir, "no-workspace"),
    "--tool", "web_search", "--provider", "openai"], { env: { ...process.env, XDG_DATA_HOME: dir, OPENAI_API_KEY: KEY }, encoding: "utf8" });
  expect(run.status).toBe(0);
  expect(run.stderr).toBe("");
  expect(JSON.parse(run.stdout)).toEqual({ status: "missing_login", models: [] });
});
