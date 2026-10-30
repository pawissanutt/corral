// Review F1 of T-479: an Anthropic host never falls back to a paid API key. With a stored
// subscription OAuth login and every ambient Anthropic auth source set (synthetic values), a
// logout must leave the provider without auth, and no request may carry anything but the OAuth
// token. Since T-493 requests run through the Claude Agent SDK lane (a fake `query` here), so the
// token is what the Claude Code subprocess receives.
import { afterEach, beforeAll, describe, expect, test } from "bun:test";
import { ModelRuntime } from "@earendil-works/pi-coding-agent";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import auth from "../src/auth/index.ts";
import { authDir } from "../src/auth/paths.ts";
import { PROVIDERS } from "../src/auth/providers.ts";
import { FileCredentialStore } from "../src/auth/store.ts";
import { overrideSdk } from "../src/claude/sdk.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { restrictProvider } from "../src/host/session.ts";

const PAID = "sk-ant-api03-SYNTHETIC-PAID-KEY";
const OAUTH = "sk-ant-oat01-SYNTHETIC-SUBSCRIPTION";
const AMBIENT: Record<string, Record<string, string>> = {
  ANTHROPIC_API_KEY: { ANTHROPIC_API_KEY: PAID },
  ANTHROPIC_AUTH_TOKEN: { ANTHROPIC_AUTH_TOKEN: PAID },
  ANTHROPIC_OAUTH_TOKEN: { ANTHROPIC_OAUTH_TOKEN: PAID },
  federation: {
    ANTHROPIC_FEDERATION_RULE_ID: "rule-synthetic", ANTHROPIC_ORGANIZATION_ID: "org-synthetic",
    ANTHROPIC_IDENTITY_TOKEN_FILE: "/nonexistent/synthetic-identity-token",
  },
};
const saved = { ...process.env };
const temps: string[] = [];

afterEach(() => {
  for (const key of Object.keys(process.env)) if (!(key in saved)) delete process.env[key];
  Object.assign(process.env, saved);
  overrideSdk(undefined);
  for (const dir of temps.splice(0)) rmSync(dir, { recursive: true, force: true });
});

async function anthropicHost(ambient: Record<string, string>) {
  const root = mkdtempSync(join(tmpdir(), "corral-pi-oauth-only-"));
  temps.push(root);
  process.env.XDG_DATA_HOME = root;
  Object.assign(process.env, ambient);
  await new FileCredentialStore(authDir(), "anthropic").modify("anthropic", async () => ({
    type: "oauth", access: OAUTH, refresh: "refresh-synthetic", expires: Date.now() + 3_600_000,
  }));
  const sent: string[] = [];
  overrideSdk({
    query: ({ options }) => {
      const env = options.env ?? {};
      const ambient = Object.keys(env).filter((key) => key.startsWith("ANTHROPIC_"));
      sent.push(`CLAUDE_CODE_OAUTH_TOKEN=${env.CLAUDE_CODE_OAUTH_TOKEN} ambient=[${ambient.join(",")}]`);
      return Object.assign((async function* () {
        yield { type: "result", subtype: "success", is_error: false, result: "ok", stop_reason: "end_turn" };
      })(), { interrupt: async () => undefined, close: () => undefined }) as never;
    },
    createSdkMcpServer: () => { throw new Error("no custom tools in this test"); },
  });
  const guard = guardedFetch(async () => new Response("synthetic", { status: 400 }), undefined, "anthropic");
  const models = await ModelRuntime.create({
    credentials: auth.credentials({ provider: "anthropic", apiEndpoint: PROVIDERS.anthropic.api,
      refreshEndpoint: PROVIDERS.anthropic.refresh }),
    modelsPath: null, refreshOnCreate: false, allowModelNetwork: false,
  });
  restrictProvider(models, "anthropic", guard.fetch as typeof fetch,
    { configDir: join(root, "claude-code"), cwd: root, executable: () => "/synthetic/claude" });
  return { models, sent, guard };
}

async function request(models: Awaited<ReturnType<typeof anthropicHost>>["models"], options: Record<string, unknown>) {
  const model = models.getModel("anthropic", "claude-opus-5-5")!;
  const provider = models.getProvider("anthropic")!;
  const context = { messages: [{ role: "user", content: "hi", timestamp: Date.now() }] } as never;
  try {
    for await (const _event of provider.stream(model, context, { maxRetries: 0, ...options } as never)) { /* drain */ }
    return "sent";
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
}

for (const [source, ambient] of Object.entries(AMBIENT)) {
  describe(`ambient ${source}`, () => {
    let host: Awaited<ReturnType<typeof anthropicHost>>;
    // Fixture construction and cold SDK initialization are not part of the logout assertion.
    beforeAll(async () => { host = await anthropicHost(ambient); });
    test(`after logout an ambient ${source} never authenticates an Anthropic request`, async () => {
      const { models, sent, guard } = host;
      // Given the stored subscription login, requests carry only its OAuth token:
      const before = await models.getAuth("anthropic");
      expect([before?.source, before?.auth.apiKey]).toEqual(["OAuth", OAUTH]);
      expect(await request(models, { apiKey: before?.auth.apiKey })).toBe("sent");
      expect(sent).toEqual([`CLAUDE_CODE_OAUTH_TOKEN=${OAUTH} ambient=[]`]);
      // When the owner logs out in the pane:
      await models.logout("anthropic");
      // Then no auth resolves from the environment, and any non-OAuth request is refused unsent:
      expect(await models.getAuth("anthropic")).toBeUndefined();
      for (const options of [{}, { apiKey: PAID }, { headers: { Authorization: `Bearer ${PAID}` } },
        { apiKey: OAUTH, headers: { "x-api-key": PAID } }]) {
        expect(await request(models, options)).toMatch(/^credentials_unavailable: anthropic requests must use the subscription/);
      }
      expect(sent).toHaveLength(1);
      expect(guard.ledger().allowed).toBe(0);
    });
  });
}

test("the host environment drops every ambient Anthropic auth source before Pi loads", () => {
  const env: NodeJS.ProcessEnv = { PATH: "/bin", ANTHROPIC_BASE_URL: "https://proxy.invalid" };
  for (const ambient of Object.values(AMBIENT)) Object.assign(env, ambient);
  restrictEnvironment(env);
  expect(Object.keys(env).filter((key) => key.startsWith("ANTHROPIC_"))).toEqual([]);
  expect(env.PATH).toBe("/bin");
});

test("an OpenAI Codex host keeps its OAuth-only provider unchanged", async () => {
  const root = mkdtempSync(join(tmpdir(), "corral-pi-oauth-only-"));
  temps.push(root);
  writeFileSync(join(root, "unused"), "");
  const models = await ModelRuntime.create({
    credentials: new FileCredentialStore(join(root, "auth")),
    modelsPath: null, refreshOnCreate: false, allowModelNetwork: false,
  });
  restrictProvider(models, "openai-codex", guardedFetch(async () => new Response("{}")).fetch as typeof fetch);
  expect(models.getProvider("openai-codex")?.auth.apiKey).toBeUndefined();
  expect(models.getProvider("openai-codex")?.auth.oauth).toBeDefined();
  expect(await models.getAuth("openai-codex")).toBeUndefined();
});
