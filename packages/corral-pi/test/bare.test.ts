import { expect, test } from "bun:test";
import { providerTransport, sessionRoutes } from "../src/host/capabilities.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { PROVIDERS } from "../src/auth/providers.ts";
import type { AuthModule } from "../src/auth/types.ts";
import { buildShellRefusal } from "../src/roles/build/guard.ts";

test("bare Build keeps OAuth and API logins without exposing paid endpoints to generic fetch", async () => {
  const calls: string[] = [];
  const auth: AuthModule = {
    provider: "openai-codex", providers: ["openai-codex", "anthropic", "openai", "amazon-bedrock"],
    credentials(context) {
      return {
        async read(id: string) {
          if (id !== context.provider) return undefined;
          return id === "openai" ? { type: "api_key", key: "sk-SYNTHETIC-fixture" }
            : { type: "oauth", access: "synthetic", refresh: "synthetic", expires: 4_000_000_000_000 };
        },
        async list() { return [{ providerId: context.provider, type: context.provider === "openai" ? "api_key" : "oauth" }]; },
        async modify() { throw new Error("read-only"); },
        async delete() { throw new Error("read-only"); },
      };
    },
    transport(context) { return async () => { calls.push(context.provider); return new Response("ok"); }; },
  };
  const allowed = await sessionRoutes(auth, "openai-codex", "build");
  expect(allowed.providers).toEqual(["openai-codex", "anthropic", "openai"]);
  expect((await allowed.credentials.list()).map(entry => entry.providerId)).toEqual(allowed.providers);
  const subscriptions = allowed.providers.filter(id => PROVIDERS[id].auth === "oauth");
  const network = guardedFetch(providerTransport(auth, subscriptions, async () => { throw new Error("unexpected fallback"); }),
    undefined, subscriptions);
  await network.fetch(PROVIDERS["openai-codex"].api, { method: "POST" });
  await network.fetch(PROVIDERS.anthropic.refresh, { method: "POST" });
  expect(calls).toEqual(["openai-codex", "anthropic"]);
  await expect(network.fetch(PROVIDERS.anthropic.api, { method: "POST" })).rejects.toThrow("network_refused");
  await expect(network.fetch(PROVIDERS.openai.api, { method: "POST", headers: { authorization: "Bearer sk-SYNTHETIC-fixture" } })).rejects.toThrow("network_refused");
  await expect(network.fetch("https://bedrock-runtime.us-east-1.amazonaws.com/model/example/converse-stream",
    { method: "POST" })).rejects.toThrow("network_refused");
  await expect(network.fetch("https://example.invalid/", { method: "POST" })).rejects.toThrow("network_refused");
});

test("bare shell guards refuse corral reads as well as writes without changing ordinary build guards", () => {
  expect(buildShellRefusal("corral list")).toBeUndefined();
  expect(buildShellRefusal("corral list", true)).toContain("do not invoke corral");
  expect(buildShellRefusal("bash -c 'corral build gate'", true)).toContain("do not invoke corral");
  expect(buildShellRefusal("corral build land", true)).toBeDefined();
});
