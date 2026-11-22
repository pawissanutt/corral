import { expect, test } from "bun:test";
import type { Credential, CredentialStore } from "@earendil-works/pi-ai";
import { CredentialsUnavailable } from "../src/auth/paths.ts";
import { PROVIDER_IDS, type ProviderId, PROVIDERS } from "../src/auth/providers.ts";
import type { AuthModule } from "../src/auth/types.ts";
import { boundFleetSettings } from "../src/extensions/fleet/settings.ts";
import { auxiliarySettings } from "../src/extensions/subagent/settings.ts";
import { childProfileProviders, providerCapabilities, providerTransport, sessionRoutes } from "../src/host/capabilities.ts";
import { workspaceSettings } from "../src/host/config.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import engineDefaults from "./fixtures/default-capabilities.json" with { type: "json" };

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

test("standalone worker and assistant child limits match the engine capability defaults", () => {
  // The Rust capability snapshot test keeps these values tied to the engine resolver.
  const { fleet, subagents } = engineDefaults.assistant.features;
  expect(fleet.settings.max_parallel).toBe(32);
  expect(subagents.settings.max_parallel).toBe(32);
  for (const role of ["worker", "assistant"] as const) {
    const { capabilities, guidance } = workspaceSettings("/unused", role, undefined, {});
    expect(boundFleetSettings(role, capabilities.enabled, guidance.settings).maxParallel).toBe(fleet.settings.max_parallel);
    expect(auxiliarySettings(role, capabilities.enabled, guidance.settings).explore).toEqual(subagents.settings);
  }
});

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

test("a worker loads its child profiles' logins; reviewers and local launches stay bound", async () => {
  const profiles = [{ model: "anthropic/claude" }, { model: "openai/gpt" }, { model: "unknown/x" }];
  const extra = childProfileProviders("worker", "openai-codex", profiles);
  expect(extra).toEqual(["anthropic", "openai"]);
  // A profile provider without a stored login is unavailable, not a startup failure.
  const auth = fixture(["openai-codex", "anthropic"]);
  expect((await sessionRoutes(auth, "openai-codex", "worker", extra)).providers).toEqual(["openai-codex", "anthropic"]);
  expect((await sessionRoutes(auth, "openai-codex", "worker")).providers).toEqual(["openai-codex"]);
  expect(childProfileProviders("reviewer", "openai-codex", profiles)).toEqual([]);
  expect(childProfileProviders("worker", "llama.cpp", profiles)).toEqual([]);
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
