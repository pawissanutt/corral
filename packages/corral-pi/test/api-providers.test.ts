import { expect, test } from "bun:test";
import { chmodSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { spawnSync } from "node:child_process";
import { PassThrough } from "node:stream";
import { apiKeyCommand, installApiKey } from "../src/auth/api-key.ts";
import { FileCredentialStore } from "../src/auth/store.ts";
import auth from "../src/auth/index.ts";
import { authDir } from "../src/auth/paths.ts";
import { authContext } from "../src/host/session.ts";
import { assertApiEffort, installApiProvider } from "../src/host/api-providers.ts";
import { createModelCatalog } from "../src/host/catalog.ts";
import type { ModelUsage } from "../src/tools/corral.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { fakeCodexTransport } from "./fixtures/auth/fake-transport.ts";

const KEY = "sk-SYNTHETIC-platform-key-not-real";
const context = { messages: [{ role: "user" as const, content: "Reply only API_OK.", timestamp: 1 }] };

for (const provider of ["anthropic-api", "openai"] as const) test(`${provider}: guard, stored credential and metered usage`, async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-api-providers-"));
  const previousXdg = process.env.XDG_DATA_HOME;
  process.env.XDG_DATA_HOME = dir;
  const owned = authDir();
  const modelId = provider === "openai" ? "gpt-5.4" : "claude-sonnet-4-6";
  const subscription = provider === "openai" ? "openai-codex" : "anthropic";
  try {
    const input = join(dir, "input"); writeFileSync(input, KEY, { mode: 0o600 });
    const subscriptions = new FileCredentialStore(owned, subscription);
    await subscriptions.modify(subscription, async () => ({ type: "oauth", access: "sk-ant-oat-SYNTHETIC", refresh: "r", expires: 9999999999999 }));
    const before = readFileSync(subscriptions.file);
    await installApiKey(owned, provider, input);
    const store = new FileCredentialStore(owned, provider);
    expect(store.file).toBe(join(owned, `${provider === "openai" ? "openai-api" : provider}.json`));
    const selectedCredentials = auth.credentials(authContext(provider));
    expect((await auth.credentials(authContext(subscription)).read(subscription))?.type).toBe("oauth");
    expect(store.file).not.toBe(subscriptions.file);
    expect((await store.read(provider))?.type).toBe("api_key");
    let sent = 0, fail = false;
    const fake = fakeCodexTransport();
    fake.script([{ text: "API_OK", usage: { input: 3, output: 2 } }]);
    const send = async (url: string | URL | Request, init?: RequestInit) => {
      sent++;
      const headers = new Headers(init?.headers);
      expect(headers.get(provider === "anthropic-api" ? "x-api-key" : "authorization")).toBe(provider === "anthropic-api" ? KEY : `Bearer ${KEY}`);
      expect(headers.has(provider === "anthropic-api" ? "authorization" : "x-api-key")).toBe(false);
      const target = new URL(url instanceof Request ? url.url : url);
      expect(target.origin + target.pathname).toBe(provider === "anthropic-api"
        ? "https://api.anthropic.com/v1/messages" : "https://api.openai.com/v1/responses");
      expect(init?.redirect).toBe("error");
      if (fail) return Response.json({ error: { type: "server_error", message: KEY } }, { status: 503 });
      return fake.send(url, init);
    };
    const models = await createModelCatalog(selectedCredentials, { providers: [provider], modelsPath: null });
    const usage: ModelUsage[] = [];
    const guard = installApiProvider(models, provider, selectedCredentials, send, report => { usage.push(report); });
    expect(models.getProvider(provider)?.auth.oauth).toBeUndefined();
    expect(models.getProvider(provider)?.classify).toBeUndefined();
    const model = models.getModel(provider, modelId)!;
    expect(() => assertApiEffort(model, "unsupported")).toThrow("model_unavailable");
    const response = await models.streamSimple(model, context, { apiKey: "sk-IGNORED-runtime-key", maxRetries: 9 }).result();
    expect(response.stopReason).toBe("stop");
    // OpenAI attaches a provider-private text signature; this contract checks public text only.
    expect(response.content).toMatchObject([{ type: "text", text: "API_OK" }]);
    expect(sent).toBe(1);
    expect(guard.ledger().allowed).toBe(1);
    expect(usage).toMatchObject([{ model: `${provider}/${modelId}`, input: 3, output: 2, billing: "api" }]);
    expect(usage[0]!.usd).toBeGreaterThan(0);
    fail = true;
    const failure = await models.streamSimple(model, context, { maxRetries: 9 }).result();
    expect(failure.stopReason).toBe("error");
    expect(JSON.stringify(failure)).not.toContain(KEY);
    expect(sent).toBe(2); // No hidden Pi/SDK transient retries.
    expect(readFileSync(subscriptions.file)).toEqual(before);
    await store.delete(provider);
    expect(subscriptions.present()).toBe(true);
    expect(store.present()).toBe(false);
    const loggedOut = await models.streamSimple(model, context, { env: { ANTHROPIC_API_KEY: KEY, OPENAI_API_KEY: KEY } }).result();
    expect(loggedOut.stopReason).toBe("error");
    expect(sent).toBe(2); // No OAuth/ambient fallback after API logout.
  } finally {
    if (previousXdg === undefined) delete process.env.XDG_DATA_HOME;
    else process.env.XDG_DATA_HOME = previousXdg;
    rmSync(dir, { recursive: true, force: true });
  }
});

test("offline login entry stores, reports and removes a protected API key without disclosing it", () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-api-login-"));
  try {
    const input = join(dir, "input");
    const auth = join(dir, "auth");
    writeFileSync(input, KEY, { mode: 0o600 });
    const run = (...args: string[]) => spawnSync(process.execPath, ["src/auth/login.ts", "--auth-dir", auth, ...args], { cwd: process.cwd(), stdio: ["ignore", "pipe", "pipe"] });
    const check = (result: ReturnType<typeof run>, code: number, text: string) => {
      const output = result.stdout.toString() + result.stderr.toString();
      expect(result.status).toBe(code);
      expect(output).toContain(text);
      expect(output).not.toContain(KEY);
    };
    chmodSync(input, 0o644);
    check(run("--connection", "anthropic-api"), 2, "--provider anthropic-api");
    check(run("--provider", "anthropic-api", "--key-file", input), 1, "refused");
    chmodSync(input, 0o600);
    check(run("--provider", "anthropic-api", "--key-file", input), 0, "stored");
    check(run("--provider", "anthropic-api", "--status"), 0, "present");
    check(run("--provider", "anthropic-api", "--logout"), 0, "removed");
    check(run("--provider", "anthropic-api", "--status"), 0, "absent");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("interactive API login accepts a hidden key and cancels without overwriting credentials", async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-api-prompt-"));
  const terminal = () => {
    const input = Object.assign(new PassThrough(), {
      isTTY: true, isRaw: false,
      setRawMode(raw: boolean) { this.isRaw = raw; return this; },
    });
    const output = Object.assign(new PassThrough(), { isTTY: true });
    let screen = "";
    output.on("data", chunk => { screen += chunk.toString(); });
    return { input, output, screen: () => screen };
  };
  try {
    for (const provider of ["openai", "anthropic-api"] as const) {
      const tty = terminal();
      const login = apiKeyCommand(dir, provider, [], tty);
      tty.input.write(`${KEY}x\x7f\r`);
      await login;
      const store = new FileCredentialStore(dir, provider);
      expect(await store.read(provider)).toEqual({ type: "api_key", key: KEY });
      expect(tty.screen()).not.toContain(KEY);
      expect(tty.input.isRaw).toBe(false);
      const before = readFileSync(store.file);
      for (const ending of ["\x03", "\x04", "\x1b", "\r", "sk-ant-oat-SYNTHETIC\r", "x".repeat(4097), null]) {
        const cancelled = terminal();
        const attempt = apiKeyCommand(dir, provider, ["--prompt"], cancelled);
        if (ending === null) cancelled.input.end();
        else cancelled.input.write(ending);
        await expect(attempt).rejects.toThrow();
        expect(cancelled.input.isRaw).toBe(false);
        expect(readFileSync(store.file)).toEqual(before);
      }
      const nonTerminal = terminal();
      nonTerminal.input.isTTY = false;
      await expect(apiKeyCommand(dir, provider, ["--prompt"], nonTerminal)).rejects.toThrow("terminal");
      expect(nonTerminal.screen()).toBe("");
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("API boundaries reject OAuth, command inputs, unsafe files and foreign endpoints", async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-api-input-"));
  try {
    await expect(apiKeyCommand(dir, "openai", ["--status", "--logout"])).rejects.toThrow("exactly one");
    const file = join(dir, "key"); writeFileSync(file, KEY, { mode: 0o600 });
    const link = join(dir, "link"); symlinkSync(file, link);
    await expect(installApiKey(dir, "openai", link)).rejects.toThrow("0600");
    chmodSync(file, 0o644);
    await expect(installApiKey(dir, "openai", file)).rejects.toThrow("0600");
    chmodSync(file, 0o600);
    for (const value of ["!touch /tmp/not-executed", "$OPENAI_API_KEY", "sk-ant-oat-SYNTHETIC", "eyJ-OAuth"]) {
      writeFileSync(file, value);
      await expect(installApiKey(dir, "openai", file)).rejects.toThrow("literal");
    }
    for (const provider of ["openai", "anthropic-api"] as const) {
      const network = guardedFetch(async () => { throw new Error("must not send"); }, undefined, provider);
      for (const url of ["https://evil.example/v1/messages", "https://api.openai.com/v1/responses?proxy=1", "https://api.anthropic.com/v1/messages#fragment"]) {
        await expect(network.fetch(url, { method: "POST", headers: provider === "openai"
          ? { authorization: `Bearer ${KEY}` } : { "x-api-key": KEY } })).rejects.toThrow("network_refused");
      }
      const endpoint = provider === "openai" ? "https://api.openai.com/v1/responses" : "https://api.anthropic.com/v1/messages";
      for (const headers of [{ authorization: KEY }, { authorization: "Bearer sk-ant-oat-SYNTHETIC" }, { "x-api-key": "sk-ant-oat-SYNTHETIC" }]) {
        await expect(network.fetch(endpoint, { method: "POST", headers })).rejects.toThrow("network_refused");
      }
      expect(network.ledger().allowed).toBe(0);
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
