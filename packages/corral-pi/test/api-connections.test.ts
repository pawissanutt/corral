import { expect, test } from "bun:test";
import { chmodSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { spawnSync } from "node:child_process";
import { PassThrough } from "node:stream";
import { CONNECTIONS, connectionRules, resolveConnection } from "../src/auth/connections.ts";
import { apiKeyCommand, installApiKey } from "../src/auth/api-key.ts";
import { FileCredentialStore } from "../src/auth/store.ts";
import auth from "../src/auth/index.ts";
import { authDir } from "../src/auth/paths.ts";
import { authContext } from "../src/host/session.ts";
import { createApiCatalog } from "../src/host/api-connections.ts";
import { NO_CATALOG_CREDENTIALS } from "../src/host/catalog.ts";
import { bindingDigest, parseBinding, type ConnectionBinding } from "../src/host/connections.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { fakeCodexTransport } from "./fixtures/auth/fake-transport.ts";
import { Spending } from "../src/capabilities/budget.ts";

const KEY = "sk-SYNTHETIC-platform-key-not-real";
const context = { messages: [{ role: "user" as const, content: "Reply only API_OK.", timestamp: 1 }] };
function binding(connection: "openai-api" | "anthropic-api"): ConnectionBinding {
  const base = { version: 1 as const, connection, identity: connection, origin: "registered" as const, candidate: "exact-api", model: connection === "openai-api" ? "gpt-5.4" : "claude-sonnet-4-6", effort: null };
  return { ...base, digest: bindingDigest(base) };
}
function anthropicResponse(): Response {
  const events = [
    { type: "message_start", message: { id: "msg_fixture", type: "message", role: "assistant", model: "claude-sonnet-4-6", content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 3, output_tokens: 0 } } },
    { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
    { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "API_OK" } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", delta: { stop_reason: "end_turn", stop_sequence: null }, usage: { output_tokens: 2 } },
    { type: "message_stop" },
  ];
  return new Response(events.map(event => `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join(""), { headers: { "content-type": "text/event-stream" } });
}

for (const connection of ["anthropic-api", "openai-api"] as const) test(`${connection}: guarded direct client uses only its own credentials`, async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-api-connections-"));
  const previousXdg = process.env.XDG_DATA_HOME;
  process.env.XDG_DATA_HOME = dir;
  const owned = authDir();
  const selected = binding(connection);
  if (connection === "openai-api") expect(selected.digest).toBe("c89f600694189d007ba94f7e886d994487cb2e4e6f2822ba1ca60cd2bb9f3e40");
  const provider = CONNECTIONS[connection].provider;
  const subscription = provider === "openai" ? "openai-codex" : provider;
  try {
    const input = join(dir, "input"); writeFileSync(input, KEY, { mode: 0o600 });
    const subscriptions = new FileCredentialStore(owned, subscription);
    await subscriptions.modify(subscription, async () => ({ type: "oauth", access: "sk-ant-oat-SYNTHETIC", refresh: "r", expires: 9999999999999 }));
    const before = readFileSync(subscriptions.file);
    await installApiKey(owned, connection, input);
    const store = new FileCredentialStore(owned, provider, {}, connection);
    const selectedCredentials = auth.credentials(authContext(provider, connection));
    expect((await auth.credentials(authContext(subscription)).read(subscription))?.type).toBe("oauth");
    expect(store.file).not.toBe(subscriptions.file);
    expect((await store.read(provider))?.type).toBe("api_key");
    expect(resolveConnection(subscription)).toBe(provider === "openai" ? "openai-subscription" : "anthropic-subscription");
    let sent = 0, fail = false;
    const fake = fakeCodexTransport();
    const send = async (url: string | URL | Request, init?: RequestInit) => {
      sent++;
      const headers = new Headers(init?.headers);
      expect(headers.get(connection === "anthropic-api" ? "x-api-key" : "authorization")).toBe(connection === "anthropic-api" ? KEY : `Bearer ${KEY}`);
      expect(headers.has(connection === "anthropic-api" ? "authorization" : "x-api-key")).toBe(false);
      expect(init?.redirect).toBe("error");
      if (fail) return Response.json({ error: { type: "server_error", message: KEY } }, { status: 503 });
      return connection === "anthropic-api" ? anthropicResponse() : fake.send(url, init);
    };
    const spending = Spending.open(dir, { maxUsd: 1 })!;
    const lane = await createApiCatalog({ credentials: selectedCredentials, connection, model: selected.model, send, spending, report() {} });
    expect(lane.models.getProvider(provider)?.auth.oauth).toBeUndefined();
    expect(lane.models.getProvider(provider)?.classify).toBeUndefined();
    const response = await lane.models.streamSimple(lane.model, context, { apiKey: "sk-IGNORED-runtime-key", maxRetries: 9 }).result();
    expect(response.stopReason).toBe("stop");
    // OpenAI attaches a provider-private text signature; this contract checks public text only.
    expect(response.content).toMatchObject([{ type: "text", text: "API_OK" }]);
    expect(sent).toBe(1);
    fail = true;
    const failure = await lane.models.streamSimple(lane.model, context, { maxRetries: 9 }).result();
    expect(failure.stopReason).toBe("error");
    expect(JSON.stringify(failure)).not.toContain(KEY);
    expect(sent).toBe(2); // No hidden Pi/SDK transient retries.
    expect(readFileSync(subscriptions.file)).toEqual(before);
    await store.delete(provider);
    expect(subscriptions.present()).toBe(true);
    expect(store.present()).toBe(false);
    const loggedOut = await lane.models.streamSimple(lane.model, context).result();
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
    const run = (...args: string[]) => spawnSync(process.execPath, ["src/auth/login.ts", "--auth-dir", auth, "--connection", "anthropic-api", ...args], { cwd: process.cwd(), stdio: ["ignore", "pipe", "pipe"] });
    const check = (result: ReturnType<typeof run>, code: number, text: string) => {
      const output = result.stdout.toString() + result.stderr.toString();
      expect(result.status).toBe(code);
      expect(output).toContain(text);
      expect(output).not.toContain(KEY);
    };
    chmodSync(input, 0o644);
    check(run("--key-file", input), 1, "refused");
    chmodSync(input, 0o600);
    check(run("--key-file", input), 0, "stored");
    check(run("--status"), 0, "present");
    check(run("--logout"), 0, "removed");
    check(run("--status"), 0, "absent");
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
    for (const connection of ["openai-api", "anthropic-api"] as const) {
      const tty = terminal();
      const login = apiKeyCommand(dir, connection, ["--prompt"], tty);
      tty.input.write(`${KEY}x\x7f\r`);
      await login;
      const store = new FileCredentialStore(dir, CONNECTIONS[connection].provider, {}, connection);
      expect(await store.read(CONNECTIONS[connection].provider)).toEqual({ type: "api_key", key: KEY });
      expect(tty.screen()).not.toContain(KEY);
      expect(tty.input.isRaw).toBe(false);
      const before = readFileSync(store.file);
      for (const ending of ["\x03", "\x04", "\x1b", "\r", "sk-ant-oat-SYNTHETIC\r", "x".repeat(4097), null]) {
        const cancelled = terminal();
        const attempt = apiKeyCommand(dir, connection, ["--prompt"], cancelled);
        if (ending === null) cancelled.input.end();
        else cancelled.input.write(ending);
        await expect(attempt).rejects.toThrow();
        expect(cancelled.input.isRaw).toBe(false);
        expect(readFileSync(store.file)).toEqual(before);
      }
      const nonTerminal = terminal();
      nonTerminal.input.isTTY = false;
      await expect(apiKeyCommand(dir, connection, ["--prompt"], nonTerminal)).rejects.toThrow("terminal");
      expect(nonTerminal.screen()).toBe("");
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("API boundary rejects OAuth, command inputs, unsafe files, foreign endpoints and stale bindings", async () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-api-input-"));
  try {
    await expect(apiKeyCommand(dir, "openai-api", ["--status", "--logout"])).rejects.toThrow("exactly one");
    const file = join(dir, "key"); writeFileSync(file, KEY, { mode: 0o600 });
    const link = join(dir, "link"); symlinkSync(file, link);
    await expect(installApiKey(dir, "openai-api", link)).rejects.toThrow("0600");
    chmodSync(file, 0o644);
    await expect(installApiKey(dir, "openai-api", file)).rejects.toThrow("0600");
    chmodSync(file, 0o600);
    for (const value of ["!touch /tmp/not-executed", "$OPENAI_API_KEY", "sk-ant-oat-SYNTHETIC", "eyJ-OAuth"]) {
      writeFileSync(file, value);
      await expect(installApiKey(dir, "openai-api", file)).rejects.toThrow("literal");
    }
    const selected = binding("openai-api");
    const legacyBase = { ...selected, connection: "openai-subscription" as const, identity: "opaque-owner-label", origin: "legacy_subscription" as const };
    const legacy = { ...legacyBase, digest: bindingDigest(legacyBase) };
    expect(parseBinding(legacy, "openai-codex", legacy.model)).toEqual(legacy);
    const falseLegacyBase = { ...selected, identity: "opaque-owner-label", origin: "legacy_subscription" as const };
    const falseLegacy = { ...falseLegacyBase, digest: bindingDigest(falseLegacyBase) };
    expect(() => parseBinding(falseLegacy, "openai", falseLegacy.model)).toThrow("binding");
    expect(() => parseBinding({ ...selected, candidate: "changed" }, "openai", selected.model)).toThrow("binding");
    expect(() => parseBinding(selected, "openai-codex", selected.model)).toThrow("incompatible");
    expect(parseBinding(undefined, "openai", selected.model)).toBeUndefined();
    const route = { connection: "openai-api" as const, credentials: NO_CATALOG_CREDENTIALS,
      send: async () => { throw new Error("must not dispatch"); }, spending: Spending.open(dir, { maxUsd: 1 })!, report() {} };
    await expect(createApiCatalog({ ...route, model: selected.model, effort: "unsupported" })).rejects.toThrow("effort");
    await expect(createApiCatalog({ ...route, model: "openai-codex/gpt-5.4" })).rejects.toThrow("model_unavailable");
    for (const connection of ["openai-api", "anthropic-api"] as const) {
      const network = guardedFetch(async () => { throw new Error("must not send"); }, undefined, CONNECTIONS[connection].provider, connectionRules(connection));
      for (const url of ["https://evil.example/v1/messages", "https://api.openai.com/v1/responses?proxy=1", "https://api.anthropic.com/v1/messages#fragment"]) {
        await expect(network.fetch(url, { method: "POST", headers: { authorization: "Bearer eyJ-OAuth" } })).rejects.toThrow("network_refused");
      }
      const endpoint = connection === "openai-api" ? "https://api.openai.com/v1/responses" : "https://api.anthropic.com/v1/messages";
      for (const headers of [{ authorization: KEY }, { authorization: "Bearer sk-ant-oat-SYNTHETIC" }, { "x-api-key": "sk-ant-oat-SYNTHETIC" }]) {
        await expect(network.fetch(endpoint, { method: "POST", headers })).rejects.toThrow("network_refused");
      }
      expect(network.ledger().allowed).toBe(0);
    }
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
