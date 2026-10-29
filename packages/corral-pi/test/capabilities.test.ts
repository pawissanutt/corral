import { expect, test } from "bun:test";
import type { Credential, CredentialStore } from "@earendil-works/pi-ai";
import { CredentialsUnavailable } from "../src/auth/paths.ts";
import { PROVIDER_IDS, type ProviderId, PROVIDERS } from "../src/auth/providers.ts";
import type { AuthModule } from "../src/auth/types.ts";
import { providerCapabilities, providerTransport } from "../src/host/capabilities.ts";
import { guardedFetch } from "../src/host/restrict.ts";

function fixture(enabled: readonly ProviderId[]): AuthModule {
  const entries = new Map<string, Credential>(enabled.map((provider) => [
    provider, { type: "oauth", access: `${provider}-synthetic`, refresh: "r", expires: 4_000_000_000_000 },
  ]));
  const store: CredentialStore = {
    async read(provider) { return entries.get(provider); },
    async list() { return [...entries.keys()].map((providerId) => ({ providerId, type: "oauth" as const })); },
    async modify(provider, change) {
      const next = await change(entries.get(provider));
      if (next) entries.set(provider, next);
      return entries.get(provider);
    },
    async delete(provider) { entries.delete(provider); },
  };
  return {
    provider: "openai-codex", providers: PROVIDER_IDS,
    credentials() { return store; },
    transport(context) { return async () => new Response(context.provider); },
  };
}

test("availability intersects policy and credentials, not the initial provider", async () => {
  // Given two corral-owned logins and a standalone session.
  const auth = fixture(PROVIDER_IDS);
  // When the session starts with OpenAI.
  const capability = await providerCapabilities(auth, "openai-codex", PROVIDER_IDS);
  // Then both adapters remain available, with isolated credential mutations.
  expect(capability.providers).toEqual(PROVIDER_IDS);
  expect(await capability.credentials.list()).toHaveLength(2);
  await capability.credentials.delete("anthropic");
  expect(await capability.credentials.read("anthropic")).toBeUndefined();
  expect((await capability.credentials.read("openai-codex"))?.type).toBe("oauth");
});

test("a bound provider policy never exposes another stored login", async () => {
  const auth = fixture(PROVIDER_IDS);
  const capability = await providerCapabilities(auth, "openai-codex", ["openai-codex"]);
  expect(capability.providers).toEqual(["openai-codex"]);
  expect(await capability.credentials.read("anthropic")).toBeUndefined();
  expect(await capability.credentials.list()).toEqual([{ providerId: "openai-codex", type: "oauth" }]);
  await expect(capability.credentials.modify("anthropic", async () => undefined))
    .rejects.toThrow(CredentialsUnavailable);
});

test("a missing optional login is explained, but a missing initial login refuses startup", async () => {
  const auth = fixture(["openai-codex"]);
  const capability = await providerCapabilities(auth, "openai-codex", PROVIDER_IDS);
  expect(capability.availability.map(({ provider, available }) => ({ provider, available }))).toEqual([
    { provider: "openai-codex", available: true }, { provider: "anthropic", available: false },
  ]);
  expect(capability.availability[1]?.reason).toBeDefined();
  await expect(providerCapabilities(auth, "anthropic", PROVIDER_IDS)).rejects.toThrow(CredentialsUnavailable);
});

test("each adapter keeps its transport and endpoint restrictions in a multi-provider session", async () => {
  const auth = fixture(PROVIDER_IDS);
  const network = guardedFetch(providerTransport(auth, PROVIDER_IDS, async () => {
    throw new Error("unexpected fallback");
  }), undefined, PROVIDER_IDS);
  expect(await (await network.fetch(PROVIDERS["openai-codex"].api, { method: "POST" })).text())
    .toBe("openai-codex");
  expect(await (await network.fetch(PROVIDERS.anthropic.refresh, { method: "POST" })).text())
    .toBe("anthropic");
  await expect(network.fetch(PROVIDERS.anthropic.api, { method: "POST" })).rejects.toThrow("network_refused");
  await expect(network.fetch("https://example.invalid", { method: "POST" })).rejects.toThrow("network_refused");
  expect(network.ledger().allowed).toBe(2);
});
