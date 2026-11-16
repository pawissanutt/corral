import { expect, test } from "bun:test";
import { bareProviders, requireBareProvider } from "../src/host/bare.ts";
import { providerTransport } from "../src/host/capabilities.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { PROVIDERS } from "../src/auth/providers.ts";
import type { AuthModule } from "../src/auth/types.ts";
import { buildShellRefusal } from "../src/roles/build/guard.ts";

test("bare provider policy retains both OAuth logins and dispatches each guarded adapter, never API or AWS", async () => {
  const calls: string[] = [];
  const auth: AuthModule = {
    provider: "openai-codex", providers: ["openai-codex", "anthropic", "openai", "amazon-bedrock"],
    credentials(context) {
      return {
        async read(id: string) { return id === context.provider
          ? { type: "oauth" as const, access: "synthetic", refresh: "synthetic", expires: 4_000_000_000_000 } : undefined; },
        async list() { return [{ providerId: context.provider, type: "oauth" as const }]; },
        async modify() { throw new Error("read-only"); },
        async delete() { throw new Error("read-only"); },
      };
    },
    transport(context) { return async () => { calls.push(context.provider); return new Response("ok"); }; },
  };
  const allowed = await bareProviders(auth, "openai-codex");
  expect(allowed.providers).toEqual(["openai-codex", "anthropic"]);
  expect((await allowed.credentials.list()).map(entry => entry.providerId)).toEqual(["openai-codex", "anthropic"]);
  const network = guardedFetch(providerTransport(auth, allowed.providers, async () => { throw new Error("unexpected fallback"); }),
    undefined, allowed.providers);
  await network.fetch(PROVIDERS["openai-codex"].api, { method: "POST" });
  await network.fetch(PROVIDERS.anthropic.refresh, { method: "POST" });
  expect(calls).toEqual(["openai-codex", "anthropic"]);
  await expect(network.fetch(PROVIDERS.anthropic.api, { method: "POST" })).rejects.toThrow("network_refused");
  await expect(network.fetch(PROVIDERS.openai.api, { method: "POST" })).rejects.toThrow("network_refused");
  await expect(network.fetch("https://bedrock-runtime.us-east-1.amazonaws.com/model/example/converse-stream",
    { method: "POST" })).rejects.toThrow("network_refused");
  await expect(network.fetch("https://example.invalid/", { method: "POST" })).rejects.toThrow("network_refused");
  expect(() => requireBareProvider("openai")).toThrow("metered admission");
  expect(() => requireBareProvider("llama.cpp")).toThrow("bound role");
});

test("bare shell guards refuse corral reads as well as writes without changing ordinary build guards", () => {
  expect(buildShellRefusal("corral list")).toBeUndefined();
  expect(buildShellRefusal("corral list", true)).toContain("do not invoke corral");
  expect(buildShellRefusal("bash -c 'corral build gate'", true)).toContain("do not invoke corral");
  expect(buildShellRefusal("corral build land", true)).toBeDefined();
});
