// The production auth module offline: Pi's own Codex OAuth login and refresh against a fake
// token server, the 0700/0600 corral-owned store, and a traced child proving no other harness
// store under HOME is ever opened. `corral agent install` never copies this file.
import { afterEach, beforeAll, describe, expect, test } from "bun:test";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import type { AuthInteraction } from "@earendil-works/pi-ai";

import auth from "./index.ts";
import { LOGIN_DESTINATIONS, login, loginFetch } from "./login.ts";
import { authDir, piNativeRoot } from "./paths.ts";
import { PROVIDERS } from "./providers.ts";
import { FileCredentialStore } from "./store.ts";

const TOKEN_URL = "https://auth.openai.com/oauth/token";
const ANTHROPIC_TOKEN_URL = "https://platform.claude.com/v1/oauth/token";
const temps: string[] = [];
const realFetch = globalThis.fetch;
const realXdg = process.env.XDG_DATA_HOME;

afterEach(() => {
  globalThis.fetch = realFetch;
  if (realXdg === undefined) delete process.env.XDG_DATA_HOME;
  else process.env.XDG_DATA_HOME = realXdg;
  for (const dir of temps.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function temp(): string {
  const dir = mkdtempSync(join(tmpdir(), "corral-pi-auth-"));
  temps.push(dir);
  return dir;
}

function segment(value: Record<string, unknown>): string {
  for (let nonce = 0; ; nonce++) {
    const text = Buffer.from(JSON.stringify({ ...value, n: nonce })).toString("base64").replace(/=+$/, "");
    if (!/[+/]/.test(text)) return text;
  }
}

function syntheticAccess(label: string): string {
  return [segment({ alg: "none" }), segment({ label, "https://api.openai.com/auth": { chatgpt_account_id: "synthetic" } }),
    "sig"].join(".");
}

interface FakeServer {
  readonly requests: string[];
}

function fakeOAuthServer(): FakeServer {
  const requests: string[] = [];
  globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    requests.push(url);
    const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { "content-type": "application/json" } });
    if (url.endsWith("/deviceauth/usercode")) return json({ device_auth_id: "dev-1", user_code: "SYN-CODE", interval: 0 });
    if (url.endsWith("/deviceauth/token")) return json({ authorization_code: "code-1", code_verifier: "verifier-1" });
    if (url === TOKEN_URL) {
      const grant = new URLSearchParams(String(init?.body)).get("grant_type");
      return json({ access_token: syntheticAccess(grant ?? "none"), refresh_token: `refresh-${grant}`, expires_in: 3600 });
    }
    return new Response("unexpected", { status: 599 });
  }) as typeof globalThis.fetch;
  return { requests };
}

const deviceCodeLogin: AuthInteraction = {
  async prompt(prompt) {
    if (prompt.type === "select") return "device_code";
    throw new Error(`unexpected prompt ${prompt.type}`);
  },
  notify() {},
};

function mode(path: string): number {
  return statSync(path).mode & 0o777;
}

describe("auth paths", () => {
  test("XDG_DATA_HOME wins when absolute; a relative one falls back to HOME", () => {
    expect(authDir({ XDG_DATA_HOME: "/data", HOME: "/home/u" })).toBe("/data/corral/pi-native/auth");
    expect(piNativeRoot({ XDG_DATA_HOME: "rel", HOME: "/home/u" })).toBe("/home/u/.local/share/corral/pi-native");
    expect(() => authDir({})).toThrow(/credentials_unavailable/);
  });
});

describe("corral agent login", () => {
  test("Pi's Codex OAuth login writes a 0600 credential into a 0700 corral-owned directory", async () => {
    const dir = join(temp(), "auth");
    const server = fakeOAuthServer();
    const result = await login(dir, deviceCodeLogin);
    expect(result.file).toBe(join(dir, "openai-codex.json"));
    expect(mode(dir)).toBe(0o700);
    expect(mode(result.file)).toBe(0o600);
    const stored = JSON.parse(readFileSync(result.file, "utf8"));
    expect(stored.type).toBe("oauth");
    expect(stored.refresh).toBe("refresh-authorization_code");
    expect(server.requests.every((url) => LOGIN_DESTINATIONS.includes(url))).toBeTrue();
  });

  test("the login network allows only POSTs to the OAuth endpoints", async () => {
    const send = loginFetch(async () => new Response("ok"));
    await expect(send(TOKEN_URL, { method: "GET" })).rejects.toThrow(/network_refused/);
    await expect(send("https://api.openai.com/v1/responses?key=x", { method: "POST" })).rejects
      .toThrow("network_refused: https://api.openai.com/v1/responses is not an OAuth login endpoint");
    expect(await (await send(TOKEN_URL, { method: "POST" })).text()).toBe("ok");
  });
});

describe("the production auth module", () => {
  test("refuses credentials_unavailable before any login", () => {
    process.env.XDG_DATA_HOME = temp();
    expect(() => auth.credentials({ provider: "openai-codex", apiEndpoint: "", refreshEndpoint: TOKEN_URL }))
      .toThrow(/credentials_unavailable: no OpenAI Codex login .*corral agent login/);
    expect(auth.state?.()).toEqual({ module: "corral-pi", credential: "absent" });
  });

  describe("expired credential refresh", () => {
    let dir: string;
    let server: FakeServer;
    let models: Awaited<ReturnType<typeof import("@earendil-works/pi-coding-agent").ModelRuntime.create>>;

    // Import and runtime initialization can be slow under contention; they are not the refresh under test.
    beforeAll(async () => {
      process.env.XDG_DATA_HOME = temp();
      dir = authDir();
      const seeded = new FileCredentialStore(dir);
      await seeded.modify("openai-codex", async () => ({
        type: "oauth", access: syntheticAccess("old"), refresh: "refresh-old", expires: Date.now() - 1000, accountId: "synthetic",
      }));
      const store = auth.credentials({ provider: "openai-codex", apiEndpoint: "", refreshEndpoint: TOKEN_URL });
      server = fakeOAuthServer();
      const { ModelRuntime } = await import("@earendil-works/pi-coding-agent");
      models = await ModelRuntime.create({ credentials: store, modelsPath: null, refreshOnCreate: false, allowModelNetwork: false });
    }, 60_000);

    test("Pi refreshes an expired credential in place inside the store's lock", async () => {
      const resolved = await models.getAuth("openai-codex");
      expect(resolved?.auth.apiKey ?? resolved?.auth.headers).toBeDefined();
      expect(server.requests).toEqual([TOKEN_URL]);
      const stored = JSON.parse(readFileSync(join(dir, "openai-codex.json"), "utf8"));
      expect(stored.refresh).toBe("refresh-refresh_token");
      expect(stored.expires).toBeGreaterThan(Date.now());
      expect(mode(join(dir, "openai-codex.json"))).toBe(0o600);
      expect(mode(join(dir, "auth.json.lock"))).toBe(0o600);
      expect(auth.state?.()).toEqual({ module: "corral-pi", credential: "present" });
    });
  });

  test("an API key is never stored: there is no paid fallback", async () => {
    const store = new FileCredentialStore(join(temp(), "auth"));
    await expect(store.modify("openai-codex", async () => ({ type: "api_key", key: "sk-synthetic" }))).rejects
      .toThrow(/only OAuth credentials/);
    expect(store.present()).toBeFalse();
  });
});

function fakeAnthropicServer(): FakeServer {
  const requests: string[] = [];
  globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    requests.push(url);
    if (url !== ANTHROPIC_TOKEN_URL) return new Response("unexpected", { status: 599 });
    const grant = JSON.parse(String(init?.body)).grant_type;
    return new Response(JSON.stringify({
      access_token: `sk-ant-oat-synthetic-${grant}`, refresh_token: `refresh-${grant}-${requests.length}`, expires_in: 3600,
    }), { headers: { "content-type": "application/json" } });
  }) as typeof globalThis.fetch;
  return { requests };
}

const copyCodeLogin: AuthInteraction = {
  async prompt(prompt) {
    if (prompt.type === "select") return "copy_code";
    if (prompt.type === "manual_code") return "synthetic-code";
    throw new Error(`unexpected prompt ${prompt.type}`);
  },
  notify() {},
};

describe("per-provider credentials (T-479)", () => {
  test("a pre-T-479 Codex auth.json keeps working and moves into openai-codex.json on its first refresh", async () => {
    process.env.XDG_DATA_HOME = temp();
    const dir = authDir();
    mkdirSync(dir, { recursive: true, mode: 0o700 });
    writeFileSync(join(dir, "auth.json"), JSON.stringify({ "openai-codex": {
      type: "oauth", access: syntheticAccess("legacy"), refresh: "refresh-legacy", expires: Date.now() - 1000, accountId: "synthetic",
    } }), { mode: 0o600 });
    const store = auth.credentials({ provider: "openai-codex", apiEndpoint: "", refreshEndpoint: TOKEN_URL });
    expect(((await store.read("openai-codex")) as { refresh?: string } | undefined)?.refresh).toBe("refresh-legacy");
    const server = fakeOAuthServer();
    const { ModelRuntime } = await import("@earendil-works/pi-coding-agent");
    const models = await ModelRuntime.create({ credentials: store, modelsPath: null, refreshOnCreate: false, allowModelNetwork: false });
    await models.getAuth("openai-codex");
    expect(server.requests).toEqual([TOKEN_URL]);
    expect(JSON.parse(readFileSync(join(dir, "openai-codex.json"), "utf8")).refresh).toBe("refresh-refresh_token");
    expect(existsSync(join(dir, "auth.json"))).toBeFalse();
  });

  test("an Anthropic profile is refused naming the provider and its login command, even with a Codex login", async () => {
    process.env.XDG_DATA_HOME = temp();
    await new FileCredentialStore(authDir()).modify("openai-codex", async () => ({
      type: "oauth", access: syntheticAccess("codex"), refresh: "r", expires: Date.now() + 3_600_000, accountId: "synthetic",
    }));
    const context = { provider: "anthropic" as const, apiEndpoint: PROVIDERS.anthropic.api, refreshEndpoint: ANTHROPIC_TOKEN_URL };
    expect(() => auth.credentials(context)).toThrow(
      /credentials_unavailable: no Anthropic Claude login \(provider anthropic\) .*run `corral agent login --provider anthropic`/);
    expect(() => auth.credentials({ provider: "openai-codex", apiEndpoint: "", refreshEndpoint: TOKEN_URL })).not.toThrow();
  });

  test("Pi's Anthropic OAuth login writes anthropic.json, reaches only Anthropic's token endpoint, and re-login replaces it", async () => {
    const dir = join(temp(), "auth");
    const server = fakeAnthropicServer();
    const send = loginFetch(globalThis.fetch, "anthropic");
    globalThis.fetch = send as typeof globalThis.fetch;
    const first = await login(dir, copyCodeLogin, "anthropic");
    expect(first).toEqual({ provider: "anthropic", file: join(dir, "anthropic.json") });
    expect(mode(first.file)).toBe(0o600);
    const stored = JSON.parse(readFileSync(first.file, "utf8"));
    expect([stored.type, stored.refresh]).toEqual(["oauth", "refresh-authorization_code-1"]);
    await login(dir, copyCodeLogin, "anthropic");
    expect(JSON.parse(readFileSync(first.file, "utf8")).refresh).toBe("refresh-authorization_code-2");
    expect(server.requests).toEqual([ANTHROPIC_TOKEN_URL, ANTHROPIC_TOKEN_URL]);
    expect(existsSync(join(dir, "openai-codex.json")) || existsSync(join(dir, "auth.json"))).toBeFalse();
  });

  test("Pi refreshes an expired Anthropic credential in place through Anthropic's token endpoint", async () => {
    process.env.XDG_DATA_HOME = temp();
    await new FileCredentialStore(authDir(), "anthropic").modify("anthropic", async () => ({
      type: "oauth", access: "sk-ant-oat-old", refresh: "refresh-old", expires: Date.now() - 1000,
    }));
    const store = auth.credentials({ provider: "anthropic", apiEndpoint: PROVIDERS.anthropic.api, refreshEndpoint: ANTHROPIC_TOKEN_URL });
    const server = fakeAnthropicServer();
    const { ModelRuntime } = await import("@earendil-works/pi-coding-agent");
    const models = await ModelRuntime.create({ credentials: store, modelsPath: null, refreshOnCreate: false, allowModelNetwork: false });
    expect((await models.getAuth("anthropic"))?.auth.apiKey).toBe("sk-ant-oat-synthetic-refresh_token");
    expect(server.requests).toEqual([ANTHROPIC_TOKEN_URL]);
    expect(JSON.parse(readFileSync(join(authDir(), "anthropic.json"), "utf8")).refresh).toBe("refresh-refresh_token-1");
    expect(await store.read("openai-codex")).toBeUndefined();
  });

  test("each provider's login network reaches only its own OAuth endpoints", async () => {
    const anthropic = loginFetch(async () => new Response("ok"), "anthropic");
    await expect(anthropic(TOKEN_URL, { method: "POST" })).rejects.toThrow(/network_refused/);
    await expect(anthropic("https://claude.ai/oauth/authorize", { method: "POST" })).rejects.toThrow(/network_refused/);
    expect(await (await anthropic(ANTHROPIC_TOKEN_URL, { method: "POST" })).text()).toBe("ok");
    const codex = loginFetch(async () => new Response("ok"));
    await expect(codex(ANTHROPIC_TOKEN_URL, { method: "POST" })).rejects.toThrow(/network_refused/);
  });
});

function straceUsable(): boolean {
  return spawnSync("strace", ["-f", "-qq", "-o", "/dev/null", "true"], { stdio: "ignore" }).status === 0;
}

describe("sentinel HOME", () => {
  test("login, credential reads and refresh never open ~/.codex or another harness store", async () => {
    if (!straceUsable()) {
      console.log("skipping the sentinel HOME trace: strace is unavailable or ptrace is not permitted");
      return;
    }
    const root = temp();
    const home = join(root, "home");
    const stores = [".codex", ".pi/agent", ".claude", ".omo", ".config/opencode", ".local/share/opencode"];
    for (const store of stores) {
      mkdirSync(join(home, store), { recursive: true });
      writeFileSync(join(home, store, "auth.json"), "{\"sentinel\":true}\n");
    }
    const data = join(root, "data");
    const log = join(root, "trace.log");
    const here = dirname(fileURLToPath(import.meta.url));
    const script = `
      const fake = async (input, init) => {
        const url = String(input instanceof Request ? input.url : input);
        const json = (b) => new Response(JSON.stringify(b), { headers: { "content-type": "application/json" } });
        if (url.endsWith("/deviceauth/usercode")) return json({ device_auth_id: "d", user_code: "C", interval: 0 });
        if (url.endsWith("/deviceauth/token")) return json({ authorization_code: "c", code_verifier: "v" });
        const access = ${JSON.stringify(syntheticAccess("traced"))};
        return json({ access_token: access, refresh_token: "r", expires_in: 1 });
      };
      globalThis.fetch = fake;
      const { login } = await import(${JSON.stringify(join(here, "login.ts"))});
      const { default: auth } = await import(${JSON.stringify(join(here, "index.ts"))});
      const { authDir } = await import(${JSON.stringify(join(here, "paths.ts"))});
      await login(authDir(), { prompt: async () => "device_code", notify() {} });
      const store = auth.credentials({ provider: "openai-codex", apiEndpoint: "", refreshEndpoint: "" });
      const { ModelRuntime } = await import("@earendil-works/pi-coding-agent");
      const models = await ModelRuntime.create({ credentials: store, modelsPath: null, refreshOnCreate: false, allowModelNetwork: false });
      await models.getAuth("openai-codex");
      console.log("TRACED_OK");
    `;
    const child = spawn("strace", ["-f", "-qq", "-e", "trace=%file", "-o", log, process.execPath, "--no-env-file", "-e", script], {
      cwd: join(here, "../.."),
      env: { PATH: process.env.PATH ?? "", HOME: home, XDG_DATA_HOME: data, PI_OFFLINE: "1", PI_TELEMETRY: "0" },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let out = "";
    let err = "";
    child.stdout.on("data", (chunk: Buffer) => { out += chunk.toString(); });
    child.stderr.on("data", (chunk: Buffer) => { err += chunk.toString(); });
    const code = await new Promise<number | null>((resolve) => child.on("close", resolve));
    expect({ code, out: out.trim(), err: code === 0 ? "" : err }).toEqual({ code: 0, out: "TRACED_OK", err: "" });
    const trace = readFileSync(log, "utf8");
    expect(trace).toContain(join(data, "corral/pi-native/auth/openai-codex.json"));
    const opened = stores.filter((store) => trace.includes(join(home, store)));
    expect(opened).toEqual([]);
  }, 60_000);
});
